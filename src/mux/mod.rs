//! Терминалы агентов — за адаптерами мультиплексоров (Orca, herdr): ядро знает только трейт `Mux`.

pub mod herdr;
pub mod orca;

use std::collections::HashMap;
use std::process::Command;
use std::sync::mpsc::Sender;
use std::time::Duration;

#[derive(Clone, Debug, PartialEq)]
pub enum Key {
    Esc,
    CtrlC,
    /// стереть строку до начала (Claude Code, bash, zsh); стёртое возвращает Ctrl+Y
    CtrlU,
    Enter,
    /// цифра 1–9: пункт меню Claude Code
    Digit(u8),
}

/// Терминал агента: какой мультиплексор и адрес в нём; via — у Orca путь к CLI, у herdr сокет сессии.
#[derive(Clone, Debug, PartialEq)]
pub struct Pane {
    pub mux: &'static str,
    pub id: String,
    pub via: String,
}

pub trait Mux: Sync {
    fn name(&self) -> &'static str;
    /// Командная строка процесса, который держит терминалы этого мультиплексора (его сервер, pty-хост).
    fn hosts(&self, cmdline: &str) -> bool;
    /// Терминал по окружению процесса агента; None — агент не в этом мультиплексоре.
    fn locate(&self, env: &HashMap<String, String>) -> Option<Pane>;
    /// Текст и Enter одной отправкой.
    fn send(&self, pane: &Pane, text: &str) -> Result<(), String>;
    fn key(&self, pane: &Pane, key: &Key) -> Result<(), String>;
    /// Текст без Enter: поле «свой ответ» в меню Claude Code.
    fn type_text(&self, pane: &Pane, text: &str) -> Result<(), String>;
    /// Что видно на экране терминала, построчно.
    fn screen(&self, pane: &Pane) -> Result<String, String>;
    /// Папки, где мультиплексор умеет запустить агента.
    fn projects(&self) -> Result<Vec<String>, String> {
        Ok(Vec::new())
    }
    /// Ответ, если папки нет в projects().
    fn no_project(&self) -> String {
        format!("этого проекта нет в {}", self.name())
    }
    /// Видны ли сейчас на экране терминалы процессов с этими окружениями (вкладка Orca, панель herdr), по ответу на
    /// каждое; None — не знает. Один вызов на всех агентов этого мультиплексора.
    fn shown(&self, envs: &[&HashMap<String, String>]) -> Vec<Option<bool>> {
        vec![None; envs.len()]
    }
    /// Новый агент Claude в папке cwd с первым сообщением prompt.
    fn launch(&self, _cwd: &str, _prompt: &str) -> Result<(), String> {
        Err(format!("{} не запускает агентов", self.name()))
    }
}

/// Вызов CLI мультиплексора дольше этого — завис: поток действий не встаёт.
const CLI_TIMEOUT: Duration = Duration::from_secs(10);

/// Порядок — вложенность: herdr бывает запущен в терминале Orca, и тогда внутренний (herdr) — терминал агента.
pub fn adapters() -> [&'static dyn Mux; 2] {
    [&herdr::Herdr, &orca::Orca]
}

/// Окружение живого процесса; нет процесса или прав — пусто.
pub fn environ(pid: u32) -> HashMap<String, String> {
    let raw = std::fs::read(format!("/proc/{pid}/environ")).unwrap_or_default();
    raw.split(|&b| b == 0)
        .filter_map(|kv| String::from_utf8_lossy(kv).split_once('=').map(|(k, v)| (k.to_string(), v.to_string())))
        .collect()
}

/// Оболочки между агентом и терминалом: их пропускаем, ища хозяина терминала.
pub const SHELLS: [&str; 5] = ["bash", "zsh", "fish", "sh", "dash"];

fn comm(pid: u32) -> String {
    std::fs::read_to_string(format!("/proc/{pid}/comm")).unwrap_or_default().trim().to_string()
}

/// Командная строка хозяина терминала агента: первый предок, который не оболочка.
fn host_cmdline(pid: u32) -> String {
    let mut p = crate::focus::ppid(pid).unwrap_or(0);
    while p > 1 && SHELLS.contains(&comm(p).as_str()) {
        p = crate::focus::ppid(p).unwrap_or(0);
    }
    let raw = std::fs::read(format!("/proc/{p}/cmdline")).unwrap_or_default();
    String::from_utf8_lossy(&raw).replace('\0', " ")
}

/// Терминал по окружению — только если его мультиплексор и держит агента: окружение наследуют и окна,
/// запущенные из терминала Orca или herdr (foot, редактор), и тогда писать туда — в чужую программу.
pub fn pick(env: &HashMap<String, String>, host: &str) -> Option<Pane> {
    adapters().iter().find_map(|m| m.locate(env).filter(|_| m.hosts(host)))
}

/// Окно приложения-мультиплексора (window — его командная строка) держит терминал агента: pty-хост Orca —
/// потомок systemd, а не окна, и по дереву процессов агенты окна Orca не находятся.
pub fn app_hosts(window: &str, env: &HashMap<String, String>, host: &str) -> bool {
    adapters().iter().any(|m| m.hosts(window) && m.locate(env).is_some() && m.hosts(host))
}

/// Агент pid — в окне win мультиплексора.
pub fn in_app_window(win: u32, pid: u32) -> bool {
    let raw = std::fs::read(format!("/proc/{win}/cmdline")).unwrap_or_default();
    app_hosts(&String::from_utf8_lossy(&raw).replace('\0', " "), &environ(pid), &host_cmdline(pid))
}

/// Терминал агента с этим pid.
pub fn locate(pid: u32) -> Option<Pane> {
    pick(&environ(pid), &host_cmdline(pid))
}

fn adapter(pane: &Pane) -> Result<&'static dyn Mux, String> {
    adapters().into_iter().find(|m| m.name() == pane.mux).ok_or_else(|| format!("нет адаптера {}", pane.mux))
}

/// Агент (pid) в видимом терминале, среди агентов под одним окном (Orca держит всех в одном окне).
/// Вопрос — один на мультиплексор, а не на агента; мультиплексоры (herdr внутри Orca) спрашиваются параллельно.
pub fn active_among(pids: &[u32]) -> Option<u32> {
    let agents: Vec<(u32, HashMap<String, String>, Option<&'static str>)> = pids.iter().map(|&pid| {
        let env = environ(pid);
        let mux = pick(&env, &host_cmdline(pid)).map(|p| p.mux);
        (pid, env, mux)
    }).collect();
    let seen: Vec<(u32, Option<bool>)> = std::thread::scope(|s| {
        let asks: Vec<_> = adapters()
            .into_iter()
            .map(|m| (m, agents.iter().filter(|a| a.2 == Some(m.name())).collect::<Vec<_>>()))
            .filter(|(_, mine)| !mine.is_empty())
            .map(|(m, mine)| s.spawn(move || {
                let envs: Vec<&HashMap<String, String>> = mine.iter().map(|a| &a.1).collect();
                mine.iter().map(|a| a.0).zip(m.shown(&envs)).collect::<Vec<_>>()
            }))
            .collect();
        asks.into_iter().flat_map(|h| h.join().unwrap_or_default()).collect()
    });
    among(pids, |pid| seen.iter().find(|(p, _)| *p == pid).and_then(|(_, s)| *s))
}

fn among(pids: &[u32], shown: impl Fn(u32) -> Option<bool>) -> Option<u32> {
    pids.iter().copied().find(|&p| shown(p) == Some(true))
}

pub fn screen(pane: &Pane) -> Result<String, String> {
    adapter(pane)?.screen(pane)
}

#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    Key(Key),
    Text(String),
    /// текст без Enter и без Ctrl+U перед ним (поле меню)
    Type(String),
    /// мс
    Pause(u64),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Job {
    pub session: String,
    pub pane: Pane,
    pub steps: Vec<Step>,
    pub what: crate::watch::What,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Done {
    pub session: String,
    pub what: crate::watch::What,
    pub err: Option<String>,
}

/// Дело потока действий: действие сторожа или чужое дело (ответ агенту от Омы, `agent-speak tell`). Очередь одна,
/// дела — по одному: две записи в один терминал не перемешаются.
pub enum Work {
    Job(Job),
    Other(Box<dyn FnOnce() + Send>),
}

/// Шаги в терминале через адаптер панели; первая ошибка обрывает.
pub fn perform(pane: &Pane, steps: &[Step]) -> Result<(), String> {
    perform_on(adapter(pane)?, pane, steps)
}

fn perform_on(m: &dyn Mux, pane: &Pane, steps: &[Step]) -> Result<(), String> {
    for step in steps {
        match step {
            Step::Key(k) => m.key(pane, k)?,
            // недонабранное пользователем иначе ушло бы вместе с текстом Омы — агенту или оболочке командой
            // shortcut: многострочный черновик Claude Code стирается только в последней строке; станет мешать — читать экран и стирать всё
            Step::Text(t) => {
                m.key(pane, &Key::CtrlU)?;
                m.send(pane, t)?
            }
            Step::Type(t) => m.type_text(pane, t)?,
            Step::Pause(ms) => std::thread::sleep(Duration::from_millis(*ms)),
        }
    }
    Ok(())
}

/// Поток действий: дела по одному, шаги по порядку; первая ошибка обрывает задание.
pub fn spawn(out: Sender<Done>) -> Sender<Work> {
    let (tx, rx) = std::sync::mpsc::channel::<Work>();
    std::thread::spawn(move || {
        for work in rx {
            let job = match work {
                Work::Job(job) => job,
                Work::Other(f) => {
                    f();
                    continue;
                }
            };
            let err = perform(&job.pane, &job.steps).err();
            if let Some(e) = &err {
                eprintln!("agent-speak: действие {:?} для {} не вышло: {e}", job.what, job.session);
            }
            if out.send(Done { session: job.session, what: job.what, err }).is_err() {
                return;
            }
        }
    });
    tx
}

/// CLI с таймаутом: stdout; ненулевой код или таймаут — Err с началом stderr.
pub fn run(cmd: &mut Command, timeout: Duration) -> Result<String, String> {
    use std::io::Read;
    use std::process::Stdio;
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| e.to_string())?;
    // вывод читается в потоках: полный канал иначе держал бы процесс до таймаута
    let (tx, rx) = std::sync::mpsc::channel();
    let (mut so, mut se, tx2) = (child.stdout.take().unwrap(), child.stderr.take().unwrap(), tx.clone());
    std::thread::spawn(move || { let mut s = String::new(); let _ = so.read_to_string(&mut s); let _ = tx.send((true, s)); });
    std::thread::spawn(move || { let mut s = String::new(); let _ = se.read_to_string(&mut s); let _ = tx2.send((false, s)); });
    let start = std::time::Instant::now();
    let Some(status) = exit_within(&mut child, timeout)? else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(format!("таймаут {} с", timeout.as_secs_f32()));
    };
    // процесс вышел, а вывод держит его потомок — ждём не дольше общего срока
    let (mut out, mut err) = (String::new(), String::new());
    for _ in 0..2 {
        match rx.recv_timeout(timeout.saturating_sub(start.elapsed()).max(Duration::from_millis(300))) {
            Ok((true, s)) => out = s,
            Ok((false, s)) => err = s,
            Err(_) => return Err(format!("{status}: вывод держат дочерние процессы")),
        }
    }
    if status.success() {
        Ok(out)
    } else {
        let msg = if err.trim().is_empty() { out } else { err };
        Err(format!("{status}: {}", msg.trim().chars().take(300).collect::<String>()))
    }
}

/// Выход потомка не позже timeout, None — не вышел: ждём pidfd через poll (Linux 5.3+), без опроса и без гонки
/// с переиспользованием pid — несобранный потомок свой pid не отдаёт.
fn exit_within(child: &mut std::process::Child, timeout: Duration) -> Result<Option<std::process::ExitStatus>, String> {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    // SAFETY: pidfd_open без побочных эффектов; дескриптор сразу переходит во владение OwnedFd
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, child.id() as libc::pid_t, 0) };
    if fd < 0 {
        return Err(format!("pidfd_open: {}", std::io::Error::last_os_error()));
    }
    let fd = unsafe { OwnedFd::from_raw_fd(fd as libc::c_int) };
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let left = deadline.saturating_duration_since(std::time::Instant::now()).as_millis().min(i32::MAX as u128) as libc::c_int;
        let mut p = libc::pollfd { fd: fd.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        // SAFETY: один pollfd на стеке
        match unsafe { libc::poll(&mut p, 1, left) } {
            0 => return Ok(None),
            n if n > 0 => return child.wait().map(Some).map_err(|e| e.to_string()),
            _ if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted => continue,
            _ => return Err(format!("poll: {}", std::io::Error::last_os_error())),
        }
    }
}

/// CLI мультиплексора с таймаутом CLI_TIMEOUT.
pub(crate) fn cli(mut cmd: Command) -> Result<String, String> {
    run(&mut cmd, CLI_TIMEOUT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_times_out_and_reports_stderr() {
        assert!(run(Command::new("sleep").arg("5"), Duration::from_millis(200)).unwrap_err().contains("таймаут"));
        assert!(run(Command::new("sh").args(["-c", "echo boom >&2; exit 3"]), Duration::from_secs(2)).unwrap_err().contains("boom"));
        assert_eq!(run(Command::new("echo").arg("hi"), Duration::from_secs(2)).unwrap(), "hi\n");
        let t = std::time::Instant::now();
        let held = run(Command::new("sh").args(["-c", "sleep 5 & echo hi"]), Duration::from_secs(1));
        assert!(t.elapsed() < Duration::from_secs(3) && held.unwrap_err().contains("вывод"), "вывод держит внук — не ждём его");
    }

    #[test]
    fn run_returns_as_soon_as_the_child_exits() {
        // выход ждётся, а не опрашивается: прежний опрос по 50 мс держал цикл на каждом вызове CLI
        let t = std::time::Instant::now();
        for _ in 0..5 {
            run(&mut Command::new("true"), Duration::from_secs(2)).unwrap();
        }
        assert!(t.elapsed() < Duration::from_millis(200), "{:?} на 5 вызовов", t.elapsed());
    }

    /// Мультиплексор, который только записывает, что ему прислали.
    struct Rec(std::sync::Mutex<Vec<String>>);
    impl Mux for Rec {
        fn name(&self) -> &'static str { "rec" }
        fn hosts(&self, _: &str) -> bool { false }
        fn locate(&self, _: &HashMap<String, String>) -> Option<Pane> { None }
        fn send(&self, _: &Pane, text: &str) -> Result<(), String> { self.0.lock().unwrap().push(text.into()); Ok(()) }
        fn key(&self, _: &Pane, key: &Key) -> Result<(), String> { self.0.lock().unwrap().push(format!("{key:?}")); Ok(()) }
        fn type_text(&self, _: &Pane, text: &str) -> Result<(), String> { self.0.lock().unwrap().push(format!("type:{text}")); Ok(()) }
        fn screen(&self, _: &Pane) -> Result<String, String> { Ok(String::new()) }
    }

    #[test]
    fn text_goes_after_clearing_the_line() {
        let m = Rec(Default::default());
        let pane = Pane { mux: "rec", id: "x".into(), via: String::new() };
        perform_on(&m, &pane, &[Step::Key(Key::Esc), Step::Text("продолжай".into())]).unwrap();
        assert_eq!(*m.0.lock().unwrap(), ["Esc", "CtrlU", "продолжай"], "недонабранное пользователем не уйдёт вместе с текстом Омы");
    }

    #[test]
    fn type_step_has_no_line_clear() {
        let m = Rec(Default::default());
        let pane = Pane { mux: "rec", id: "x".into(), via: String::new() };
        perform_on(&m, &pane, &[Step::Key(Key::Digit(3)), Step::Type("зелёный".into()), Step::Key(Key::Enter)]).unwrap();
        assert_eq!(*m.0.lock().unwrap(), ["Digit(3)", "type:зелёный", "Enter"], "поле меню: Ctrl+U там не нужен");
    }

    #[test]
    fn shown_terminal_picks_its_agent() {
        assert_eq!(among(&[1, 2, 3], |p| Some(p == 2)), Some(2));
        assert_eq!(among(&[1, 2], |_| None), None, "мультиплексор не знает — фокус неизвестен");
    }

    #[test]
    fn environ_of_self_has_path() {
        assert!(environ(std::process::id()).contains_key("PATH"));
    }

    #[test]
    fn locate_picks_the_adapter_that_knows_the_env() {
        assert_eq!(adapters().map(|m| m.name()), ["herdr", "orca"]);
        // процесс тестов запускает cargo, а не оболочка терминала: даже в Orca это не терминал агента
        assert_eq!(locate(std::process::id()), None);
    }

    #[test]
    fn pane_only_when_the_mux_hosts_the_agent() {
        let env = HashMap::from([("HERDR_PANE_ID".to_string(), "w1:p1".to_string()), ("HERDR_SOCKET_PATH".into(), "/s.sock".into()),
            ("ORCA_TERMINAL_HANDLE".into(), "term_1".into()), ("ORCA_CLI_BIN_DIR".into(), "/x".into())]);
        assert_eq!(pick(&env, "herdr --session omatest server").unwrap().mux, "herdr");
        assert_eq!(pick(&env, "/home/u/.cache/orca/appimage/a/b/resources/node-runtime/bin/node /home/u/.cache/orca/x.js").unwrap().mux, "orca");
        assert_eq!(pick(&env, "foot --app-id=org.omarchy.agent -e claude"), None, "окно, запущенное из терминала Orca, наследует ORCA_* — но это не его терминал");
        assert_eq!(pick(&HashMap::new(), "herdr server"), None);
    }

    #[test]
    fn orca_window_owns_agents_of_its_pty_host() {
        // pty-хост Orca — потомок systemd, а не окна: агента окна Orca узнаём по мультиплексору, а не по дереву процессов
        let env = HashMap::from([("ORCA_TERMINAL_HANDLE".to_string(), "term_1".to_string()), ("ORCA_CLI_BIN_DIR".into(), "/x".into())]);
        let host = "/home/u/.cache/orca/appimage/a/b/resources/node-runtime/bin/node /home/u/.cache/orca/x.js";
        assert!(app_hosts("/tmp/.mount_Orca.Ab/orca-ide", &env, host));
        assert!(!app_hosts("/usr/bin/ghostty", &env, host), "агент Orca — не в чужом окне");
        assert!(!app_hosts("/tmp/.mount_Orca.Ab/orca-ide", &HashMap::new(), "bash"), "агент вне Orca — не в окне Orca");
    }

    #[test]
    fn herdr_inside_orca_wins() {
        // herdr, запущенный в терминале Orca, передаёт панелям ORCA_*: писать надо в панель herdr, а не в Orca
        let env = HashMap::from([
            ("ORCA_TERMINAL_HANDLE".to_string(), "term_1".to_string()), ("ORCA_CLI_BIN_DIR".into(), "/x".into()),
            ("HERDR_PANE_ID".into(), "w1:p1".into()), ("HERDR_SOCKET_PATH".into(), "/s.sock".into()),
        ]);
        assert_eq!(adapters().iter().find_map(|m| m.locate(&env)).unwrap().mux, "herdr");
    }

    #[test]
    fn job_steps_run_in_order_and_first_error_stops() {
        let (dtx, drx) = std::sync::mpsc::channel();
        let jobs = spawn(dtx);
        let pane = Pane { mux: "nowhere", id: "x".into(), via: String::new() };
        jobs.send(Work::Job(Job { session: "s".into(), pane, steps: vec![Step::Pause(1), Step::Text("hi".into())], what: crate::watch::What::Wake })).unwrap();
        let d = drx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!((d.session.as_str(), d.what), ("s", crate::watch::What::Wake));
        assert!(d.err.unwrap().contains("nowhere"));
    }

    #[test]
    fn other_work_waits_its_turn_in_the_same_queue() {
        // ответ Омы агенту и действие сторожа — один поток, по очереди: в одну панель не вперемешку
        let (dtx, drx) = std::sync::mpsc::channel();
        let jobs = spawn(dtx);
        let (ttx, trx) = std::sync::mpsc::channel();
        let pane = Pane { mux: "herdr", id: "x".into(), via: String::new() }; // только пауза: herdr не зовётся
        jobs.send(Work::Job(Job { session: "s".into(), pane, steps: vec![Step::Pause(300)], what: crate::watch::What::Wake })).unwrap();
        jobs.send(Work::Other(Box::new(move || ttx.send(std::time::Instant::now()).unwrap()))).unwrap();
        let start = std::time::Instant::now();
        let ran = trx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(ran.duration_since(start) >= Duration::from_millis(250), "чужое дело ждёт, пока доиграет действие сторожа");
        assert!(drx.recv_timeout(Duration::from_secs(1)).is_ok());
    }
}
