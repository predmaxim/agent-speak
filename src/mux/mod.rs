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
    /// Терминал по окружению процесса агента; None — агент не в этом мультиплексоре.
    fn locate(&self, env: &HashMap<String, String>) -> Option<Pane>;
    /// Текст и Enter одной отправкой.
    fn send(&self, pane: &Pane, text: &str) -> Result<(), String>;
    fn key(&self, pane: &Pane, key: &Key) -> Result<(), String>;
    /// Что видно на экране терминала, построчно.
    fn screen(&self, pane: &Pane) -> Result<String, String>;
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

/// Терминал агента с этим pid — первый адаптер, узнавший его окружение.
pub fn locate(pid: u32) -> Option<Pane> {
    let env = environ(pid);
    adapters().iter().find_map(|m| m.locate(&env))
}

fn adapter(pane: &Pane) -> Result<&'static dyn Mux, String> {
    adapters().into_iter().find(|m| m.name() == pane.mux).ok_or_else(|| format!("нет адаптера {}", pane.mux))
}

pub fn screen(pane: &Pane) -> Result<String, String> {
    adapter(pane)?.screen(pane)
}

#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    Key(Key),
    Text(String),
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

fn perform(job: &Job) -> Result<(), String> {
    let m = adapter(&job.pane)?;
    for step in &job.steps {
        match step {
            Step::Key(k) => m.key(&job.pane, k)?,
            Step::Text(t) => m.send(&job.pane, t)?,
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
            let err = perform(&job).err();
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
    let mut so = child.stdout.take().unwrap();
    let mut se = child.stderr.take().unwrap();
    let out = std::thread::spawn(move || { let mut s = String::new(); let _ = so.read_to_string(&mut s); s });
    let err = std::thread::spawn(move || { let mut s = String::new(); let _ = se.read_to_string(&mut s); s });
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
    let (out, err) = (out.join().unwrap_or_default(), err.join().unwrap_or_default());
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
    }

    #[test]
    fn environ_of_self_has_path() {
        assert!(environ(std::process::id()).contains_key("PATH"));
    }

    #[test]
    fn locate_picks_the_adapter_that_knows_the_env() {
        assert_eq!(adapters().map(|m| m.name()), ["herdr", "orca"]);
        // этот процесс тестов — не в мультиплексоре, если тесты гоняют не из Orca/herdr
        let env = environ(std::process::id());
        let expect = adapters().iter().find_map(|m| m.locate(&env));
        assert_eq!(locate(std::process::id()), expect);
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
