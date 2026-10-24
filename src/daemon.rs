//! Цикл событий сервиса: команды и хуки из сокета, изменения транскриптов (inotify).

use crate::agents::{self, Live, Registry};
use crate::assemble::{Assembler, MdEvent};
use crate::config::{hhmm, in_window, Config};
use crate::digest::{self, Ask, Digest, Job};
use crate::focus::{self, Agent, SessionRef};
use crate::ipc::{self, Msg};
use crate::news::{Board, Class, News};
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
    Tick, // раз в секунду: Ома, граница тихого расписания, состав агентов
    Digest(Digest), // пересказ хода агента от воркеров digest
}

/// Как часто сверять реестр агентов с диском и /proc.
const SCAN_EVERY: Duration = Duration::from_secs(2);

/// Текст работающего агента копится столько, затем уходит на пересказ промежуточного.
const PROGRESS_EVERY: Duration = Duration::from_secs(60);

/// После новостей Ома слушает ответ столько секунд.
const LISTEN_SECS: u64 = 5;

/// Символов промпта пользователя в задании пересказа — хвост: длинный промпт не упрётся в предел argv `claude -p`.
const CONTEXT_MAX: usize = 1000;

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
    speaker: &'a str,
    rate: &'a str,
    project: &'a str,
    oma: &'a str,
    talking: bool,
    agents_voice: bool,
    progress_digest: bool,
    quiet: bool,
    quiet_schedule: bool,
    quiet_from: &'a str,
    quiet_to: &'a str,
    orb_pos: &'a str,
    orb_over_fullscreen: bool,
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

/// Что звучит: тихий режим глушит всё, кроме чтения по хоткею и образца голоса.
pub fn audible(kind: &Kind, quiet: bool) -> bool {
    match kind {
        Kind::Read | Kind::Preview => true,
        Kind::News | Kind::Voice => !quiet,
    }
}

struct State {
    cfg: Config,
    shared: Arc<Shared>,
    terms: Arc<Mutex<Terms>>,
    learn: Sender<(Agent, String)>,
    asm: Assembler,
    tail: Tailer,
    focus_cache: Option<(Instant, Option<SessionRef>)>, // None — устарел
    focus_src: Box<dyn FnMut() -> Option<SessionRef>>, // окно в фокусе; в тестах — подделка
    clock: Box<dyn Fn() -> Instant>,
    subs: Vec<UnixStream>,
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
    digest: Sender<Job>, // задания на пересказ; в тестах — канал без воркеров
    board: Board,        // что Ома скажет следующим
    progress: HashMap<String, (String, Instant)>, // сессия → текст хода с прошлого пересказа, время первого предложения
    listen_after: bool,  // звучат новости: по их концу Ома слушает ответ
    listen_at: Option<Instant>, // новости смолкли: окно ответа — когда доиграет хвост pw-cat
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default())
}

pub fn run() {
    let data = home().join(".local/share/agent-speak");
    let cfg = Config::load();
    let shared = Arc::new(Shared::new(Queue::new(), cfg.speaker.clone(), cfg.rate.clone()));
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
    let (dtx, drx) = mpsc::channel::<Digest>();
    let digest = digest::spawn(dtx);
    {
        let tx = tx.clone();
        std::thread::spawn(move || {
            for d in drx {
                let _ = tx.send(Event::Digest(d));
            }
        });
    }
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
        asm: Assembler::new(),
        tail: Tailer::new(),
        focus_cache: None,
        focus_src: Box::new(focus::focused),
        clock: Box::new(Instant::now),
        subs: Vec::new(),
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
        digest,
        board: Board::default(),
        progress: HashMap::new(),
        listen_after: false,
        listen_at: None,
    };
    st.oma_spawn();
    for ev in rx {
        match ev {
            Event::Tick => st.tick(),
            Event::Msg(m) => st.on_msg(m),
            Event::File(p) => st.on_file(&p),
            Event::Config => st.reload(),
            Event::Subscribe(s) => st.subscribe(s),
            Event::Busy => {
                st.broadcast();
                st.after_speech();
            }
            Event::Digest(d) => st.on_digest(d),
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

    fn quiet(&self) -> bool {
        self.cfg.quiet((self.minutes)())
    }

    /// Очередь и доска — только то, что слышно сейчас: настала тишина — лишнее выбросить. Звучащая фраза доигрывает.
    fn purge(&mut self) {
        if !self.cfg.progress_digest || !self.cfg.agents_voice {
            self.progress.clear(); // пересказ выключен — накопленное не нужно
        }
        let quiet = self.quiet();
        self.shared.queue.lock().unwrap().retain_audible(quiet);
        // новости тишины не ждут утра; без «Озвучивать агентов» звучат только вопросы
        if quiet {
            self.board.retain_classes(&[]);
            // новости выброшены — окно ответа по чужой речи не откроется
            (self.listen_after, self.listen_at) = (false, None);
        } else if !self.cfg.agents_voice {
            self.board.retain_classes(&[]); // за агентами не следим — о них ни слова
        }
    }

    /// Удержание новостей, пока Ома думает или говорит; снятие будит поток воспроизведения.
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
            speaker: &self.cfg.speaker,
            rate: &self.cfg.rate,
            project: &project,
            oma: &self.oma.state,
            talking: self.oma.talking,
            agents_voice: self.cfg.agents_voice,
            progress_digest: self.cfg.progress_digest,
            quiet: self.quiet(),
            quiet_schedule: self.cfg.quiet_schedule,
            quiet_from: &self.cfg.quiet_from,
            quiet_to: &self.cfg.quiet_to,
            orb_pos: &self.cfg.orb_pos,
            orb_over_fullscreen: self.cfg.orb_over_fullscreen,
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
        let reg = &self.registry;
        self.board.retain_sessions(|s| reg.get(s).is_some());
        self.progress.retain(|s, _| reg.get(s).is_some());
        // разрешение дали в терминале (хука нет, файл сессии — «работает»): вопрос не звучит и итогов не держит
        for s in self.board.question_sessions() {
            if self.registry.get(&s).is_none_or(|e| e.state != agents::State::Waiting) {
                self.board.answered(&s);
            }
        }
        self.push_agents();
    }

    /// Пузырь агента session (по нему окно открывает чат агента); единственный агент
    /// в фокусе пузырей не даёт — пользователь и так смотрит на него.
    fn bubble(&mut self, session: &str, id: &str, who: &str, text: &str, kind: &str) {
        let alone = self.registry.only().is_some_and(|e| e.id == session);
        if alone && self.focused().is_some_and(|f| f.id == session) {
            return;
        }
        let text = crate::text::clean::plain(text); // разметка markdown пузырю не нужна
        let b = serde_json::json!({"bubble": {"id": id, "agent": session, "who": who, "text": text, "kind": kind, "append": false, "drop": false}});
        self.broadcast_line(&format!("{b}\n"));
    }

    /// Текст хода — в накопленное для пересказа промежуточного; срок PROGRESS_EVERY — от первого предложения.
    fn note(&mut self, session: &str, text: &str) {
        if !self.cfg.progress_digest || !self.cfg.agents_voice {
            return;
        }
        let at = (self.clock)();
        let (acc, _) = self.progress.entry(session.to_string()).or_insert_with(|| (String::new(), at));
        if !acc.is_empty() {
            acc.push(' ');
        }
        acc.push_str(text);
    }

    /// Задание на пересказ; сессии не из реестра (не пользователя) не пересказываем.
    fn ask(&mut self, session: &str, ask: Ask, text: &str, fallback: Option<String>) {
        if !self.cfg.agents_voice {
            return; // «Следить за агентами» выключено: модель не зовём
        }
        let Some(e) = self.registry.get(session) else { return };
        let prompt = self.registry.last_prompt(session);
        let skip = prompt.chars().count().saturating_sub(CONTEXT_MAX);
        let context = prompt.chars().skip(skip).collect();
        let job = Job { session: session.to_string(), project: e.project.clone(), ask, text: text.to_string(), context, fallback };
        let _ = self.digest.send(job);
    }

    /// Пересказ пришёл: строка «что сейчас», пузырь, а вне тишины — на доску новостей.
    fn on_digest(&mut self, d: Digest) {
        if !self.cfg.agents_voice {
            return; // пересказ начат до выключения слежки
        }
        let Some(e) = self.registry.get(&d.session) else { return };
        // устарел, пока шёл пересказ: ход кончился (промежуточное звучало бы поверх итога)
        // или на вопрос уже ответили (промпт, разрешение в терминале, конец хода)
        let stale = match d.ask {
            Ask::Progress => e.state != agents::State::Working,
            Ask::Question => e.state != agents::State::Waiting,
            Ask::Final => false,
        };
        if stale {
            return;
        }
        let who = e.title.clone();
        self.registry.summary(&d.session, &d.say);
        let kind = match d.ask {
            Ask::Final => Some("final"),
            Ask::Question => Some("question"),
            Ask::Progress => d.important.then_some("status"),
        };
        if let Some(kind) = kind {
            self.bubble(&d.session, &format!("{kind}-{}", d.session), &who, &d.say, kind);
        }
        let class = match d.ask {
            Ask::Question => Some(Class::Question),
            Ask::Final => Some(Class::Final),
            Ask::Progress => d.important.then_some(Class::Progress),
        };
        if let Some(class) = class.filter(|_| !self.quiet()) {
            if class == Class::Question {
                self.board.answered(&d.session); // новый вопрос — прежний уже решён
            }
            let say = format!("{}: {}", d.project, d.say);
            self.board.push(News { session: d.session, class, say, born: (self.now_secs)() });
        }
        self.push_agents();
    }

    /// Новости — когда Ома молчит и ничего не звучит; слух греется заранее, чтобы ответ услышать сразу.
    fn speak_news(&mut self) {
        // идёт разговор или окно ответа: микрофон открыт — Ома услышала бы себя
        if self.quiet() || self.oma.talking || !self.silent() || self.shared.queue.lock().unwrap().held() {
            return;
        }
        let Some(text) = self.board.take((self.now_secs)()) else { return };
        if self.enqueue(Agent::Claude, "news", &text, Kind::News) > 0 {
            self.oma_send(&serde_json::json!({"do": "warm"}));
            self.listen_after = true;
        }
    }

    /// Ничего не звучит и не ждёт очереди.
    fn silent(&self) -> bool {
        !self.shared.busy.load(Ordering::SeqCst) && !self.shared.queue.lock().unwrap().ready()
    }

    /// Новости смолкли: busy гаснет, пока pw-cat доигрывает хвост, — окно ответа через speaker::TAIL, чтобы Ома не слышала себя.
    fn after_speech(&mut self) {
        self.listen_at = (self.listen_after && self.silent()).then(|| (self.clock)() + speaker::TAIL);
    }

    /// Из tick: хвост доиграл и всё ещё тихо — Ома слушает ответ LISTEN_SECS.
    fn listen_if_due(&mut self, now: Instant) {
        if !self.listen_at.is_some_and(|t| now >= t) {
            return;
        }
        self.listen_at = None;
        if self.silent() {
            self.listen_after = false;
            if !self.quiet() {
                let mut cmd = serde_json::json!({"do": "listen", "secs": LISTEN_SECS});
                if let Some(s) = self.board.asked() {
                    cmd["key"] = format!("agent:{s}").into(); // ответ на вопрос — в чат спросившего агента
                }
                self.oma_send(&cmd);
            }
        }
    }

    /// Подготовить и поставить в очередь; незнакомые слова — в фон. Возвращает число фраз.
    fn enqueue(&mut self, agent: Agent, session: &str, raw: &str, kind: Kind) -> usize {
        if !audible(&kind, self.quiet()) {
            eprintln!("agent-speak: пропущено (тихо): {}", raw.chars().take(40).collect::<String>());
            return 0;
        }
        let (sentences, unknown) = prepare(raw, &self.terms.lock().unwrap());
        for w in unknown {
            let _ = self.learn.send((agent.clone(), w));
        }
        let n = sentences.len();
        // образец — один элемент: каждый push Preview вытесняет прежний
        let sentences = if kind == Kind::Preview && !sentences.is_empty() { vec![sentences.join(" ")] } else { sentences };
        let mut q = self.shared.queue.lock().unwrap();
        for text in sentences {
            q.push(Item { session: session.into(), text, kind: kind.clone() });
        }
        drop(q);
        self.shared.cv.notify_all();
        n
    }

    fn stop(&mut self) {
        self.shared.stop();
        (self.listen_after, self.listen_at) = (false, None); // новости оборвали сами — ответа не ждём
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

    /// Ома упала: выключена, разговора (talking) и удержания нет; поднять через OMA_BACKOFF.
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
        let due: Vec<String> = self.progress.iter().filter(|(_, (_, at))| now.saturating_duration_since(*at) >= PROGRESS_EVERY).map(|(s, _)| s.clone()).collect();
        for s in due {
            if let Some((text, _)) = self.progress.remove(&s) {
                self.ask(&s, Ask::Progress, &text, None);
            }
        }
        self.speak_news(); // раньше окна ответа: новые новости встают перед ним, а не поверх
        self.listen_if_due(now);
    }

    fn on_msg(&mut self, m: Msg) {
        let changes = matches!(m, Msg::Stop | Msg::Hush | Msg::Pause | Msg::VoiceState { .. });
        match m {
            Msg::Stop => self.stop(),
            Msg::Hush => self.shared.hush(), // новость, удержанная за Омой, не теряется
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
                let muted = key == "agents_voice" && value == false && self.cfg.agents_voice;
                if self.cfg.set(&key, &value) {
                    self.cfg.save();
                    if muted {
                        self.stop(); // выключили озвучку — и то, что звучит, обрываем, а не ждём конца фразы
                    }
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
        // говорит или есть что сказать — read работает как стоп
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
                self.board.answered(&session); // ответили в терминале — Оме спрашивать нечего
                self.progress.remove(&session);
            }
            "notification" => {
                // «ждёт ввода» — простой, а не вопрос: молчим, реестр не трогаем
                let raw = p["message"].as_str().unwrap_or_default();
                if !matches!(raw, "" | "Claude is waiting for your input") {
                    let tail = tail_of(p["transcript_path"].as_str().unwrap_or_default());
                    let msg = notification_text(raw, || claude::last_tool(&tail));
                    let question = claude::last_question(&tail);
                    let text = question.as_ref().map_or(msg.clone(), |q| format!("вопрос: {q}"));
                    self.registry.question(&session, &text, now);
                    // модели — суть и совет агента: его текст и ввод вызова; перевод уведомления — запасная фраза
                    self.ask(&session, Ask::Question, &claude::question_context(&tail), Some(msg));
                }
            }
            "message-display" => {
                let Some(ev) = MdEvent::from_json(p) else {
                    eprintln!("agent-speak: непонятный MessageDisplay: {p}");
                    return;
                };
                // предложения — в ленту и на пересказ: дословно агенты не звучат
                for s in self.asm.push(&ev) {
                    if self.registry.status(&ev.session, &s, now) {
                        self.note(&ev.session, &s);
                    }
                }
            }
            "stop" => {
                self.asm.flush_session(&session); // недосказанный хвост есть в финале; в следующий ход не тянется
                let transcript = PathBuf::from(p["transcript_path"].as_str().unwrap_or_default());
                let last = final_message(p, &transcript).unwrap_or_default();
                self.registry.finished(&session, &last, now);
                self.registry.reload(&session, &last);
                self.board.answered(&session); // ход кончился — вопрос решён, хоть разрешение и дали в терминале без промпта
                self.progress.remove(&session); // промежуточное покрывает итог
                self.ask(&session, Ask::Final, &last, Some("готово".into()));
            }
            "codex-notify" => {
                let thread = p["thread-id"].as_str().unwrap_or_default().to_string();
                let last = p["last-assistant-message"].as_str().unwrap_or_default();
                self.registry.finished(&thread, last, now);
                self.board.answered(&thread);
                self.progress.remove(&thread);
                self.ask(&thread, Ask::Final, last, Some("готово".into()));
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
        // сообщения — в ленту и на пересказ: дословно агенты не звучат
        let now = (self.now_secs)();
        for line in lines {
            let blocks = match agent {
                Agent::Claude => claude::parse_line(&line),
                Agent::Codex => codex::parse_line(&line),
            };
            for b in blocks {
                if self.registry.status(&id, &b.text, now) {
                    self.note(&id, &b.text);
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
        let shared = Arc::new(Shared::new(Queue::new(), "xenia".into(), "medium".into()));
        State {
            cfg: Config::default(),
            shared,
            terms: Arc::new(Mutex::new(Terms::from_str(""))),
            learn: mpsc::channel().0,
            asm: Assembler::new(),
            tail: Tailer::new(),
            focus_cache: None,
            focus_src: Box::new(|| None), // hyprctl в тестах не зовётся
            clock: Box::new(Instant::now),
            subs: Vec::new(),
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
            digest: mpsc::channel().0, // claude в тестах не зовётся
            board: Board::default(),
            progress: HashMap::new(),
            listen_after: false,
            listen_at: None,
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
            "{\"running\":true,\"speaking\":false,\"paused\":false,\"speaker\":\"xenia\",\"rate\":\"medium\",\"project\":\"\",\"oma\":\"off\",\"talking\":false,\"agents_voice\":true,\"progress_digest\":false,\"quiet\":false,\"quiet_schedule\":true,\"quiet_from\":\"23:00\",\"quiet_to\":\"08:00\",\"orb_pos\":\"bottom-right,0,0\",\"orb_over_fullscreen\":true,\"waiting\":0}\n"
        );
    }

    #[test]
    fn voice_text_queued() {
        let mut st = test_state();
        st.on_msg(Msg::Voice { text: "Привет, слушаю.".into() });
        assert_eq!(st.shared.queue.lock().unwrap().pop().unwrap().kind, Kind::Voice);
    }

    #[test]
    fn voice_text_plays_without_agents_voice() {
        let mut st = test_state();
        st.cfg.agents_voice = false; // помощник говорит и без слежки за агентами
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
        for k in [Read, Preview] { assert!(audible(&k, false) && audible(&k, true)); }
        for k in [Voice, News] { assert!(audible(&k, false) && !audible(&k, true)); }
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
    fn quiet_on_drops_queued_keeps_preview() {
        let _h = TempHome::new("quiet-purge");
        let mut st = test_state();
        st.enqueue(Agent::Claude, "news", "Фича готова.", Kind::News);
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
    fn hush_cuts_oma_keeps_held_news() {
        let mut st = test_state();
        st.on_msg(Msg::VoiceState { state: "speaking".into(), talking: true });
        st.enqueue(Agent::Claude, "news", "проект: Нужно разрешение.", Kind::News);
        st.on_msg(Msg::Voice { text: "Ответ Омы дальше.".into() });
        let g0 = st.shared.generation.load(Ordering::SeqCst);
        st.on_msg(Msg::Hush);
        assert!(st.shared.generation.load(Ordering::SeqCst) > g0, "звучащая фраза оборвана");
        st.on_msg(Msg::VoiceState { state: "listening".into(), talking: true });
        assert_eq!(texts(&st), ["проект: Нужно разрешение."]);
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
        st.on_msg(Msg::Set { key: "agents_voice".into(), value: serde_json::json!("true") });
        st.on_msg(Msg::Set { key: "volume".into(), value: serde_json::json!("loud") });
        let mut l = String::new();
        assert!(r.read_line(&mut l).is_err(), "неверный set ничего не рассылает: {l}");
        assert_eq!((st.cfg.speaker.as_str(), st.cfg.agents_voice), ("baya", true));
    }

    #[test]
    fn agents_voice_off_cuts_what_is_playing() {
        let _home = TempHome::new("mute"); // set сохраняет config.toml
        let mut st = test_state();
        st.cfg.agents_voice = true;
        with_agents(&mut st, &["s"]);
        st.on_digest(dg("s", Ask::Final, false, "Фича готова."));
        st.tick();
        assert_eq!(texts(&st), ["плуг: Фича готова."]);
        st.on_msg(Msg::Set { key: "agents_voice".into(), value: serde_json::json!(false) });
        assert!(texts(&st).is_empty() && !st.listen_after); // тумблер — и способ замолчать сейчас
    }

    fn say(st: &mut State) {
        st.on_msg(Msg::Say { text: "Так звучит этот голос.".into() });
    }

    #[test]
    fn say_queues_preview_when_idle() {
        let mut st = test_state();
        say(&mut st);
        let it = st.shared.queue.lock().unwrap().pop().unwrap();
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
        assert_eq!(q.pop().unwrap().kind, Kind::Preview);
        assert!(q.pop().is_none() && q.paused());
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

    // --- подставные окно в фокусе и часы ---

    type Env = (Arc<Mutex<Option<SessionRef>>>, Arc<Mutex<Instant>>);

    fn sref(id: &str) -> SessionRef {
        SessionRef { agent: Agent::Claude, id: id.into(), transcript: PathBuf::from("/nonexistent"), project: id.into() }
    }

    fn env_state() -> (State, Env) {
        let mut st = test_state();
        let focus: Arc<Mutex<Option<SessionRef>>> = Default::default();
        let now = Arc::new(Mutex::new(Instant::now()));
        let (f, n) = (focus.clone(), now.clone());
        st.focus_src = Box::new(move || f.lock().unwrap().clone());
        st.clock = Box::new(move || *n.lock().unwrap());
        (st, (focus, now))
    }

    fn texts(st: &State) -> Vec<String> {
        let mut q = st.shared.queue.lock().unwrap();
        std::iter::from_fn(|| q.pop()).map(|i| i.text).collect()
    }

    fn md(st: &mut State, session: &str, text: &str) {
        st.on_hook("message-display", &serde_json::json!({"session_id": session, "message_id": "m", "delta": text, "final": true}));
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

    /// В фокусе окно агента id, его последний ответ — text.
    fn focus_with_answer(env: &Env, id: &str, text: &str) {
        let dir = std::env::temp_dir().join(format!("agent-speak-read-{}-{id}-{}", std::process::id(), text.len()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{id}.jsonl"));
        let line = serde_json::json!({"type":"assistant","message":{"id":"msg_1","content":[{"type":"text","text": text}]}});
        std::fs::write(&path, line.to_string()).unwrap();
        *env.0.lock().unwrap() = Some(SessionRef { transcript: path, ..sref(id) });
    }

    #[test]
    fn read_speaks_last_turn_of_focused() {
        let (mut st, env) = env_state();
        focus_with_answer(&env, "b", "Ответ бэ.");
        st.on_msg(Msg::Read);
        assert_eq!(texts(&st), vec!["Ответ бэ."]);
    }

    #[test]
    fn prompt_submit_never_interrupts() {
        let mut st = test_state();
        st.shared.set_busy(Some("a"));
        let g0 = st.shared.generation.load(Ordering::SeqCst);
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

    fn pop1(st: &State) -> String {
        st.shared.queue.lock().unwrap().pop().unwrap().text
    }

    #[test]
    fn read_ended_by_read_or_stop_not_by_prompt_submit() {
        let read = || {
            let (mut st, env) = env_state();
            focus_with_answer(&env, "a", "Раз. Два.");
            st.on_msg(Msg::Read);
            st
        };
        let mut st = read();
        st.on_hook("user-prompt-submit", &serde_json::json!({"session_id": "a"}));
        assert_eq!(pop1(&st), "Раз.");
        assert_eq!(texts(&st), vec!["Два."]);

        let mut st = read();
        st.on_msg(Msg::Read); // повторный read — стоп
        assert!(texts(&st).is_empty());

        let mut st = read();
        st.on_msg(Msg::Stop);
        assert!(texts(&st).is_empty());
    }

    // --- реестр агентов и пузыри ---

    fn live(id: &str) -> Live {
        Live { id: id.to_string(), agent: "claude", pid: 1, title: format!("T-{id}"),
            cwd: format!("/p/{id}"), transcript: PathBuf::new(), state: crate::agents::State::Working, status_at: 0 }
    }

    fn with_agents(st: &mut State, ids: &[&str]) {
        let live: Vec<Live> = ids.iter().map(|id| live(id)).collect();
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
    fn agent_text_is_not_spoken_but_reaches_feed() {
        let mut st = test_state();
        st.cfg.agents_voice = true;
        with_agents(&mut st, &["s"]);
        let mut r = sub(&mut st);
        md(&mut st, "s", "Делаю экран настроек. ");
        st.on_hook("stop", &serde_json::json!({"session_id": "s", "transcript_path": "/nonexistent", "last_assistant_message": "Готово."}));
        assert!(texts(&st).is_empty());
        assert!(bubbles(&mut r).is_empty(), "пузыри — только из пересказа");
        assert!(st.registry.get("s").unwrap().feed.iter().any(|l| l.text.contains("Делаю экран настроек")));
    }

    #[test]
    fn claude_transcript_lines_not_in_feed_twice() {
        let root = std::env::temp_dir().join(format!("agent-speak-onfile-{}", std::process::id()));
        let dir = root.join(".claude/projects/x");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("s.jsonl");
        std::fs::write(&path, "").unwrap();
        let mut st = test_state();
        with_agents(&mut st, &["s"]);
        st.on_file(&path); // первая встреча — с конца
        md(&mut st, "s", "Делаю экран. ");
        let line = serde_json::json!({"type": "assistant", "message": {"id": "msg_1", "content": [{"type": "text", "text": "Делаю экран."}]}});
        std::fs::write(&path, format!("{line}\n")).unwrap();
        st.on_file(&path); // Claude вживую — только хук
        assert_eq!(st.registry.get("s").unwrap().feed.iter().filter(|l| l.text == "Делаю экран.").count(), 1);
        let _ = std::fs::remove_dir_all(root);
    }

    // --- пересказ и новости ---

    fn jobs(st: &mut State) -> mpsc::Receiver<Job> {
        let (tx, rx) = mpsc::channel();
        st.digest = tx;
        rx
    }

    fn dg(session: &str, ask: Ask, important: bool, say: &str) -> Digest {
        Digest { session: session.into(), project: "plug".into(), ask, important, say: say.into() }
    }

    /// Агент спросил (ждёт ответа, как после уведомления) — и пришёл пересказ вопроса.
    fn question(st: &mut State, session: &str, important: bool, say: &str) {
        st.registry.question(session, "Клоду нужно разрешение", 1000);
        st.on_digest(dg(session, Ask::Question, important, say));
    }

    fn stop(st: &mut State, session: &str, last: &str) {
        st.on_hook("stop", &serde_json::json!({"session_id": session, "transcript_path": "/nonexistent", "last_assistant_message": last}));
    }

    #[test]
    fn stop_asks_final_digest_with_last_prompt() {
        let mut st = test_state();
        with_agents(&mut st, &["s"]);
        let rx = jobs(&mut st);
        st.on_hook("user-prompt-submit", &serde_json::json!({"session_id": "s", "prompt": "сделай фичу"}));
        stop(&mut st, "s", "Сделал фичу.");
        let job = rx.try_recv().unwrap();
        assert_eq!((job.ask, job.text.as_str(), job.context.as_str(), job.fallback.as_deref()), (Ask::Final, "Сделал фичу.", "сделай фичу", Some("готово")));
        assert_eq!((job.session.as_str(), job.project.as_str()), ("s", "s"));
        stop(&mut st, "чужой", "Готово."); // не из реестра — не пересказываем
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn job_context_is_tail_of_long_prompt() {
        let mut st = test_state();
        with_agents(&mut st, &["s"]);
        let rx = jobs(&mut st);
        st.on_hook("user-prompt-submit", &serde_json::json!({"session_id": "s", "prompt": format!("{}КОНЕЦ", "а".repeat(5000))}));
        stop(&mut st, "s", "Сделал.");
        let job = rx.try_recv().unwrap();
        assert!(job.context.ends_with("КОНЕЦ"));
        assert_eq!(job.context.chars().count(), CONTEXT_MAX);
    }

    #[test]
    fn final_digest_bubbles_then_speaks_when_idle() {
        let mut st = test_state();
        st.cfg.agents_voice = true;
        with_agents(&mut st, &["s"]);
        let mut r = sub(&mut st);
        st.on_digest(dg("s", Ask::Final, false, "Фича готова."));
        let b = bubbles(&mut r);
        assert_eq!(b.len(), 1);
        assert_eq!((b[0]["kind"].as_str(), b[0]["text"].as_str(), b[0]["who"].as_str()), (Some("final"), Some("Фича готова."), Some("T-s")));
        st.tick();
        assert_eq!(pop1(&st), "плуг: Фича готова.");
    }

    #[test]
    fn question_notification_asks_digest_not_urgent() {
        let mut st = test_state();
        with_agents(&mut st, &["s"]);
        let rx = jobs(&mut st);
        let mut r = sub(&mut st);
        st.on_hook("notification", &serde_json::json!({"session_id": "s", "message": "Claude needs your permission to use Bash", "transcript_path": "/nonexistent", "cwd": "/x/plug"}));
        let job = rx.try_recv().unwrap();
        assert_eq!(job.ask, Ask::Question);
        assert_eq!(job.fallback.as_deref(), Some("Клоду нужно разрешение на команду"));
        assert!(texts(&st).is_empty());
        assert!(bubbles(&mut r).is_empty(), "пузырь — из пересказа");
        assert_eq!(st.registry.waiting(), 1);
    }

    #[test]
    fn agents_watch_off_is_silent_and_asks_no_model() {
        let mut st = test_state();
        st.cfg.agents_voice = false; // «Следить за агентами» выключено: Ома — только помощник
        with_agents(&mut st, &["s", "t"]);
        let rx = jobs(&mut st);
        let mut r = sub(&mut st);
        stop(&mut st, "s", "Готово.");
        assert!(rx.try_recv().is_err(), "без слежки Haiku не зовём");
        st.on_digest(dg("s", Ask::Final, true, "Фича готова.")); // пересказ, начатый до выключения
        question(&mut st, "t", true, "Удалить старый код?");
        st.tick();
        assert!(texts(&st).is_empty() && bubbles(&mut r).is_empty());
    }

    #[test]
    fn agents_watch_on_speaks_questions_even_unimportant() {
        let mut st = test_state();
        st.cfg.agents_voice = true;
        with_agents(&mut st, &["t"]);
        question(&mut st, "t", false, "Удалить старый код?"); // вопрос звучит, даже если модель сочла его неважным
        st.tick();
        assert_eq!(texts(&st), ["плуг: Удалить старый код?"]);
    }

    #[test]
    fn quiet_digest_bubbles_but_never_speaks_later() {
        let mut st = test_state();
        st.cfg.agents_voice = true;
        with_agents(&mut st, &["s", "t"]);
        let mut r = sub(&mut st);
        st.cfg.quiet_override = "on".into();
        st.on_digest(dg("s", Ask::Final, true, "Фича готова."));
        question(&mut st, "t", true, "Удалить старый код?");
        assert_eq!(bubbles(&mut r).len(), 2);
        st.cfg.quiet_override = String::new();
        st.tick();
        assert!(texts(&st).is_empty());
    }

    #[test]
    fn muting_clears_board() {
        let mut st = test_state();
        st.cfg.agents_voice = true;
        with_agents(&mut st, &["s", "t"]);
        st.on_digest(dg("s", Ask::Final, true, "Фича готова."));
        question(&mut st, "t", true, "Удалить старый код?");
        st.cfg.agents_voice = false;
        st.apply();
        st.tick();
        assert!(texts(&st).is_empty()); // слежка выключена — и вопросы с доски ушли
        st.cfg.agents_voice = true;
        question(&mut st, "t", true, "Удалить и тесты?");
        st.cfg.quiet_override = "on".into();
        st.apply();
        st.cfg.quiet_override = String::new();
        st.tick();
        assert!(texts(&st).is_empty());
    }

    #[test]
    fn follow_up_question_not_held_by_the_first() {
        let mut st = test_state();
        with_agents(&mut st, &["s"]);
        question(&mut st, "s", true, "Удалить старый код?");
        st.tick();
        pop1(&st);
        question(&mut st, "s", true, "Удалить и тесты?"); // прежний вопрос уже решён
        st.tick();
        assert_eq!(texts(&st), ["плуг: Удалить и тесты?"]);
    }

    #[test]
    fn progress_flushes_after_a_minute_and_stop_drops_it() {
        let (mut st, env) = env_state();
        st.cfg.progress_digest = true;
        with_agents(&mut st, &["s"]);
        let rx = jobs(&mut st);
        let later = |st: &mut State, secs| {
            *env.1.lock().unwrap() += Duration::from_secs(secs);
            st.tick();
        };
        md(&mut st, "s", "Шаг один. ");
        later(&mut st, 59);
        assert!(rx.try_recv().is_err());
        later(&mut st, 2);
        let job = rx.try_recv().unwrap();
        assert_eq!((job.ask, job.text.as_str(), job.fallback), (Ask::Progress, "Шаг один.", None));
        md(&mut st, "s", "Шаг два. ");
        stop(&mut st, "s", "Готово.");
        assert_eq!(rx.try_recv().unwrap().ask, Ask::Final);
        later(&mut st, 61);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn progress_digest_off_keeps_raw_status_and_drops_accumulated() {
        let (mut st, env) = env_state();
        with_agents(&mut st, &["s"]);
        let rx = jobs(&mut st);
        let later = |st: &mut State, secs| {
            *env.1.lock().unwrap() += Duration::from_secs(secs);
            st.tick();
        };
        md(&mut st, "s", "Шаг один. ");
        later(&mut st, 61);
        assert!(rx.try_recv().is_err()); // по умолчанию промежуточное модели не уходит
        assert_eq!(st.registry.get("s").unwrap().status, "Шаг один.");
        st.cfg.progress_digest = true;
        md(&mut st, "s", "Шаг два. ");
        st.cfg.progress_digest = false;
        st.apply();
        later(&mut st, 61);
        assert!(rx.try_recv().is_err()); // выключили — накопленное не пересказывается
    }

    #[test]
    fn unimportant_progress_only_updates_summary() {
        let mut st = test_state();
        st.cfg.agents_voice = true;
        with_agents(&mut st, &["s"]);
        let mut r = sub(&mut st);
        st.on_digest(dg("s", Ask::Progress, false, "Правит орб."));
        assert!(bubbles(&mut r).is_empty());
        assert_eq!(st.registry.get("s").unwrap().status, "Правит орб.");
        st.tick();
        assert!(texts(&st).is_empty());
        st.on_digest(dg("s", Ask::Progress, true, "Нашла утечку памяти."));
        let b = bubbles(&mut r);
        assert_eq!((b.len(), b[0]["kind"].as_str()), (1, Some("status")));
        st.tick();
        assert_eq!(texts(&st), ["плуг: Нашла утечку памяти."]);
    }

    #[test]
    fn stale_progress_digest_dropped() {
        let mut st = test_state();
        st.cfg.agents_voice = true;
        with_agents(&mut st, &["s"]);
        stop(&mut st, "s", "Готово.");
        let status = st.registry.get("s").unwrap().status.clone();
        let mut r = sub(&mut st);
        st.on_digest(dg("s", Ask::Progress, true, "Шаг закрыт."));
        st.on_digest(dg("чужой", Ask::Final, true, "Фича готова.")); // сессии нет в реестре
        assert!(bubbles(&mut r).is_empty());
        assert_eq!(st.registry.get("s").unwrap().status, status);
        st.tick();
        assert!(texts(&st).is_empty());
    }

    #[test]
    fn prompt_submit_answers_question() {
        let mut st = test_state();
        st.cfg.agents_voice = true;
        with_agents(&mut st, &["s", "t"]);
        question(&mut st, "s", true, "Удалить старый код?");
        st.on_digest(dg("t", Ask::Final, false, "Фича готова."));
        st.tick();
        assert_eq!(pop1(&st), "плуг: Удалить старый код?");
        st.tick();
        assert!(texts(&st).is_empty(), "итог ждёт ответа на вопрос");
        st.on_hook("user-prompt-submit", &serde_json::json!({"session_id": "s", "prompt": "да"}));
        st.tick();
        assert_eq!(texts(&st), ["плуг: Фича готова."]);
    }

    #[test]
    fn stop_ends_wait_for_its_question() {
        let mut st = test_state();
        st.cfg.agents_voice = true;
        with_agents(&mut st, &["s", "t"]);
        question(&mut st, "s", true, "Можно запустить команду?");
        st.on_digest(dg("t", Ask::Final, false, "Фича готова."));
        st.tick();
        pop1(&st);
        stop(&mut st, "s", "Готово."); // разрешение дали в терминале — промпта не было
        st.tick();
        assert_eq!(texts(&st), ["плуг: Фича готова."]);
    }

    #[test]
    fn news_waits_while_oma_talks() {
        let mut st = test_state();
        st.cfg.agents_voice = true;
        with_agents(&mut st, &["s"]);
        st.on_msg(Msg::VoiceState { state: "thinking".into(), talking: true });
        st.on_digest(dg("s", Ask::Final, false, "Фича готова."));
        st.tick();
        assert!(texts(&st).is_empty());
        st.on_msg(Msg::VoiceState { state: "idle".into(), talking: false });
        st.shared.set_busy(Some("r")); // ещё звучит другое
        st.tick();
        assert!(texts(&st).is_empty());
        st.shared.set_busy(None);
        st.tick();
        assert_eq!(texts(&st), ["плуг: Фича готова."]);
    }

    #[test]
    fn listen_after_news_played() {
        let h = TempHome::new("oma-listen");
        std::fs::create_dir_all(&h.dir).unwrap();
        let out = h.dir.join("in.txt");
        let (mut st, env) = env_state();
        st.cfg.agents_voice = true;
        with_agents(&mut st, &["s"]);
        st.voice_cmd = vec!["sh".into(), "-c".into(), format!("cat > {}", out.display())];
        st.oma_spawn();
        st.on_digest(dg("s", Ask::Final, false, "Фича готова."));
        st.tick();
        st.after_speech(); // новость ещё в очереди — рано
        pop1(&st);
        st.shared.set_busy(None); // busy гаснет, а pw-cat ещё доигрывает хвост
        st.after_speech();
        st.tick();
        assert!(st.listen_after, "хвост не доиграл — окно не открываем");
        *env.1.lock().unwrap() += speaker::TAIL;
        st.tick();
        assert!(!st.listen_after);
        st.after_speech(); // окно уже открыто — второй раз не открываем
        *env.1.lock().unwrap() += speaker::TAIL;
        st.tick();
        drop(st); // stdin закрыт → cat допишет и выйдет
        let want = "{\"do\":\"warm\"}\n{\"do\":\"listen\",\"secs\":5}\n";
        let end = Instant::now() + Duration::from_secs(2);
        while std::fs::read_to_string(&out).unwrap_or_default() != want && Instant::now() < end { std::thread::sleep(Duration::from_millis(20)); }
        assert_eq!(std::fs::read_to_string(&out).unwrap(), want);
    }

    #[test]
    fn listen_after_question_opens_asking_agents_chat() {
        let h = TempHome::new("oma-listen-q");
        std::fs::create_dir_all(&h.dir).unwrap();
        let out = h.dir.join("in.txt");
        let (mut st, env) = env_state();
        with_agents(&mut st, &["s"]);
        st.voice_cmd = vec!["sh".into(), "-c".into(), format!("cat > {}", out.display())];
        st.oma_spawn();
        question(&mut st, "s", true, "Удалить старый код?");
        st.tick();
        pop1(&st);
        st.after_speech();
        *env.1.lock().unwrap() += speaker::TAIL;
        st.tick();
        drop(st);
        let end = Instant::now() + Duration::from_secs(2);
        let listen = || std::fs::read_to_string(&out).unwrap_or_default().lines().find(|l| l.contains("listen")).map(String::from);
        while listen().is_none() && Instant::now() < end { std::thread::sleep(Duration::from_millis(20)); }
        let v: serde_json::Value = serde_json::from_str(&listen().unwrap()).unwrap();
        assert_eq!(v, serde_json::json!({"do": "listen", "secs": 5, "key": "agent:s"}));
    }

    #[test]
    fn stopped_or_muted_news_opens_no_listen_window() {
        let mut st = test_state();
        st.cfg.agents_voice = true;
        with_agents(&mut st, &["s"]);
        let news = |st: &mut State| {
            st.on_digest(dg("s", Ask::Final, false, "Фича готова."));
            st.tick();
            assert!(st.listen_after);
        };
        news(&mut st);
        pop1(&st);
        st.after_speech();
        assert!(st.listen_at.is_some());
        st.on_msg(Msg::Stop);
        assert!(!st.listen_after && st.listen_at.is_none());
        news(&mut st);
        st.cfg.quiet_override = "on".into();
        st.apply(); // тишина настала раньше, чем новость зазвучала: она выброшена
        assert!(!st.listen_after && texts(&st).is_empty());
    }

    /// Разрешение агенту id дали в терминале: хука нет, файл сессии — «работает» и новее уведомления.
    fn approve(st: &mut State, ids: &[&str], id: &str) {
        let v: Vec<Live> = ids.iter().map(|i| Live { status_at: if *i == id { 2000 } else { 0 }, ..live(i) }).collect();
        st.scan = Box::new(move || v.clone());
        st.rescan();
    }

    #[test]
    fn late_question_digest_after_answer_not_spoken() {
        let mut st = test_state();
        st.cfg.agents_voice = true;
        with_agents(&mut st, &["s", "t"]);
        st.on_hook("notification", &serde_json::json!({"session_id": "s", "message": "Claude needs your permission to use Bash", "transcript_path": "/nonexistent", "cwd": "/p/s"}));
        approve(&mut st, &["s", "t"], "s"); // одобрили раньше, чем вернулся пересказ
        assert_eq!(st.registry.get("s").unwrap().state, crate::agents::State::Working);
        let status = st.registry.get("s").unwrap().status.clone();
        let mut r = sub(&mut st);
        st.on_digest(dg("s", Ask::Question, true, "Можно запустить команду?"));
        assert!(st.board.question_sessions().is_empty());
        assert!(bubbles(&mut r).is_empty(), "устаревший вопрос не висит пузырём");
        assert_eq!(st.registry.get("s").unwrap().status, status);
        st.on_digest(dg("t", Ask::Final, false, "Фича готова."));
        st.tick();
        assert_eq!(texts(&st), ["плуг: Фича готова."]);
    }

    #[test]
    fn news_waits_for_conversation_and_listen_window() {
        let mut st = test_state();
        st.cfg.agents_voice = true;
        with_agents(&mut st, &["s"]);
        st.on_msg(Msg::VoiceState { state: "listening".into(), talking: true }); // окно ответа или разговор: микрофон открыт
        st.on_digest(dg("s", Ask::Final, false, "Фича готова."));
        st.tick();
        assert!(texts(&st).is_empty());
        st.on_msg(Msg::VoiceState { state: "idle".into(), talking: false });
        st.tick();
        assert_eq!(texts(&st), ["плуг: Фича готова."]);
    }

    #[test]
    fn question_job_carries_agent_text_and_call() {
        let dir = std::env::temp_dir().join(format!("agent-speak-qjob-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("s.jsonl");
        let said = serde_json::json!({"type": "assistant", "message": {"content": [{"type": "text", "text": "Сборка устарела."}]}});
        let call = serde_json::json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "name": "Bash", "input": {"command": "rm -rf build", "description": "Чищу сборку"}}]}});
        std::fs::write(&path, format!("{said}\n{call}\n")).unwrap();
        let mut st = test_state();
        with_agents(&mut st, &["s"]);
        let rx = jobs(&mut st);
        st.on_hook("notification", &serde_json::json!({"session_id": "s", "message": "Claude needs your permission", "transcript_path": path, "cwd": "/p/s"}));
        let job = rx.try_recv().unwrap();
        assert_eq!(job.text, "Сборка устарела.\n\nЖдёт ответа на Bash: Чищу сборку: rm -rf build");
        assert_eq!(job.fallback.as_deref(), Some("Клоду нужно разрешение на команду"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn codex_notify_asks_final_digest() {
        let mut st = test_state();
        let codex = vec![Live { agent: "codex", ..live("th") }];
        st.scan = Box::new(move || codex.clone());
        st.rescan();
        let rx = jobs(&mut st);
        st.on_hook("codex-notify", &serde_json::json!({"thread-id": "th", "last-assistant-message": "Сделал экран настроек."}));
        let job = rx.try_recv().unwrap();
        assert_eq!((job.session.as_str(), job.ask, job.text.as_str(), job.fallback.as_deref()), ("th", Ask::Final, "Сделал экран настроек.", Some("готово")));
        assert_eq!(st.registry.get("th").unwrap().state, crate::agents::State::Done);
    }

    #[test]
    fn rescan_drops_question_no_longer_waiting() {
        let mut st = test_state();
        st.cfg.agents_voice = true;
        with_agents(&mut st, &["s", "t"]);
        st.on_msg(Msg::VoiceState { state: "thinking".into(), talking: true });
        question(&mut st, "s", true, "Можно запустить команду?"); // ждёт, пока Ома занята
        st.on_digest(dg("t", Ask::Final, false, "Фича готова."));
        st.tick();
        approve(&mut st, &["s", "t"], "s");
        st.on_msg(Msg::VoiceState { state: "idle".into(), talking: false });
        st.tick();
        assert_eq!(texts(&st), ["плуг: Фича готова."]);
    }

    #[test]
    fn answer_in_terminal_frees_finals_waiting_on_asked_question() {
        let mut st = test_state();
        st.cfg.agents_voice = true;
        with_agents(&mut st, &["s", "t"]);
        question(&mut st, "s", true, "Можно запустить команду?");
        st.on_digest(dg("t", Ask::Final, false, "Фича готова."));
        st.tick();
        assert_eq!(pop1(&st), "плуг: Можно запустить команду?");
        approve(&mut st, &["s", "t"], "s");
        st.tick();
        assert_eq!(texts(&st), ["плуг: Фича готова."]);
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
    fn lone_focused_agent_gives_no_bubbles() {
        let (mut st, env) = env_state();
        with_agents(&mut st, &["s1"]);
        *env.0.lock().unwrap() = Some(sref("s1"));
        let mut r = sub(&mut st);
        let ask = |st: &mut State| question(st, "s1", true, "Удалить старый код?");
        ask(&mut st);
        assert!(bubbles(&mut r).is_empty());
        *env.0.lock().unwrap() = None; // фокус ушёл на не-агента
        *env.1.lock().unwrap() += Duration::from_secs(1); // прежний фокус в кэше устарел
        ask(&mut st);
        let b = bubbles(&mut r);
        assert_eq!((b.len(), b[0]["agent"].as_str()), (1, Some("s1")));
    }

    #[test]
    fn idle_notice_asks_nothing() {
        let mut st = test_state();
        with_agents(&mut st, &["s2"]);
        let rx = jobs(&mut st);
        st.on_hook("notification", &serde_json::json!({"session_id": "s2", "message": "Claude is waiting for your input", "cwd": "/p/s2"}));
        assert!(rx.try_recv().is_err());
        assert!(texts(&st).is_empty());
        assert_eq!(st.registry.waiting(), 0);
    }

    #[test]
    fn stop_marks_done_without_bubble() {
        let mut st = test_state();
        with_agents(&mut st, &["s1", "s2"]);
        let mut r = sub(&mut st);
        st.on_hook("stop", &serde_json::json!({"session_id": "s2", "last_assistant_message": "Готово. PR создан.", "transcript_path": ""}));
        assert!(bubbles(&mut r).is_empty());
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
