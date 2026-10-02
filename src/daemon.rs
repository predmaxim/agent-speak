//! Цикл событий сервиса: команды и хуки из сокета, изменения транскриптов (inotify).

use crate::assemble::{Assembler, MdEvent};
use crate::config::Config;
use crate::dedup::Dedup;
use crate::focus::{self, Agent, SessionRef};
use crate::ipc::{self, Msg};
use crate::notice::notify as notice;
use crate::queue::{Item, Kind, Queue};
use crate::source::{claude, codex, tail::Tailer};
use crate::speaker::{self, Shared};
use crate::text::{prepare, terms::Terms};
use crate::tts::Tts;
use ::notify::{RecursiveMode, Watcher};
use serde::Serialize;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

enum Event {
    Msg(Msg),
    File(PathBuf),
    Config,
    Subscribe(UnixStream),
    Busy, // поток воспроизведения: начал или закончил говорить
}

/// Строка состояния для подписчиков (плагин панели). Порядок полей — протокол.
#[derive(Serialize)]
struct Status<'a> {
    running: bool,
    speaking: bool,
    paused: bool,
    mode: &'a str,
    read_intermediate: bool,
    speaker: &'a str,
    rate: &'a str,
    project: &'a str,
}

struct State {
    cfg: Config,
    shared: Arc<Shared>,
    terms: Arc<Mutex<Terms>>,
    learn: Sender<(Agent, String)>,
    dedup: Dedup,
    asm: Assembler,
    tail: Tailer,
    focus_cache: (Instant, Option<SessionRef>),
    subs: Vec<UnixStream>,
    projects: HashMap<String, String>, // сессия → имя папки проекта (из хуков и окна в фокусе)
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default())
}

pub fn run() {
    let data = home().join(".local/share/agent-speak");
    let cfg = Config::load();
    let shared = Arc::new(Shared::new(
        Queue::new(Duration::from_secs(cfg.max_age_secs)),
        cfg.speaker.clone(),
        cfg.rate.clone(),
    ));
    let tts = Tts::new(
        data.join("venv/bin/python"),
        data.join("silero_tts.py"),
        data.join("v5_ru.pt"),
        PathBuf::from(std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into())).join("agent-speak-tts.sock"),
    );
    {
        let shared = shared.clone();
        std::thread::spawn(move || speaker::run(shared, tts));
    }
    let terms = Arc::new(Mutex::new(Terms::load(&data.join("terms.tsv"))));
    let learn = crate::learn::spawn(terms.clone());

    let (tx, rx) = mpsc::channel::<Event>();
    {
        // «говорит/молчит» из потока воспроизведения — в основной цикл
        let (btx, brx) = mpsc::channel::<()>();
        *shared.changed.lock().unwrap() = Some(btx);
        let tx = tx.clone();
        std::thread::spawn(move || {
            for () in brx {
                let _ = tx.send(Event::Busy);
            }
        });
    }
    let sock = ipc::socket_path();
    let _ = std::fs::remove_file(&sock);
    let listener = UnixListener::bind(&sock).expect("bind agent-speak.sock");
    {
        let tx = tx.clone();
        std::thread::spawn(move || {
            for conn in listener.incoming().flatten() {
                let tx = tx.clone();
                std::thread::spawn(move || serve(conn, tx));
            }
        });
    }
    let ftx = tx.clone();
    let mut watcher = ::notify::recommended_watcher(move |res: ::notify::Result<::notify::Event>| {
        if let Ok(ev) = res {
            if matches!(ev.kind, ::notify::EventKind::Modify(_) | ::notify::EventKind::Create(_)) {
                for p in ev.paths {
                    if p.extension().is_some_and(|e| e == "jsonl") {
                        let _ = ftx.send(Event::File(p));
                    } else if p == crate::config::path() {
                        let _ = ftx.send(Event::Config);
                    }
                }
            }
        }
    })
    .expect("inotify");
    for dir in [home().join(".claude/projects"), home().join(".codex/sessions")] {
        if let Err(e) = watcher.watch(&dir, RecursiveMode::Recursive) {
            eprintln!("agent-speak: не слежу за {}: {e}", dir.display());
        }
    }
    let cfg_dir = crate::config::path().parent().unwrap().to_path_buf();
    let _ = std::fs::create_dir_all(&cfg_dir);
    if let Err(e) = watcher.watch(&cfg_dir, RecursiveMode::NonRecursive) {
        eprintln!("agent-speak: не слежу за настройками: {e}");
    }

    let mut st = State {
        cfg,
        shared,
        terms,
        learn,
        dedup: Dedup::new(),
        asm: Assembler::new(),
        tail: Tailer::new(),
        focus_cache: (Instant::now() - Duration::from_secs(10), None),
        subs: Vec::new(),
        projects: HashMap::new(),
    };
    for ev in rx {
        match ev {
            Event::Msg(m) => st.on_msg(m),
            Event::File(p) => st.on_file(&p),
            Event::Config => st.reload(),
            Event::Subscribe(s) => st.subscribe(s),
            Event::Busy => st.broadcast(),
        }
    }
}

/// Финальный ответ: из payload хука, иначе из транскрипта (после паузы — запись ещё не сброшена на диск).
fn final_message(p: &serde_json::Value, transcript: &Path) -> Option<String> {
    if let Some(m) = p["last_assistant_message"].as_str().filter(|m| !m.trim().is_empty()) {
        return Some(m.to_string());
    }
    std::thread::sleep(Duration::from_millis(300));
    claude::last_turn(&std::fs::read_to_string(transcript).unwrap_or_default()).pop()
}

/// Одно соединение: команды построчно до закрытия (CLI и хуки — одна строка).
/// subscribe — копия потока уходит в основной цикл подписчиком, чтение команд продолжается.
// ponytail: поток на соединение без лимита — клиенты только свои (CLI, хуки, плагин)
fn serve(conn: UnixStream, tx: Sender<Event>) {
    let Ok(input) = conn.try_clone() else { return };
    // молчун без первой строки не держит поток вечно; подписка снимает таймаут
    let _ = input.set_read_timeout(Some(Duration::from_secs(1)));
    for line in BufReader::new(input).lines() {
        let Ok(line) = line else { break };
        match serde_json::from_str::<Msg>(&line) {
            Ok(Msg::Subscribe) => match conn.try_clone() {
                Ok(w) => {
                    let _ = conn.set_read_timeout(None); // таймаут общий для клонов сокета
                    let _ = tx.send(Event::Subscribe(w));
                }
                Err(_) => break,
            },
            Ok(m) => {
                let _ = tx.send(Event::Msg(m));
            }
            Err(e) => eprintln!("agent-speak: плохое сообщение: {e}"),
        }
    }
}

impl State {
    fn focused(&mut self) -> Option<SessionRef> {
        if self.focus_cache.0.elapsed() > Duration::from_millis(500) {
            self.focus_cache = (Instant::now(), focus::focused());
            if let Some(f) = &self.focus_cache.1 {
                self.projects.insert(f.id.clone(), f.project.clone());
            }
        }
        self.focus_cache.1.clone()
    }

    fn auto(&self) -> bool {
        self.cfg.mode == "auto"
    }

    /// Промежуточные статусы читаются по ходу работы (авто + read_intermediate).
    fn live(&self) -> bool {
        self.auto() && self.cfg.read_intermediate
    }

    /// config.toml изменён (интерфейсом или руками) — применить без перезапуска.
    fn reload(&mut self) {
        self.cfg = Config::load();
        self.apply();
    }

    /// Настройки → поток воспроизведения и очередь, затем — подписчикам.
    fn apply(&mut self) {
        *self.shared.voice.lock().unwrap() = (self.cfg.speaker.clone(), self.cfg.rate.clone());
        self.shared.queue.lock().unwrap().set_max_age(Duration::from_secs(self.cfg.max_age_secs));
        self.broadcast();
    }

    fn status_line(&self) -> String {
        let speaking = self.shared.busy.load(Ordering::SeqCst);
        let paused = self.shared.queue.lock().unwrap().paused();
        let project = if speaking {
            self.projects.get(&*self.shared.current.lock().unwrap()).cloned().unwrap_or_default()
        } else {
            String::new()
        };
        let st = Status {
            running: true,
            speaking,
            paused,
            mode: &self.cfg.mode,
            read_intermediate: self.cfg.read_intermediate,
            speaker: &self.cfg.speaker,
            rate: &self.cfg.rate,
            project: &project,
        };
        format!("{}\n", serde_json::to_string(&st).unwrap())
    }

    fn subscribe(&mut self, mut s: UnixStream) {
        let _ = s.set_write_timeout(Some(Duration::from_millis(100))); // зависший подписчик не держит цикл
        if s.write_all(self.status_line().as_bytes()).is_ok() {
            self.subs.push(s);
        }
    }

    /// Состояние всем подписчикам; отвалившиеся удаляются молча.
    fn broadcast(&mut self) {
        let line = self.status_line();
        self.subs.retain_mut(|s| s.write_all(line.as_bytes()).is_ok());
    }

    /// Подготовить и поставить в очередь; незнакомые слова — в фон.
    fn enqueue(&mut self, agent: Agent, session: &str, raw: &str, kind: Kind) -> usize {
        let (sentences, unknown) = prepare(raw, &self.terms.lock().unwrap());
        for w in unknown {
            let _ = self.learn.send((agent.clone(), w));
        }
        let n = sentences.len();
        let mut q = self.shared.queue.lock().unwrap();
        for text in sentences {
            q.push(Item { session: session.into(), text, kind: kind.clone(), born: Instant::now() });
        }
        drop(q);
        self.shared.cv.notify_all();
        n
    }

    fn stop(&mut self) {
        self.shared.stop();
    }

    fn on_msg(&mut self, m: Msg) {
        let changes = matches!(m, Msg::Stop | Msg::Pause | Msg::Mode);
        match m {
            Msg::Stop => self.stop(),
            Msg::Pause => {
                let paused = self.shared.queue.lock().unwrap().paused();
                if paused {
                    self.shared.resume();
                } else {
                    self.shared.pause();
                }
            }
            Msg::Mode => {
                self.cfg.mode = if self.auto() { "manual".into() } else { "auto".into() };
                self.cfg.save();
                eprintln!("agent-speak: режим: {}", self.cfg.mode);
            }
            Msg::Read => self.read(),
            Msg::Hook { kind, payload } => self.on_hook(&kind, &payload),
            Msg::Set { key, value } => {
                if self.cfg.set(&key, &value) {
                    self.cfg.save();
                    self.apply();
                } else {
                    eprintln!("agent-speak: set: неверно {key} = {value}");
                }
            }
            Msg::Say { text } => {
                self.enqueue(Agent::Claude, "say", &text, Kind::Manual);
            }
            Msg::Subscribe => {} // приходит как Event::Subscribe
        }
        if changes {
            self.broadcast();
        }
    }

    fn read(&mut self) {
        let busy = self.shared.busy.load(Ordering::SeqCst) || !self.shared.queue.lock().unwrap().is_empty();
        if busy {
            self.stop();
            eprintln!("agent-speak: read: остановлено");
            return;
        }
        let Some(s) = self.focused() else {
            eprintln!("agent-speak: read: нет агента в фокусе");
            notice("В фокусе нет агента", "");
            return;
        };
        let content = std::fs::read_to_string(&s.transcript).unwrap_or_default();
        let texts = match s.agent {
            Agent::Claude => claude::last_turn(&content),
            Agent::Codex => codex::last_turn(&content),
        };
        let joined = texts.join("\n\n");
        if self.enqueue(s.agent.clone(), &s.id, &joined, Kind::Manual) == 0 {
            eprintln!("agent-speak: read: нечего читать ({})", s.transcript.display());
            notice("Нечего читать", "");
        } else {
            eprintln!("agent-speak: read: читаю {} из {}", texts.len(), s.transcript.display());
        }
    }

    fn on_hook(&mut self, kind: &str, p: &serde_json::Value) {
        let session = p["session_id"].as_str().unwrap_or_default().to_string();
        if let Some(cwd) = p["cwd"].as_str().filter(|_| !session.is_empty()) {
            let name = Path::new(cwd).file_name().and_then(|n| n.to_str()).unwrap_or_default();
            self.projects.insert(session.clone(), name.to_string());
        }
        match kind {
            "user-prompt-submit" => {
                self.shared.queue.lock().unwrap().clear_session(&session);
                self.dedup.forget(&session);
                if self.focused().is_some_and(|f| f.id == session) {
                    self.shared.interrupt();
                }
            }
            "notification" => {
                let msg = p["message"].as_str().unwrap_or("Агент ждёт ответа");
                let prefix = if self.focused().is_some_and(|f| f.id == session) {
                    String::new()
                } else {
                    let cwd = p["cwd"].as_str().unwrap_or_default();
                    format!("{}: ", Path::new(cwd).file_name().and_then(|n| n.to_str()).unwrap_or("Агент"))
                };
                self.enqueue(Agent::Claude, &session, &format!("{prefix}{msg}"), Kind::Urgent);
            }
            "message-display" => {
                let Some(ev) = MdEvent::from_json(p) else {
                    eprintln!("agent-speak: непонятный MessageDisplay: {p}");
                    return;
                };
                self.dedup.mark_display(&ev.session, &ev.message_id);
                if !self.live() || !self.focused().is_some_and(|f| f.id == ev.session) {
                    self.asm.push(&ev); // держим буфер, чтобы не потерять при смене фокуса
                    return;
                }
                for s in self.asm.push(&ev) {
                    if self.dedup.first_time(&ev.session, &s) {
                        self.enqueue(Agent::Claude, &ev.session, &s, Kind::Status);
                    }
                }
            }
            "stop" => {
                let rest = self.asm.flush_session(&session);
                let Some(f) = self.focused().filter(|f| self.auto() && f.id == session) else { return };
                if self.cfg.read_intermediate {
                    for s in rest {
                        if self.dedup.first_time(&session, &s) {
                            self.enqueue(Agent::Claude, &session, &s, Kind::Status);
                        }
                    }
                } else if let Some(last) = final_message(p, &f.transcript) {
                    self.enqueue(Agent::Claude, &session, &last, Kind::Manual); // только финальный ответ
                }
            }
            "codex-notify" => {
                // в живом режиме весь текст уже пришёл из rollout; без промежуточных — читаем финальный
                let thread = p["thread-id"].as_str().unwrap_or_default().to_string();
                if self.auto() && !self.cfg.read_intermediate && self.focused().is_some_and(|f| f.id == thread) {
                    if let Some(last) = p["last-assistant-message"].as_str() {
                        self.enqueue(Agent::Codex, &thread, last, Kind::Manual);
                    }
                }
            }
            other => eprintln!("agent-speak: неизвестный хук {other}"),
        }
    }

    fn on_file(&mut self, path: &Path) {
        let lines = self.tail.read_new(path);
        if lines.is_empty() || !self.live() {
            return;
        }
        let Some((agent, id)) = focus::session_of(path) else { return };
        if !self.focused().is_some_and(|f| f.id == id) {
            return;
        }
        for line in lines {
            let blocks = match agent {
                Agent::Claude => claude::parse_line(&line),
                Agent::Codex => codex::parse_line(&line),
            };
            for b in blocks {
                if self.dedup.from_transcript_ok(&id, &b.message_id) && self.dedup.first_time(&id, &b.text) {
                    self.enqueue(agent.clone(), &id, &b.text, Kind::Status);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::terms::Terms;

    fn test_state() -> State {
        let shared = Arc::new(Shared::new(Queue::new(Duration::from_secs(30)), "xenia".into(), "medium".into()));
        State {
            cfg: Config::default(),
            shared,
            terms: Arc::new(Mutex::new(Terms::from_str(""))),
            learn: mpsc::channel().0,
            dedup: Dedup::new(),
            asm: Assembler::new(),
            tail: Tailer::new(),
            focus_cache: (Instant::now(), None), // свежий кэш: hyprctl в тестах не зовётся
            subs: Vec::new(),
            projects: HashMap::new(),
        }
    }

    fn next(r: &mut BufReader<UnixStream>) -> serde_json::Value {
        let mut l = String::new();
        r.read_line(&mut l).unwrap();
        serde_json::from_str(&l).unwrap()
    }

    fn reader(s: UnixStream) -> BufReader<UnixStream> {
        s.set_read_timeout(Some(Duration::from_millis(500))).unwrap();
        BufReader::new(s)
    }

    #[test]
    fn serve_reads_every_line_and_survives_garbage() {
        let (mut client, server) = UnixStream::pair().unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || serve(server, tx));
        client.write_all(b"{\"cmd\":\"subscribe\"}\n\xd0\xbc\xd1\x83\xd1\x81\xd0\xbe\xd1\x80\n{\"cmd\":\"nope\"}\n{\"cmd\":\"pause\"}\n").unwrap();
        assert!(matches!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), Event::Subscribe(_)));
        assert!(matches!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), Event::Msg(Msg::Pause)));
    }

    #[test]
    fn serve_drops_silent_connection() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let (server, _client) = UnixStream::pair().unwrap();
        let t = std::thread::spawn(move || serve(server, tx));
        let start = Instant::now();
        while !t.is_finished() && start.elapsed() < Duration::from_secs(3) {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(t.is_finished());
    }

    #[test]
    fn status_line_shape() {
        let st = test_state();
        assert_eq!(
            st.status_line(),
            "{\"running\":true,\"speaking\":false,\"paused\":false,\"mode\":\"manual\",\"read_intermediate\":true,\"speaker\":\"xenia\",\"rate\":\"medium\",\"project\":\"\"}\n"
        );
    }

    #[test]
    fn subscriber_gets_state_now_and_on_change() {
        let mut st = test_state();
        let (ours, theirs) = UnixStream::pair().unwrap();
        let mut r = reader(ours);
        st.subscribe(theirs);
        assert_eq!(next(&mut r)["paused"], false);
        st.on_msg(Msg::Pause);
        assert_eq!(next(&mut r)["paused"], true);
        st.shared.set_busy(Some("s"));
        st.broadcast(); // так цикл отвечает на Event::Busy
        assert_eq!(next(&mut r)["speaking"], true);
    }

    #[test]
    fn dead_subscriber_dropped_live_one_kept() {
        let mut st = test_state();
        let (dead, a) = UnixStream::pair().unwrap();
        let (live, b) = UnixStream::pair().unwrap();
        let mut r = reader(live);
        st.subscribe(a);
        st.subscribe(b);
        drop(dead);
        st.on_msg(Msg::Stop);
        assert_eq!(st.subs.len(), 1);
        next(&mut r); // при подписке
        next(&mut r); // после стопа
    }

    #[test]
    fn final_message_prefers_payload() {
        let p = serde_json::json!({"last_assistant_message": "Готово"});
        assert_eq!(final_message(&p, Path::new("/nonexistent")), Some("Готово".into()));
        assert_eq!(final_message(&serde_json::json!({"last_assistant_message": " "}), Path::new("/nonexistent")), None);
    }

    #[test]
    fn set_applies_saves_broadcasts_and_rejects_bad() {
        // единственный тест, трогающий HOME: config::path() читает его при каждом вызове
        let home = std::env::temp_dir().join(format!("agent-speak-set-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        unsafe { std::env::set_var("HOME", &home) };
        let mut st = test_state();
        let (ours, theirs) = UnixStream::pair().unwrap();
        let mut r = reader(ours);
        st.subscribe(theirs);
        next(&mut r);
        st.on_msg(Msg::Set { key: "speaker".into(), value: serde_json::json!("baya") });
        assert_eq!(next(&mut r)["speaker"], "baya");
        assert_eq!(st.shared.voice.lock().unwrap().0, "baya");
        let saved = std::fs::read_to_string(home.join(".config/agent-speak/config.toml")).unwrap();
        assert!(saved.contains("speaker = \"baya\""), "{saved}");
        st.on_msg(Msg::Set { key: "speaker".into(), value: serde_json::json!("nobody") });
        st.on_msg(Msg::Set { key: "read_intermediate".into(), value: serde_json::json!("false") });
        st.on_msg(Msg::Set { key: "volume".into(), value: serde_json::json!("loud") });
        let mut l = String::new();
        assert!(r.read_line(&mut l).is_err(), "неверный set ничего не рассылает: {l}");
        assert_eq!((st.cfg.speaker.as_str(), st.cfg.read_intermediate), ("baya", true));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn say_queues_manual_reading() {
        let mut st = test_state();
        st.on_msg(Msg::Say { text: "Так звучит этот голос.".into() });
        let it = st.shared.queue.lock().unwrap().pop(Instant::now()).unwrap();
        assert_eq!(it.kind, Kind::Manual);
        assert!(it.text.contains("голос"), "{}", it.text);
    }

    #[test]
    fn project_of_spoken_session_from_hook_cwd() {
        let mut st = test_state();
        st.on_msg(Msg::Hook {
            kind: "user-prompt-submit".into(),
            payload: serde_json::json!({"session_id": "s1", "cwd": "/home/u/Projects/agent-speak"}),
        });
        st.shared.set_busy(Some("s1"));
        let v: serde_json::Value = serde_json::from_str(&st.status_line()).unwrap();
        assert_eq!((v["speaking"].clone(), v["project"].clone()), (serde_json::json!(true), serde_json::json!("agent-speak")));
        st.shared.set_busy(None);
        let v: serde_json::Value = serde_json::from_str(&st.status_line()).unwrap();
        assert_eq!(v["project"], "");
    }
}
