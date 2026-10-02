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
use std::io::BufRead;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

enum Event {
    Msg(Msg),
    File(PathBuf),
    Config,
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
    let sock = ipc::socket_path();
    let _ = std::fs::remove_file(&sock);
    let listener = UnixListener::bind(&sock).expect("bind agent-speak.sock");
    {
        let tx = tx.clone();
        std::thread::spawn(move || {
            for conn in listener.incoming().flatten() {
                let _ = conn.set_read_timeout(Some(Duration::from_secs(1)));
                let mut line = String::new();
                if std::io::BufReader::new(conn).read_line(&mut line).is_ok() {
                    match serde_json::from_str::<Msg>(&line) {
                        Ok(m) => {
                            let _ = tx.send(Event::Msg(m));
                        }
                        Err(e) => eprintln!("agent-speak: плохое сообщение: {e}"),
                    }
                }
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
    };
    for ev in rx {
        match ev {
            Event::Msg(m) => st.on_msg(m),
            Event::File(p) => st.on_file(&p),
            Event::Config => st.reload(),
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

impl State {
    fn focused(&mut self) -> Option<SessionRef> {
        if self.focus_cache.0.elapsed() > Duration::from_millis(500) {
            self.focus_cache = (Instant::now(), focus::focused());
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
        *self.shared.voice.lock().unwrap() = (self.cfg.speaker.clone(), self.cfg.rate.clone());
        self.shared.queue.lock().unwrap().set_max_age(Duration::from_secs(self.cfg.max_age_secs));
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
                if self.auto() {
                    notice("Режим: авто", "Агент в фокусе читается по ходу работы");
                } else {
                    notice("Режим: вручную", "Чтение по хоткею");
                }
            }
            Msg::Read => self.read(),
            Msg::Hook { kind, payload } => self.on_hook(&kind, &payload),
        }
    }

    fn read(&mut self) {
        let busy = self.shared.busy.load(Ordering::SeqCst) || !self.shared.queue.lock().unwrap().is_empty();
        if busy {
            self.stop();
            eprintln!("agent-speak: read: остановлено");
            notice("Чтение остановлено", "");
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
            notice("Читаю…", &joined.chars().take(120).collect::<String>());
        }
    }

    fn on_hook(&mut self, kind: &str, p: &serde_json::Value) {
        let session = p["session_id"].as_str().unwrap_or_default().to_string();
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

    #[test]
    fn final_message_prefers_payload() {
        let p = serde_json::json!({"last_assistant_message": "Готово"});
        assert_eq!(final_message(&p, Path::new("/nonexistent")), Some("Готово".into()));
        assert_eq!(final_message(&serde_json::json!({"last_assistant_message": " "}), Path::new("/nonexistent")), None);
    }
}
