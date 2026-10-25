//! Цикл событий сервиса: команды и хуки из сокета, изменения транскриптов (inotify).

use crate::agents::{self, Entry, Live, Registry};
use crate::assemble::{Assembler, MdEvent};
use crate::config::{hhmm, in_window, Config};
use crate::digest::{self, Ask, Digest, Job};
use crate::focus::{self, Agent, SessionRef};
use crate::ipc::{self, Msg};
use crate::limits::{self, Limits};
use crate::mux::{self, Pane};
use crate::news::{Board, Class, News, SUMMARY};
use crate::notice::notify as notice;
use crate::presence::{self, Mode, Presence};
use crate::queue::{Item, Kind, Queue};
use crate::source::{claude, codex, tail::{tail_of, Tailer}};
use crate::speaker::{self, Shared};
use crate::text::{prepare, terms::Terms};
use crate::tts::Tts;
use crate::watch::{self, Act, Ctx, Facts, Watch};
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
    Presence(String), // новый снимок сервиса присутствия
    Acted(mux::Done), // поток действий: исход действия сторожа в терминале агента
}

/// Как часто сверять реестр агентов с диском и /proc.
const SCAN_EVERY: Duration = Duration::from_secs(2);

/// Текст работающего агента копится столько, затем уходит на пересказ промежуточного.
const PROGRESS_EVERY: Duration = Duration::from_secs(60);

/// После новостей Ома слушает ответ столько секунд.
const LISTEN_SECS: u64 = 5;

/// После «Ты тут?» ждём ответа столько секунд, затем пользователь — «нет на месте».
const HERE_WAIT: u64 = 20;

/// Не уверена, что слушают: сначала спросить, новости — после ответа.
const ARE_YOU_THERE: &str = "Эй, ты тут? Есть новости."; // «Эй» — с ним Silero читает «ты тут» вопросом (выбор пользователя из десяти вариантов)

/// Первая фраза после долгого отсутствия, перед обзором доски.
const WELCOME: &str = "С возвращением.";

/// Итогов на доске к возвращению больше — их сжимает одна сводка модели, а не склейка подряд.
const SUMMARY_OVER: usize = 3;

/// Символов промпта пользователя в задании пересказа — хвост: длинный промпт не упрётся в предел argv `claude -p`.
const CONTEXT_MAX: usize = 1000;

/// Не запустилась — пробуем снова через OMA_RETRY.
const OMA_RETRY: Duration = Duration::from_secs(10);

/// Упавшую Ому поднимаем не сразу и всё реже (с): падающая на старте не крутится в цикле.
const OMA_BACKOFF: [u64; 4] = [10, 30, 60, 300];

/// Прожила столько — падение случайное, отсрочка снова с начала.
const OMA_STABLE: Duration = Duration::from_secs(60);

/// Хороший прогноз у окна, ещё стоящего у порога, кончает эпизод лимита не раньше, чем через столько секунд.
const LIMIT_HOLD: u64 = 600;
/// Строк экрана терминала в вопросе модели о зависании.
const SCREEN_LINES: usize = 25;



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
    trouble: usize, // агентов со срочной нерешённой проблемой — сигнал орба
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
    helper_turn: HashMap<String, bool>, // ход начал отчёт помощника (true), пока пользователь в нём не писал
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
    listen_after: Option<(&'static str, String)>, // звучат новости: по их концу Ома слушает ответ — (kind, сказанное)
    listen_at: Option<Instant>, // новости смолкли: окно ответа — когда доиграет хвост pw-cat
    presence: Presence,         // слушает ли пользователь: снимки сервиса оболочки и его слова
    here_ask: Option<u64>,      // «Ты тут?» спрошен: ответа ждём HERE_WAIT с открытия окна ответа
    seen_mode: Option<Mode>,    // режим присутствия прошлого тика — смену пишем в журнал
    here_speakers: bool,        // «Ты тут?» в наушники без ответа — спросили ещё раз в динамики: выход — на них, пока не ответит
    presence_at: u64,           // время прошлого снимка присутствия — перерыв пишем в журнал
    presence_seen: String,      // прошлые факты снимка словами — смену пишем в журнал
    welcome: bool,              // вернулся после долгого отсутствия: следующие новости — с приветствием и обзором
    night_keep: bool,           // тишина настала, пока пользователя нет: накопленное ждёт утра
    facts_src: Box<dyn Fn() -> Option<String>>, // снимок присутствия при старте; в тестах — подделка
    speakers: Box<dyn Fn() -> String>, // динамики ноутбука для Омы, пока наушники вынуты; в тестах — подделка
    bt_out: Box<dyn Fn() -> bool>, // звук по умолчанию — в Bluetooth-наушники; в тестах — подделка
    watch: Watch,              // сторож: здоровье агентов и что делать
    limits: Limits,            // проценты лимитов и прогноз
    limits_bad: bool,          // лимит кончится раньше сброса: промежуточное не пересказываем
    limits_good: Option<u64>,  // с этого момента прогноз хороший, а эпизод ещё держится
    act: Sender<mux::Job>,     // действия в терминалах агентов; в тестах — канал без потока
    next_watch: Instant,
    activity: HashMap<String, u64>, // сессия → последний хук, с
    hints: PathBuf,                  // подсказки агентам про лимит для хука PostToolUse
    facts_of: Box<dyn Fn(&Entry, u64) -> Facts>, // факты об агенте для сторожа; в тестах — подделка
    screen: fn(&Pane) -> Option<String>,         // экран терминала — в отдельном потоке, CLI не держит цикл
    shell_idle: Box<dyn Fn(u32) -> bool>,       // оболочка жива и ничего не запускает
    alive: fn(u32) -> bool,                      // процесс жив: пропавшая из реестра сессия — не падение
    trouble_sent: usize,                         // trouble последней разосланной строки состояния
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default())
}

pub fn run() {
    let data = home().join(".local/share/agent-speak");
    let cfg = Config::load();
    let runtime = PathBuf::from(std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into()));
    let shared = Arc::new(Shared::new(Queue::new(), cfg.speaker.clone(), cfg.rate.clone()));
    let tts = Tts::new(
        data.join("venv/bin/python"),
        data.join("silero_tts.py"),
        data.join("v5_ru.pt"),
        runtime.join("agent-speak-tts.sock"),
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
                    } else if p.file_name().is_some_and(|n| n == presence::FILE) {
                        if let Ok(t) = std::fs::read_to_string(&p) {
                            let _ = ftx.send(Event::Presence(t));
                        }
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
    // папка, а не файл: атомарная запись подменяет файл, и слежка за ним оборвалась бы
    if let Err(e) = watcher.watch(&runtime, RecursiveMode::NonRecursive) {
        eprintln!("agent-speak: не слежу за присутствием: {e}");
    }
    let facts = runtime.join(presence::FILE);
    let (atx, arx) = mpsc::channel::<mux::Done>();
    let act = mux::spawn(atx);
    {
        let tx = tx.clone();
        std::thread::spawn(move || {
            for d in arx {
                let _ = tx.send(Event::Acted(d));
            }
        });
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
        helper_turn: HashMap::new(),
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
        listen_after: None,
        listen_at: None,
        presence: Presence::default(),
        here_ask: None,
        seen_mode: None,
        here_speakers: false,
        presence_at: 0,
        presence_seen: String::new(),
        welcome: false,
        night_keep: false,
        facts_src: Box::new(move || std::fs::read_to_string(&facts).ok()),
        speakers: Box::new(crate::audio::speakers),
        bt_out: Box::new(crate::audio::bluetooth_out),
        watch: Watch::default(),
        limits: Limits::default(),
        limits_bad: false,
        limits_good: None,
        act,
        next_watch: Instant::now(),
        activity: HashMap::new(),
        hints: limits::hints_dir(),
        facts_of: Box::new(agent_facts),
        screen: |p| mux::screen(p).ok(),
        shell_idle: Box::new(shell_idle),
        alive: |pid| pid > 0 && Path::new(&format!("/proc/{pid}")).exists(),
        trouble_sent: 0,
    };
    st.oma_spawn();
    if let Some(t) = (st.facts_src)() {
        st.on_presence(&t);
    }
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
            Event::Presence(t) => st.on_presence(&t),
            Event::Acted(d) => st.on_acted(d),
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

/// Факты о агенте без чтения диска: то, что знает реестр.
fn blank_facts(e: &Entry, activity: u64) -> Facts {
    Facts {
        id: e.id.clone(), project: e.project.clone(), state: e.state, since: e.since, status: e.status.clone(),
        active_at: activity, helpers: 0, call: None, error: None, pane: None, shell: None, resume: String::new(),
        pid: e.pid, turn_over: false, sub_call: None,
    }
}

fn mtime(p: &Path) -> u64 {
    std::fs::metadata(p).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs())
}

fn cmdline(pid: u32) -> Vec<String> {
    let raw = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
    raw.split(|&b| b == 0).filter(|a| !a.is_empty()).map(|a| String::from_utf8_lossy(a).into_owned()).collect()
}

/// Родитель агента, если это оболочка: в ней агента можно поднять снова.
fn shell_of(pid: u32) -> Option<u32> {
    let parent = focus::ppid(pid)?;
    let comm = std::fs::read_to_string(format!("/proc/{parent}/comm")).ok()?;
    mux::SHELLS.contains(&comm.trim()).then_some(parent)
}

/// Оболочка жива и ничего не запускает — стоит на приглашении.
fn shell_idle(pid: u32) -> bool {
    let Ok(tasks) = std::fs::read_dir(format!("/proc/{pid}/task")) else { return false };
    tasks.flatten().all(|t| std::fs::read_to_string(t.path().join("children")).is_ok_and(|c| c.trim().is_empty()))
}

/// Фоновые помощники агента, которые ещё не отчитались: по хвосту транскрипта и по их метафайлам с запуска процесса (since).
fn helpers(transcript: &Path, tail: &str, since: u64) -> usize {
    claude::pending_background(tail).max(claude::running_helpers(&transcript.with_extension("").join("subagents"), since))
}

/// Старт процесса, секунды Unix: загрузка системы (btime) плюс такты от неё; нет процесса — 0.
fn proc_started(pid: u32) -> u64 {
    let Some(ticks) = agents::proc_start(pid).and_then(|t| t.parse::<u64>().ok()) else { return 0 };
    let stat = std::fs::read_to_string("/proc/stat").unwrap_or_default();
    let btime = stat.lines().find_map(|l| l.strip_prefix("btime ")?.trim().parse::<u64>().ok()).unwrap_or(0);
    // SAFETY: sysconf без побочных эффектов
    let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) }.max(1) as u64;
    btime + ticks / hz
}

/// Вызов в полёте у сабагента, которого ждёт вызов call (Agent/Task): его метафайл хранит toolUseId.
fn sub_call(transcript: &Path, call: &serde_json::Value) -> Option<serde_json::Value> {
    let dir = transcript.with_extension("").join("subagents");
    let meta = std::fs::read_dir(&dir).ok()?.flatten().map(|f| f.path()).find(|p| {
        p.to_string_lossy().ends_with(".meta.json")
            && std::fs::read_to_string(p).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()).is_some_and(|m| m["toolUseId"] == call["id"])
    })?;
    let jsonl = meta.to_string_lossy().strip_suffix(".meta.json")?.to_string() + ".jsonl";
    claude::sub_in_flight(&tail_of(&jsonl))
}

/// Факты сторожа: активность — новейшее из хуков, транскрипта и транскриптов сабагентов; у Claude — хвост транскрипта.
fn agent_facts(e: &Entry, activity: u64) -> Facts {
    let mut f = blank_facts(e, activity);
    let t = Path::new(&e.transcript);
    f.active_at = f.active_at.max(mtime(t));
    (f.pane, f.shell) = (mux::locate(e.pid), shell_of(e.pid));
    f.resume = watch::resume_cmd(e.agent, &e.id, &cmdline(e.pid), watch::RESUME_PROMPT);
    let tail = tail_of(&e.transcript);
    if e.agent == "claude" {
        for s in std::fs::read_dir(t.with_extension("").join("subagents")).into_iter().flatten().flatten() {
            f.active_at = f.active_at.max(mtime(&s.path()));
        }
        (f.helpers, f.call, f.error) = (helpers(t, &tail, proc_started(e.pid)), claude::in_flight(&tail), claude::last_api_error(&tail));
        f.turn_over = claude::turn_over(&tail);
        f.sub_call = f.call.as_ref().filter(|c| matches!(c["name"].as_str(), Some("Agent" | "Task"))).and_then(|c| sub_call(t, c));
    } else {
        (f.turn_over, f.call) = (codex::turn_over(&tail), codex::in_flight(&tail));
    }
    f
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

/// Команда Оме открыть окно ответа; speakers — спросила в динамики: слушать ноутбуком, через эхоподавление.
fn listen_cmd(kind: &str, said: &str, key: Option<String>, speakers: bool) -> serde_json::Value {
    let mut cmd = serde_json::json!({"do": "listen", "secs": LISTEN_SECS, "kind": kind, "said": said});
    if let Some(k) = key {
        cmd["key"] = k.into();
    }
    if speakers {
        cmd["speakers"] = true.into();
    }
    cmd
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
        self.night_keep &= quiet; // тишина кончилась (и вручную) — держать до утра нечего
        self.shared.queue.lock().unwrap().retain_audible(quiet);
        // новости тишины не ждут утра — кроме накопленного, пока пользователя не было; без «Озвучивать агентов» звучат только вопросы
        if quiet {
            if !self.night_keep {
                self.board.retain_classes(&[]);
            }
            // новости выброшены — окно ответа по чужой речи не откроется
            (self.listen_after, self.listen_at) = (None, None);
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
        if !self.cfg.agents_voice {
            limits::clear_hints(&self.hints, |_| false); // не следим — и подсказок агентам нет
            self.limits_bad = false;
        }
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
            trouble: self.trouble(),
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
    fn trouble(&self) -> usize {
        if self.cfg.agents_voice { self.watch.trouble() } else { 0 }
    }

    fn broadcast(&mut self) {
        let line = self.status_line();
        self.waiting_sent = self.registry.waiting();
        self.trouble_sent = self.trouble();
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
        if self.registry.waiting() != self.waiting_sent || self.trouble() != self.trouble_sent {
            self.broadcast();
        }
    }

    /// Состав живых агентов — с диска и из /proc.
    fn rescan(&mut self) {
        let live = (self.scan)();
        // агент пропал — сторож разбирает это сразу, пока простой пользователя тот же, что в миг исчезновения
        if self.registry.entries().iter().any(|e| !live.iter().any(|l| l.id == e.id)) {
            self.next_watch = (self.clock)();
        }
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
        // плохой прогноз лимита — свой расход Омы тоже: промежуточное не пересказываем
        if !self.cfg.progress_digest || !self.cfg.agents_voice || self.limits_bad {
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
        if d.ask == Ask::Hung {
            self.watch.judged(&d.session, d.important, (self.now_secs)()); // вердикт сторожу, это не новость
            return;
        }
        if d.ask == Ask::Summary {
            // сводка возвращения — от Омы, не от агента: в реестре её нет, пузырь без агента открывает чат Омы; звучит итогом и без important
            self.bubble("", &format!("final-{SUMMARY}"), "Ома", &d.say, "final");
            if !self.quiet() {
                self.board.push(News { session: d.session, class: Class::Final, say: d.say, born: (self.now_secs)() });
            }
            return;
        }
        let Some(e) = self.registry.get(&d.session) else { return };
        // устарел, пока шёл пересказ: ход кончился (промежуточное звучало бы поверх итога)
        // или на вопрос уже ответили (промпт, разрешение в терминале, конец хода)
        let stale = match d.ask {
            Ask::Progress => e.state != agents::State::Working,
            Ask::Question => e.state != agents::State::Waiting,
            Ask::Final | Ask::Step | Ask::Summary | Ask::Hung => false,
        };
        if stale {
            return;
        }
        let who = e.title.clone();
        eprintln!("agent-speak: пересказ {:?} {} ({}): {}", d.ask, e.project, &d.session[..8.min(d.session.len())], d.say);
        self.registry.summary(&d.session, &d.say);
        let kind = match d.ask {
            Ask::Final | Ask::Summary => Some("final"),
            Ask::Step => d.important.then_some("final"),
            Ask::Question => Some("question"),
            Ask::Progress => d.important.then_some("status"),
            Ask::Hung => None,
        };
        if let Some(kind) = kind {
            self.bubble(&d.session, &format!("{kind}-{}", d.session), &who, &d.say, kind);
        }
        let class = match d.ask {
            Ask::Question => Some(Class::Question),
            Ask::Final | Ask::Summary => Some(Class::Final),
            Ask::Step => d.important.then_some(Class::Final),
            Ask::Progress => d.important.then_some(Class::Progress),
            Ask::Hung => None,
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

    /// Новости — когда Ома молчит, ничего не звучит и пользователь слушает; слух греется заранее, чтобы ответ услышать сразу.
    fn speak_news(&mut self) {
        // идёт разговор или окно ответа: микрофон открыт — Ома услышала бы себя
        if self.quiet() || self.oma.talking || !self.silent() || self.shared.queue.lock().unwrap().held() {
            return;
        }
        let now = (self.now_secs)();
        let mode = self.presence.mode(now);
        if matches!(mode, Mode::Busy | Mode::Away) || (mode == Mode::Unsure && self.here_ask.is_some()) {
            // занят — срочное сторожа всё равно звучит, без окна ответа: микрофон ему ни к чему
            if mode == Mode::Busy {
                if let Some(t) = self.board.take_urgent(now) {
                    eprintln!("agent-speak: говорю (срочное, {mode:?}): {}", t.chars().take(200).collect::<String>());
                    self.enqueue(Agent::Claude, "news", &t, Kind::News);
                }
            }
            return;
        }
        // hyprctl — только когда есть что сказать
        let focused = if self.board.is_empty() { None } else { self.focused().map(|f| f.id) };
        let (kind, text) = if mode == Mode::Unsure {
            if !self.board.sayable(now, focused.as_deref()) {
                return; // «Есть новости» не врёт: после «да» прозвучит хоть что-то
            }
            self.here_ask = Some(now);
            ("here", ARE_YOU_THERE.to_string())
        } else {
            let mut parts = Vec::new();
            if std::mem::take(&mut self.welcome) {
                if !self.board.sayable(now, focused.as_deref()) {
                    return; // вводить в курс нечем — приветствие без новостей не нужно
                }
                self.board.drop_focused(focused.as_deref()); // не в обзор и не в сводку
                parts.push(WELCOME.to_string());
                parts.extend(self.board.overview());
                if self.board.finals() > SUMMARY_OVER {
                    let lines: Vec<String> = self.board.take_finals().into_iter().map(|n| n.say).collect();
                    let job = Job { session: SUMMARY.into(), project: String::new(), ask: Ask::Summary, text: lines.join("\n"), context: String::new(), fallback: Some(lines.join(" ")) };
                    let _ = self.digest.send(job);
                }
            }
            parts.extend(self.board.take(now, focused.as_deref()));
            if parts.is_empty() {
                return;
            }
            (if self.board.asked().is_some() { "question" } else { "news" }, parts.join(" "))
        };
        eprintln!("agent-speak: говорю ({kind}, {mode:?}): {}", text.chars().take(200).collect::<String>());
        if self.enqueue(Agent::Claude, "news", &text, Kind::News) > 0 {
            self.oma_send(&serde_json::json!({"do": "warm"}));
            self.listen_after = Some((kind, text));
        }
    }

    /// Ничего не звучит и не ждёт очереди.
    fn silent(&self) -> bool {
        !self.shared.busy.load(Ordering::SeqCst) && !self.shared.queue.lock().unwrap().ready()
    }

    /// Новости смолкли: busy гаснет, пока pw-cat доигрывает хвост, — окно ответа через speaker::TAIL, чтобы Ома не слышала себя.
    fn after_speech(&mut self) {
        self.listen_at = (self.listen_after.is_some() && self.silent()).then(|| (self.clock)() + speaker::TAIL);
    }

    /// Из tick: хвост доиграл и всё ещё тихо — Ома слушает ответ LISTEN_SECS.
    fn listen_if_due(&mut self, now: Instant) {
        if !self.listen_at.is_some_and(|t| now >= t) {
            return;
        }
        self.listen_at = None;
        if self.silent() {
            if let Some((kind, said)) = self.listen_after.take().filter(|_| !self.quiet()) {
                // ответ на вопрос — в чат спросившего агента; на «Ты тут?» — Оме, даже пока вопрос ждёт ответа
                let key = self.board.asked().filter(|_| kind == "question").map(|s| format!("agent:{s}"));
                let cmd = listen_cmd(kind, &said, key, kind == "here" && self.here_speakers);
                self.oma_send(&cmd);
                // холодный TTS или удержанная речь не съедают время ответа; вернувшийся, пока звучало, ответ уже дал
                if kind == "here" && self.here_ask.is_some() {
                    self.here_ask = Some((self.now_secs)());
                }
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
        (self.listen_after, self.listen_at) = (None, None); // новости оборвали сами — ответа не ждём
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
            .env("AGENT_SPEAK_BIN", std::env::current_exe().unwrap_or_default()) // Ома передаёт агентам через `agent-speak tell`
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
            if gap > presence::STALE_AFTER {
                self.presence.woke(psecs, secs); // таймеры сервиса монотонны: после сна снимок придёт не сразу
            }
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
                    // слушал — тишина стирает доску; не было на месте — не услышанное ждёт его возвращения
                    self.night_keep = self.quiet() && matches!(self.presence.mode(secs), Mode::Unsure | Mode::Away);
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
        if self.cfg.agents_voice && now >= self.next_watch {
            self.next_watch = now + Duration::from_secs(self.cfg.watch_every);
            self.watch_tick();
        }
        let due: Vec<String> = self.progress.iter().filter(|(_, (_, at))| now.saturating_duration_since(*at) >= PROGRESS_EVERY).map(|(s, _)| s.clone()).collect();
        for s in due {
            if let Some((text, _)) = self.progress.remove(&s) {
                self.ask(&s, Ask::Progress, &text, None);
            }
        }
        // «Ты тут?» отзвучал (окно ответа открыто или фраза оборвана), а ни слова, ни ввода — пользователя нет на месте
        if let Some(t) = self.here_ask.filter(|t| secs >= t + HERE_WAIT && !self.oma.talking && self.listen_after.is_none()) {
            // вставил наушники или взял звонок — слушает и молча
            if self.presence.mode(secs) == Mode::Unsure && self.presence.last_input(secs) < t {
                if !self.here_speakers && self.presence.buds() != presence::Buds::Out && (self.bt_out)() {
                    // в наушники не ответил — они, может, лежат на столе (датчик путает или молчит): ещё раз в динамики, слушать ноутбуком
                    self.here_speakers = true;
                    self.route();
                    eprintln!("agent-speak: говорю (here, в динамики): {ARE_YOU_THERE}");
                    if self.enqueue(Agent::Claude, "news", ARE_YOU_THERE, Kind::News) > 0 {
                        self.oma_send(&serde_json::json!({"do": "warm"}));
                        self.listen_after = Some(("here", ARE_YOU_THERE.to_string()));
                    }
                    self.here_ask = Some(secs);
                } else {
                    self.presence.unanswered();
                }
            }
            if self.here_ask == Some(t) {
                self.here_ask = None;
                if std::mem::take(&mut self.here_speakers) {
                    self.route(); // не ответил и в динамики — нет на месте; выход — обратно
                }
            }
        }
        let mode = self.presence.mode(secs);
        if self.seen_mode.replace(mode).is_some_and(|m| m != mode) {
            eprintln!("agent-speak: присутствие: {mode:?}");
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
            Msg::Presence { state } => {
                let now = (self.now_secs)();
                let back = match state.as_str() {
                    "here" => self.presence.said(Mode::Here, now),
                    "busy" => {
                        let back = self.presence.said(Mode::Busy, now);
                        // «не сейчас» на вопрос: он снова на доске, итоги его не ждут; без снимка новости не держатся — вопрос прозвучал бы сразу
                        if self.presence.mode(now) != Mode::Here {
                            self.board.unask();
                        }
                        back
                    }
                    "away" => self.presence.said(Mode::Away, now),
                    s => {
                        eprintln!("agent-speak: presence: неизвестно {s}");
                        None
                    }
                };
                self.on_back(back);
            }
            Msg::Subscribe => {} // приходит как Event::Subscribe
        }
        if changes {
            self.broadcast();
        }
    }

    /// Снимок сервиса присутствия; битый пропускаем — прежние факты в силе.
    fn on_presence(&mut self, text: &str) {
        let Some(f) = presence::parse_facts(text) else { return };
        let now = (self.now_secs)();
        // журнал: смена фактов и перерыв в снимках — по ним видно, почему Ома решила «он тут»
        let idle = if f.idle_since == 0 { "ввод сейчас".to_string() } else { format!("ввода нет с {}", f.idle_since) };
        let facts = format!("{idle}, блок {}, микрофон {}, камера {}, видео {}, наушники {:?}", f.locked, f.mic, f.camera, f.video, f.buds);
        let gap = f.at.saturating_sub(self.presence_at);
        if self.presence_at != 0 && gap > 45 {
            eprintln!("agent-speak: снимок присутствия: перерыв {gap} с");
        }
        if facts != self.presence_seen {
            eprintln!("agent-speak: снимок присутствия: {facts}");
        }
        (self.presence_at, self.presence_seen) = (f.at, facts);
        let back = self.presence.apply(f, now);
        self.on_back(back);
        self.route();
    }

    /// Куда говорит Ома: наушники вынуты, но подключены (звук по умолчанию идёт в них, лежащих на столе),
    /// или «Ты тут?» в наушники остался без ответа — в динамики ноутбука; иначе — выход по умолчанию.
    fn route(&mut self) {
        let sink = if self.here_speakers || self.presence.buds() == presence::Buds::Out { (self.speakers)() } else { String::new() };
        *self.shared.sink.lock().unwrap() = sink; // плеер перезапускает pw-cat, только если выход сменился
    }

    /// Пользователь вернулся (absence — сколько его не было): «Ты тут?» ответа больше не ждёт.
    /// Долгое отсутствие — приветствие, обзор и повтор неуслышанного; короткое — журнал сказанного забыт.
    fn on_back(&mut self, absence: Option<u64>) {
        let Some(a) = absence else { return };
        eprintln!("agent-speak: присутствие: вернулся, не было {a} с");
        self.here_ask = None;
        if std::mem::take(&mut self.here_speakers) {
            self.route(); // ответил на «Ты тут?» в динамиках — дальше снова в наушники
        }
        let long = a >= presence::BACK_AFTER;
        self.welcome |= long;
        self.board.returned((self.now_secs)().saturating_sub(a), long);
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
        if kind == "statusline" {
            // рисуется на каждое событие и в покое — не активность агента, только проценты лимитов
            if self.cfg.agents_voice {
                let now = (self.now_secs)();
                for w in limits::from_statusline(p) {
                    self.limits.sample(w, now);
                }
            }
            return;
        }
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
        if !id.is_empty() {
            self.activity.insert(id.to_string(), now);
        }
        match kind {
            "user-prompt-submit" => {
                let prompt = p["prompt"].as_str().unwrap_or_default();
                // отчёт помощника под видом промпта: ход оркестратора с его помощниками; слово пользователя в ходе — ход его
                let by_helper = agents::technical(prompt);
                *self.helper_turn.entry(session.clone()).or_insert(by_helper) &= by_helper;
                self.registry.prompt(&session, prompt, now);
                self.board.answered(&session); // ответили в терминале — Оме спрашивать нечего
                self.watch.prompted(&session);
                self.registry.set_health(&session, &self.watch.health(&session));
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
                    // мозг Омы отвечает по тем же вариантам и видит план целиком
                    self.registry.asking(&session, &claude::question_context(&tail, claude::ASK_MAX));
                    self.ask(&session, Ask::Question, &claude::question_context(&tail, claude::TOOL_MAX), Some(msg));
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
                // помощники в фоне — «Жду ревью» не итог, а агент работает
                let since = self.registry.get(&session).map_or(0, |e| proc_started(e.pid));
                let waiting = helpers(&transcript, &tail_of(&transcript.to_string_lossy()), since) > 0;
                if waiting {
                    self.registry.status(&session, &last, now);
                } else {
                    self.registry.finished(&session, &last, now);
                }
                self.registry.reload(&session, &last);
                self.board.answered(&session); // ход кончился — вопрос решён, хоть разрешение и дали в терминале без промпта
                self.progress.remove(&session); // промежуточное покрывает итог
                // shortcut: вопрос текстом в конце такого хода не звучит (AskUserQuestion — звучит), научить, если начнёт теряться; помощник без отчёта держит ход «работает», пока его запуск не уйдёт из хвоста 256 КБ
                if !waiting {
                    if self.helper_turn.remove(&session).unwrap_or(false) {
                        self.ask(&session, Ask::Step, &last, None); // переписка с помощниками звучит, только если важна
                    } else {
                        self.ask(&session, Ask::Final, &last, Some("готово".into()));
                    }
                } else {
                    self.helper_turn.remove(&session);
                }
                // строка — последняя в ответе: цитата фразы посреди текста (доки, сводки) — не остановка
                let line = last.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or_default();
                if line.trim_matches(|c: char| !c.is_alphanumeric()) == limits::LIMIT_STOP {
                    // встал по подсказке про лимит — разбудить после сброса; сброс неизвестен — через час
                    let until = self.limits.reset_of("claude").unwrap_or(now + 3600);
                    self.watch.limited(&session, until);
                    self.registry.set_health(&session, &self.watch.health(&session));
                }
            }
            "codex-notify" => {
                let thread = p["thread-id"].as_str().unwrap_or_default().to_string();
                let last = p["last-assistant-message"].as_str().unwrap_or_default();
                self.registry.finished(&thread, last, now);
                self.board.answered(&thread);
                self.progress.remove(&thread);
                self.ask(&thread, Ask::Final, last, Some("готово".into()));
            }
            "session-end" => self.watch.ended(&session), // вышел сам — исчезновение не падение
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
            if self.cfg.agents_voice {
                for w in limits::from_codex(&line) {
                    self.limits.sample(w, now);
                }
            }
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

    /// Раз в watch_every: факты всех агентов → сторож → действия, фразы, подсказки про лимит, здоровье в окне.
    fn watch_tick(&mut self) {
        let now = (self.now_secs)();
        let facts: Vec<Facts> = self.registry.entries().iter().map(|e| (self.facts_of)(e, self.activity.get(&e.id).copied().unwrap_or(0))).collect();
        let panes: HashMap<String, Pane> = facts.iter().filter_map(|f| f.pane.clone().map(|p| (f.id.clone(), p))).collect();
        let ctx = Ctx { now, idle: now.saturating_sub(self.presence.last_input(now)), hung_after: self.cfg.hung_after * 60, remind_after: self.cfg.remind_after * 60 };
        let (shell_idle, alive) = (&self.shell_idle, self.alive);
        let acts = self.watch.check(&ctx, facts, |pid| shell_idle(pid), alive);
        for a in acts {
            self.on_act(a, &panes);
        }
        self.hint_limits(now);
        self.refresh_health();
    }

    fn on_act(&mut self, a: Act, panes: &HashMap<String, Pane>) {
        match a {
            Act::Say { session, text, urgent } => self.watch_say(&session, &text, if urgent { Class::Urgent } else { Class::Restore }),
            Act::Remind { session, text } => self.watch_say(&session, &text, Class::Question),
            Act::Judge { session, text } => {
                // экран — CLI мультиплексора, до 10 с: в своём потоке, затем задание модели
                let (digest, pane, screen) = (self.digest.clone(), panes.get(&session).cloned(), self.screen);
                let project = self.registry.get(&session).map(|e| e.project.clone()).unwrap_or_default();
                std::thread::spawn(move || {
                    let text = match pane.as_ref().and_then(screen) {
                        Some(sc) => {
                            let lines: Vec<&str> = sc.lines().collect();
                            format!("{text}\nЭкран:\n{}", lines[lines.len().saturating_sub(SCREEN_LINES)..].join("\n"))
                        }
                        None => text,
                    };
                    let _ = digest.send(Job { session, project, ask: Ask::Hung, text, context: String::new(), fallback: Some("нет ответа модели".into()) });
                });
            }
            Act::Run(job) => {
                let _ = self.act.send(job);
            }
        }
    }

    /// Фраза сторожа: пузырь всегда, вне тишины — на доску. Не агент из реестра (лимит, закрывшийся) — пузырь Омы.
    fn watch_say(&mut self, session: &str, text: &str, class: Class) {
        eprintln!("agent-speak: сторож {class:?} ({}): {text}", &session[..8.min(session.len())]);
        let known = self.registry.get(session).map(|e| e.title.clone());
        let who = known.clone().unwrap_or_else(|| "Ома".into());
        let kind = if class == Class::Question { "question" } else { "status" };
        self.bubble(if known.is_some() { session } else { "" }, &format!("watch-{session}"), &who, text, kind);
        if !self.quiet() {
            self.board.push(News { session: session.into(), class, say: text.into(), born: (self.now_secs)() });
        }
    }

    /// Исход действия в терминале: фраза пользователю и здоровье в окне.
    fn on_acted(&mut self, d: mux::Done) {
        if let Some(a) = self.watch.done(&d.session, d.what, d.err, (self.now_secs)()) {
            self.on_act(a, &HashMap::new());
        }
        self.refresh_health();
    }

    /// Плохой прогноз — факты агентам того же аккаунта через хук; хороший — подсказки убрать.
    fn hint_limits(&mut self, now: u64) {
        let threshold = self.cfg.limit_threshold as f64;
        let bad = self.limits.forecast(now, threshold);
        // агент встал — темпа нет, прогноз «хороший»: пока окно у порога, эпизод держим LIMIT_HOLD, иначе подсказки уходят заново
        if bad.is_empty() && self.limits_bad && self.limits.near(now, threshold) && now < *self.limits_good.get_or_insert(now) + LIMIT_HOLD {
            return;
        }
        self.limits_good = None;
        self.limits_bad = !bad.is_empty();
        let mut hinted = std::collections::HashSet::new();
        for f in &bad {
            let agent = f.key.split(':').next().unwrap_or_default();
            let text = limits::hint(f);
            for e in self.registry.entries().iter().filter(|e| e.agent == agent) {
                // два плохих окна — подсказка по первому, как и прежде
                if hinted.insert(e.id.clone()) {
                    limits::put_hint(&self.hints, &e.id, &text);
                }
            }
        }
        limits::clear_hints(&self.hints, |s| hinted.contains(s));
    }

    fn refresh_health(&mut self) {
        let ids: Vec<String> = self.registry.entries().iter().map(|e| e.id.clone()).collect();
        for id in ids {
            let h = self.watch.health(&id);
            self.registry.set_health(&id, &h);
        }
        self.push_agents();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::terms::Terms;
    use crate::watch::What;

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
            helper_turn: HashMap::new(),
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
            listen_after: None,
            listen_at: None,
            presence: Presence::default(),
            here_ask: None,
            seen_mode: None,
            here_speakers: false,
            presence_at: 0,
            presence_seen: String::new(),
            welcome: false,
            night_keep: false,
            facts_src: Box::new(|| None), // снимка нет — как без сервиса присутствия
            speakers: Box::new(|| "speakers".into()),
            bt_out: Box::new(|| false),
            watch: Watch::default(),
            limits: Limits::default(),
            limits_bad: false,
            limits_good: None,
            act: mpsc::channel().0, // терминалы в тестах не трогаются
            next_watch: Instant::now() + Duration::from_secs(86_400), // tick сторожа не зовёт: тесты зовут watch_tick сами
            activity: HashMap::new(),
            hints: std::env::temp_dir().join(format!("agent-speak-hints-{}-{}", std::process::id(), HINTS_N.fetch_add(1, Ordering::SeqCst))),
            facts_of: Box::new(blank_facts),
            screen: |_| None,
            shell_idle: Box::new(|_| false),
            alive: |_| false,
            trouble_sent: 0,
        }
    }

    static HINTS_N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    fn acts(st: &mut State) -> mpsc::Receiver<mux::Job> {
        let (tx, rx) = mpsc::channel();
        st.act = tx;
        rx
    }

    fn herdr_pane() -> Option<mux::Pane> {
        Some(mux::Pane { mux: "herdr", id: "w1:p1".into(), via: "/s".into() })
    }

    fn statusline(st: &mut State, pct: f64, resets_at: u64) {
        st.on_hook("statusline", &serde_json::json!({"session_id": "a", "rate_limits": {"five_hour": {"used_percentage": pct, "resets_at": resets_at}}}));
    }

    #[test]
    fn watch_tick_runs_actions_and_speaks_outcome() {
        let mut st = test_state();
        with_agents(&mut st, &["a"]);
        let rx = acts(&mut st);
        st.facts_of = Box::new(|e, a| Facts { active_at: 1, call: Some(serde_json::json!({"name": "Agent", "input": {}})), pane: herdr_pane(), ..blank_facts(e, a) });
        st.watch_tick();
        assert_eq!(rx.try_recv().unwrap().what, What::Interrupt);
        assert_eq!(st.registry.get("a").unwrap().health, "завис");
        let mut r = sub(&mut st);
        st.on_acted(mux::Done { session: "a".into(), what: What::Interrupt, err: None });
        let b = bubbles(&mut r);
        assert_eq!((b.len(), b[0]["agent"].as_str(), b[0]["text"].as_str()), (1, Some("a"), Some("a: сабагент завис — прервала, агент перезапустит.")));
        assert_eq!(st.board.take(2000, None).unwrap(), "a: сабагент завис — прервала, агент перезапустит.");
    }

    #[test]
    fn urgent_speaks_while_busy_not_while_away() {
        let mut st = test_state();
        snap(&mut st, serde_json::json!({"at": 1000, "camera": true}));
        st.board.push(News { session: "f".into(), class: Class::Final, say: "Фича готова.".into(), born: 1 });
        st.board.push(News { session: "u".into(), class: Class::Urgent, say: "Агент завис, посмотри сам.".into(), born: 2 });
        st.speak_news();
        assert_eq!(texts(&st), ["Агент завис, посмотри сам."]);
        assert!(st.listen_after.is_none(), "занятому микрофон не открываем");
        snap(&mut st, serde_json::json!({"at": 1000, "locked": true}));
        st.board.push(News { session: "u".into(), class: Class::Urgent, say: "Агент снова завис.".into(), born: 3 });
        st.speak_news();
        assert!(texts(&st).is_empty(), "нет на месте — в пустую комнату не говорим");
    }

    #[test]
    fn statusline_forecast_writes_hints_and_mutes_progress() {
        let mut st = test_state();
        st.cfg.progress_digest = true;
        with_agents(&mut st, &["a", "b"]);
        let now = Arc::new(Mutex::new(1000u64));
        let n = now.clone();
        st.now_secs = Box::new(move || *n.lock().unwrap());
        statusline(&mut st, 93.0, 99_999);
        *now.lock().unwrap() = 1300;
        statusline(&mut st, 96.0, 99_999);
        st.watch_tick();
        let hint = std::fs::read_to_string(st.hints.join("b")).unwrap();
        assert!(hint.contains("96%") && hint.contains("Остановился по лимиту"), "{hint}");
        md(&mut st, "a", "Делаю экран. ");
        assert!(st.progress.is_empty(), "плохой прогноз — промежуточное не копим");
        *now.lock().unwrap() = 1400;
        statusline(&mut st, 10.0, 200_000);
        st.watch_tick();
        assert!(!st.hints.join("b").exists() && !st.limits_bad);
        let _ = std::fs::remove_dir_all(&st.hints);
    }

    #[test]
    fn paused_agent_keeps_the_limit_episode() {
        let mut st = test_state();
        with_agents(&mut st, &["a", "b"]);
        let now = Arc::new(Mutex::new(1000u64));
        let n = now.clone();
        st.now_secs = Box::new(move || *n.lock().unwrap());
        statusline(&mut st, 93.0, 99_999);
        *now.lock().unwrap() = 1300;
        statusline(&mut st, 96.0, 99_999);
        st.watch_tick();
        assert!(crate::limits::take_hint(&st.hints, "b", "main").is_some());
        *now.lock().unwrap() = 2201; // агент встал: процент тот же, темпа нет — прогноз «хороший»
        statusline(&mut st, 96.0, 99_999);
        st.watch_tick();
        assert!(st.limits_bad && st.hints.join("b").exists(), "пауза не кончает эпизод");
        assert_eq!(crate::limits::take_hint(&st.hints, "b", "main"), None, "и не шлёт подсказку заново");
        *now.lock().unwrap() = 2201 + 600;
        st.watch_tick();
        assert!(!st.limits_bad && !st.hints.join("b").exists(), "хороший прогноз продержался — эпизод кончен");
        let _ = std::fs::remove_dir_all(&st.hints);
    }

    #[test]
    fn stop_with_limit_line_marks_limited() {
        let mut st = test_state();
        with_agents(&mut st, &["a"]);
        let rx = jobs(&mut st);
        statusline(&mut st, 96.0, 99_999);
        stop(&mut st, "a", "Записал в доку, что осталось.\nОстановился по лимиту");
        assert!(st.registry.get("a").unwrap().health.starts_with("лимит до "));
        assert_eq!(rx.try_recv().unwrap().ask, Ask::Final, "итог по-прежнему пересказывается");
    }

    #[test]
    fn hung_digest_goes_to_watch_not_board() {
        let mut st = test_state();
        with_agents(&mut st, &["a"]);
        let (rx, arx) = (jobs(&mut st), acts(&mut st));
        st.facts_of = Box::new(|e, a| Facts { active_at: 1, call: Some(serde_json::json!({"name": "Bash", "input": {"command": "cargo build"}})), pane: herdr_pane(), ..blank_facts(e, a) });
        st.screen = |_| Some("Compiling agent-speak".into());
        st.watch_tick();
        let job = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!((job.ask, job.session.as_str()), (Ask::Hung, "a"));
        assert!(job.text.contains("cargo build") && job.text.contains("Экран:\nCompiling agent-speak"), "{}", job.text);
        st.on_digest(dg("a", Ask::Hung, true, "висит"));
        assert!(st.board.is_empty());
        st.watch_tick();
        assert_eq!(arx.try_recv().unwrap().what, What::Interrupt);
    }

    #[test]
    fn agents_voice_off_stops_watch_and_clears_hints() {
        let mut st = test_state();
        with_agents(&mut st, &["a"]);
        let rx = acts(&mut st);
        st.facts_of = Box::new(|e, a| Facts { active_at: 1, call: Some(serde_json::json!({"name": "Agent", "input": {}})), pane: herdr_pane(), ..blank_facts(e, a) });
        crate::limits::put_hint(&st.hints, "a", "лимит");
        st.cfg.agents_voice = false;
        st.apply();
        assert!(!st.hints.join("a").exists());
        st.next_watch = Instant::now();
        st.tick();
        assert!(rx.try_recv().is_err(), "сторож выключен вместе со слежкой");
    }

    #[test]
    fn trouble_in_status_line() {
        let mut st = test_state();
        with_agents(&mut st, &["a"]);
        st.facts_of = Box::new(|e, a| Facts { active_at: 1, call: Some(serde_json::json!({"name": "Bash", "input": {"command": "git push"}})), pane: herdr_pane(), ..blank_facts(e, a) });
        let mut r = sub(&mut st);
        st.watch_tick();
        let lines = drain(&mut r);
        assert!(lines.iter().any(|v| v["running"] == true && v["trouble"] == 1), "{lines:?}");
        assert_eq!(st.board.take(2000, None).unwrap(), "a: агент завис посреди push — не трогаю, посмотри сам.");
    }

    #[test]
    fn hooks_count_as_activity_statusline_not() {
        let mut st = test_state();
        with_agents(&mut st, &["a"]);
        statusline(&mut st, 10.0, 99_999);
        assert!(!st.activity.contains_key("a"));
        md(&mut st, "a", "Делаю. ");
        assert_eq!(st.activity.get("a"), Some(&1000));
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
            "{\"running\":true,\"speaking\":false,\"paused\":false,\"speaker\":\"xenia\",\"rate\":\"medium\",\"project\":\"\",\"oma\":\"off\",\"talking\":false,\"agents_voice\":true,\"progress_digest\":false,\"quiet\":false,\"quiet_schedule\":true,\"quiet_from\":\"23:00\",\"quiet_to\":\"08:00\",\"orb_pos\":\"bottom-right,0,0\",\"orb_over_fullscreen\":true,\"waiting\":0,\"trouble\":0}\n"
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
        assert!(texts(&st).is_empty() && st.listen_after.is_none()); // тумблер — и способ замолчать сейчас
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
    fn stop_with_pending_helpers_is_not_a_final() {
        let dir = std::env::temp_dir().join(format!("agent-speak-pending-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.jsonl");
        let line = serde_json::json!({"type": "user", "message": {"content": [{"type": "tool_result", "content": [{"type": "text", "text": "Async agent launched successfully.\nagentId: aaa (internal ID)"}]}]}, "toolUseResult": {"isAsync": true, "status": "async_launched", "agentId": "aaa"}});
        std::fs::write(&path, format!("{line}\n")).unwrap();
        let mut st = test_state();
        with_agents(&mut st, &["s"]);
        let rx = jobs(&mut st);
        st.on_hook("stop", &serde_json::json!({"session_id": "s", "transcript_path": path, "last_assistant_message": "Жду ревью 7."}));
        assert!(rx.try_recv().is_err(), "модель не звана");
        let a = st.registry.get("s").unwrap();
        assert_eq!((a.state, a.status.as_str()), (agents::State::Working, "Жду ревью 7."));
        let _ = std::fs::remove_dir_all(dir);
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
    fn helper_report_turn_speaks_only_if_important() {
        let mut st = test_state();
        with_agents(&mut st, &["s"]);
        let rx = jobs(&mut st);
        let report = serde_json::json!({"session_id": "s", "prompt": "<task-notification> <task-id>a1</task-id> <status>completed</status> </task-notification>"});
        st.on_hook("user-prompt-submit", &report);
        stop(&mut st, "s", "Ревьювер принял ответ, исполнитель закрывает находку.");
        let job = rx.try_recv().unwrap();
        assert_eq!((job.ask, job.fallback), (Ask::Step, None), "ход начал отчёт помощника, а не пользователь; без модели — молчим");
        st.on_digest(dg("s", Ask::Step, false, "Ревьювер принял ответ."));
        assert!(st.board.is_empty(), "переписка оркестратора с помощниками — не новость");
        st.on_digest(dg("s", Ask::Step, true, "Вся работа готова и влита."));
        assert!(!st.board.is_empty(), "работа целиком готова — звучит");
        st.on_hook("user-prompt-submit", &report);
        st.on_hook("user-prompt-submit", &serde_json::json!({"session_id": "s", "prompt": "а что с тестами?"}));
        stop(&mut st, "s", "Тесты зелёные.");
        assert_eq!(rx.try_recv().unwrap().ask, Ask::Final, "пользователь написал посреди хода — итог его");
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
        assert!(st.listen_after.is_some(), "хвост не доиграл — окно не открываем");
        *env.1.lock().unwrap() += speaker::TAIL;
        st.tick();
        assert!(st.listen_after.is_none());
        st.after_speech(); // окно уже открыто — второй раз не открываем
        *env.1.lock().unwrap() += speaker::TAIL;
        st.tick();
        drop(st); // stdin закрыт → cat допишет и выйдет
        let want = "{\"do\":\"warm\"}\n{\"do\":\"listen\",\"kind\":\"news\",\"said\":\"plug: Фича готова.\",\"secs\":5}\n";
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
        let now = secs_at(&mut st, 1000);
        with_agents(&mut st, &["s", "t"]);
        st.voice_cmd = vec!["sh".into(), "-c".into(), format!("cat > {}", out.display())];
        st.oma_spawn();
        question(&mut st, "s", true, "Удалить старый код?");
        st.tick();
        heard(&mut st, &env);
        // ответа на вопрос уже не ждём, но он ещё числится озвученным, а пользователь затих: ответ на «Ты тут?» — Оме, не спросившему агенту
        let t = 1000 + crate::news::ANSWER_WAIT;
        *now.lock().unwrap() = t;
        snap(&mut st, serde_json::json!({"at": t, "idle_since": 1000}));
        st.on_digest(dg("t", Ask::Final, false, "Фича готова."));
        st.tick();
        heard(&mut st, &env);
        drop(st);
        let end = Instant::now() + Duration::from_secs(2);
        let listens = || -> Vec<serde_json::Value> {
            std::fs::read_to_string(&out).unwrap_or_default().lines().filter(|l| l.contains("listen")).filter_map(|l| serde_json::from_str(l).ok()).collect()
        };
        while listens().len() < 2 && Instant::now() < end { std::thread::sleep(Duration::from_millis(20)); }
        assert_eq!(listens(), [
            serde_json::json!({"do": "listen", "secs": 5, "kind": "question", "said": "plug: Удалить старый код?", "key": "agent:s"}),
            serde_json::json!({"do": "listen", "secs": 5, "kind": "here", "said": ARE_YOU_THERE}),
        ]);
    }

    #[test]
    fn stopped_or_muted_news_opens_no_listen_window() {
        let mut st = test_state();
        st.cfg.agents_voice = true;
        with_agents(&mut st, &["s"]);
        let news = |st: &mut State| {
            st.on_digest(dg("s", Ask::Final, false, "Фича готова."));
            st.tick();
            assert!(st.listen_after.is_some());
        };
        news(&mut st);
        pop1(&st);
        st.after_speech();
        assert!(st.listen_at.is_some());
        st.on_msg(Msg::Stop);
        assert!(st.listen_after.is_none() && st.listen_at.is_none());
        news(&mut st);
        st.cfg.quiet_override = "on".into();
        st.apply(); // тишина настала раньше, чем новость зазвучала: она выброшена
        assert!(st.listen_after.is_none() && texts(&st).is_empty());
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
        assert_eq!(st.registry.get("s").unwrap().ask, job.text, "мозг Омы видит то же, что пересказ");
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

    // --- присутствие пользователя ---

    /// Часы эпохи под рукой теста: снимок присутствия старше STALE_AFTER — «фактов нет».
    fn secs_at(st: &mut State, t: u64) -> Arc<Mutex<u64>> {
        let now = Arc::new(Mutex::new(t));
        let n = now.clone();
        st.now_secs = Box::new(move || *n.lock().unwrap());
        now
    }

    /// Агенты s и t, часы эпохи с t.
    fn presence_state(t: u64) -> (State, Arc<Mutex<u64>>) {
        let mut st = test_state();
        with_agents(&mut st, &["s", "t"]);
        let now = secs_at(&mut st, t);
        (st, now)
    }

    /// Снимок от сервиса присутствия.
    fn snap(st: &mut State, v: serde_json::Value) {
        st.on_presence(&v.to_string());
    }

    fn final_s(st: &mut State) {
        st.on_digest(dg("s", Ask::Final, false, "Фича готова."));
    }

    #[test]
    fn presence_file_change_reaches_state() {
        let mut st = test_state();
        st.on_presence(r#"{"at":1000,"idle_since":300}"#);
        assert_eq!(st.presence.mode(1000), Mode::Away);
        st.on_presence("{");
        assert_eq!(st.presence.mode(1000), Mode::Away);
    }

    #[test]
    fn away_user_hears_nothing() {
        let (mut st, _now) = presence_state(1700);
        snap(&mut st, serde_json::json!({"at": 1700, "idle_since": 1000}));
        final_s(&mut st);
        st.tick();
        assert!(texts(&st).is_empty());
    }

    #[test]
    fn unsure_asks_are_you_there_once() {
        let (mut st, _now) = presence_state(1100);
        snap(&mut st, serde_json::json!({"at": 1100, "idle_since": 1000}));
        st.tick();
        assert!(texts(&st).is_empty(), "доска пуста — спрашивать незачем");
        final_s(&mut st);
        st.tick();
        assert_eq!(texts(&st).join(" "), ARE_YOU_THERE); // в очереди — по предложению
        assert_eq!(st.listen_after, Some(("here", ARE_YOU_THERE.to_string())));
        st.tick();
        assert!(texts(&st).is_empty());
    }

    /// Сказанное отзвучало: хвост pw-cat доиграл, окно ответа открыто.
    fn heard(st: &mut State, env: &Env) {
        texts(st);
        st.after_speech();
        *env.1.lock().unwrap() += speaker::TAIL;
        st.tick();
    }

    /// Пользователь простаивает 100 с, итог на доске: Ома спросила «Ты тут?» (фраза ещё звучит).
    fn asked_are_you_there() -> (State, Env, Arc<Mutex<u64>>) {
        let (mut st, env) = env_state();
        with_agents(&mut st, &["s", "t"]);
        let now = secs_at(&mut st, 1100);
        snap(&mut st, serde_json::json!({"at": 1100, "idle_since": 1000}));
        final_s(&mut st);
        st.tick();
        assert_eq!(texts(&st).join(" "), ARE_YOU_THERE);
        (st, env, now)
    }

    #[test]
    fn bluetooth_out_without_sensor_asks_again_through_speakers() {
        // датчик молчит («неизвестно»), а звук идёт в Bluetooth-наушники — они могут лежать на столе
        let (mut st, env) = env_state();
        st.bt_out = Box::new(|| true);
        with_agents(&mut st, &["s", "t"]);
        let now = secs_at(&mut st, 1100);
        snap(&mut st, serde_json::json!({"at": 1100, "idle_since": 1000}));
        final_s(&mut st);
        st.tick();
        heard(&mut st, &env);
        *now.lock().unwrap() += HERE_WAIT;
        st.tick();
        assert_eq!(texts(&st).join(" "), ARE_YOU_THERE);
        assert_eq!(*st.shared.sink.lock().unwrap(), "speakers");
    }

    #[test]
    fn laptop_speakers_ask_once() {
        let (mut st, env, now) = asked_are_you_there(); // звук и так в динамиках ноутбука
        heard(&mut st, &env);
        *now.lock().unwrap() += HERE_WAIT;
        st.tick();
        assert!(texts(&st).is_empty() && !st.here_speakers, "второй раз спрашивать некуда");
        assert_eq!(st.presence.mode(*now.lock().unwrap()), Mode::Away);
    }

    #[test]
    fn buds_in_unanswered_asks_again_through_speakers() {
        let (mut st, env) = env_state();
        with_agents(&mut st, &["s", "t"]);
        st.bt_out = Box::new(|| true);
        let now = secs_at(&mut st, 1100);
        snap(&mut st, serde_json::json!({"at": 1100, "idle_since": 1000, "buds": "in"}));
        final_s(&mut st);
        st.tick();
        assert_eq!(texts(&st).join(" "), ARE_YOU_THERE, "наушники «в ушах» — тоже спросить: они, может, лежат на столе");
        assert_eq!(*st.shared.sink.lock().unwrap(), "", "сначала — в наушники");
        heard(&mut st, &env);
        *now.lock().unwrap() += HERE_WAIT;
        st.tick();
        assert_eq!(texts(&st).join(" "), ARE_YOU_THERE, "в наушники не ответил — ещё раз, в динамики");
        assert_eq!((st.shared.sink.lock().unwrap().as_str(), st.here_speakers), ("speakers", true));
        heard(&mut st, &env);
        *now.lock().unwrap() += HERE_WAIT;
        st.tick();
        assert_eq!(st.presence.mode(*now.lock().unwrap()), Mode::Away);
        assert!(texts(&st).is_empty());
        assert_eq!((st.shared.sink.lock().unwrap().as_str(), st.here_speakers), ("", false), "выход — обратно в наушники");
    }

    #[test]
    fn answer_to_speakers_ask_returns_to_buds() {
        let (mut st, env) = env_state();
        with_agents(&mut st, &["s", "t"]);
        st.bt_out = Box::new(|| true);
        let now = secs_at(&mut st, 1100);
        snap(&mut st, serde_json::json!({"at": 1100, "idle_since": 1000, "buds": "in"}));
        final_s(&mut st);
        st.tick();
        heard(&mut st, &env);
        *now.lock().unwrap() += HERE_WAIT;
        st.tick();
        assert!(st.here_speakers);
        st.on_msg(Msg::Presence { state: "here".into() });
        assert_eq!((st.shared.sink.lock().unwrap().as_str(), st.here_speakers), ("", false));
    }

    #[test]
    fn listen_command_marks_speakers() {
        assert_eq!(listen_cmd("here", "т", None, true), serde_json::json!({"do": "listen", "secs": LISTEN_SECS, "kind": "here", "said": "т", "speakers": true}));
        assert_eq!(listen_cmd("question", "т", Some("agent:s".into()), false)["key"], "agent:s");
        assert!(listen_cmd("news", "т", None, false).get("speakers").is_none());
    }

    #[test]
    fn are_you_there_unanswered_is_away() {
        let (mut st, env, now) = asked_are_you_there();
        *now.lock().unwrap() += HERE_WAIT;
        st.tick();
        assert_eq!(st.presence.mode(1100 + HERE_WAIT), Mode::Unsure, "фраза ещё звучит — ответа не ждали");
        heard(&mut st, &env); // окно ответа открылось: HERE_WAIT — с него
        *now.lock().unwrap() += HERE_WAIT - 1;
        st.tick();
        assert_eq!(st.presence.mode(1100 + 2 * HERE_WAIT - 1), Mode::Unsure);
        *now.lock().unwrap() += 1;
        st.tick();
        assert_eq!(st.presence.mode(1100 + 2 * HERE_WAIT), Mode::Away);
        assert!(texts(&st).is_empty());
    }

    #[test]
    fn buds_in_after_are_you_there_is_not_away() {
        let (mut st, env, now) = asked_are_you_there();
        heard(&mut st, &env);
        snap(&mut st, serde_json::json!({"at": 1105, "idle_since": 1000, "buds": "out"})); // лежали вынутыми
        *now.lock().unwrap() = 1110;
        snap(&mut st, serde_json::json!({"at": 1110, "idle_since": 1000, "buds": "in"})); // вставил наушники молча
        st.tick();
        assert_eq!(texts(&st), ["плуг: Фича готова."]);
        heard(&mut st, &env);
        *now.lock().unwrap() = 1110 + HERE_WAIT;
        st.tick();
        assert_eq!(st.presence.mode(1110 + HERE_WAIT), Mode::Here);
    }

    #[test]
    fn unsure_with_only_focused_final_asks_nothing() {
        let (mut st, env) = env_state();
        with_agents(&mut st, &["s", "t"]);
        let _now = secs_at(&mut st, 1100);
        *env.0.lock().unwrap() = Some(sref("s"));
        snap(&mut st, serde_json::json!({"at": 1100, "idle_since": 1000}));
        final_s(&mut st);
        st.tick();
        assert!(texts(&st).is_empty() && st.here_ask.is_none(), "сказать нечего — и «Ты тут?» не звучит");
        assert!(!st.board.is_empty(), "его, может, нет на месте — итог агента в фокусе не снят");
    }

    #[test]
    fn voice_here_after_are_you_there_speaks_news() {
        let (mut st, _env, _now) = asked_are_you_there();
        st.on_msg(Msg::Presence { state: "here".into() });
        st.tick();
        assert_eq!(texts(&st), ["плуг: Фича готова."]);
    }

    #[test]
    fn busy_holds_news_and_releases_without_greeting() {
        let (mut st, _now) = presence_state(1000);
        snap(&mut st, serde_json::json!({"at": 1000, "idle_since": 990, "video": true}));
        final_s(&mut st);
        st.tick();
        assert!(texts(&st).is_empty());
        snap(&mut st, serde_json::json!({"at": 1000, "idle_since": 990}));
        st.tick();
        assert_eq!(texts(&st), ["плуг: Фича готова."]);
    }

    #[test]
    fn video_ends_after_long_idle_speaks_without_greeting() {
        let (mut st, now) = presence_state(1000);
        snap(&mut st, serde_json::json!({"at": 1000, "idle_since": 990, "video": true}));
        final_s(&mut st);
        st.tick();
        *now.lock().unwrap() = 1900;
        snap(&mut st, serde_json::json!({"at": 1900, "idle_since": 990, "video": true}));
        st.tick();
        assert!(texts(&st).is_empty());
        snap(&mut st, serde_json::json!({"at": 1900, "idle_since": 990})); // видео смолкло само, мышь не шевелилась
        assert_eq!(st.presence.mode(1900), Mode::Here);
        st.tick();
        assert_eq!(texts(&st), ["плуг: Фича готова."]);
    }

    #[test]
    fn not_now_after_question_requeues_it() {
        let (mut st, now) = presence_state(1000);
        snap(&mut st, serde_json::json!({"at": 1000, "idle_since": 0}));
        question(&mut st, "s", true, "Удалить старый код?");
        st.tick();
        assert_eq!(texts(&st), ["плуг: Удалить старый код?"]);
        st.on_msg(Msg::Presence { state: "busy".into() });
        st.on_digest(dg("t", Ask::Final, false, "Фича готова."));
        st.tick();
        assert!(texts(&st).is_empty());
        *now.lock().unwrap() += presence::BUSY_FOR;
        snap(&mut st, serde_json::json!({"at": 1000 + presence::BUSY_FOR, "idle_since": 0}));
        st.tick();
        assert_eq!(texts(&st), ["плуг: Удалить старый код?"]); // итог ждёт ответа на вопрос
    }

    #[test]
    fn not_now_without_presence_file_does_not_repeat_question() {
        let mut st = test_state();
        with_agents(&mut st, &["s"]);
        question(&mut st, "s", true, "Удалить старый код?");
        st.tick();
        pop1(&st);
        st.on_msg(Msg::Presence { state: "busy".into() });
        st.tick();
        assert!(texts(&st).is_empty());
    }

    #[test]
    fn buds_in_ear_no_longer_beats_idle() {
        // «в ушах» читается и у наушника на столе датчиком вниз: 11 минут без ввода — нет на месте, молчит
        let (mut st, _now) = presence_state(1700);
        snap(&mut st, serde_json::json!({"at": 1700, "idle_since": 1000, "buds": "in"}));
        final_s(&mut st);
        st.tick();
        assert!(texts(&st).is_empty());
    }

    #[test]
    fn focused_agent_final_not_spoken() {
        let (mut st, env) = env_state();
        with_agents(&mut st, &["s"]);
        *env.0.lock().unwrap() = Some(sref("s"));
        final_s(&mut st);
        st.tick();
        assert!(texts(&st).is_empty());
        question(&mut st, "s", true, "Удалить старый код?");
        st.tick();
        assert!(texts(&st).is_empty(), "вопрос ждёт, пока агент в фокусе");
        *env.0.lock().unwrap() = None;
        *env.1.lock().unwrap() += Duration::from_secs(1); // прежний фокус в кэше устарел
        st.tick();
        assert_eq!(texts(&st), ["плуг: Удалить старый код?"]); // итог снят без речи
    }

    #[test]
    fn no_presence_file_behaves_as_before() {
        let (mut st, now) = presence_state(1000);
        final_s(&mut st);
        st.tick();
        assert_eq!(texts(&st), ["плуг: Фича готова."]);
        snap(&mut st, serde_json::json!({"at": 1000, "idle_since": 300})); // сервис замолк на «нет на месте»
        *now.lock().unwrap() += presence::STALE_AFTER / 2;
        st.tick(); // демон тикает, а не спит: разрыв без tick — сон, снимок бы продлился
        *now.lock().unwrap() += presence::STALE_AFTER / 2 + 1;
        st.on_digest(dg("t", Ask::Final, false, "Тесты зелёные."));
        st.tick();
        assert_eq!(texts(&st), ["плуг: Тесты зелёные."]);
    }

    // --- возвращение ---

    const GREETING: [&str; 3] = ["С возвращением.", "Пока тебя не было: один проект закончил.", "плуг: Фича готова."];

    #[test]
    fn return_after_long_absence_greets_with_overview() {
        let (mut st, now) = presence_state(1600);
        snap(&mut st, serde_json::json!({"at": 1600, "idle_since": 1000}));
        final_s(&mut st);
        st.tick();
        assert!(texts(&st).is_empty(), "нет на месте — молчит");
        *now.lock().unwrap() = 1700;
        snap(&mut st, serde_json::json!({"at": 1700, "idle_since": 0}));
        st.tick();
        assert_eq!(texts(&st), GREETING);
    }

    #[test]
    fn news_spoken_into_void_repeated_on_return() {
        let (mut st, now) = presence_state(1050);
        snap(&mut st, serde_json::json!({"at": 1050, "idle_since": 1000}));
        final_s(&mut st);
        st.tick();
        assert_eq!(texts(&st), ["плуг: Фича готова."]);
        *now.lock().unwrap() = 1300;
        snap(&mut st, serde_json::json!({"at": 1300, "idle_since": 0}));
        st.tick();
        assert_eq!(texts(&st), GREETING);
    }

    #[test]
    fn news_into_buds_on_desk_repeated_on_return() {
        // «в ушах» — не «точно слышит»: сказанное в лежащие на столе наушники повторится после возвращения
        let (mut st, now) = presence_state(1040);
        snap(&mut st, serde_json::json!({"at": 1040, "idle_since": 1000, "buds": "in"}));
        final_s(&mut st);
        st.tick();
        assert_eq!(texts(&st), ["плуг: Фича готова."]);
        *now.lock().unwrap() = 1300;
        snap(&mut st, serde_json::json!({"at": 1300, "idle_since": 0, "buds": "in"}));
        st.tick();
        assert_eq!(texts(&st), GREETING);
    }

    #[test]
    fn listener_sitting_still_hears_no_repeat() {
        let (mut st, now) = presence_state(1050);
        snap(&mut st, serde_json::json!({"at": 1050, "idle_since": 1000}));
        final_s(&mut st);
        st.tick();
        assert_eq!(texts(&st), ["плуг: Фича готова."]);
        *now.lock().unwrap() = 1150;
        snap(&mut st, serde_json::json!({"at": 1150, "idle_since": 0}));
        st.tick();
        assert!(texts(&st).is_empty());
    }

    #[test]
    fn short_absence_no_greeting() {
        let (mut st, _now) = presence_state(1120);
        snap(&mut st, serde_json::json!({"at": 1120, "idle_since": 1000}));
        final_s(&mut st); // без tick: «не уверена» с новостью спросила бы «Ты тут?»
        snap(&mut st, serde_json::json!({"at": 1120, "idle_since": 0}));
        st.tick();
        assert_eq!(texts(&st), ["плуг: Фича готова."]);
    }

    #[test]
    fn locked_then_unlocked_is_return() {
        let (mut st, now) = presence_state(1000);
        snap(&mut st, serde_json::json!({"at": 1000, "idle_since": 1000, "locked": true}));
        final_s(&mut st);
        st.tick();
        assert!(texts(&st).is_empty());
        *now.lock().unwrap() = 1400;
        snap(&mut st, serde_json::json!({"at": 1400, "idle_since": 0}));
        st.tick();
        assert_eq!(texts(&st), GREETING);
    }

    #[test]
    fn wake_from_sleep_keeps_lock_until_fresh_snapshot() {
        let (mut st, now) = presence_state(1000);
        snap(&mut st, serde_json::json!({"at": 1000, "idle_since": 990, "locked": true}));
        final_s(&mut st);
        st.tick();
        *now.lock().unwrap() = 1000 + 3600; // ноутбук спал: сервис ещё не написал снимок
        st.tick();
        assert!(texts(&st).is_empty(), "до свежего снимка экран всё ещё заблокирован");
        snap(&mut st, serde_json::json!({"at": 1000 + 3600, "idle_since": 0}));
        st.tick();
        assert_eq!(texts(&st), GREETING);
    }

    /// Пользователя нет на месте с 1000, агент s в фокусе.
    fn away_with_focus_on_s(ids: &[&str]) -> (State, Arc<Mutex<u64>>) {
        let (mut st, env) = env_state();
        with_agents(&mut st, ids);
        let now = secs_at(&mut st, 1600);
        *env.0.lock().unwrap() = Some(sref("s"));
        snap(&mut st, serde_json::json!({"at": 1600, "idle_since": 1000}));
        (st, now)
    }

    #[test]
    fn return_with_only_focused_final_no_greeting() {
        let (mut st, now) = away_with_focus_on_s(&["s", "t"]);
        final_s(&mut st);
        st.tick();
        *now.lock().unwrap() = 1700;
        snap(&mut st, serde_json::json!({"at": 1700, "idle_since": 0}));
        st.tick();
        assert!(texts(&st).is_empty(), "итог агента в фокусе не звучит — и вводить в курс нечем");
    }

    #[test]
    fn return_overview_and_summary_skip_focused_agent() {
        let ids = ["a", "b", "c", "d", "s"];
        let (mut st, now) = away_with_focus_on_s(&ids);
        let rx = jobs(&mut st);
        for id in ids {
            st.on_digest(dg(id, Ask::Final, false, "Готово."));
        }
        st.tick();
        *now.lock().unwrap() = 1700;
        snap(&mut st, serde_json::json!({"at": 1700, "idle_since": 0}));
        st.tick();
        assert_eq!(texts(&st), ["С возвращением.", "Пока тебя не было: четыре проекта закончили."]);
        assert_eq!(rx.try_recv().unwrap().text.lines().count(), 4);
    }

    /// Четыре итога, пока пользователя не было, и он вернулся: приветствие с обзором, итоги ушли на сводку.
    fn back_to_four_finals() -> (State, mpsc::Receiver<Job>) {
        let mut st = test_state();
        let ids = ["a", "b", "c", "d"];
        with_agents(&mut st, &ids);
        let now = secs_at(&mut st, 1600);
        let rx = jobs(&mut st);
        snap(&mut st, serde_json::json!({"at": 1600, "idle_since": 1000}));
        for (id, say) in ids.iter().zip(["Фича готова.", "Тесты зелёные.", "Баг найден.", "Сборка прошла."]) {
            st.on_digest(dg(id, Ask::Final, false, say));
        }
        st.tick();
        assert!(texts(&st).is_empty());
        *now.lock().unwrap() = 1700;
        snap(&mut st, serde_json::json!({"at": 1700, "idle_since": 0}));
        st.tick();
        assert_eq!(texts(&st), ["С возвращением.", "Пока тебя не было: четыре проекта закончили."]);
        (st, rx)
    }

    #[test]
    fn long_absence_summarizes_finals_once() {
        let (mut st, rx) = back_to_four_finals();
        let job = rx.try_recv().unwrap();
        assert_eq!((job.session.as_str(), job.ask, job.text.lines().count()), (SUMMARY, Ask::Summary, 4));
        st.tick();
        assert!(rx.try_recv().is_err() && texts(&st).is_empty(), "сводка — одна, итоги не звучат по отдельности");
        let mut r = sub(&mut st);
        st.on_digest(Digest { session: job.session, project: job.project, ask: Ask::Summary, important: true, say: "Четыре проекта готовы.".into() });
        let b = bubbles(&mut r);
        assert_eq!((b.len(), b[0]["who"].as_str(), b[0]["kind"].as_str(), b[0]["agent"].as_str()), (1, Some("Ома"), Some("final"), Some("")));
        st.tick();
        assert_eq!(texts(&st), ["Четыре проекта готовы."]);
    }

    #[test]
    fn summary_without_model_speaks_finals() {
        let (mut st, rx) = back_to_four_finals();
        let d = digest::digest(rx.try_recv().unwrap(), |_| None).unwrap(); // модель не ответила — склейка, important false
        st.on_digest(d);
        st.tick();
        assert_eq!(texts(&st), ["плуг: Фича готова.", "плуг: Тесты зелёные.", "плуг: Баг найден.", "плуг: Сборка прошла."]);
    }

    /// Минуты местного времени под рукой теста: граница тихого расписания.
    fn mins_at(st: &mut State, hm: &str) -> Arc<Mutex<u32>> {
        let m = Arc::new(Mutex::new(crate::config::hhmm(hm).unwrap()));
        let mm = m.clone();
        st.minutes = Box::new(move || *mm.lock().unwrap());
        m
    }

    #[test]
    fn night_keeps_board_when_away_until_morning() {
        let (mut st, now) = presence_state(1700);
        let min = mins_at(&mut st, "22:59");
        snap(&mut st, serde_json::json!({"at": 1700, "idle_since": 1000}));
        final_s(&mut st);
        st.tick();
        *min.lock().unwrap() = crate::config::hhmm("23:00").unwrap();
        *now.lock().unwrap() = 1701;
        st.tick();
        assert!(st.quiet() && !st.board.is_empty(), "тишина, пока пользователя нет, доску не стирает");
        *min.lock().unwrap() = crate::config::hhmm("08:00").unwrap();
        *now.lock().unwrap() = 1701 + 9 * 3600;
        snap(&mut st, serde_json::json!({"at": 1701 + 9 * 3600, "idle_since": 1000}));
        st.tick();
        assert!(texts(&st).is_empty(), "утро, но пользователя нет");
        snap(&mut st, serde_json::json!({"at": 1701 + 9 * 3600, "idle_since": 0}));
        st.tick();
        assert_eq!(texts(&st), GREETING);
    }

    #[test]
    fn night_clears_board_when_here() {
        let (mut st, now) = presence_state(1000);
        let min = mins_at(&mut st, "22:59");
        snap(&mut st, serde_json::json!({"at": 1000, "idle_since": 0}));
        st.tick();
        final_s(&mut st); // после tick: на месте — прозвучал бы до тишины
        *min.lock().unwrap() = crate::config::hhmm("23:00").unwrap();
        *now.lock().unwrap() = 1001;
        st.tick();
        assert!(st.board.is_empty());
        *min.lock().unwrap() = crate::config::hhmm("08:00").unwrap();
        *now.lock().unwrap() = 1002;
        snap(&mut st, serde_json::json!({"at": 1002, "idle_since": 0}));
        st.tick();
        assert!(texts(&st).is_empty());
    }

    #[test]
    fn night_keep_ends_when_quiet_switched_off_by_hand() {
        let (mut st, now) = presence_state(1700);
        let min = mins_at(&mut st, "22:59");
        snap(&mut st, serde_json::json!({"at": 1700, "idle_since": 1000}));
        st.tick();
        *min.lock().unwrap() = crate::config::hhmm("23:00").unwrap();
        st.tick();
        assert!(st.night_keep);
        st.cfg.quiet_override = "off".into();
        st.apply();
        *now.lock().unwrap() = 1710;
        snap(&mut st, serde_json::json!({"at": 1710, "idle_since": 0}));
        final_s(&mut st);
        st.cfg.quiet_override = "on".into(); // включил тишину сам, на месте — доска стирается
        st.apply();
        assert!(st.board.is_empty());
    }

    #[test]
    fn buds_out_speaks_through_speakers() {
        let mut st = test_state();
        st.on_presence(r#"{"at":1000,"buds":"out"}"#);
        assert_eq!(*st.shared.sink.lock().unwrap(), "speakers");
        st.on_presence(r#"{"at":1000,"buds":"in"}"#);
        assert_eq!(*st.shared.sink.lock().unwrap(), "");
    }

    #[test]
    fn limit_line_counts_only_at_the_end_and_prompt_cancels() {
        let mut st = test_state();
        with_agents(&mut st, &["a"]);
        stop(&mut st, "a", "В доке сказано: при «Остановился по лимиту» Ома будит агента. Готово.");
        assert_eq!(st.registry.get("a").unwrap().health, "", "цитата фразы — не остановка");
        stop(&mut st, "a", "Записал, что осталось.\n\nОстановился по лимиту.");
        assert!(st.registry.get("a").unwrap().health.starts_with("лимит до "));
        st.on_hook("user-prompt-submit", &serde_json::json!({"session_id": "a", "prompt": "делай другое"}));
        assert_eq!(st.registry.get("a").unwrap().health, "", "новая задача — будить по записи нечего");
    }

    #[test]
    fn session_end_hook_silences_the_exit() {
        let mut st = test_state();
        with_agents(&mut st, &["a"]);
        st.shell_idle = Box::new(|_| true);
        let rx = acts(&mut st);
        st.facts_of = Box::new(|e, a| Facts { active_at: 999, pane: herdr_pane(), shell: Some(5), ..blank_facts(e, a) });
        st.watch_tick();
        st.on_hook("session-end", &serde_json::json!({"session_id": "a", "reason": "prompt_input_exit"}));
        with_agents(&mut st, &[]);
        st.watch_tick();
        assert!(rx.try_recv().is_err() && st.board.is_empty(), "агент вышел сам — не падение и не новость");
    }

    #[test]
    fn lost_session_brings_the_watch_tick_forward() {
        let mut st = test_state();
        with_agents(&mut st, &["a"]);
        st.watch_tick();
        with_agents(&mut st, &[]);
        assert!(st.next_watch <= Instant::now(), "исчезновение разбираем сразу, пока «простой пользователя» ещё тот же");
    }

    #[test]
    fn sub_call_found_by_tool_use_id() {
        let dir = std::env::temp_dir().join(format!("agent-speak-sub-{}", std::process::id()));
        let subs = dir.join("s/subagents");
        std::fs::create_dir_all(&subs).unwrap();
        std::fs::write(subs.join("agent-x.meta.json"), r#"{"toolUseId":"toolu_1","requestShape":"foreground"}"#).unwrap();
        let line = serde_json::json!({"type":"assistant","isSidechain":true,"message":{"content":[{"type":"tool_use","id":"t9","name":"Bash","input":{"command":"git push"}}]}});
        std::fs::write(subs.join("agent-x.jsonl"), format!("{line}\n")).unwrap();
        let call = serde_json::json!({"id": "toolu_1", "name": "Agent"});
        assert_eq!(sub_call(&dir.join("s.jsonl"), &call).unwrap()["input"]["command"], "git push");
        assert_eq!(sub_call(&dir.join("s.jsonl"), &serde_json::json!({"id": "другой"})), None);
        let _ = std::fs::remove_dir_all(dir);
    }
}
