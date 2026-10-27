//! herdr (Apache-2.0): панели — через его CLI, сессия выбирается сокетом `HERDR_SOCKET_PATH`.

use super::{cli, run, Key, Mux, Pane};
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

pub struct Herdr;

impl Mux for Herdr {
    fn name(&self) -> &'static str {
        "herdr"
    }
    fn hosts(&self, cmdline: &str) -> bool {
        cmdline.contains("herdr") // панели держит сервер herdr
    }
    fn locate(&self, env: &HashMap<String, String>) -> Option<Pane> {
        let id = env.get("HERDR_PANE_ID").filter(|p| !p.is_empty())?;
        let sock = env.get("HERDR_SOCKET_PATH").filter(|s| !s.is_empty())?;
        Some(Pane { mux: "herdr", id: id.clone(), via: sock.clone() })
    }
    fn send(&self, pane: &Pane, text: &str) -> Result<(), String> {
        // agent prompt учитывает bracketed paste агента; не распознал агента — текст и Enter как команда
        let prompted = cli(herdr(pane, &["agent", "prompt", &pane.id, text])).and_then(|o| ok(&o));
        prompted.or_else(|e| if as_command(&e) { cli(herdr(pane, &["pane", "run", &pane.id, text])).and_then(|o| ok(&o)) } else { Err(e) })
    }
    fn key(&self, pane: &Pane, key: &Key) -> Result<(), String> {
        ok(&cli(herdr(pane, &["pane", "send-keys", &pane.id, &key_name(key)]))?)
    }
    fn type_text(&self, pane: &Pane, text: &str) -> Result<(), String> {
        ok(&cli(herdr(pane, &["pane", "send-text", &pane.id, text]))?)
    }
    fn shown(&self, env: &HashMap<String, String>) -> Option<bool> {
        let pane = self.locate(env)?;
        Some(focused_pane(&cli(herdr(&pane, &["pane", "list"])).ok()?)? == pane.id)
    }
    fn screen(&self, pane: &Pane) -> Result<String, String> {
        cli(herdr(pane, &["pane", "read", &pane.id, "--source", "visible", "--lines", "40"]))
    }
    fn projects(&self) -> Result<Vec<String>, String> {
        let home = std::env::var("HOME").map_err(|_| "нет HOME".to_string())?;
        Ok(repos(Path::new(&format!("{home}/Projects")), 3))
    }
    fn no_project(&self) -> String {
        "это не git-репозиторий в ~/Projects — в herdr запускаю только там".into()
    }
    /// Новое рабочее пространство в папке, в нём Claude, затем задание. Сессия — по умолчанию (сокет из окружения, иначе общий).
    fn launch(&self, cwd: &str, prompt: &str) -> Result<(), String> {
        let label = cwd.trim_end_matches('/').rsplit('/').next().unwrap_or(cwd);
        let created = cli(top(&["workspace", "create", "--cwd", cwd, "--label", label, "--focus"])).map_err(not_running)?;
        let (pane, ws) = root_pane(&created).ok_or_else(|| not_running(created.clone()))?;
        // имя агента в herdr уникально, а id рабочих пространств не переиспользуются
        let name = format!("{label}-{ws}");
        // флаги — как у агентов пользователя в Orca (agentDefaultArgs.claude); у herdr своих настроек агента нет
        let start = ["agent", "start", &name, "--kind", "claude", "--pane", &pane, "--", "--dangerously-skip-permissions"];
        run(&mut top(&start), LAUNCH_TIMEOUT).and_then(|o| ok(&o)).map_err(|e| {
            if e.contains("not ready") { "агент в herdr запустился, но ждёт ответа в терминале — ответь там".into() } else { e }
        })?;
        cli(top(&["agent", "prompt", &pane, prompt])).and_then(|o| ok(&o))
    }
}

/// Запуск агента дольше обычного вызова: herdr ждёт, пока Claude будет готов к вводу (у него самого до 30 с).
const LAUNCH_TIMEOUT: Duration = Duration::from_secs(40);

/// `herdr <args>` без панели агента: сессия — из HERDR_SOCKET_PATH, иначе общая.
fn top(args: &[&str]) -> Command {
    let mut c = Command::new("herdr");
    c.args(args);
    c
}

/// Ответ `workspace create`: панель и рабочее пространство.
fn root_pane(out: &str) -> Option<(String, String)> {
    let v: Value = serde_json::from_str(out.trim()).ok()?;
    let p = &v["result"]["root_pane"];
    Some((p["pane_id"].as_str()?.to_string(), p["workspace_id"].as_str()?.to_string()))
}

/// Сервер herdr не запущен — понятной фразой; иначе ошибка как есть.
fn not_running(err: String) -> String {
    if err.contains("server_not_running") { "herdr не запущен — открой его, и я запущу".into() } else { err }
}

/// git-репозитории (и worktree, у них .git — файл) внутри dir до глубины depth; сам dir не считается (~/Projects
/// бывает репозиторием), внутрь репозитория и в скрытые папки не заходим.
fn repos(dir: &Path, depth: u32) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut v: Vec<String> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()) && !e.file_name().to_string_lossy().starts_with('.'))
        .flat_map(|e| match e.path() {
            p if p.join(".git").exists() => vec![p.to_string_lossy().into_owned()],
            p if depth > 1 => repos(&p, depth - 1),
            _ => Vec::new(),
        })
        .collect();
    v.sort();
    v
}

/// Ответ `pane list`: панель в фокусе.
fn focused_pane(out: &str) -> Option<String> {
    let v: Value = serde_json::from_str(out.trim()).ok()?;
    let panes = v["result"]["panes"].as_array()?;
    panes.iter().find(|p| p["focused"] == true)?["pane_id"].as_str().map(str::to_string)
}

/// Отказ `agent prompt` — повод послать текст командой; агент в диалоге (agent_blocked) — нет: Enter выбрал бы пункт.
fn as_command(err: &str) -> bool {
    !err.contains("blocked")
}

/// Имя клавиши для `pane send-keys`.
fn key_name(key: &Key) -> String {
    match key {
        Key::Esc => "esc".into(),
        Key::CtrlC => "ctrl+c".into(),
        Key::CtrlU => "ctrl+u".into(),
        Key::Enter => "enter".into(),
        Key::Digit(d) => d.to_string(),
    }
}

/// `herdr <args>` у сессии агента: CLI выбирает сервер по сокету.
fn herdr(pane: &Pane, args: &[&str]) -> Command {
    let mut c = Command::new("herdr");
    c.env("HERDR_SOCKET_PATH", &pane.via).args(args);
    c
}

/// Ответ CLI: JSON с `error` — ошибка (код выхода при этом бывает и нулевым).
pub fn ok(out: &str) -> Result<(), String> {
    let v: Value = serde_json::from_str(out.trim()).unwrap_or_default();
    match v["error"]["message"].as_str() {
        Some(m) => Err(m.to_string()),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locates_by_pane_and_socket() {
        let env = HashMap::from([("HERDR_PANE_ID".into(), "w1:p1".into()), ("HERDR_SOCKET_PATH".into(), "/s.sock".into())]);
        assert_eq!(Herdr.locate(&env), Some(Pane { mux: "herdr", id: "w1:p1".into(), via: "/s.sock".into() }));
        assert_eq!(Herdr.locate(&HashMap::from([("HERDR_PANE_ID".into(), "w1:p1".into())])), None);
    }

    #[test]
    fn error_reply_is_err() {
        assert!(ok(r#"{"error":{"code":"agent_not_found","message":"agent target w1:p1 not found"},"id":"x"}"#).unwrap_err().contains("not found"));
        assert_eq!(ok(r#"{"id":"x","result":{}}"#), Ok(()));
    }

    #[test]
    fn blocked_agent_gets_no_command_fallback() {
        assert!(!as_command("agent w1:p1 is blocked and requires interactive input"), "текст с Enter в диалог — выбор пункта");
        assert!(as_command("agent target w1:p1 not found"), "herdr не распознал агента — команда в оболочку");
    }

    #[test]
    fn focused_pane_reply() {
        let out = r#"{"result":{"panes":[{"pane_id":"w1:p1","focused":false},{"pane_id":"w1:p2","focused":true}],"type":"pane_list"}}"#;
        assert_eq!(focused_pane(out), Some("w1:p2".into()));
        assert_eq!(focused_pane(r#"{"error":{"message":"no server"}}"#), None);
    }

    #[test]
    fn launch_replies() {
        // так отвечает herdr 0.9.1 на workspace create
        let out = r#"{"id":"cli:workspace:create","result":{"root_pane":{"pane_id":"w2:p1","workspace_id":"w2"},"type":"workspace_created"}}"#;
        assert_eq!(root_pane(out), Some(("w2:p1".into(), "w2".into())));
        assert_eq!(root_pane("мусор"), None);
        let down = r#"exit status: 1: {"error":{"code":"server_not_running","message":"no herdr server is running"}}"#;
        assert!(not_running(down.into()).starts_with("herdr не запущен"));
        assert_eq!(not_running("таймаут".into()), "таймаут");
    }

    #[test]
    fn repos_under_projects() {
        let root = std::env::temp_dir().join(format!("agent-speak-repos-{}", std::process::id()));
        for d in [".git", "a/.git", "a/sub/.git", "omarchy/own/b/.git", ".hidden/c/.git", "deep/x/y/z/.git", "plain"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        std::fs::create_dir_all(root.join("wt")).unwrap();
        std::fs::write(root.join("wt/.git"), "gitdir: /x").unwrap(); // worktree: .git — файл
        let got = repos(&root, 3);
        let _ = std::fs::remove_dir_all(&root);
        let names: Vec<_> = got.iter().map(|p| p.strip_prefix(&*root.to_string_lossy()).unwrap().to_string()).collect();
        assert_eq!(names, ["/a", "/omarchy/own/b", "/wt"], "сам корень, внутрь репозитория, в скрытые и глубже трёх — нет");
    }

    #[test]
    fn key_names() {
        assert_eq!(key_name(&Key::Digit(2)), "2");
        assert_eq!(key_name(&Key::Enter), "enter");
        assert_eq!(key_name(&Key::Esc), "esc");
    }
}
