//! Цикл событий сервиса: команды и хуки из сокета, изменения транскриптов (inotify).

use crate::agents::{self, Live, Registry};
use crate::assemble::{Assembler, MdEvent};
use crate::config::{hhmm, in_window, Config};
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
use std::process::{Child, ChildStdin};
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

/// Как часто сверять реестр агентов с диском и /proc.
const SCAN_EVERY: Duration = Duration::from_secs(2);

/// Не запустилась — пробуем снова через OMA_RETRY.
const OMA_RETRY: Duration = Duration::from_secs(10);

/// Упавшую Ому поднимаем не сразу и всё реже (с): падающая на старте не крутится в цикле.
const OMA_BACKOFF: [u64; 4] = [10, 30, 60, 300];

/// Прожила столько — падение случайное, отсрочка снова с начала.
const OMA_STABLE: Duration = Duration::from_secs(60);

/// Строка состояния для подписчиков (плагин панели). Порядок полей — протокол.
#[derive(Serialize)]
struct Status<'a> {
    running: bool,
    speaking: bool,
    paused: bool,
    read_intermediate: bool,
    speaker: &'a str,
    rate: &'a str,
    project: &'a str,
    oma: &'a str,
    talking: bool,
    agents_voice: bool,
    quiet: bool,
    quiet_schedule: bool,
    quiet_from: &'a str,
    quiet_to: &'a str,
    orb_pos: &'a str,
    waiting: usize,
}

/// Процесс Омы (python -m agent_voice): команды — строками в stdin, события — через сокет.
#[derive(Default)]
struct Oma {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    state: String,
    talking: bool,             // идёт разговор с Омой (для окна)
    retry_at: Option<Instant>, // упала или не запустилась — когда поднять
    failing: bool,             // запуск не удаётся: в журнал — только первая ошибка подряд
    started: Option<Instant>,  // когда запущена нынешняя
    crashes: usize,            // падений подряд — шаг OMA_BACKOFF
}

/// Что звучит: тихий режим глушит всё, кроме чтения по хоткею и образца голоса;
/// статусы и финалы агентов — только при agents_voice; срочное и ответы Омы — всегда.
pub fn audible(kind: &Kind, agents: bool, quiet: bool) -> bool {
    match kind {
        Kind::Read | Kind::Preview => true,
        _ if quiet => false,
        Kind::Status | Kind::Manual => agents,
        Kind::Urgent | Kind::Voice => true,
    }
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
    voices: Vec<(String, String)>, // сессия → голос в порядке выдачи; первая держит голос по умолчанию
    seq: u64, // свежие ключи сообщений для фраз без своего id
    projects: HashMap<String, String>, // сессия → имя папки проекта (из хуков и окна в фокусе)
    oma: Oma,
    voice_cmd: Vec<String>, // запуск Омы; в тестах — заглушка
    minutes: Box<dyn Fn() -> u32>, // местное время, минуты от полуночи; в тестах — 12:00
    sched: Option<(bool, u32, u64)>, // прошлый tick: (тихо по расписанию, минуты, эпоха в с); None — ещё не было
    registry: Registry,
    scan: Box<dyn FnMut() -> Vec<Live>>, // живые сессии агентов; в тестах — подделка
    now_secs: Box<dyn Fn() -> u64>,      // эпоха в секундах — время событий реестра
    next_scan: Instant,
    unknown_scan_at: Instant, // раньше — сессия не из реестра пересканирования не вызывает
    agents_sent: String, // последняя разосланная строка агентов
    waiting_sent: usize, // waiting последней разосланной строки состояния
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
        voices: Vec::new(),
        seq: 0,
        projects: HashMap::new(),
        oma: Oma { state: "off".into(), ..Default::default() },
        voice_cmd: vec![
            data.join("voice-venv/bin/python").to_string_lossy().into(),
            "-m".into(),
            "agent_voice".into(),
        ],
        minutes: Box::new(|| {
            // SAFETY: localtime_r потокобезопасна, tm — локальный буфер
            let mut tm: libc::tm = unsafe { std::mem::zeroed() };
            unsafe { libc::localtime_r(&libc::time(std::ptr::null_mut()), &mut tm) };
            (tm.tm_hour * 60 + tm.tm_min) as u32
        }),
        sched: None,
        registry: Registry::new(),
        scan: Box::new(agents::scan_all),
        now_secs: Box::new(|| std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())),
        next_scan: Instant::now(),
        unknown_scan_at: Instant::now(),
        agents_sent: String::new(),
        waiting_sent: 0,
    };
    st.oma_spawn();
    for ev in rx {
        st.poll_focus();
        match ev {
            Event::Tick => st.tick(),
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

/// Хвост транскрипта: последний вызов инструмента всегда в конце, весь файл (мегабайты) читать незачем.
fn tail_of(path: &str) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let mut buf = Vec::new();
    if let Ok(mut f) = std::fs::File::open(path) {
        let _ = f.seek(SeekFrom::End(-256 * 1024)).or_else(|_| f.seek(SeekFrom::Start(0)));
        let _ = f.read_to_end(&mut buf);
    }
    String::from_utf8_lossy(&buf).into_owned()
}

/// Известные уведомления Claude Code по-русски. Незнакомые — как есть (транслитом):
/// так слышно, что текст новый и его пора добавить сюда.
/// Общий «Claude needs your permission» шлётся на любой диалог, даже на вопрос, — что именно ждёт,
/// видно по последнему инструменту в транскрипте (`last_tool`).
fn notification_text(msg: &str, last_tool: impl FnOnce() -> Option<String>) -> String {
    let tool = match msg {
        "Claude needs your permission" => last_tool(),
        _ => msg.strip_prefix("Claude needs your permission to use ").map(str::to_string),
    };
    if let Some(tool) = tool {
        return match tool.as_str() {
            "AskUserQuestion" => "Клод задал вопрос".into(),
            "ExitPlanMode" => "Клоду нужно одобрение плана".into(),
            t => format!("Клоду нужно разрешение на {}", match t {
                "Bash" => "команду",
                "Edit" | "MultiEdit" => "правку файла",
                "Write" => "запись файла",
                "Read" => "чтение файла",
                "NotebookEdit" => "правку блокнота",
                "Glob" | "Grep" => "поиск по файлам",
                "WebFetch" => "загрузку страницы",
                "WebSearch" => "поиск в интернете",
                "Agent" | "Task" => "запуск агента",
                "Skill" => "запуск навыка",
                "Workflow" => "запуск сценария",
                // незнакомое — транслитом; у mcp__server__tool только имя без служебных частей
                t => t.rsplit("__").next().unwrap_or(t),
            }),
        };
    }
    if let Some(what) = msg.strip_prefix("Claude needs your permission to reply to comments on ") {
        return format!("Клоду нужно разрешение ответить на комментарии в {what}");
    }
    match msg {
        "" => "Клод ждёт ответа",
        "Claude is waiting for your input" => "Клод ждёт ответа",
        "Claude needs your input" | "Claude Code needs your input" => "Клоду нужен ответ",
        "Claude needs your permission" => "Клоду нужно разрешение",
        "Claude Code needs your approval for the plan" => "Клоду нужно одобрение плана",
        "Claude needs your approval for a review artifact" => "Клоду нужно одобрение артефакта",
        _ => msg,
    }
    .into()
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
        let _ = conn.set_read_timeout(None); // первая строка пришла — постоянное соединение (agent_voice) живёт молча
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

    /// Ручное чтение ждёт или звучит — живые статусы молчат.
    fn reading(&self) -> bool {
        let (queued, popped) = self.shared.queue.lock().unwrap().reading();
        queued || (popped && self.shared.busy.load(Ordering::SeqCst))
    }

    /// Финальный ответ сессии ждёт в очереди или звучит — статусы за ним не ставим.
    fn final_pending(&self, session: &str) -> bool {
        let q = self.shared.queue.lock().unwrap();
        q.has_final(session)
            || (self.shared.busy.load(Ordering::SeqCst) && *self.shared.current.lock().unwrap() == session && q.popped_final(session))
    }

    /// Агенты звучат (настройка agents_voice).
    fn auto(&self) -> bool {
        self.cfg.agents_voice
    }

    /// Промежуточные статусы читаются по ходу работы (agents_voice + read_intermediate).
    fn live(&self) -> bool {
        self.cfg.agents_voice && self.cfg.read_intermediate
    }

    fn quiet(&self) -> bool {
        self.cfg.quiet((self.minutes)())
    }

    /// Очередь — только то, что слышно сейчас: выключен голос агентов или настала тишина — лишнее выбросить.
    /// Звучащая фраза доигрывает.
    fn purge(&self) {
        self.shared.queue.lock().unwrap().retain_audible(self.cfg.agents_voice, self.quiet());
    }

    /// Удержание фраз агентов, пока Ома думает или говорит; снятие будит поток воспроизведения.
    fn hold(&self, on: bool) {
        self.shared.queue.lock().unwrap().set_hold(on);
        self.shared.cv.notify_all();
    }

    /// config.toml изменён (интерфейсом или руками) — применить без перезапуска.
    fn reload(&mut self) {
        self.cfg = Config::load();
        self.apply();
    }

    /// Настройки → поток воспроизведения и очередь, затем — подписчикам.
    fn apply(&mut self) {
        *self.shared.voice.lock().unwrap() = (self.cfg.speaker.clone(), self.cfg.rate.clone());
        self.reassign_default();
        self.shared.queue.lock().unwrap().set_max_age(Duration::from_secs(self.cfg.max_age_secs));
        self.purge(); // могла включиться тишина
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
            read_intermediate: self.cfg.read_intermediate,
            speaker: &self.cfg.speaker,
            rate: &self.cfg.rate,
            project: &project,
            oma: &self.oma.state,
            talking: self.oma.talking,
            agents_voice: self.cfg.agents_voice,
            quiet: self.quiet(),
            quiet_schedule: self.cfg.quiet_schedule,
            quiet_from: &self.cfg.quiet_from,
            quiet_to: &self.cfg.quiet_to,
            orb_pos: &self.cfg.orb_pos,
            waiting: self.registry.waiting(),
        };
        format!("{}\n", serde_json::to_string(&st).unwrap())
    }

    /// Новому подписчику — строка состояния, затем строка агентов.
    fn subscribe(&mut self, mut s: UnixStream) {
        let _ = s.set_write_timeout(Some(Duration::from_millis(100))); // зависший подписчик не держит цикл
        if s.write_all((self.status_line() + &self.registry.line()).as_bytes()).is_ok() {
            self.subs.push(s);
        }
    }

    /// Состояние всем подписчикам.
    fn broadcast(&mut self) {
        let line = self.status_line();
        self.waiting_sent = self.registry.waiting();
        self.broadcast_line(&line);
    }

    /// Строку (с \n) всем подписчикам; отвалившиеся удаляются молча.
    /// Сокет закрывается целиком: копию держит ещё поток serve, а клиенту нужен EOF, чтобы переподключиться.
    fn broadcast_line(&mut self, line: &str) {
        self.subs.retain_mut(|s| {
            let ok = s.write_all(line.as_bytes()).is_ok();
            if !ok {
                let _ = s.shutdown(std::net::Shutdown::Both);
            }
            ok
        });
    }

    /// Строка агентов — подписчикам, если отличается от последней разосланной;
    /// сменилось число ждущих — ещё и строка состояния (в ней waiting для орба).
    fn push_agents(&mut self) {
        let line = self.registry.line();
        if line != self.agents_sent {
            self.broadcast_line(&line);
            self.agents_sent = line;
        }
        if self.registry.waiting() != self.waiting_sent {
            self.broadcast();
        }
    }

    /// Состав живых агентов — с диска и из /proc.
    fn rescan(&mut self) {
        let live = (self.scan)();
        self.registry.sync(live, (self.now_secs)());
        self.push_agents();
    }

    fn bubble(&mut self, id: &str, who: &str, text: &str, kind: &str, append: bool) {
        let text = crate::text::clean::plain(text); // разметка markdown пузырю не нужна
        let b = serde_json::json!({"bubble": {"id": id, "who": who, "text": text, "kind": kind, "append": append, "drop": false}});
        self.broadcast_line(&format!("{b}\n"));
    }

    /// Имя агента для пузыря; сессии не из реестра (не пользователя) пузырей не дают.
    fn who(&self, session: &str) -> Option<String> {
        self.registry.get(session).map(|e| e.title.clone())
    }

    /// Конец хода в реестре; фоновой сессии — пузырь «готово», активной — недосказанный хвост.
    fn turn_done(&mut self, session: &str, rest: &[String], last: &str) {
        self.registry.finished(session, last, (self.now_secs)());
        let Some(who) = self.who(session) else { return };
        if !self.is_active(session) {
            self.bubble(&format!("f-{session}"), &who, "готово", "final", false);
        } else if !rest.is_empty() {
            self.seq += 1;
            self.bubble(&format!("r-{}", self.seq), &who, &rest.join(" "), "status", false);
        }
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
        if !audible(&kind, self.cfg.agents_voice, self.quiet()) {
            eprintln!("agent-speak: пропущено (агенты: {}, тихо: {}): {}", self.cfg.agents_voice, self.quiet(), raw.chars().take(40).collect::<String>());
            return 0;
        }
        let (sentences, unknown) = prepare(raw, &self.terms.lock().unwrap());
        for w in unknown {
            let _ = self.learn.send((agent.clone(), w));
        }
        let all = sentences.clone();
        let sentences: Vec<String> = if only_new { sentences.into_iter().filter(|t| !self.dedup.seen(session, t)).collect() } else { sentences };
        // статусы только вживую: не для активной сессии или за финалом — опоздали; финал доберёт
        if kind == Kind::Status && self.reading() {
            for t in &sentences {
                eprintln!("agent-speak: статус пропущен: идёт ручное чтение ({session}): {}", t.chars().take(40).collect::<String>());
            }
            return 0;
        }
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
        let speaker = if matches!(kind, Kind::Preview | Kind::Voice) { String::new() } else { self.voice_of(session) };
        let item = |text: String| Item { session: session.into(), text, kind: kind.clone(), born: Instant::now(), msg: msg.into(), speaker: speaker.clone() };
        let mut q = self.shared.queue.lock().unwrap();
        if only_new && kind == Kind::Manual {
            // добор финала — на свои места среди ещё не прозвучавших фраз хода
            q.fill(all.into_iter().map(|t| (sentences.contains(&t), item(t))).collect());
        } else {
            for text in sentences {
                q.push(item(text));
            }
        }
        drop(q);
        self.shared.cv.notify_all();
        n
    }

    /// Голос сессии: первая — по умолчанию, новые — следующий свободный из SPEAKERS,
    /// все заняты — тот, что выдавался давнее всех.
    fn voice_of(&mut self, session: &str) -> String {
        if let Some((_, sp)) = self.voices.iter().find(|(s, _)| s == session) {
            return sp.clone();
        }
        let default = self.cfg.speaker.clone();
        let others = crate::config::SPEAKERS.iter().map(|s| s.to_string()).filter(|s| *s != default);
        let last = |sp: &String| self.voices.iter().rposition(|(_, v)| v == sp);
        let sp = if self.voices.is_empty() { default } else { others.min_by_key(|sp| last(sp).map_or(0, |i| i + 1)).unwrap() };
        self.voices.push((session.to_string(), sp.clone()));
        sp
    }

    /// Новый голос по умолчанию — первой сессии; кто держал его — получает прежний.
    fn reassign_default(&mut self) {
        let new = self.cfg.speaker.clone();
        let Some(old) = self.voices.first().map(|(_, sp)| sp.clone()).filter(|o| *o != new) else { return };
        for (i, (_, sp)) in self.voices.iter_mut().enumerate() {
            if i == 0 {
                *sp = new.clone();
            } else if *sp == new {
                *sp = old.clone();
            }
        }
    }

    /// Конец хода: вживую — добрать непрозвучавшие фразы (начало до активации, хвост после ухода);
    /// без промежуточных — весь финальный ответ.
    fn final_answer(&mut self, agent: Agent, session: &str, text: &str) {
        if self.cfg.read_intermediate {
            self.enqueue_new(agent, session, text, Kind::Manual);
        } else {
            self.enqueue(agent, session, text, Kind::Manual);
        }
        self.dedup.forget(session); // ход закончен: те же фразы в следующем звучат снова
    }

    fn stop(&mut self) {
        self.shared.stop();
    }

    /// Запуск Омы: EOF её stdin (демон умер) — Ома выходит сама.
    fn oma_spawn(&mut self) {
        self.oma.retry_at = None;
        let data = home().join(".local/share/agent-speak");
        let _ = std::fs::create_dir_all(&data); // current_dir должен существовать
        let child = std::process::Command::new(&self.voice_cmd[0])
            .args(&self.voice_cmd[1..])
            .current_dir(&data)
            .env("AGENT_SPEAK", "1") // хуки сессий Омы до демона не доходят
            .stdin(std::process::Stdio::piped())
            .spawn();
        match child {
            Ok(mut c) => {
                eprintln!("agent-speak: Ома запущена (pid {})", c.id());
                self.oma.stdin = c.stdin.take();
                self.oma.child = Some(c);
                self.oma.failing = false;
                self.oma.started = Some((self.clock)());
            }
            Err(e) => {
                if !self.oma.failing {
                    eprintln!("agent-speak: Ома не запустилась, пробую раз в {} с: {e}", OMA_RETRY.as_secs());
                }
                self.oma.failing = true;
                self.oma.retry_at = Some((self.clock)() + OMA_RETRY);
            }
        }
    }

    /// Команда Оме — строкой JSON в stdin; не пишется — Ома считается упавшей.
    fn oma_send(&mut self, action: &serde_json::Value) {
        let Some(w) = self.oma.stdin.as_mut() else {
            eprintln!("agent-speak: Ома не запущена, команда пропущена: {action}");
            return;
        };
        if let Err(e) = w.write_all(format!("{action}\n").as_bytes()) {
            eprintln!("agent-speak: Ома не принимает команды: {e}");
            self.oma_down();
        }
    }

    /// Ома упала: выключена, диалога и удержания нет; поднять через OMA_BACKOFF.
    fn oma_down(&mut self) {
        let now = (self.clock)();
        if self.oma.started.take().is_some_and(|t| now.saturating_duration_since(t) >= OMA_STABLE) {
            self.oma.crashes = 0;
        }
        let wait = OMA_BACKOFF[self.oma.crashes.min(OMA_BACKOFF.len() - 1)];
        self.oma.crashes += 1;
        eprintln!("agent-speak: Ома поднимется через {wait} с");
        self.oma.stdin = None; // EOF — если ещё жива, выйдет сама
        if let Some(mut c) = self.oma.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        self.oma.state = "off".into();
        self.oma.talking = false;
        self.hold(false);
        self.purge();
        self.oma.retry_at = Some(now + Duration::from_secs(wait));
        self.broadcast();
    }

    /// Раз в секунду: Ома упала — выключить, срок вышел — поднять; граница тихого расписания.
    /// Раз в SCAN_EVERY: состав агентов, «готово» → «простаивает», агент в фокусе.
    fn tick(&mut self) {
        let dead = self.oma.child.as_mut().is_some_and(|c| !matches!(c.try_wait(), Ok(None)));
        if dead {
            eprintln!("agent-speak: Ома завершилась");
            self.oma_down();
        }
        let now = (self.clock)();
        if self.oma.retry_at.is_some_and(|t| now >= t) {
            self.oma_spawn();
        }
        let (min, secs) = ((self.minutes)(), (self.now_secs)());
        let sched = self.cfg.scheduled_quiet(min);
        if let Some((was, pmin, psecs)) = self.sched {
            // ноутбук спал: tick не шёл, граница могла пройти и дважды — значение то же
            let gap = secs.saturating_sub(psecs);
            let slept_over = self.cfg.quiet_schedule
                && gap > 60
                && (gap >= 24 * 3600 || [&self.cfg.quiet_from, &self.cfg.quiet_to].into_iter().filter_map(|b| hhmm(b)).any(|b| in_window(pmin, min, b)));
            if was != sched || slept_over {
                // граница расписания: ручное переключение снимается
                let before = match self.cfg.quiet_override.as_str() {
                    "on" => true,
                    "off" => false,
                    _ => was,
                };
                if !self.cfg.quiet_override.is_empty() {
                    eprintln!("agent-speak: граница расписания — ручной тихий режим ({}) снят", self.cfg.quiet_override);
                    self.cfg.quiet_override.clear();
                    self.cfg.save();
                }
                if self.quiet() != before {
                    self.purge();
                    self.broadcast();
                }
            }
        }
        self.sched = Some((sched, min, secs));
        if now >= self.next_scan {
            self.next_scan = now + SCAN_EVERY;
            self.rescan();
            self.registry.tick((self.now_secs)());
            let focused = self.focused().map(|f| f.id);
            self.registry.set_focused(focused.as_deref());
            self.push_agents();
        }
    }

    fn on_msg(&mut self, m: Msg) {
        let changes = matches!(m, Msg::Stop | Msg::Hush | Msg::Pause | Msg::VoiceState { .. });
        match m {
            Msg::Stop => self.stop(),
            Msg::Hush => self.shared.hush(), // финалы агентов, удержанные за Омой, не теряются
            Msg::Voice { text } => {
                self.enqueue(Agent::Claude, "voice", &text, Kind::Voice); // звучит ли — решает audible
            }
            Msg::VoiceState { state, talking } => {
                self.hold(matches!(state.as_str(), "thinking" | "speaking"));
                self.oma.state = state;
                self.oma.talking = talking;
                self.purge();
            }
            Msg::Oma { action } => self.oma_send(&action),
            Msg::Emit { line } => self.broadcast_line(&format!("{line}\n")),
            Msg::Pause => {
                let paused = self.shared.queue.lock().unwrap().paused();
                if paused {
                    self.shared.resume();
                } else {
                    self.shared.pause();
                }
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
        if self.enqueue(s.agent.clone(), &s.id, &joined, Kind::Read) == 0 {
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
        // новая сессия ещё не в реестре — пересканировать до обработки, не чаще SCAN_EVERY:
        // `claude -p` и SDK с хуками пользователя в реестр не попадают никогда, а MessageDisplay идёт на каждый кусок
        // ponytail: общий срок на все незнакомые сессии — новая сессия в первые 2 с после чужой ждёт tick
        let id = p["session_id"].as_str().or(p["thread-id"].as_str()).unwrap_or_default();
        let at = (self.clock)();
        if !id.is_empty() && self.registry.get(id).is_none() && at >= self.unknown_scan_at {
            self.unknown_scan_at = at + SCAN_EVERY;
            self.rescan();
        }
        let now = (self.now_secs)();
        match kind {
            "user-prompt-submit" => {
                self.registry.prompt(&session, p["prompt"].as_str().unwrap_or_default(), now);
                // новый ход ничего не обрывает: финалы, чтение и начатые сообщения дочитываются
                let (dropped, kept) = self.shared.queue.lock().unwrap().new_turn(&session);
                for t in dropped {
                    eprintln!("agent-speak: статус пропущен: новое сообщение ({session}): {}", t.chars().take(40).collect::<String>());
                }
                self.dedup.forget(&session);
                for t in kept {
                    self.dedup.first_time(&session, &t); // оставшееся не доберётся повторно в следующем Stop
                }
            }
            "notification" => {
                // проект называем всегда, даже для окна в фокусе: сразу ясно, где ждут
                let transcript = p["transcript_path"].as_str().unwrap_or_default();
                let raw = p["message"].as_str().unwrap_or_default();
                let tail = tail_of(transcript);
                let msg = notification_text(raw, || claude::last_tool(&tail));
                let cwd = p["cwd"].as_str().unwrap_or_default();
                let project = Path::new(cwd).file_name().and_then(|n| n.to_str()).unwrap_or("Агент");
                self.enqueue(Agent::Claude, &session, &format!("{project}: {msg}"), Kind::Urgent);
                // «ждёт ввода» — простой, а не вопрос: реестр и пузыри не трогаем
                if !matches!(raw, "" | "Claude is waiting for your input") {
                    let text = claude::last_question(&tail).map_or(msg, |q| format!("вопрос: {q}"));
                    self.registry.question(&session, &text, now);
                    if let Some(who) = self.who(&session) {
                        self.bubble(&format!("q-{session}"), &who, &text, "question", false);
                    }
                }
            }
            "message-display" => {
                let Some(ev) = MdEvent::from_json(p) else {
                    eprintln!("agent-speak: непонятный MessageDisplay: {p}");
                    return;
                };
                // предложения — всегда в ленту, у активной ещё и в пузырь;
                // голосом — только активная вживую, неактивной финал придёт в Stop
                let active = self.is_active(&ev.session);
                let speak = active && self.live();
                let who = self.who(&ev.session).filter(|_| active);
                for s in self.asm.push(&ev) {
                    self.registry.status(&ev.session, &s, now);
                    if let Some(who) = &who {
                        self.bubble(&ev.message_id, who, &s, "status", true);
                    }
                    if speak {
                        self.enqueue_msg(Agent::Claude, &ev.session, &s, Kind::Status, &ev.message_id, true);
                    }
                }
            }
            "stop" => {
                let rest = self.asm.flush_session(&session);
                let transcript = PathBuf::from(p["transcript_path"].as_str().unwrap_or_default());
                let last = final_message(p, &transcript);
                self.turn_done(&session, &rest, last.as_deref().unwrap_or_default());
                if self.auto() {
                    if self.cfg.read_intermediate {
                        for s in rest {
                            self.enqueue_new(Agent::Claude, &session, &s, Kind::Status); // хвост вживую
                        }
                    }
                    if let Some(last) = last {
                        self.final_answer(Agent::Claude, &session, &last);
                    }
                }
            }
            "codex-notify" => {
                let thread = p["thread-id"].as_str().unwrap_or_default().to_string();
                let last = p["last-assistant-message"].as_str();
                self.turn_done(&thread, &[], last.unwrap_or_default());
                if let Some(last) = last.filter(|_| self.auto()) {
                    self.final_answer(Agent::Codex, &thread, last);
                }
            }
            other => eprintln!("agent-speak: неизвестный хук {other}"),
        }
        self.push_agents();
    }

    fn on_file(&mut self, path: &Path) {
        let lines = self.tail.read_new(path);
        if lines.is_empty() {
            return;
        }
        let Some((agent, id)) = focus::session_of(path) else { return };
        // Claude вживую — только хук MessageDisplay: id там UUID, в транскрипте msg_…, тексты не совпадают
        if agent == Agent::Claude {
            return;
        }
        // сообщения — в ленту; у активной ещё в пузырь и, вживую, голосом
        let active = self.is_active(&id);
        let speak = active && self.live();
        let who = self.who(&id).filter(|_| active);
        let now = (self.now_secs)();
        for line in lines {
            let blocks = match agent {
                Agent::Claude => claude::parse_line(&line),
                Agent::Codex => codex::parse_line(&line),
            };
            for b in blocks {
                self.registry.status(&id, &b.text, now);
                if let Some(who) = &who {
                    self.seq += 1;
                    self.bubble(&format!("r-{}", self.seq), who, &b.text, "status", false);
                }
                if speak {
                    self.enqueue_new(agent.clone(), &id, &b.text, Kind::Status);
                }
            }
        }
        self.push_agents();
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
            voices: Vec::new(),
            seq: 0,
        projects: HashMap::new(),
            oma: Oma { state: "off".into(), ..Default::default() },
            voice_cmd: vec!["sh".into(), "-c".into(), "sleep 30".into(), "x".into()],
            minutes: Box::new(|| 720), // 12:00 — вне тихого расписания
            registry: Registry::new(),
            scan: Box::new(Vec::new),
            now_secs: Box::new(|| 1000),
            next_scan: Instant::now(),
            unknown_scan_at: Instant::now(),
            agents_sent: String::new(),
            waiting_sent: 0,
            sched: None,
        }
    }

    static HOME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Тесты, доходящие до cfg.save()/oma_spawn: под замком, HOME — временная папка, по выходу восстановлен.
    struct TempHome {
        _lock: std::sync::MutexGuard<'static, ()>,
        old: Option<std::ffi::OsString>,
        dir: PathBuf,
    }

    impl TempHome {
        fn new(tag: &str) -> TempHome {
            let lock = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let dir = std::env::temp_dir().join(format!("agent-speak-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            let old = std::env::var_os("HOME");
            unsafe { std::env::set_var("HOME", &dir) };
            TempHome { _lock: lock, old, dir }
        }
    }

    impl Drop for TempHome {
        fn drop(&mut self) {
            match &self.old {
                Some(h) => unsafe { std::env::set_var("HOME", h) },
                None => unsafe { std::env::remove_var("HOME") },
            }
            let _ = std::fs::remove_dir_all(&self.dir);
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
            "{\"running\":true,\"speaking\":false,\"paused\":false,\"read_intermediate\":true,\"speaker\":\"xenia\",\"rate\":\"medium\",\"project\":\"\",\"oma\":\"off\",\"talking\":false,\"agents_voice\":false,\"quiet\":false,\"quiet_schedule\":true,\"quiet_from\":\"23:00\",\"quiet_to\":\"08:00\",\"orb_pos\":\"bottom-right,0,0\",\"waiting\":0}\n"
        );
    }

    #[test]
    fn voice_text_queued_with_default_voice() {
        let mut st = test_state();
        st.on_msg(Msg::Voice { text: "Привет, слушаю.".into() });
        let it = st.shared.queue.lock().unwrap().pop(Instant::now()).unwrap();
        assert_eq!(it.kind, Kind::Voice);
        assert_eq!(it.speaker, "");
    }

    #[test]
    fn voice_text_plays_without_agents_voice() {
        let mut st = test_state();
        assert!(!st.cfg.agents_voice);
        st.on_msg(Msg::Voice { text: "Ответ Омы здесь.".into() });
        assert!(!st.shared.queue.lock().unwrap().is_empty());
    }

    #[test]
    fn voice_state_broadcast() {
        let mut st = test_state();
        let (ours, theirs) = UnixStream::pair().unwrap();
        let mut r = reader(ours);
        st.subscribe(theirs);
        assert_eq!(next(&mut r)["oma"], "off");
        next(&mut r); // строка агентов
        st.on_msg(Msg::VoiceState { state: "thinking".into(), talking: true });
        let s = next(&mut r);
        assert_eq!((s["oma"].as_str(), s["talking"].as_bool()), (Some("thinking"), Some(true)));
    }

    #[test]
    fn audible_table() {
        use Kind::*;
        for k in [Read, Preview] { assert!(audible(&k, false, true)); }
        for k in [Voice, Urgent] { assert!(audible(&k, false, false) && !audible(&k, true, true)); }
        for k in [Status, Manual] { assert!(!audible(&k, false, false) && audible(&k, true, false) && !audible(&k, true, true)); }
    }

    #[test]
    fn quiet_mutes_all_but_read_and_preview() {
        let mut st = test_state();
        st.cfg.quiet_override = "on".into();
        st.on_msg(Msg::Voice { text: "Ответ Омы здесь.".into() });
        st.on_hook("notification", &serde_json::json!({"session_id": "s", "message": "Claude needs your permission", "cwd": "/p/x"}));
        assert!(st.shared.queue.lock().unwrap().is_empty());
        st.on_msg(Msg::Say { text: "Так звучит этот голос.".into() });
        assert!(!st.shared.queue.lock().unwrap().is_empty());
    }

    #[test]
    fn override_cleared_at_schedule_boundary() {
        let _h = TempHome::new("quiet");
        let mut st = test_state();
        let m = Arc::new(std::sync::atomic::AtomicU32::new(crate::config::hhmm("12:00").unwrap()));
        let mm = m.clone();
        st.minutes = Box::new(move || mm.load(Ordering::SeqCst));
        st.cfg.quiet_override = "on".into();
        st.tick();
        m.store(crate::config::hhmm("12:01").unwrap(), Ordering::SeqCst);
        st.tick();
        assert_eq!(st.cfg.quiet_override, "on");
        m.store(crate::config::hhmm("23:00").unwrap(), Ordering::SeqCst);
        st.tick();
        assert_eq!(st.cfg.quiet_override, "");
        assert!(st.quiet());
    }

    #[test]
    fn override_cleared_when_boundary_crossed_in_sleep() {
        let _h = TempHome::new("quiet-sleep");
        let mut st = test_state();
        let hm = |s| crate::config::hhmm(s).unwrap();
        let m = Arc::new(std::sync::atomic::AtomicU32::new(hm("21:00")));
        let secs = Arc::new(std::sync::atomic::AtomicU64::new(1000));
        let (mm, ss) = (m.clone(), secs.clone());
        st.minutes = Box::new(move || mm.load(Ordering::SeqCst));
        st.now_secs = Box::new(move || ss.load(Ordering::SeqCst));
        st.cfg.quiet_override = "on".into();
        st.tick();
        secs.fetch_add(1, Ordering::SeqCst); // обычный tick, ничего не пересёк
        st.tick();
        assert_eq!(st.cfg.quiet_override, "on");
        m.store(hm("22:00"), Ordering::SeqCst); // долгий сон без границы
        secs.fetch_add(3600, Ordering::SeqCst);
        st.tick();
        assert_eq!(st.cfg.quiet_override, "on");
        m.store(hm("09:00"), Ordering::SeqCst); // проспал 23:00 и 08:00: расписание то же — false
        secs.fetch_add(11 * 3600, Ordering::SeqCst);
        st.tick();
        assert_eq!(st.cfg.quiet_override, "");
        assert!(!st.quiet());
    }

    #[test]
    fn agents_voice_off_drops_held_final() {
        let (mut st, env) = env_state();
        at(&mut st, &env, Some("s1"), 0);
        st.on_msg(Msg::VoiceState { state: "thinking".into(), talking: true });
        st.on_hook("stop", &serde_json::json!({"session_id": "s1", "last_assistant_message": "Финал агента готов."}));
        assert!(!st.shared.queue.lock().unwrap().is_empty());
        st.on_msg(Msg::Set { key: "agents_voice".into(), value: serde_json::json!(false) });
        assert!(texts(&st).is_empty());
    }

    #[test]
    fn quiet_on_drops_queued_keeps_preview() {
        let _h = TempHome::new("quiet-purge");
        let mut st = test_state();
        st.enqueue(Agent::Claude, "a", "Нужно разрешение.", Kind::Urgent);
        st.on_msg(Msg::Voice { text: "Ответ Омы здесь.".into() });
        say(&mut st);
        st.on_msg(Msg::Set { key: "quiet_override".into(), value: serde_json::json!("on") });
        assert_eq!(texts(&st), ["Так звучит этот голос."]);
    }

    #[test]
    fn failed_spawn_retried_after_backoff() {
        let _h = TempHome::new("oma-fail");
        let (mut st, env) = env_state();
        st.voice_cmd = vec!["/nonexistent/agent-voice".into()];
        st.oma_spawn();
        assert!(st.oma.child.is_none() && st.oma.retry_at.is_some());
        st.voice_cmd = vec!["true".into()];
        *env.1.lock().unwrap() += Duration::from_secs(11);
        st.tick();
        assert!(st.oma.child.is_some());
    }

    #[test]
    fn oma_actions_go_to_child_stdin() {
        let h = TempHome::new("oma-stdin");
        std::fs::create_dir_all(&h.dir).unwrap();
        let out = h.dir.join("in.txt");
        let mut st = test_state();
        st.voice_cmd = vec!["sh".into(), "-c".into(), format!("cat > {}", out.display())];
        st.oma_spawn();
        st.on_msg(Msg::Oma { action: serde_json::json!({"do": "talk"}) });
        drop(st); // stdin закрыт → cat допишет и выйдет
        let end = Instant::now() + Duration::from_secs(2);
        while std::fs::read_to_string(&out).unwrap_or_default().is_empty() && Instant::now() < end { std::thread::sleep(Duration::from_millis(20)); }
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "{\"do\":\"talk\"}\n");
    }

    #[test]
    fn voice_state_sets_talking_and_hold() {
        let mut st = test_state();
        st.on_msg(Msg::VoiceState { state: "thinking".into(), talking: true });
        assert!(st.oma.talking && st.shared.queue.lock().unwrap().held());
        st.on_msg(Msg::VoiceState { state: "listening".into(), talking: true });
        assert!(!st.shared.queue.lock().unwrap().held());
    }

    #[test]
    fn emit_broadcast_verbatim() {
        let mut st = test_state();
        let mut r = sub(&mut st);
        next(&mut r);
        next(&mut r);
        st.on_msg(Msg::Emit { line: serde_json::json!({"chat": {"key": "new", "messages": []}}) });
        assert_eq!(next(&mut r)["chat"]["key"], "new");
    }

    #[test]
    fn subscriber_gets_state_now_and_on_change() {
        let mut st = test_state();
        let (ours, theirs) = UnixStream::pair().unwrap();
        let mut r = reader(ours);
        st.subscribe(theirs);
        assert_eq!(next(&mut r)["paused"], false);
        next(&mut r); // строка агентов
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
        // одного drop мало: соседний тест запускает процесс, и до exec копия дескриптора живёт в его fork
        dead.shutdown(std::net::Shutdown::Both).unwrap();
        drop(dead);
        st.on_msg(Msg::Stop);
        assert_eq!(st.subs.len(), 1);
        next(&mut r); // при подписке
        next(&mut r); // строка агентов
        next(&mut r); // после стопа
    }

    #[test]
    fn failed_subscriber_shut_down_so_client_reconnects() {
        use std::io::Read;
        let mut st = test_state();
        let (mut ours, theirs) = UnixStream::pair().unwrap();
        let _serve = theirs.try_clone().unwrap(); // как в serve: исходный дескриптор живёт в потоке соединения
        st.subscribe(theirs);
        let big = format!("{}\n", "x".repeat(64 * 1024));
        for _ in 0..100 {
            if st.subs.is_empty() {
                break;
            }
            st.broadcast_line(&big); // клиент не читает — буфер забит, запись упирается в таймаут
        }
        assert!(st.subs.is_empty());
        ours.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let mut buf = Vec::new();
        assert!(ours.read_to_end(&mut buf).is_ok(), "клиент получил EOF, а не тишину");
    }

    #[test]
    fn waiting_change_resends_status_line() {
        let mut st = test_state();
        st.cfg.quiet_override = "on".into(); // тишина: звука нет — Busy строку состояния не пришлёт
        with_agents(&mut st, &["s2"]);
        let mut r = sub(&mut st);
        next(&mut r);
        next(&mut r);
        let waiting = |r: &mut BufReader<UnixStream>| -> Vec<u64> {
            drain(r).into_iter().filter(|v| v.get("running").is_some()).map(|v| v["waiting"].as_u64().unwrap()).collect()
        };
        st.on_hook("notification", &serde_json::json!({"session_id": "s2", "message": "Claude needs your permission to use Bash", "cwd": "/p/s2"}));
        assert_eq!(waiting(&mut r), [1]);
        st.on_hook("user-prompt-submit", &serde_json::json!({"session_id": "s2", "prompt": "да"}));
        assert_eq!(waiting(&mut r), [0]);
    }

    #[test]
    fn hush_cuts_oma_keeps_held_final_of_focused_agent() {
        let (mut st, env) = env_state();
        at(&mut st, &env, Some("s1"), 0);
        st.on_msg(Msg::VoiceState { state: "speaking".into(), talking: true });
        st.on_hook("stop", &serde_json::json!({"session_id": "s1", "last_assistant_message": "Финал агента готов."}));
        st.on_msg(Msg::Voice { text: "Ответ Омы дальше.".into() });
        let g0 = st.shared.generation.load(Ordering::SeqCst);
        st.on_msg(Msg::Hush);
        assert!(st.shared.generation.load(Ordering::SeqCst) > g0, "звучащая фраза оборвана");
        st.on_msg(Msg::VoiceState { state: "listening".into(), talking: true });
        assert_eq!(texts(&st), ["Финал агента готов."]);
    }

    #[test]
    fn oma_respawn_backs_off_and_resets_after_long_life() {
        let _h = TempHome::new("oma-backoff");
        let (mut st, env) = env_state();
        st.voice_cmd = vec!["true".into()];
        let now = |env: &Env| *env.1.lock().unwrap();
        // Ома умерла: через сколько секунд её поднимут; потом поднять
        let crash = |st: &mut State, env: &Env, lived: u64| -> u64 {
            *env.1.lock().unwrap() += Duration::from_secs(lived);
            let end = Instant::now() + Duration::from_secs(2);
            while st.oma.child.is_some() && Instant::now() < end {
                std::thread::sleep(Duration::from_millis(20));
                st.tick();
            }
            let wait = st.oma.retry_at.unwrap().saturating_duration_since(now(env)).as_secs();
            *env.1.lock().unwrap() += Duration::from_secs(wait - 1);
            st.tick();
            assert!(st.oma.child.is_none(), "раньше срока ({wait} с)");
            *env.1.lock().unwrap() += Duration::from_secs(1);
            st.tick();
            assert!(st.oma.child.is_some(), "в срок ({wait} с)");
            wait
        };
        st.oma_spawn();
        let waits: Vec<u64> = (0..5).map(|_| crash(&mut st, &env, 0)).collect();
        assert_eq!(waits, [10, 30, 60, 300, 300]);
        assert_eq!(crash(&mut st, &env, 60), 10); // прожила минуту — снова 10 с
        assert_eq!(crash(&mut st, &env, 0), 30);
    }

    #[test]
    fn final_message_prefers_payload() {
        let p = serde_json::json!({"last_assistant_message": "Готово"});
        assert_eq!(final_message(&p, Path::new("/nonexistent")), Some("Готово".into()));
        assert_eq!(final_message(&serde_json::json!({"last_assistant_message": " "}), Path::new("/nonexistent")), None);
    }

    #[test]
    fn set_applies_saves_broadcasts_and_rejects_bad() {
        let _home = TempHome::new("set"); // config::path() читает HOME при каждом вызове
        let mut st = test_state();
        let (ours, theirs) = UnixStream::pair().unwrap();
        let mut r = reader(ours);
        st.subscribe(theirs);
        next(&mut r);
        next(&mut r); // строка агентов
        st.on_msg(Msg::Set { key: "speaker".into(), value: serde_json::json!("baya") });
        assert_eq!(next(&mut r)["speaker"], "baya");
        assert_eq!(st.shared.voice.lock().unwrap().0, "baya");
        let saved = std::fs::read_to_string(_home.dir.join(".config/agent-speak/config.toml")).unwrap();
        assert!(saved.contains("speaker = \"baya\""), "{saved}");
        st.on_msg(Msg::Set { key: "speaker".into(), value: serde_json::json!("nobody") });
        st.on_msg(Msg::Set { key: "read_intermediate".into(), value: serde_json::json!("false") });
        st.on_msg(Msg::Set { key: "volume".into(), value: serde_json::json!("loud") });
        let mut l = String::new();
        assert!(r.read_line(&mut l).is_err(), "неверный set ничего не рассылает: {l}");
        assert_eq!((st.cfg.speaker.as_str(), st.cfg.read_intermediate), ("baya", true));
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
        st.cfg.agents_voice = true;
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
        st.cfg.agents_voice = true;
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
    fn statuses_spoken_by_agents_voice() {
        let (mut st, env) = env_state();
        st.cfg.agents_voice = false;
        at(&mut st, &env, Some("s1"), 0);
        md(&mut st, "s1", "Запускаю тесты сейчас. ");
        assert!(texts(&st).is_empty());
        st.oma.talking = true; // разговор сам по себе агентов не озвучивает
        md(&mut st, "s1", "Ещё один статус тут. ");
        assert!(texts(&st).is_empty());
        st.cfg.agents_voice = true;
        md(&mut st, "s1", "Тесты прошли успешно. ");
        assert_eq!(texts(&st), ["Тесты прошли успешно."]);
    }

    #[test]
    fn dead_oma_respawned_after_backoff() {
        let _h = TempHome::new("oma-dead"); // oma_spawn создаёт папку в HOME
        let (mut st, env) = env_state();
        st.voice_cmd = vec!["true".into()];
        st.oma_spawn();
        st.on_msg(Msg::VoiceState { state: "thinking".into(), talking: true });
        std::thread::sleep(Duration::from_millis(200));
        st.tick();
        assert!(st.oma.child.is_none() && st.oma.state == "off");
        assert!(!st.oma.talking && !st.shared.queue.lock().unwrap().held());
        *env.1.lock().unwrap() += Duration::from_secs(9);
        st.tick();
        assert!(st.oma.child.is_none());
        *env.1.lock().unwrap() += Duration::from_secs(2);
        st.tick();
        assert!(st.oma.child.is_some());
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
    fn prompt_submit_never_interrupts() {
        let (mut st, env) = env_state();
        at(&mut st, &env, Some("b"), 0);
        st.shared.set_busy(Some("a"));
        let g = || st.shared.generation.load(Ordering::SeqCst);
        let g0 = g();
        st.on_hook("user-prompt-submit", &serde_json::json!({"session_id": "b"}));
        assert_eq!(st.shared.generation.load(Ordering::SeqCst), g0);
        st.on_hook("user-prompt-submit", &serde_json::json!({"session_id": "a"}));
        assert_eq!(st.shared.generation.load(Ordering::SeqCst), g0);
    }

    #[test]
    fn notification_text_is_russian() {
        let t = |m: &str| notification_text(m, || None);
        let with = |tool: &str| notification_text("Claude needs your permission", || Some(tool.into()));
        assert_eq!(with("AskUserQuestion"), "Клод задал вопрос");
        assert_eq!(with("ExitPlanMode"), "Клоду нужно одобрение плана");
        assert_eq!(with("Edit"), "Клоду нужно разрешение на правку файла");
        assert_eq!(t("Claude needs your permission"), "Клоду нужно разрешение");
        assert_eq!(t("Claude needs your permission to use AskUserQuestion"), "Клод задал вопрос");
        assert_eq!(t("Claude needs your permission to use Bash"), "Клоду нужно разрешение на команду");
        assert_eq!(t("Claude needs your permission to use mcp__srv__query"), "Клоду нужно разрешение на query");
        assert_eq!(t("Claude is waiting for your input"), "Клод ждёт ответа");
        assert_eq!(t("Claude Code needs your approval for the plan"), "Клоду нужно одобрение плана");
        assert_eq!(t("Claude Code needs your input"), "Клоду нужен ответ");
        assert_eq!(t(""), "Клод ждёт ответа");
        assert_eq!(t("Something new"), "Something new"); // незнакомое — транслитом
    }

    #[test]
    fn notification_names_project_even_in_focus() {
        let (mut st, env) = env_state();
        at(&mut st, &env, Some("a"), 0);
        st.on_hook("notification", &serde_json::json!({"session_id": "a", "cwd": "/p/проект", "message": "Claude is waiting for your input"}));
        assert_eq!(texts(&st), vec!["проект: Клод ждёт ответа."]);
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

    #[test]
    fn gap_fill_plays_before_queued_rest_of_turn() {
        let (mut st, env) = env_state();
        at(&mut st, &env, Some("a"), 0);
        md_id(&mut st, "b", "m1", "Раз. ");
        at(&mut st, &env, Some("b"), 1);
        at(&mut st, &env, Some("b"), 5);
        md_id(&mut st, "b", "m2", "Два. Три. ");
        assert_eq!(pop1(&st), "Два.");
        st.on_hook("stop", &serde_json::json!({"session_id": "b", "last_assistant_message": "Раз.\n\nДва.\n\nТри.\n\nЧетыре."}));
        assert_eq!(texts(&st), vec!["Раз.", "Три.", "Четыре."]);
    }

    #[test]
    fn same_sentence_spoken_again_next_codex_turn() {
        let (mut st, env) = env_state();
        at(&mut st, &env, Some("x"), 0);
        st.enqueue_new(Agent::Codex, "x", "Готово.", Kind::Status);
        assert_eq!(texts(&st), vec!["Готово."]);
        st.on_hook("codex-notify", &serde_json::json!({"thread-id": "x", "last-assistant-message": "Готово."}));
        assert!(texts(&st).is_empty());
        st.enqueue_new(Agent::Codex, "x", "Готово.", Kind::Status);
        assert_eq!(texts(&st), vec!["Готово."]);
    }

    /// Ручное чтение в сессии a (фокус на a), затем фокус ≥ 5 с на b — активной стала b.
    fn read_then_switch(text: &str) -> (State, Env) {
        let dir = std::env::temp_dir().join(format!("agent-speak-pin-{}-{}", std::process::id(), text.len()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.jsonl");
        let line = serde_json::json!({"type":"assistant","message":{"id":"msg_1","content":[{"type":"text","text": text}]}});
        std::fs::write(&path, line.to_string()).unwrap();
        let (mut st, env) = env_state();
        *env.0.lock().unwrap() = Some(SessionRef { transcript: path, ..sref("a") });
        st.on_msg(Msg::Read);
        at(&mut st, &env, Some("b"), 1);
        at(&mut st, &env, Some("b"), 5);
        assert_eq!(active(&st).as_deref(), Some("b"));
        (st, env)
    }

    #[test]
    fn read_is_pinned_across_switch_and_blocks_statuses() {
        let (mut st, _env) = read_then_switch("Раз. Два. Три.");
        assert_eq!(pop1(&st), "Раз.");
        st.shared.set_busy(Some("a"));
        md_id(&mut st, "b", "m1", "Статус бэ. ");
        st.on_hook("stop", &serde_json::json!({"session_id": "b", "last_assistant_message": "Финал бэ."}));
        assert_eq!(texts(&st), vec!["Два.", "Три.", "Финал бэ."]);
    }

    #[test]
    fn stop_ends_pinned_read_prompt_submit_does_not() {
        let (mut st, _env) = read_then_switch("Раз. Два.");
        st.on_hook("user-prompt-submit", &serde_json::json!({"session_id": "b"}));
        assert_eq!(pop1(&st), "Раз.");
        st.on_hook("user-prompt-submit", &serde_json::json!({"session_id": "a"}));
        assert_eq!(texts(&st), vec!["Два."]);

        let (mut st, _env) = read_then_switch("Раз. Два.");
        st.on_msg(Msg::Read); // повторный read — стоп
        assert!(texts(&st).is_empty());

        let (mut st, _env) = read_then_switch("Раз. Два.");
        st.on_msg(Msg::Stop);
        assert!(texts(&st).is_empty());
    }

    #[test]
    fn prompt_submit_keeps_playing_final_and_next_stop_adds_only_new() {
        let (mut st, env) = env_state();
        at(&mut st, &env, Some("a"), 0);
        st.on_hook("stop", &serde_json::json!({"session_id": "a", "last_assistant_message": "Раз.\n\nДва."}));
        assert_eq!(pop1(&st), "Раз.");
        st.shared.set_busy(Some("a"));
        let g0 = st.shared.generation.load(Ordering::SeqCst);
        st.on_hook("user-prompt-submit", &serde_json::json!({"session_id": "a"}));
        assert_eq!(st.shared.generation.load(Ordering::SeqCst), g0);
        md_id(&mut st, "a", "n1", "Новый статус. "); // за финалом — выброшен
        st.on_hook("stop", &serde_json::json!({"session_id": "a", "last_assistant_message": "Два.\n\nНовый ответ."}));
        assert_eq!(texts(&st), vec!["Два.", "Новый ответ."]);
    }

    #[test]
    fn prompt_submit_drops_unstarted_statuses_finishes_started() {
        let (mut st, env) = env_state();
        at(&mut st, &env, Some("a"), 0);
        md_id(&mut st, "a", "m1", "Раз. Два. ");
        md_id(&mut st, "a", "m2", "Три. ");
        assert_eq!(pop1(&st), "Раз.");
        st.on_hook("user-prompt-submit", &serde_json::json!({"session_id": "a"}));
        assert_eq!(texts(&st), vec!["Два."]);
    }

    /// (сессия, голос) всех фраз очереди по порядку.
    fn voices(st: &State) -> Vec<(String, String)> {
        let mut q = st.shared.queue.lock().unwrap();
        std::iter::from_fn(|| q.pop(Instant::now())).map(|i| (i.session, i.speaker)).collect()
    }

    fn v(s: &str, sp: &str) -> (String, String) {
        (s.into(), sp.into())
    }

    #[test]
    fn each_session_gets_own_stable_voice() {
        let mut st = test_state(); // голос по умолчанию xenia
        st.cfg.agents_voice = true;
        st.enqueue(Agent::Claude, "a", "Раз.", Kind::Manual);
        st.enqueue(Agent::Claude, "b", "Два.", Kind::Manual);
        st.enqueue(Agent::Claude, "a", "Три.", Kind::Urgent);
        assert_eq!(voices(&st), vec![v("a", "xenia"), v("a", "xenia"), v("b", "baya")]);
    }

    #[test]
    fn voices_wrap_around_least_recently_assigned() {
        let mut st = test_state();
        st.cfg.agents_voice = true;
        for s in ["a", "b", "c", "d", "e", "f", "g"] {
            st.enqueue(Agent::Claude, s, "Раз.", Kind::Manual);
        }
        let got: Vec<String> = voices(&st).into_iter().map(|(_, sp)| sp).collect();
        assert_eq!(got, vec!["xenia", "baya", "kseniya", "aidar", "eugene", "baya", "kseniya"]);
    }

    #[test]
    fn default_change_moves_first_session_and_frees_old_default() {
        let mut st = test_state();
        st.cfg.agents_voice = true;
        st.enqueue(Agent::Claude, "a", "Раз.", Kind::Manual);
        st.enqueue(Agent::Claude, "b", "Два.", Kind::Manual); // baya
        voices(&st);
        st.cfg.speaker = "baya".into();
        st.apply();
        st.enqueue(Agent::Claude, "a", "Раз.", Kind::Manual);
        st.enqueue(Agent::Claude, "b", "Два.", Kind::Manual);
        assert_eq!(voices(&st), vec![v("a", "baya"), v("b", "xenia")]);
    }

    #[test]
    fn preview_uses_default_voice() {
        let mut st = test_state();
        st.enqueue(Agent::Claude, "a", "Раз.", Kind::Manual);
        say(&mut st);
        let it = st.shared.queue.lock().unwrap().pop(Instant::now()).unwrap();
        assert_eq!((it.kind, it.speaker.as_str()), (Kind::Preview, ""));
    }

    // --- реестр агентов и пузыри ---

    fn with_agents(st: &mut State, ids: &[&str]) {
        let live: Vec<Live> = ids.iter().map(|id| Live { id: id.to_string(), agent: "claude", pid: 1, title: format!("T-{id}"),
            cwd: format!("/p/{id}"), transcript: PathBuf::new(), state: crate::agents::State::Working, status_at: 0 }).collect();
        st.scan = Box::new(move || live.clone());
        st.rescan();
    }

    fn sub(st: &mut State) -> BufReader<UnixStream> {
        let (a, b) = UnixStream::pair().unwrap();
        st.subscribe(a);
        reader(b)
    }

    /// Все строки до таймаута чтения.
    fn drain(r: &mut BufReader<UnixStream>) -> Vec<serde_json::Value> {
        let mut out = vec![];
        let mut l = String::new();
        while r.read_line(&mut l).is_ok_and(|n| n > 0) { out.push(serde_json::from_str(&l).unwrap()); l.clear(); }
        out
    }

    fn bubbles(r: &mut BufReader<UnixStream>) -> Vec<serde_json::Value> {
        drain(r).into_iter().filter_map(|v| v.get("bubble").cloned()).collect()
    }

    #[test]
    fn subscribe_sends_state_then_agents() {
        let mut st = test_state();
        with_agents(&mut st, &["s1"]);
        let mut r = sub(&mut st);
        assert_eq!(next(&mut r)["running"], true);
        assert_eq!(next(&mut r)["agents"][0]["title"], "T-s1");
    }

    #[test]
    fn active_status_bubbles_others_only_feed() {
        let (mut st, env) = env_state();
        with_agents(&mut st, &["s1", "s2"]);
        at(&mut st, &env, Some("s1"), 0);
        let mut r = sub(&mut st);
        md(&mut st, "s1", "Запускаю тесты. ");
        md(&mut st, "s2", "Читаю план. ");
        let b = bubbles(&mut r);
        assert_eq!(b.len(), 1);
        assert_eq!((b[0]["who"].as_str(), b[0]["kind"].as_str(), b[0]["append"].as_bool()), (Some("T-s1"), Some("status"), Some(true)));
        assert!(st.registry.get("s2").unwrap().feed.iter().any(|l| l.text.contains("Читаю план")));
    }

    #[test]
    fn question_bubbles_idle_notice_does_not() {
        let mut st = test_state();
        with_agents(&mut st, &["s2"]);
        let mut r = sub(&mut st);
        st.on_hook("notification", &serde_json::json!({"session_id": "s2", "message": "Claude needs your permission to use Bash", "cwd": "/p/s2"}));
        st.on_hook("user-prompt-submit", &serde_json::json!({"session_id": "s2", "prompt": "да"}));
        st.on_hook("notification", &serde_json::json!({"session_id": "s2", "message": "Claude is waiting for your input", "cwd": "/p/s2"}));
        let b = bubbles(&mut r);
        assert_eq!(b.len(), 1);
        assert_eq!((b[0]["kind"].as_str(), b[0]["text"].as_str()), (Some("question"), Some("Клоду нужно разрешение на команду")));
        assert_eq!(st.registry.waiting(), 0);
    }

    #[test]
    fn background_stop_bubbles_done() {
        let (mut st, env) = env_state();
        with_agents(&mut st, &["s1", "s2"]);
        at(&mut st, &env, Some("s1"), 0);
        let mut r = sub(&mut st);
        st.on_hook("stop", &serde_json::json!({"session_id": "s2", "last_assistant_message": "Готово. PR создан.", "transcript_path": ""}));
        let b = bubbles(&mut r);
        assert_eq!((b[0]["who"].as_str(), b[0]["kind"].as_str(), b[0]["text"].as_str()), (Some("T-s2"), Some("final"), Some("готово")));
        assert_eq!(st.registry.get("s2").unwrap().state, crate::agents::State::Done);
    }

    #[test]
    fn unknown_session_rescans() {
        let mut st = test_state();
        let n = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let c = n.clone();
        st.scan = Box::new(move || { c.fetch_add(1, Ordering::SeqCst); vec![] });
        st.on_hook("user-prompt-submit", &serde_json::json!({"session_id": "zz", "prompt": "x"}));
        assert_eq!(n.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn unknown_session_rescan_at_most_every_2s() {
        let (mut st, env) = env_state();
        let n = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let c = n.clone();
        st.scan = Box::new(move || { c.fetch_add(1, Ordering::SeqCst); vec![] });
        let hook = |st: &mut State| st.on_hook("message-display", &serde_json::json!({"session_id": "zz", "message_id": "m", "delta": "кусок"}));
        hook(&mut st);
        *env.1.lock().unwrap() += Duration::from_secs(1);
        hook(&mut st);
        assert_eq!(n.load(Ordering::SeqCst), 1);
        *env.1.lock().unwrap() += Duration::from_secs(1);
        hook(&mut st);
        assert_eq!(n.load(Ordering::SeqCst), 2);
    }
}
