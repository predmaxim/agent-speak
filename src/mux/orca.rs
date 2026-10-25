//! Orca (MIT): терминалы — через его CLI `orca terminal …`.

use super::{cli, run, Key, Mux, Pane};
use serde_json::Value;
use std::collections::HashMap;
use std::process::Command;
use std::time::Duration;

/// Запуск агента дольше обычного вызова CLI: Orca открывает вкладку и стартует оболочку.
const LAUNCH_TIMEOUT: Duration = Duration::from_secs(30);

pub struct Orca;

impl Mux for Orca {
    fn name(&self) -> &'static str {
        "orca"
    }
    fn hosts(&self, cmdline: &str) -> bool {
        cmdline.contains("orca") // pty-хост — node из AppImage Orca
    }
    fn locate(&self, env: &HashMap<String, String>) -> Option<Pane> {
        let id = env.get("ORCA_TERMINAL_HANDLE").filter(|h| !h.is_empty())?;
        let dir = env.get("ORCA_CLI_BIN_DIR").filter(|d| !d.is_empty())?;
        Some(Pane { mux: "orca", id: id.clone(), via: format!("{dir}/orca") })
    }
    fn send(&self, pane: &Pane, text: &str) -> Result<(), String> {
        accepted(&cli(orca(pane, &["send", "--text", text, "--enter"]))?)
    }
    fn key(&self, pane: &Pane, key: &Key) -> Result<(), String> {
        let out = match key_text(key) {
            Some(t) => cli(orca(pane, &["send", "--text", &t]))?,
            None => cli(orca(pane, &["send", "--interrupt"]))?,
        };
        accepted(&out)
    }
    fn type_text(&self, pane: &Pane, text: &str) -> Result<(), String> {
        accepted(&cli(orca(pane, &["send", "--text", text]))?)
    }
    fn screen(&self, pane: &Pane) -> Result<String, String> {
        let out = cli(orca(pane, &["read", "--screen"]))?;
        screen_text(&out).ok_or_else(|| error_of(&out))
    }
    fn projects(&self) -> Result<Vec<String>, String> {
        let mut c = Command::new(orca_bin());
        c.args(["worktree", "list", "--json"]);
        Ok(worktree_paths(&cli(c)?))
    }
    fn launch(&self, cwd: &str, prompt: &str) -> Result<(), String> {
        let mut c = Command::new(orca_bin());
        let wt = format!("path:{cwd}");
        // флаги агента Orca берёт из своих настроек — те же, с какими его запускает пользователь
        c.args(["agent", "launch", "--agent", "claude", "--worktree", &wt, "--surface", "terminal", "--prompt", prompt, "--json"]);
        succeeded(&run(&mut c, LAUNCH_TIMEOUT)?)
    }
}

/// Клавиша — текстом в `terminal send --text`; Ctrl+C — свой флаг `--interrupt` (None).
fn key_text(key: &Key) -> Option<String> {
    Some(match key {
        Key::Esc => "\x1b".into(),
        Key::CtrlC => return None,
        Key::CtrlU => "\x15".into(),
        Key::Enter => "\r".into(),
        Key::Digit(d) => d.to_string(),
    })
}

/// CLI Orca без терминала агента (проекты, запуск): обёртка из его настроек — у служб systemd PATH бывает без неё.
fn orca_bin() -> String {
    let config = std::env::var("XDG_CONFIG_HOME").unwrap_or_else(|_| format!("{}/.config", std::env::var("HOME").unwrap_or_default()));
    let shim = format!("{config}/orca/linux-orca-cli-shim/orca");
    if std::path::Path::new(&shim).exists() { shim } else { "orca".into() }
}

/// Ответ `worktree list`: папки worktree; мусор — пусто.
fn worktree_paths(out: &str) -> Vec<String> {
    let v: Value = serde_json::from_str(out).unwrap_or_default();
    v["result"]["worktrees"].as_array().into_iter().flatten().filter_map(|w| w["path"].as_str().map(str::to_string)).collect()
}

/// Ответ команды Orca без особого поля: `ok` — успех.
fn succeeded(out: &str) -> Result<(), String> {
    let v: Value = serde_json::from_str(out).map_err(|_| error_of(out))?;
    if v["ok"] == true { Ok(()) } else { Err(error_of(out)) }
}

/// `orca terminal <sub> --terminal <id> … --json`.
fn orca(pane: &Pane, args: &[&str]) -> Command {
    let mut c = Command::new(&pane.via);
    c.arg("terminal").arg(args[0]).args(["--terminal", &pane.id]).args(&args[1..]).arg("--json");
    c
}

fn error_of(out: &str) -> String {
    let v: Value = serde_json::from_str(out).unwrap_or_default();
    v["error"]["message"].as_str().map_or_else(|| out.chars().take(200).collect(), str::to_string)
}

/// Ответ `terminal send`: принят ли ввод.
pub fn accepted(out: &str) -> Result<(), String> {
    let v: Value = serde_json::from_str(out).map_err(|_| error_of(out))?;
    if v["ok"] == true && v["result"]["send"]["accepted"] == true { Ok(()) } else { Err(error_of(out)) }
}

/// Ответ `terminal read --screen`: строки экрана.
pub fn screen_text(out: &str) -> Option<String> {
    let v: Value = serde_json::from_str(out).ok()?;
    let tail = v["result"]["terminal"]["tail"].as_array()?;
    Some(tail.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locates_by_terminal_handle() {
        let env = HashMap::from([("ORCA_TERMINAL_HANDLE".into(), "term_1".into()), ("ORCA_CLI_BIN_DIR".into(), "/x/shim".into())]);
        assert_eq!(Orca.locate(&env), Some(Pane { mux: "orca", id: "term_1".into(), via: "/x/shim/orca".into() }));
        assert_eq!(Orca.locate(&HashMap::new()), None);
    }

    #[test]
    fn reply_parsing() {
        assert_eq!(accepted(r#"{"ok":true,"result":{"send":{"accepted":true}}}"#), Ok(()));
        assert!(accepted(r#"{"ok":false,"error":{"message":"no terminal"}}"#).unwrap_err().contains("no terminal"));
        assert!(accepted(r#"{"ok":true,"result":{"send":{"accepted":false}}}"#).is_err());
        assert_eq!(screen_text(r#"{"ok":true,"result":{"terminal":{"tail":["a","b"]}}}"#).unwrap(), "a\nb");
        assert_eq!(screen_text("мусор"), None);
    }

    #[test]
    fn keys_as_text() {
        assert_eq!(key_text(&Key::Enter), Some("\r".into()));
        assert_eq!(key_text(&Key::Digit(3)), Some("3".into()));
        assert_eq!(key_text(&Key::Esc), Some("\x1b".into()));
        assert_eq!(key_text(&Key::CtrlU), Some("\x15".into()));
        assert_eq!(key_text(&Key::CtrlC), None, "у Ctrl+C — --interrupt");
    }

    #[test]
    fn worktrees_and_launch_reply() {
        assert_eq!(worktree_paths(r#"{"ok":true,"result":{"worktrees":[{"path":"/p/a"},{"path":"/p/b"}]}}"#), ["/p/a", "/p/b"]);
        assert!(worktree_paths("мусор").is_empty());
        assert_eq!(succeeded(r#"{"ok":true,"result":{}}"#), Ok(()));
        assert!(succeeded(r#"{"ok":false,"error":{"message":"selector_not_found"}}"#).unwrap_err().contains("selector_not_found"));
    }
}
