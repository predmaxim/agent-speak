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
    Tick, // раз в секунду: заметить смену фокуса, даже когда больше ничего не происходит
}

/// Сколько фокус должен простоять на другом агенте, чтобы озвучка переключилась на него.
const SWITCH_AFTER: Duration = Duration::from_secs(5);

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
    focus_cache: Option<(Instant, Option<SessionRef>)>, // None — устарел
    focus_src: Box<dyn FnMut() -> Option<SessionRef>>, // окно в фокусе; в тестах — подделка
    clock: Box<dyn Fn() -> Instant>,
    active: Option<SessionRef>,          // озвучиваемая сессия
    candidate: Option<(String, Instant)>, // агент в фокусе, отличный от активного, и с какого момента
    subs: Vec<UnixStream>,
    seq: u64, // свежие ключи сообщений для фраз без своего id
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
    {
        let tx = tx.clone();
        std::thread::spawn(move || {
            while tx.send(Event::Tick).is_ok() {
                std::thread::sleep(Duration::from_secs(1));
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
        focus_cache: None,
        focus_src: Box::new(focus::focused),
        clock: Box::new(Instant::now),
        active: None,
        candidate: None,
        subs: Vec::new(),
        seq: 0,
        projects: HashMap::new(),
    };
    for ev in rx {
        st.poll_focus();
        match ev {
            Event::Tick => {}
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
        let now = (self.clock)();
        if !self.focus_cache.as_ref().is_some_and(|(t, _)| now.saturating_duration_since(*t) < Duration::from_millis(500)) {
            let f = (self.focus_src)();
            if let Some(f) = &f {
                self.projects.insert(f.id.clone(), f.project.clone());
            }
            self.focus_cache = Some((now, f));
        }
        self.focus_cache.as_ref().and_then(|(_, f)| f.clone())
    }

    /// Активная сессия меняется, только если фокус ≥ SWITCH_AFTER стоит на другом агенте;
    /// окна не-агентов не в счёт. Текущую фразу не обрываем — speaker сам возьмёт следующую.
    fn poll_focus(&mut self) {
        let now = (self.clock)();
        let Some(f) = self.focused() else {
            self.candidate = None;
            return;
        };
        if self.active.as_ref().is_some_and(|a| a.id == f.id) {
            self.candidate = None;
            return;
        }
        match &self.candidate {
            _ if self.active.is_none() => self.set_active(f),
            Some((id, since)) if *id == f.id => {
                if now.saturating_duration_since(*since) >= SWITCH_AFTER {
                    self.set_active(f);
                }
            }
            _ => self.candidate = Some((f.id, now)),
        }
    }

    fn set_active(&mut self, s: SessionRef) {
        eprintln!("agent-speak: озвучиваю {} ({})", s.project, s.id);
        let dropped = self.shared.queue.lock().unwrap().set_active(&s.id);
        for it in dropped {
            // не прозвучало — финал доберёт
            self.dedup.unrecord(&it.session, &it.text);
            eprintln!("agent-speak: статус пропущен при смене ({}): {}", it.session, it.text.chars().take(40).collect::<String>());
        }
        self.shared.cv.notify_all();
        self.active = Some(s);
        self.candidate = None;
    }

    fn is_active(&self, session: &str) -> bool {
        self.active.as_ref().is_some_and(|a| a.id == session)
    }

    /// Финальный ответ сессии ждёт в очереди или звучит — статусы за ним не ставим.
    fn final_pending(&self, session: &str) -> bool {
        let q = self.shared.queue.lock().unwrap();
        q.has_final(session)
            || (self.shared.busy.load(Ordering::SeqCst) && *self.shared.current.lock().unwrap() == session && q.popped_final(session))
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
        self.seq += 1;
        let msg = format!("#{}", self.seq); // одна постановка — одно сообщение
        self.enqueue_msg(agent, session, raw, kind, &msg, false)
    }

    /// Только ещё не поставленные в ход фразы сессии (живой текст и добор финала).
    fn enqueue_new(&mut self, agent: Agent, session: &str, raw: &str, kind: Kind) -> usize {
        self.seq += 1;
        let msg = format!("#{}", self.seq);
        self.enqueue_msg(agent, session, raw, kind, &msg, true)
    }

    /// only_new: пропустить уже поставленные фразы, поставленные — запомнить (dedup).
    fn enqueue_msg(&mut self, agent: Agent, session: &str, raw: &str, kind: Kind, msg: &str, only_new: bool) -> usize {
        let (sentences, unknown) = prepare(raw, &self.terms.lock().unwrap());
        for w in unknown {
            let _ = self.learn.send((agent.clone(), w));
        }
        let sentences: Vec<String> = if only_new { sentences.into_iter().filter(|t| !self.dedup.seen(session, t)).collect() } else { sentences };
        // статусы только вживую: не для активной сессии или за финалом — опоздали; финал доберёт
        if kind == Kind::Status && (!self.is_active(session) || self.final_pending(session)) {
            for t in &sentences {
                eprintln!("agent-speak: статус пропущен ({session}): {}", t.chars().take(40).collect::<String>());
            }
            return 0;
        }
        if only_new {
            for t in &sentences {
                self.dedup.first_time(session, t);
            }
        }
        let n = sentences.len();
        // образец — один элемент: каждый push Preview вытесняет прежний
        let sentences = if kind == Kind::Preview && !sentences.is_empty() { vec![sentences.join(" ")] } else { sentences };
        let mut q = self.shared.queue.lock().unwrap();
        for text in sentences {
            q.push(Item { session: session.into(), text, kind: kind.clone(), born: Instant::now(), msg: msg.into() });
        }
        drop(q);
        self.shared.cv.notify_all();
        n
    }

    /// Конец хода: вживую — добрать непрозвучавшие фразы (начало до активации, хвост после ухода);
    /// без промежуточных — весь финальный ответ.
    fn final_answer(&mut self, agent: Agent, session: &str, text: &str) {
        if self.cfg.read_intermediate {
            self.enqueue_new(agent, session, text, Kind::Manual);
        } else {
            self.enqueue(agent, session, text, Kind::Manual);
        }
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
                // говорит и не на паузе — смену голоса слышно в самом чтении, образец не нужен
                if self.shared.busy.load(Ordering::SeqCst) && !self.shared.queue.lock().unwrap().paused() {
                    eprintln!("agent-speak: say: образец пропущен, идёт чтение");
                } else {
                    self.enqueue(Agent::Claude, "say", &text, Kind::Preview);
                }
            }
            Msg::Subscribe => {} // приходит как Event::Subscribe
        }
        if changes {
            self.broadcast();
        }
    }

    fn read(&mut self) {
        // фразы неактивных сессий ждут в очереди — чтению не мешают
        let busy = self.shared.busy.load(Ordering::SeqCst) || self.shared.queue.lock().unwrap().ready();
        if busy {
            self.stop();
            eprintln!("agent-speak: read: остановлено");
            return;
        }
        self.focus_cache = None; // ручное чтение — по свежему фокусу
        let Some(s) = self.focused() else {
            eprintln!("agent-speak: read: нет агента в фокусе");
            notice("В фокусе нет агента", "");
            return;
        };
        if !self.is_active(&s.id) {
            self.set_active(s.clone()); // без выдержки: пользователь сам попросил
        }
        self.shared.queue.lock().unwrap().clear_session(&s.id); // ждавший финал — часть читаемого хода
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
                // обрываем только речь этой же сессии
                if self.shared.busy.load(Ordering::SeqCst) && *self.shared.current.lock().unwrap() == session {
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
                if !self.live() || !self.is_active(&ev.session) {
                    self.asm.push(&ev); // держим буфер; неактивной сессии финал придёт в Stop
                    return;
                }
                for s in self.asm.push(&ev) {
                    self.enqueue_msg(Agent::Claude, &ev.session, &s, Kind::Status, &ev.message_id, true);
                }
            }
            "stop" => {
                let rest = self.asm.flush_session(&session);
                if !self.auto() {
                    return;
                }
                if self.cfg.read_intermediate {
                    for s in rest {
                        self.enqueue_new(Agent::Claude, &session, &s, Kind::Status); // хвост вживую
                    }
                }
                let transcript = PathBuf::from(p["transcript_path"].as_str().unwrap_or_default());
                if let Some(last) = final_message(p, &transcript) {
                    self.final_answer(Agent::Claude, &session, &last);
                }
            }
            "codex-notify" => {
                let thread = p["thread-id"].as_str().unwrap_or_default().to_string();
                if let Some(last) = p["last-assistant-message"].as_str().filter(|_| self.auto()) {
                    self.final_answer(Agent::Codex, &thread, last);
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
        // Claude вживую — только хук MessageDisplay: id там UUID, в транскрипте msg_…, тексты не совпадают
        if agent == Agent::Claude {
            return;
        }
        if !self.is_active(&id) {
            return;
        }
        for line in lines {
            let blocks = match agent {
                Agent::Claude => claude::parse_line(&line),
                Agent::Codex => codex::parse_line(&line),
            };
            for b in blocks {
                self.enqueue_new(agent.clone(), &id, &b.text, Kind::Status);
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
            focus_cache: None,
            focus_src: Box::new(|| None), // hyprctl в тестах не зовётся
            clock: Box::new(Instant::now),
            active: None,
            candidate: None,
            subs: Vec::new(),
            seq: 0,
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

    fn say(st: &mut State) {
        st.on_msg(Msg::Say { text: "Так звучит этот голос.".into() });
    }

    #[test]
    fn say_queues_preview_when_idle() {
        let mut st = test_state();
        say(&mut st);
        let it = st.shared.queue.lock().unwrap().pop(Instant::now()).unwrap();
        assert_eq!(it.kind, Kind::Preview);
        assert!(it.text.contains("голос"), "{}", it.text);
    }

    #[test]
    fn say_ignored_while_speaking_not_paused() {
        let mut st = test_state();
        st.shared.set_busy(Some("s"));
        say(&mut st);
        assert!(st.shared.queue.lock().unwrap().is_empty());
    }

    #[test]
    fn say_queued_while_paused_and_plays_on_pause() {
        let mut st = test_state();
        st.shared.set_busy(Some("s"));
        st.shared.pause();
        say(&mut st);
        say(&mut st); // второй образец вытесняет первый
        let mut q = st.shared.queue.lock().unwrap();
        assert_eq!(q.pop(Instant::now()).unwrap().kind, Kind::Preview);
        assert!(q.pop(Instant::now()).is_none() && q.paused());
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

    #[test]
    fn live_claude_speaks_only_from_message_display_not_transcript() {
        let dir = std::env::temp_dir().join(format!("agent-speak-live-{}/.claude/projects/p", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("sess1.jsonl");
        std::fs::write(&path, "").unwrap();
        let mut st = test_state();
        st.cfg.mode = "auto".into();
        st.set_active(SessionRef { agent: Agent::Claude, id: "sess1".into(), transcript: path.clone(), project: "p".into() });
        st.on_file(&path); // регистрирует смещение
        let line = r#"{"type":"assistant","message":{"id":"msg_1","content":[{"type":"text","text":"Привет, это текст."}]}}"#;
        std::fs::write(&path, format!("{line}\n")).unwrap();
        st.on_file(&path);
        assert!(st.shared.queue.lock().unwrap().is_empty(), "транскрипт Claude в живом режиме не читается");
        st.on_hook("message-display", &serde_json::json!({"session_id": "sess1", "message_id": "uuid-1", "delta": "Привет, это текст. ", "final": true}));
        assert!(!st.shared.queue.lock().unwrap().is_empty());
    }

    // --- фокус и активная сессия: подставные окно в фокусе и часы ---

    type Env = (Arc<Mutex<Option<SessionRef>>>, Arc<Mutex<Instant>>);

    fn sref(id: &str) -> SessionRef {
        SessionRef { agent: Agent::Claude, id: id.into(), transcript: PathBuf::from("/nonexistent"), project: id.into() }
    }

    fn env_state() -> (State, Env) {
        let mut st = test_state();
        st.cfg.mode = "auto".into();
        let focus: Arc<Mutex<Option<SessionRef>>> = Default::default();
        let now = Arc::new(Mutex::new(Instant::now()));
        let (f, n) = (focus.clone(), now.clone());
        st.focus_src = Box::new(move || f.lock().unwrap().clone());
        st.clock = Box::new(move || *n.lock().unwrap());
        (st, (focus, now))
    }

    /// Фокус на окно (None — не агент) и прошло secs секунд, затем опрос.
    fn at(st: &mut State, env: &Env, focus: Option<&str>, secs: u64) {
        *env.0.lock().unwrap() = focus.map(sref);
        *env.1.lock().unwrap() += Duration::from_secs(secs);
        st.poll_focus();
    }

    fn active(st: &State) -> Option<String> {
        st.active.as_ref().map(|a| a.id.clone())
    }

    fn texts(st: &State) -> Vec<String> {
        let mut q = st.shared.queue.lock().unwrap();
        std::iter::from_fn(|| q.pop(Instant::now())).map(|i| i.text).collect()
    }

    fn md(st: &mut State, session: &str, text: &str) {
        st.on_hook("message-display", &serde_json::json!({"session_id": session, "message_id": "m", "delta": text, "final": true}));
    }

    #[test]
    fn active_switches_only_after_5s_on_other_agent() {
        let (mut st, env) = env_state();
        at(&mut st, &env, Some("a"), 0);
        assert_eq!(active(&st).as_deref(), Some("a")); // активной не было — сразу
        at(&mut st, &env, Some("b"), 1);
        at(&mut st, &env, Some("b"), 4);
        assert_eq!(active(&st).as_deref(), Some("a"));
        at(&mut st, &env, Some("b"), 1);
        assert_eq!(active(&st).as_deref(), Some("b"));
    }

    #[test]
    fn short_visits_and_non_agent_windows_keep_active() {
        let (mut st, env) = env_state();
        at(&mut st, &env, Some("a"), 0);
        at(&mut st, &env, Some("b"), 1);
        at(&mut st, &env, Some("a"), 3);
        at(&mut st, &env, Some("b"), 1);
        at(&mut st, &env, Some("b"), 3);
        assert_eq!(active(&st).as_deref(), Some("a"));
        at(&mut st, &env, None, 1);
        at(&mut st, &env, None, 60);
        assert_eq!(active(&st).as_deref(), Some("a"));
    }

    #[test]
    fn read_switches_active_immediately() {
        let dir = std::env::temp_dir().join(format!("agent-speak-read-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("b.jsonl");
        std::fs::write(&path, r#"{"type":"assistant","message":{"id":"msg_1","content":[{"type":"text","text":"Ответ бэ."}]}}"#).unwrap();
        let (mut st, env) = env_state();
        at(&mut st, &env, Some("a"), 0);
        *env.0.lock().unwrap() = Some(SessionRef { transcript: path, ..sref("b") });
        st.on_msg(Msg::Read);
        assert_eq!(active(&st).as_deref(), Some("b"));
        assert_eq!(texts(&st), vec!["Ответ бэ."]);
    }

    #[test]
    fn status_of_non_active_session_dropped() {
        let (mut st, env) = env_state();
        at(&mut st, &env, Some("a"), 0);
        md(&mut st, "b", "Статус бэ. ");
        md(&mut st, "a", "Статус а. ");
        assert_eq!(texts(&st), vec!["Статус а."]);
    }

    #[test]
    fn status_not_queued_behind_final_of_same_session() {
        let (mut st, env) = env_state();
        at(&mut st, &env, Some("a"), 0);
        st.enqueue(Agent::Claude, "a", "Финал.", Kind::Manual);
        md(&mut st, "a", "Поздний статус. ");
        assert_eq!(texts(&st), vec!["Финал."]); // финал «играет» после pop
        st.shared.set_busy(Some("a"));
        md(&mut st, "a", "Ещё статус. ");
        assert!(texts(&st).is_empty());
        st.shared.set_busy(None);
        md(&mut st, "a", "Новый статус. ");
        assert_eq!(texts(&st), vec!["Новый статус."]);
    }

    #[test]
    fn final_of_non_active_waits_until_focused() {
        let (mut st, env) = env_state();
        at(&mut st, &env, Some("a"), 0);
        md(&mut st, "b", "Живой текст бэ. ");
        st.on_hook("stop", &serde_json::json!({"session_id": "b", "last_assistant_message": "Готово, бэ."}));
        assert!(texts(&st).is_empty());
        at(&mut st, &env, Some("b"), 1);
        at(&mut st, &env, Some("b"), 5);
        assert_eq!(texts(&st), vec!["Готово, бэ."]);
    }

    #[test]
    fn stop_of_active_live_session_does_not_repeat_final() {
        let (mut st, env) = env_state();
        at(&mut st, &env, Some("a"), 0);
        md(&mut st, "a", "Готово. ");
        st.on_hook("stop", &serde_json::json!({"session_id": "a", "last_assistant_message": "Готово."}));
        assert_eq!(texts(&st), vec!["Готово."]);
    }

    #[test]
    fn codex_final_of_non_active_queued() {
        let (mut st, env) = env_state();
        at(&mut st, &env, Some("a"), 0);
        st.on_hook("codex-notify", &serde_json::json!({"thread-id": "x", "last-assistant-message": "Кодекс готов."}));
        st.on_hook("codex-notify", &serde_json::json!({"thread-id": "a", "last-assistant-message": "Живой уже прочитан."}));
        at(&mut st, &env, Some("x"), 1);
        at(&mut st, &env, Some("x"), 5);
        assert_eq!(texts(&st), vec!["Кодекс готов."]);
    }

    #[test]
    fn switch_back_continues_where_left() {
        let (mut st, env) = env_state();
        at(&mut st, &env, Some("a"), 0);
        st.enqueue(Agent::Claude, "a", "Раз. Два.", Kind::Manual);
        assert_eq!(st.shared.queue.lock().unwrap().pop(Instant::now()).unwrap().text, "Раз.");
        at(&mut st, &env, Some("b"), 1);
        at(&mut st, &env, Some("b"), 5);
        assert!(texts(&st).is_empty());
        at(&mut st, &env, Some("a"), 1);
        at(&mut st, &env, Some("a"), 5);
        assert_eq!(texts(&st), vec!["Два."]);
    }

    #[test]
    fn prompt_submit_interrupts_only_own_speech() {
        let (mut st, env) = env_state();
        at(&mut st, &env, Some("b"), 0);
        st.shared.set_busy(Some("a"));
        let g = || st.shared.generation.load(Ordering::SeqCst);
        let g0 = g();
        st.on_hook("user-prompt-submit", &serde_json::json!({"session_id": "b"}));
        assert_eq!(st.shared.generation.load(Ordering::SeqCst), g0);
        st.on_hook("user-prompt-submit", &serde_json::json!({"session_id": "a"}));
        assert_eq!(st.shared.generation.load(Ordering::SeqCst), g0 + 1);
    }

    #[test]
    fn urgent_from_any_session_plays() {
        let (mut st, env) = env_state();
        at(&mut st, &env, Some("a"), 0);
        st.on_hook("notification", &serde_json::json!({"session_id": "b", "cwd": "/p/проект", "message": "Нужно разрешение"}));
        assert_eq!(texts(&st), vec!["проект: Нужно разрешение."]);
    }

    #[test]
    fn read_does_not_repeat_queued_final_of_same_session() {
        let dir = std::env::temp_dir().join(format!("agent-speak-read2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("b.jsonl");
        std::fs::write(&path, r#"{"type":"assistant","message":{"id":"msg_1","content":[{"type":"text","text":"Ответ бэ."}]}}"#).unwrap();
        let (mut st, env) = env_state();
        at(&mut st, &env, Some("a"), 0);
        st.on_hook("stop", &serde_json::json!({"session_id": "b", "last_assistant_message": "Ответ бэ."}));
        *env.0.lock().unwrap() = Some(SessionRef { transcript: path, ..sref("b") });
        st.on_msg(Msg::Read);
        assert_eq!(texts(&st), vec!["Ответ бэ."]);
    }

    fn md_id(st: &mut State, session: &str, id: &str, text: &str) {
        st.on_hook("message-display", &serde_json::json!({"session_id": session, "message_id": id, "delta": text, "final": true}));
    }

    fn pop1(st: &State) -> String {
        st.shared.queue.lock().unwrap().pop(Instant::now()).unwrap().text
    }

    #[test]
    fn became_active_mid_turn_gets_missing_beginning_at_stop() {
        let (mut st, env) = env_state();
        at(&mut st, &env, Some("a"), 0);
        md_id(&mut st, "b", "m1", "Раз. "); // b ещё не активна — выброшено
        at(&mut st, &env, Some("b"), 1);
        at(&mut st, &env, Some("b"), 5);
        md_id(&mut st, "b", "m2", "Два. ");
        assert_eq!(pop1(&st), "Два.");
        st.on_hook("stop", &serde_json::json!({"session_id": "b", "last_assistant_message": "Раз.\n\nДва."}));
        assert_eq!(texts(&st), vec!["Раз."]);
    }

    #[test]
    fn lost_active_mid_answer_queues_only_unspoken_at_stop() {
        let (mut st, env) = env_state();
        at(&mut st, &env, Some("b"), 0);
        md_id(&mut st, "b", "m1", "Раз. ");
        md_id(&mut st, "b", "m2", "Два. ");
        assert_eq!(pop1(&st), "Раз.");
        at(&mut st, &env, Some("a"), 1);
        at(&mut st, &env, Some("a"), 5); // «Два.» не начато — выброшено
        st.on_hook("stop", &serde_json::json!({"session_id": "b", "last_assistant_message": "Раз.\n\nДва.\n\nТри."}));
        at(&mut st, &env, Some("b"), 1);
        at(&mut st, &env, Some("b"), 5);
        assert_eq!(texts(&st), vec!["Два.", "Три."]);
    }

    #[test]
    fn fully_spoken_live_queues_nothing_at_stop() {
        let (mut st, env) = env_state();
        at(&mut st, &env, Some("a"), 0);
        md_id(&mut st, "a", "m1", "Раз. Два. ");
        assert_eq!(texts(&st), vec!["Раз.", "Два."]);
        st.on_hook("stop", &serde_json::json!({"session_id": "a", "last_assistant_message": "Раз. Два."}));
        assert!(texts(&st).is_empty());
    }
}
