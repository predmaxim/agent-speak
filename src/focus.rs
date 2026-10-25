//! Окно в фокусе → сессия агента; путь транскрипта → сессия.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};

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

fn under(mut pid: u32, ancestor: u32) -> bool {
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

/// Агент окна в фокусе. Несколько агентов под одним окном (Orca, herdr) — без precise фокус неизвестен,
/// с precise — спрашиваем мультиплексор, чей терминал активен (CLI Orca — полсекунды процессора).
pub fn focused(precise: bool) -> Option<SessionRef> {
    // timeout: зависший hyprctl не должен вешать цикл событий
    let out = Command::new("timeout").args(["2", "hyprctl", "activewindow", "-j"]).output().ok()?;
    let win = serde_json::from_slice::<Value>(&out.stdout).ok().and_then(|v| v["pid"].as_u64());
    // опрос раз в секунду: пишем в журнал только переход «есть окно → нет окна»
    if NO_WINDOW.swap(win.is_none(), Ordering::Relaxed) || win.is_some() {
        let w = win? as u32;
        let mut all = claude_under(w);
        all.extend(codex_under(w));
        return pick(all, precise, crate::mux::active_among);
    }
    eprintln!("agent-speak: hyprctl: нет pid окна ({}): {}", out.status, String::from_utf8_lossy(&out.stderr).trim());
    None
}

static NO_WINDOW: AtomicBool = AtomicBool::new(false);

/// Агент из кандидатов (pid, сессия): один — он; несколько — по активному терминалу, и только если precise.
fn pick(all: Vec<(u32, SessionRef)>, precise: bool, active: impl Fn(&[u32]) -> Option<u32>) -> Option<SessionRef> {
    if all.len() <= 1 {
        return all.into_iter().next().map(|(_, s)| s);
    }
    if !precise {
        return None;
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
        if !under(pid as u32, win) {
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
        if !std::fs::read_to_string(format!("/proc/{pid}/comm")).is_ok_and(|c| c.trim() == "codex") || !under(pid, win) {
            continue;
        }
        if let Some((id, transcript)) = codex_rollout(pid) {
            out.push((pid, SessionRef { agent: Agent::Codex, id, transcript, project: project_of(pid) }));
        }
    }
    out
}

pub fn parse_learned(out: &str, asked: &[String]) -> Vec<(String, String)> {
    out.lines()
        .filter_map(|l| l.split_once('\t'))
        .map(|(w, p)| (w.trim().to_lowercase(), crate::text::plus_stress(p.trim())))
        .filter(|(w, p)| {
            asked.contains(w) && !p.is_empty() && p.chars().any(|c| ('а'..='я').contains(&c.to_lowercase().next().unwrap()))
                && !p.chars().any(|c| c.is_alphabetic() && !('\u{400}'..='\u{4FF}').contains(&c)) // латиница (и é) — движок давится
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn several_agents_under_one_window_need_the_mux() {
        let r = |id: &str| SessionRef { agent: Agent::Claude, id: id.into(), transcript: PathBuf::new(), project: String::new() };
        let two = vec![(1, r("a")), (2, r("b"))];
        assert_eq!(pick(two.clone(), false, |_| panic!("дешёвый путь CLI не зовёт")), None, "Orca — одно окно на всех: первый попавшийся — не фокус");
        assert_eq!(pick(two, true, |p| (p == [1, 2]).then_some(2)).unwrap().id, "b");
        assert_eq!(pick(vec![(1, r("a"))], false, |_| panic!("один агент — спрашивать нечего")).unwrap().id, "a");
        assert_eq!(pick(Vec::new(), true, |_| panic!("нет агентов")), None);
    }

    use super::*;

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
