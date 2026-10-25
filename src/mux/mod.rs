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
    /// Терминал, активный в сессии мультиплексора, где живёт pane (вкладка в фокусе); None — не знает.
    fn active(&self, _pane: &Pane) -> Result<Option<String>, String> {
        Ok(None)
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

/// Терминал агента с этим pid.
pub fn locate(pid: u32) -> Option<Pane> {
    pick(&environ(pid), &host_cmdline(pid))
}

fn adapter(pane: &Pane) -> Result<&'static dyn Mux, String> {
    adapters().into_iter().find(|m| m.name() == pane.mux).ok_or_else(|| format!("нет адаптера {}", pane.mux))
}

/// Агент (pid) в активном терминале, среди агентов под одним окном (Orca держит всех в одном окне).
pub fn active_among(pids: &[u32]) -> Option<u32> {
    let panes: Vec<(u32, Pane)> = pids.iter().filter_map(|&p| locate(p).map(|pane| (p, pane))).collect();
    among(&panes, |pane| adapter(pane).ok()?.active(pane).ok().flatten())
}

/// Чей терминал активен: спрашиваем сессию первого терминала, она у агентов одного окна общая.
fn among(panes: &[(u32, Pane)], active: impl Fn(&Pane) -> Option<String>) -> Option<u32> {
    let (_, first) = panes.first()?;
    let id = active(first)?;
    panes.iter().find(|(_, p)| p.mux == first.mux && p.via == first.via && p.id == id).map(|(pid, _)| *pid)
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

/// Поток действий: задания по одному, шаги по порядку; первая ошибка обрывает задание.
pub fn spawn(out: Sender<Done>) -> Sender<Job> {
    let (tx, rx) = std::sync::mpsc::channel::<Job>();
    std::thread::spawn(move || {
        for job in rx {
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
    let status = loop {
        if let Some(st) = child.try_wait().map_err(|e| e.to_string())? {
            break st;
        }
        if start.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("таймаут {} с", timeout.as_secs_f32()));
        }
        std::thread::sleep(Duration::from_millis(50));
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
    fn active_pane_picks_its_agent() {
        let p = |id: &str| Pane { mux: "orca", id: id.into(), via: "/o".into() };
        let panes = [(1, p("t1")), (2, p("t2"))];
        assert_eq!(among(&panes, |_| Some("t2".into())), Some(2));
        assert_eq!(among(&panes, |_| None), None, "мультиплексор не ответил — фокус неизвестен");
        assert_eq!(among(&[], |_| panic!("терминалов нет")), None);
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
        jobs.send(Job { session: "s".into(), pane, steps: vec![Step::Pause(1), Step::Text("hi".into())], what: crate::watch::What::Wake }).unwrap();
        let d = drx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!((d.session.as_str(), d.what), ("s", crate::watch::What::Wake));
        assert!(d.err.unwrap().contains("nowhere"));
    }
}
