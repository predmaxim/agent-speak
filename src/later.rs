//! Отложенные поручения Омы, без модели: «когда datebook закончит, передай: …» — ровно одна передача на конце хода
//! агента (одно «да, передай» при создании — одна доставка); «скажи, когда пройдут проверки PR 12» — `gh pr checks
//! --watch` в своём потоке. Список — в ~/.local/share/agent-speak/later.json: перезапуск службы его не теряет.

use serde::{Deserialize, Serialize};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::Sender;
use std::time::Duration;

/// Проверки PR ждём не дольше этого: потом «перестала ждать».
const CHECKS_MAX: Duration = Duration::from_secs(2 * 3600);

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Order {
    pub id: String,
    pub kind: String,    // tell | checks
    pub agent: String,   // tell: сессия агента; checks — пусто
    pub who: String,     // проект: в окне и в речи
    pub text: String,    // tell: что передать, целиком
    pub gist: String,    // tell: смысл в двух словах — его говорит Ома
    #[serde(default)]
    pub path: String,    // checks: папка проекта
    #[serde(default)]
    pub pr: u64,
    pub since: u64,
    #[serde(default)]
    pub due: bool, // ход агента кончился — передать, как только можно (агент «готово», пользователь не печатает в нём); в файле — тоже: после перезапуска не ждёт нового Stop
}

/// Исход из чужого потока: передача (поток действий) или проверки (поток `gh`).
pub enum Done {
    Told { order: Order, result: Result<(), String> }, // поручение уже снято со списка: одна передача
    Checked { id: String, result: Result<bool, String> },
}

/// Запуск проверок: pidfd процесса `gh` (отмена гасит именно его) или ошибка словами; исход — в out.
pub type Spawn = fn(&Order, Sender<Done>) -> Result<OwnedFd, String>;

pub struct Later {
    pub list: Vec<Order>,
    n: u64,
    file: PathBuf, // пусто — не сохранять (тесты)
    pub out: Sender<Done>,
    spawn: Spawn,
    procs: Vec<(String, OwnedFd)>, // идущие проверки: id → pidfd
}

impl Later {
    /// Список из файла (битый или нет — пустой); проверки запускаются заново, поручения агентов ждут своего Stop.
    pub fn load(file: PathBuf, out: Sender<Done>, spawn: Spawn) -> Later {
        let list: Vec<Order> = std::fs::read_to_string(&file).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        let n = list.iter().filter_map(|o| o.id.strip_prefix("later-")?.parse().ok()).max().unwrap_or(0);
        let mut l = Later { list, n, file, out, spawn, procs: Vec::new() };
        for o in l.list.clone().iter().filter(|o| o.kind == "checks") {
            if let Err(e) = l.start(o) {
                // не запустились заново (нет gh, нет папки) — исход ошибкой: Ома скажет его, поручение снимется
                let _ = l.out.send(Done::Checked { id: o.id.clone(), result: Err(e) });
            }
        }
        l
    }

    fn start(&mut self, o: &Order) -> Result<(), String> {
        let fd = (self.spawn)(o, self.out.clone())?;
        self.procs.push((o.id.clone(), fd));
        Ok(())
    }

    fn next_id(&mut self) -> String {
        self.n += 1;
        format!("later-{}", self.n)
    }

    /// Поручение агенту session (он работает — это проверяет вызывающий): одно на агента.
    pub fn tell(&mut self, session: &str, who: &str, text: &str, gist: &str, now: u64) -> Result<String, String> {
        if let Some(o) = self.list.iter().find(|o| o.kind == "tell" && o.agent == session) {
            return Err(format!("у {who} уже есть: «{}» — сначала отмени его", o.gist));
        }
        if text.trim().is_empty() {
            return Err("пустое поручение".into());
        }
        let id = self.next_id();
        let gist = if gist.trim().is_empty() { text } else { gist };
        self.list.push(Order { id: id.clone(), kind: "tell".into(), agent: session.into(), who: who.into(), text: text.into(), gist: gist.into(), path: String::new(), pr: 0, since: now, due: false });
        self.save();
        Ok(id)
    }

    /// Ход агента кончился (true) или он снова работает (false): его поручение — к передаче или ждёт; файл — тоже.
    pub fn due(&mut self, agent: &str, due: bool) {
        let mut changed = false;
        for o in self.list.iter_mut().filter(|o| o.kind == "tell" && o.agent == agent && o.due != due) {
            o.due = due;
            changed = true;
        }
        if changed {
            self.save();
        }
    }

    /// Проверки PR pr в папке path: `gh` с --watch в своём потоке.
    pub fn checks(&mut self, path: &str, pr: u64, now: u64) -> Result<String, String> {
        if pr == 0 {
            return Err("нужен номер PR".into());
        }
        if !Path::new(path).is_dir() {
            return Err(format!("нет папки {path}"));
        }
        let who = Path::new(path).file_name().and_then(|n| n.to_str()).unwrap_or(path).to_string();
        let id = self.next_id();
        let o = Order { id: id.clone(), kind: "checks".into(), agent: String::new(), who, text: String::new(), gist: format!("проверки PR {pr}"), path: path.into(), pr, since: now, due: false };
        self.start(&o)?;
        self.list.push(o);
        self.save();
        Ok(id)
    }

    /// Снять поручение (отмена, передано, агент закрылся); у проверок — погасить их `gh`.
    pub fn take(&mut self, id: &str) -> Option<Order> {
        let i = self.list.iter().position(|o| o.id == id)?;
        let o = self.list.remove(i);
        if let Some(j) = self.procs.iter().position(|(p, _)| p == id) {
            let (_, fd) = self.procs.remove(j);
            // SAFETY: pidfd живого или ещё не собранного потомка; сигнал уходит ровно ему, без гонки с повтором pid
            unsafe { libc::syscall(libc::SYS_pidfd_send_signal, fd.as_raw_fd(), libc::SIGTERM, std::ptr::null::<libc::siginfo_t>(), 0) };
        }
        self.save();
        Some(o)
    }

    /// Проверки кончились сами: их pidfd больше не нужен.
    pub fn finished(&mut self, id: &str) -> Option<Order> {
        self.procs.retain(|(p, _)| p != id);
        self.take(id)
    }

    fn save(&self) {
        if self.file.as_os_str().is_empty() {
            return;
        }
        let tmp = self.file.with_extension("json.tmp");
        if let Some(d) = self.file.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        if std::fs::write(&tmp, serde_json::to_string(&self.list).unwrap()).is_ok() {
            let _ = std::fs::rename(&tmp, &self.file);
        }
    }

    /// Строка подписчикам: `{"later":[{id, kind, agent, who, text, gist, path, pr, since, due}]}`.
    pub fn line(&self) -> String {
        format!("{}\n", serde_json::json!({"later": self.list}))
    }

    /// Тот же список JSON-массивом — `agent-speak later list` (Ома проверяет по нему «одно на агента» до вопроса
    /// подтверждения и пересказывает его мозгу).
    pub fn json(&self) -> String {
        serde_json::to_string(&self.list).unwrap()
    }
}

/// Ход кончился вопросом агента (последняя строка ответа — на «?»): поручение ждёт следующего конца хода, чтобы не
/// ответить на вопрос чужим текстом.
pub fn ends_with_question(last: &str) -> bool {
    last.lines().rev().find(|l| !l.trim().is_empty()).is_some_and(|l| l.trim_end().ends_with('?'))
}

/// Исход `gh pr checks --watch --fail-fast`: 0 — прошли, 1 без stderr — упали (проверено 2026-11-12, gh 2.102.1: у
/// упавших stderr пуст, «no checks reported» — 1 с текстом), иначе — первая строка stderr.
pub fn outcome(code: Option<i32>, stderr: &str) -> Result<bool, String> {
    let first = stderr.lines().find(|l| !l.trim().is_empty()).unwrap_or_default().trim().to_string();
    match code {
        Some(0) => Ok(true),
        Some(1) if first.is_empty() => Ok(false),
        _ if !first.is_empty() => Err(first),
        Some(c) => Err(format!("gh вышел с кодом {c}")),
        None => Err("gh остановлен".into()),
    }
}

/// `gh pr checks <pr> --watch --fail-fast --interval 60` в папке проекта: ждёт свой поток, без модели и своего опроса.
/// gh не установлен — ошибка «не умею» сразу.
pub fn spawn_gh(o: &Order, out: Sender<Done>) -> Result<OwnedFd, String> {
    let mut child = Command::new("gh")
        .args(["pr", "checks", &o.pr.to_string(), "--watch", "--fail-fast", "--interval", "60"])
        .current_dir(&o.path)
        .stdin(Stdio::null())
        .stdout(Stdio::null()) // таблица перерисовывается на каждый интервал: исход — по коду
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| if e.kind() == std::io::ErrorKind::NotFound { "gh не установлен — проверки PR не умею".to_string() } else { format!("gh не запустился: {e}") })?;
    // SAFETY: pidfd_open без побочных эффектов; дескриптор сразу во владении OwnedFd
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, child.id() as libc::pid_t, 0) };
    if fd < 0 {
        let _ = child.kill();
        let _ = child.wait();
        return Err(format!("pidfd_open: {}", std::io::Error::last_os_error()));
    }
    let fd = unsafe { OwnedFd::from_raw_fd(fd as libc::c_int) };
    let id = o.id.clone();
    std::thread::spawn(move || {
        use std::io::Read;
        let mut err = String::new();
        let result = match crate::mux::exit_within(&mut child, CHECKS_MAX) {
            Ok(Some(st)) => {
                if let Some(mut e) = child.stderr.take() {
                    let _ = e.read_to_string(&mut err);
                }
                outcome(st.code(), &err)
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                Err("проверки не кончились за 2 часа — перестала ждать".into())
            }
            Err(e) => Err(e),
        };
        let _ = out.send(Done::Checked { id, result });
    });
    Ok(fd)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn no_gh(_: &Order, _: Sender<Done>) -> Result<OwnedFd, String> {
        Err("gh не установлен — проверки PR не умею".into())
    }

    /// Вместо gh — `sleep`: pidfd настоящего процесса, исхода нет.
    fn sleeper(_: &Order, _: Sender<Done>) -> Result<OwnedFd, String> {
        let c = Command::new("sleep").arg("30").spawn().map_err(|e| e.to_string())?;
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, c.id() as libc::pid_t, 0) };
        std::mem::forget(c); // pid держит зомби до сигнала: проверка ниже ждёт его выхода по pidfd
        Ok(unsafe { OwnedFd::from_raw_fd(fd as libc::c_int) })
    }

    #[test]
    fn one_order_per_agent_kept_in_file_and_cancelled() {
        let file = std::env::temp_dir().join(format!("oma-later-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&file);
        let mut l = Later::load(file.clone(), mpsc::channel().0, no_gh);
        assert_eq!(l.tell("s1", "datebook", "Прогони тесты и пришли итог", "прогони тесты", 100), Ok("later-1".into()));
        assert_eq!(l.tell("s1", "datebook", "Ещё", "ещё", 101), Err("у datebook уже есть: «прогони тесты» — сначала отмени его".into()));
        assert!(l.tell("s2", "x", " ", "", 1).is_err());
        let line: serde_json::Value = serde_json::from_str(&l.line()).unwrap();
        assert_eq!(line["later"][0], serde_json::json!({"id": "later-1", "kind": "tell", "agent": "s1", "who": "datebook", "text": "Прогони тесты и пришли итог", "gist": "прогони тесты", "path": "", "pr": 0, "since": 100, "due": false}));
        assert_eq!(serde_json::from_str::<serde_json::Value>(&l.json()).unwrap(), line["later"]);
        // перезапуск службы: поручение на месте, номера не повторяются
        let mut again = Later::load(file.clone(), mpsc::channel().0, no_gh);
        assert_eq!(again.list.len(), 1);
        assert_eq!(again.tell("s2", "plug", "Глянь логи", "", 102), Ok("later-2".into()));
        assert_eq!(again.list[1].gist, "Глянь логи", "без смысла — сам текст");
        assert_eq!(again.take("later-1").map(|o| o.agent), Some("s1".into()));
        assert!(again.take("later-1").is_none());
        assert_eq!(Later::load(file.clone(), mpsc::channel().0, no_gh).list.len(), 1);
        let _ = std::fs::remove_file(&file);
        assert_eq!(Later::load(PathBuf::new(), mpsc::channel().0, no_gh).json(), "[]");
    }

    #[test]
    fn due_survives_a_restart() {
        // поручение ждало только конца ввода пользователя: после перезапуска — не до следующего Stop агента
        let file = std::env::temp_dir().join(format!("oma-later-due-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&file);
        let mut l = Later::load(file.clone(), mpsc::channel().0, no_gh);
        l.tell("s1", "datebook", "Прогони тесты", "", 100).unwrap();
        l.due("s1", true);
        assert!(Later::load(file.clone(), mpsc::channel().0, no_gh).list[0].due);
        l.due("s1", false);
        assert!(!Later::load(file.clone(), mpsc::channel().0, no_gh).list[0].due);
        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn checks_need_gh_a_folder_and_a_number_and_cancel_kills_them() {
        let mut l = Later::load(PathBuf::new(), mpsc::channel().0, no_gh);
        assert_eq!(l.checks("/tmp", 12, 1), Err("gh не установлен — проверки PR не умею".into()));
        assert!(l.list.is_empty(), "не запустились — поручения нет");
        assert!(l.checks("/nonexistent-oma", 12, 1).is_err() && l.checks("/tmp", 0, 1).is_err());
        let mut l = Later::load(PathBuf::new(), mpsc::channel().0, sleeper);
        let id = l.checks("/tmp", 12, 1).unwrap();
        assert_eq!((l.list[0].who.as_str(), l.list[0].gist.as_str()), ("tmp", "проверки PR 12"));
        let fd = l.procs[0].1.try_clone().unwrap();
        l.take(&id);
        // отмена погасила процесс: его pidfd стал читаемым (процесс вышел)
        let mut p = libc::pollfd { fd: fd.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        assert_eq!(unsafe { libc::poll(&mut p, 1, 3000) }, 1, "gh отменённых проверок не ходит в GitHub дальше");
    }

    #[test]
    fn checks_that_cannot_restart_end_with_their_error() {
        let file = std::env::temp_dir().join(format!("oma-later-checks-{}.json", std::process::id()));
        let o = Order { id: "later-4".into(), kind: "checks".into(), agent: String::new(), who: "x".into(), text: String::new(), gist: "проверки PR 12".into(), path: "/gone".into(), pr: 12, since: 1, due: false };
        std::fs::write(&file, serde_json::to_string(&vec![o]).unwrap()).unwrap();
        let (tx, rx) = mpsc::channel();
        let l = Later::load(file.clone(), tx, no_gh);
        let Ok(Done::Checked { id, result: Err(e) }) = rx.try_recv() else { panic!("исход ошибкой — иначе висело бы вечно") };
        assert_eq!((id.as_str(), e.as_str(), l.list.len()), ("later-4", "gh не установлен — проверки PR не умею", 1), "снимает его служба по этому исходу");
        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn checks_outcome_by_code_and_stderr() {
        assert_eq!(outcome(Some(0), ""), Ok(true));
        assert_eq!(outcome(Some(1), ""), Ok(false));
        assert_eq!(outcome(Some(1), "no checks reported on the 'x' branch\n"), Err("no checks reported on the 'x' branch".into()));
        assert_eq!(outcome(Some(4), "\nauthentication required\nmore"), Err("authentication required".into()));
        assert_eq!(outcome(Some(8), ""), Err("gh вышел с кодом 8".into()));
        assert_eq!(outcome(None, ""), Err("gh остановлен".into()));
    }

    #[test]
    fn question_at_the_end_of_the_turn_waits() {
        assert!(ends_with_question("Сделал.\nЗапушить?\n\n"));
        assert!(!ends_with_question("Зачем? Потому что так.") && !ends_with_question(""));
    }
}
