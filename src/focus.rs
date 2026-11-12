//! Окно в фокусе → сессия агента; путь транскрипта → сессия.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Debug, PartialEq)]
pub enum Agent {
    Claude,
    Codex,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SessionRef {
    pub agent: Agent,
    pub id: String,
    pub transcript: PathBuf,
    pub project: String, // имя папки, где запущен агент — для «Читаю: <проект>»
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default())
}

pub fn session_of(path: &Path) -> Option<(Agent, String)> {
    let stem = path.file_name()?.to_str()?.strip_suffix(".jsonl")?;
    let s = path.to_str()?;
    if s.contains("/.claude/projects/") {
        Some((Agent::Claude, stem.to_string()))
    } else if s.contains("/.codex/sessions/") && stem.len() >= 36 {
        Some((Agent::Codex, stem[stem.len() - 36..].to_string()))
    } else {
        None
    }
}

pub(crate) fn ppid(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    stat.rsplit_once(')')?.1.split_whitespace().nth(1)?.parse().ok()
}

pub(crate) fn under(mut pid: u32, ancestor: u32) -> bool {
    while pid > 1 {
        if pid == ancestor {
            return true;
        }
        match ppid(pid) {
            Some(p) => pid = p,
            None => return false,
        }
    }
    false
}

/// Имя папки рабочего каталога процесса; нет процесса — пустая строка.
pub fn project_of(pid: u32) -> String {
    std::fs::read_link(format!("/proc/{pid}/cwd"))
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_default()
}

/// Окно в фокусе: его агент на экране и все агенты под окном.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Focus {
    /// Агент на экране: один под окном — он; несколько (Orca, herdr) — чей терминал виден, говорит мультиплексор.
    pub agent: Option<SessionRef>,
    /// Сессии всех агентов под окном (процесс — потомок окна или терминал его мультиплексора): чат Orca под окном Orca
    /// в фокусе — «занят», какой из чатов на экране, Orca не сообщает.
    pub under: Vec<String>,
}

/// Окно в фокусе и его агенты; hyprctl не ответил или стол пуст — пусто.
pub fn focused() -> Focus {
    // timeout: зависший hyprctl не должен вешать цикл событий
    let Ok(out) = Command::new("timeout").args(["2", "hyprctl", "activewindow", "-j"]).output() else { return Focus::default() };
    let Ok(win) = window_pid(&out.stdout) else {
        eprintln!("agent-speak: hyprctl activewindow не ответил ({}): {}", out.status, String::from_utf8_lossy(&out.stderr).trim());
        return Focus::default();
    };
    let Some(w) = win else { return Focus::default() };
    let mut all = claude_under(w);
    all.extend(codex_under(w));
    let under = all.iter().map(|(_, s)| s.id.clone()).collect();
    Focus { agent: pick(all, crate::mux::active_among), under }
}

/// Ответ `hyprctl activewindow -j`: pid окна; пустой стол — `{}` (Hyprland, activeWindowRequest), не ответ — Err.
fn window_pid(stdout: &[u8]) -> Result<Option<u32>, ()> {
    let v: Value = serde_json::from_slice(stdout).map_err(|_| ())?;
    Ok(v["pid"].as_u64().and_then(|p| u32::try_from(p).ok()))
}

/// Агент из кандидатов (pid, сессия): один — он; несколько — по видимому терминалу (Orca — одно окно на всех).
fn pick(all: Vec<(u32, SessionRef)>, active: impl Fn(&[u32]) -> Option<u32>) -> Option<SessionRef> {
    if all.len() <= 1 {
        return all.into_iter().next().map(|(_, s)| s);
    }
    let pid = active(&all.iter().map(|(p, _)| *p).collect::<Vec<_>>())?;
    all.into_iter().find(|(p, _)| *p == pid).map(|(_, s)| s)
}

fn claude_under(win: u32) -> Vec<(u32, SessionRef)> {
    let mut out = Vec::new();
    let Ok(dir) = std::fs::read_dir(home().join(".claude/sessions")) else { return out };
    for f in dir.flatten() {
        let Ok(s) = std::fs::read_to_string(f.path()) else { continue };
        let Ok(v) = serde_json::from_str::<Value>(&s) else { continue };
        let (Some(pid), Some(sid)) = (v["pid"].as_u64(), v["sessionId"].as_str()) else { continue };
        if !under(pid as u32, win) && !crate::mux::in_app_window(win, pid as u32) {
            continue;
        }
        let name = format!("{sid}.jsonl");
        for d in std::fs::read_dir(home().join(".claude/projects")).into_iter().flatten().flatten() {
            let p = d.path().join(&name);
            if p.exists() {
                out.push((pid as u32, SessionRef { agent: Agent::Claude, id: sid.to_string(), transcript: p, project: project_of(pid as u32) }));
                break;
            }
        }
    }
    out
}

/// Открытый процессом rollout Codex: (id сессии, путь).
pub fn codex_rollout(pid: u32) -> Option<(String, PathBuf)> {
    for fd in std::fs::read_dir(format!("/proc/{pid}/fd")).ok()?.flatten() {
        let Ok(target) = std::fs::read_link(fd.path()) else { continue };
        if let Some((Agent::Codex, id)) = session_of(&target) {
            return Some((id, target));
        }
    }
    None
}

/// Codex пользователя: не `exec` (разовые запуски) и не `app-server`. cmdline — аргументы через \0.
pub fn is_user_codex(cmdline: &str) -> bool {
    !cmdline.split('\0').skip(1).any(|a| a == "exec" || a == "app-server")
}

fn codex_under(win: u32) -> Vec<(u32, SessionRef)> {
    let mut out = Vec::new();
    let Ok(dir) = std::fs::read_dir("/proc") else { return out };
    for p in dir.flatten() {
        let Some(pid) = p.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else { continue };
        if !std::fs::read_to_string(format!("/proc/{pid}/comm")).is_ok_and(|c| c.trim() == "codex") || !(under(pid, win) || crate::mux::in_app_window(win, pid)) {
            continue;
        }
        if let Some((id, transcript)) = codex_rollout(pid) {
            out.push((pid, SessionRef { agent: Agent::Codex, id, transcript, project: project_of(pid) }));
        }
    }
    out
}

/// `agent-speak show`: агент (pid) — на экран. Вкладку Orca или панель herdr выбирает его мультиплексор, окно поднимает
/// Hyprland; в терминал ничего не пишется. Окна нет — Ома ничего не открывает и говорит, почему.
pub fn bring(pid: u32) -> Result<(), String> {
    let pane = crate::mux::locate(pid);
    let clients = crate::mux::run(Command::new("hyprctl").args(["clients", "-j"]), HYPRCTL_TIMEOUT)?;
    let win = app_window(&clients, pid, ppid, |w| crate::mux::in_app_window(w, pid));
    show_steps(pane.as_ref(), win.as_deref(), crate::mux::focus, raise)
}

/// Окно агента: сначала то, чей процесс — его предок (терминал; окно Orca, когда pty-хост — потомок окна), и только
/// потом окно приложения-мультиплексора, которое держит его терминал (in_app_window).
/// shortcut: второе — первое окно с подстрокой «orca»/«herdr» в командной строке (терминал с такой папкой в аргументах
/// стоит раньше — поднимется он), а окна однопроцессного терминала (foot --server) — предки всем его агентам, и берётся
/// первое; станет мешать — спрашивать у мультиплексора окно его терминала.
fn app_window(clients: &str, pid: u32, parent: impl Fn(u32) -> Option<u32>, app: impl Fn(u32) -> bool) -> Option<String> {
    nearest_window(clients, pid, parent).or_else(|| window_of(clients, app))
}

/// Окно агента — ближайший к нему предок, у которого есть окно (идём вверх от агента): Orca, запущенная из терминала, —
/// это окно Orca, а не терминал, хотя предки агента оба.
fn nearest_window(clients: &str, pid: u32, parent: impl Fn(u32) -> Option<u32>) -> Option<String> {
    let mut p = pid;
    while p > 1 {
        if let Some(w) = window_of(clients, |w| w == p) {
            return Some(w);
        }
        p = parent(p)?;
    }
    None
}

const HYPRCTL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// Порядок показа: окно найдено — вкладка, потом окно. Окна нет — CLI мультиплексора не зовётся (открыл бы закрытое), кроме
/// herdr: окно его клиента — терминал, где он запущен, а агент — потомок сервера herdr, и по дереву процессов окно не найти.
fn show_steps(pane: Option<&crate::mux::Pane>, win: Option<&str>, focus: impl FnOnce(&crate::mux::Pane) -> Result<(), String>,
              raise: impl FnOnce(&str) -> Result<(), String>) -> Result<(), String> {
    match (pane, win) {
        (Some(p), None) if p.mux == "orca" => Err("Orca закрыта — открой её".into()),
        (Some(p), None) => {
            focus(p)?;
            Err(format!("в {} выбрала его панель, а окно {} не нашла", p.mux, p.mux))
        }
        (None, None) => Err("окно агента не нашла".into()),
        (pane, Some(w)) => {
            if let Some(p) = pane {
                focus(p)?;
            }
            raise(w)
        }
    }
}

/// Окно из ответа `hyprctl clients -j`, чей процесс держит агента (holds); адрес — только 0x и hex: он уходит в Lua.
fn window_of(clients: &str, holds: impl Fn(u32) -> bool) -> Option<String> {
    let v: Value = serde_json::from_str(clients).ok()?;
    let w = v.as_array()?.iter().find(|w| w["pid"].as_u64().and_then(|p| u32::try_from(p).ok()).is_some_and(&holds))?;
    let a = w["address"].as_str()?;
    a.strip_prefix("0x").filter(|h| !h.is_empty() && h.chars().all(|c| c.is_ascii_hexdigit())).map(|_| a.to_string())
}

/// Окно вперёд — как док оболочки Omarchy (hl.dsp.focus по адресу): на Hyprland с конфигом Lua `dispatch` принимает
/// только выражение Lua, прежний `focuswindow pid:…` — ошибка синтаксиса. Успех — ответ «ok», окна нет — warning.
fn raise(address: &str) -> Result<(), String> {
    let lua = format!("hl.dsp.focus({{ window = \"address:{address}\" }})");
    let out = crate::mux::run(Command::new("hyprctl").args(["dispatch", &lua]), HYPRCTL_TIMEOUT)?;
    if out.trim() == "ok" { Ok(()) } else { Err(format!("окно не подняла: {}", out.trim())) }
}

pub fn parse_learned(out: &str, asked: &[String]) -> Vec<(String, String)> {
    out.lines()
        .filter_map(|l| l.split_once('\t'))
        .filter_map(|(w, p)| Some((w.trim().to_lowercase(), crate::text::terms::speakable(&crate::text::plus_stress(p.trim()))?)))
        .filter(|(w, _)| asked.contains(w))
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn empty_desktop_is_no_window_not_an_error() {
        assert_eq!(window_pid(b"{}"), Ok(None), "Hyprland на пустом столе отвечает {{}}");
        assert_eq!(window_pid(br#"{"pid": 42, "class": "foot"}"#), Ok(Some(42)));
        assert!(window_pid(b"").is_err(), "hyprctl упал или не ответил — это в журнал");
    }

    #[test]
    fn several_agents_under_one_window_need_the_mux() {
        let r = |id: &str| SessionRef { agent: Agent::Claude, id: id.into(), transcript: PathBuf::new(), project: String::new() };
        let two = vec![(1, r("a")), (2, r("b"))];
        assert_eq!(pick(two.clone(), |p| (p == [1, 2]).then_some(2)).unwrap().id, "b", "Orca — одно окно на всех: не первый попавшийся");
        assert_eq!(pick(two, |_| None), None, "мультиплексор не знает — фокус неизвестен");
        assert_eq!(pick(vec![(1, r("a"))], |_| panic!("один агент — спрашивать нечего")).unwrap().id, "a");
        assert_eq!(pick(Vec::new(), |_| panic!("нет агентов")), None);
    }

    use super::*;

    #[test]
    fn window_of_the_agent_by_its_address() {
        let clients = r#"[{"address":"0x5a","pid":10},{"address":"0x5b","pid":20},{"address":"0x5c\") os.exit(","pid":30},{"pid":40}]"#;
        assert_eq!(window_of(clients, |w| w == 20).as_deref(), Some("0x5b"));
        assert_eq!(window_of(clients, |w| w == 99), None);
        assert_eq!(window_of(clients, |w| w == 30), None, "адрес уходит в Lua — только 0x и hex");
        assert_eq!(window_of(clients, |w| w == 40), None);
        assert_eq!(window_of("мусор", |_| true), None);
        // по дереву процессов: окно — предок агента (терминал без мультиплексора)
        let me = std::process::id();
        let parent = ppid(me).unwrap();
        let live = format!(r#"[{{"address":"0xab","pid":{parent}}}]"#);
        assert_eq!(window_of(&live, |w| under(me, w)).as_deref(), Some("0xab"));
    }

    #[test]
    fn the_window_holding_the_agent_before_one_named_like_its_app() {
        // терминал (10) запустил Orca (20), её pty-хост (25) держит агента (30): оба окна — предки агента, первым в списке
        // стоит терминал, а вперёд должна выйти Orca — ближайший к агенту предок с окном
        let clients = r#"[{"address":"0x1","pid":10},{"address":"0x2","pid":20}]"#;
        let tree = |p: u32| match p { 30 => Some(25), 25 => Some(20), 20 => Some(10), 10 => Some(1), _ => None };
        assert_eq!(app_window(clients, 30, tree, |w| w == 10 || w == 20).as_deref(), Some("0x2"), "ближайший предок, а не первый в списке");
        let apart = |p: u32| match p { 30 => Some(5), 5 => Some(1), _ => None };
        assert_eq!(app_window(clients, 30, apart, |w| w == 10 || w == 20).as_deref(), Some("0x1"), "pty-хост не под окном — по имени приложения");
        assert_eq!(app_window(clients, 30, apart, |_| false), None);
    }

    #[test]
    fn show_order_and_closed_apps() {
        use crate::mux::Pane;
        let calls = std::cell::RefCell::new(Vec::<String>::new());
        let go = |pane: Option<&Pane>, win: Option<&str>| {
            calls.borrow_mut().clear();
            let r = show_steps(pane, win, |p| { calls.borrow_mut().push(format!("focus {}", p.mux)); Ok(()) },
                               |w| { calls.borrow_mut().push(format!("raise {w}")); Ok(()) });
            (r, calls.borrow().clone())
        };
        let orca = Pane { mux: "orca", id: "term_1".into(), via: "/x/orca".into() };
        let herdr = Pane { mux: "herdr", id: "w1:p1".into(), via: "/s.sock".into() };
        assert_eq!(go(Some(&orca), Some("0x1")), (Ok(()), vec!["focus orca".into(), "raise 0x1".into()]), "вкладка, потом окно");
        assert_eq!(go(Some(&orca), None), (Err("Orca закрыта — открой её".into()), vec![]), "закрытую Orca CLI открыл бы — не зовём");
        let (r, c) = go(Some(&herdr), None);
        assert!(r.unwrap_err().contains("окно herdr не нашла") && c == ["focus herdr"], "окно клиента herdr по дереву не найти — панель всё же выбрана");
        assert_eq!(go(None, Some("0x2")), (Ok(()), vec!["raise 0x2".into()]), "агент в простом терминале — только окно");
        assert_eq!(go(None, None).0, Err("окно агента не нашла".into()));
        let failed = show_steps(Some(&orca), Some("0x1"), |_| Err("нет терминала".into()), |_| panic!("вкладка не вышла — окно не трогаем"));
        assert_eq!(failed, Err("нет терминала".into()));
    }

    #[test]
    fn user_codex_only() {
        assert!(is_user_codex("codex\0") && is_user_codex("codex\0resume\0"));
        assert!(!is_user_codex("codex\0exec\0слово") && !is_user_codex("codex\0app-server\0"));
    }

    #[test]
    fn session_from_paths() {
        let c = Path::new("/home/u/.claude/projects/-x/00000000-0000-0000-0000-000000000001.jsonl");
        assert_eq!(session_of(c), Some((Agent::Claude, "00000000-0000-0000-0000-000000000001".into())));
        let x = Path::new("/home/u/.codex/sessions/2026/10/02/rollout-2026-10-02T19-44-44-00000000-0000-0000-0000-000000000002.jsonl");
        assert_eq!(session_of(x), Some((Agent::Codex, "00000000-0000-0000-0000-000000000002".into())));
        assert_eq!(session_of(Path::new("/tmp/a.txt")), None);
    }

    #[test]
    fn learned_lines_validated() {
        let asked = vec!["tokio".to_string(), "serde".to_string()];
        let out = "tokio\tток+ио\nserde\tserde\nfoo\tфу\nмусор";
        assert_eq!(parse_learned(out, &asked), vec![("tokio".to_string(), "ток+ио".to_string())]);
    }

    #[test]
    fn learned_with_any_latin_letter_rejected() {
        let asked = vec!["demo".to_string(), "mode".to_string(), "ok".to_string()];
        let out = "demo\tдé+мо\nmode\tмóуд\nok\tок-ей\n";
        assert_eq!(parse_learned(out, &asked), vec![("ok".to_string(), "ок-ей".to_string())]);
    }

    #[test]
    fn learned_explanations_and_foreign_letters_rejected() {
        let asked: Vec<String> = ["stdin", "key", "toggleswitch", "plugins"].map(String::from).to_vec();
        let out = "stdin\tстандартный вход (или \"стдин\")\nkey\tкѝ\ntoggleswitch\tт+оггл свич\nplugins\tпла\u{301}гинс\n";
        assert_eq!(parse_learned(out, &asked), vec![("plugins".to_string(), "пл+агинс".to_string())]);
    }

    #[test]
    fn combining_stress_becomes_plus() {
        let asked = vec!["tokio".to_string(), "ssh".to_string()];
        let out = "tokio\tто\u{301}кио\nssh\tэс-эс-э\u{301}йч";
        assert_eq!(parse_learned(out, &asked), vec![("tokio".to_string(), "т+окио".to_string()), ("ssh".to_string(), "эс-эс-+эйч".to_string())]);
    }

    #[test]
    fn project_is_process_cwd_dir_name() {
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(project_of(std::process::id()), cwd.file_name().unwrap().to_str().unwrap());
        assert_eq!(project_of(u32::MAX), "");
    }
}
