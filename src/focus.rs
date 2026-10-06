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

fn ppid(pid: u32) -> Option<u32> {
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
fn project_of(pid: u32) -> String {
    std::fs::read_link(format!("/proc/{pid}/cwd"))
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_default()
}

pub fn focused() -> Option<SessionRef> {
    // timeout: зависший hyprctl не должен вешать цикл событий
    let out = Command::new("timeout").args(["2", "hyprctl", "activewindow", "-j"]).output().ok()?;
    let win = serde_json::from_slice::<Value>(&out.stdout).ok().and_then(|v| v["pid"].as_u64());
    // опрос раз в секунду: пишем в журнал только переход «есть окно → нет окна»
    if NO_WINDOW.swap(win.is_none(), Ordering::Relaxed) || win.is_some() {
        return win.and_then(|w| claude_under(w as u32).or_else(|| codex_under(w as u32)));
    }
    eprintln!("agent-speak: hyprctl: нет pid окна ({}): {}", out.status, String::from_utf8_lossy(&out.stderr).trim());
    None
}

static NO_WINDOW: AtomicBool = AtomicBool::new(false);

fn claude_under(win: u32) -> Option<SessionRef> {
    for f in std::fs::read_dir(home().join(".claude/sessions")).ok()?.flatten() {
        let Ok(s) = std::fs::read_to_string(f.path()) else { continue };
        let Ok(v) = serde_json::from_str::<Value>(&s) else { continue };
        let (Some(pid), Some(sid)) = (v["pid"].as_u64(), v["sessionId"].as_str()) else { continue };
        if !under(pid as u32, win) {
            continue;
        }
        let name = format!("{sid}.jsonl");
        for d in std::fs::read_dir(home().join(".claude/projects")).ok()?.flatten() {
            let p = d.path().join(&name);
            if p.exists() {
                return Some(SessionRef { agent: Agent::Claude, id: sid.to_string(), transcript: p, project: project_of(pid as u32) });
            }
        }
    }
    None
}

fn codex_under(win: u32) -> Option<SessionRef> {
    for p in std::fs::read_dir("/proc").ok()?.flatten() {
        let Some(pid) = p.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else { continue };
        if !std::fs::read_to_string(format!("/proc/{pid}/comm")).is_ok_and(|c| c.trim() == "codex") || !under(pid, win) {
            continue;
        }
        for fd in std::fs::read_dir(format!("/proc/{pid}/fd")).ok()?.flatten() {
            let Ok(target) = std::fs::read_link(fd.path()) else { continue };
            if let Some((Agent::Codex, id)) = session_of(&target) {
                return Some(SessionRef { agent: Agent::Codex, id, transcript: target, project: project_of(pid) });
            }
        }
    }
    None
}

/// Haiku ставит ударение комбинирующим знаком U+0301 после гласной; Silero ждёт «+» перед ней.
fn plus_stress(p: &str) -> String {
    let mut v: Vec<char> = Vec::new();
    for c in p.chars() {
        if c == '\u{301}' && !v.is_empty() {
            v.insert(v.len() - 1, '+');
        } else {
            v.push(c);
        }
    }
    v.into_iter().collect()
}

pub fn parse_learned(out: &str, asked: &[String]) -> Vec<(String, String)> {
    out.lines()
        .filter_map(|l| l.split_once('\t'))
        .map(|(w, p)| (w.trim().to_lowercase(), plus_stress(p.trim())))
        .filter(|(w, p)| {
            asked.contains(w) && !p.is_empty() && p.chars().any(|c| ('а'..='я').contains(&c.to_lowercase().next().unwrap()))
                && !p.chars().any(|c| c.is_alphabetic() && !('\u{400}'..='\u{4FF}').contains(&c)) // латиница (и é) — движок давится
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

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
