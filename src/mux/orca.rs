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
    fn shown(&self, env: &HashMap<String, String>) -> Option<bool> {
        // открытую вкладку Orca хранит в своей базе (обновляет за секунду); CLI её не знает, а стоит полсекунды процессора
        let (tab, worktree, data) = (env.get("ORCA_TAB_ID")?, env.get("ORCA_WORKTREE_ID")?, env.get("ORCA_USER_DATA_PATH")?);
        let index: Value = serde_json::from_str(&std::fs::read_to_string(format!("{data}/orca-profile-index.json")).ok()?).ok()?;
        let db = format!("file:{data}/profiles/{}/profile-state.db?mode=ro", index["activeProfileId"].as_str()?);
        let mut c = Command::new("sqlite3");
        c.args(["-json", "-readonly", &db, SHOWN_QUERY]);
        tab_shown(&cli(c).ok()?, worktree, tab)
    }
    fn projects(&self) -> Result<Vec<String>, String> {
        let mut c = Command::new(orca_bin());
        c.args(["worktree", "list", "--json"]);
        Ok(worktree_paths(&cli(c).map_err(closed)?))
    }
    fn no_project(&self) -> String {
        "этого проекта нет в Orca — добавь его, и я запущу".into()
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

/// Ошибка CLI без терминала агента: закрытая Orca — понятной фразой. Обёртка её CLI тогда падает
/// («Orca CLI is unavailable»), а без обёртки нет и `orca` в PATH.
fn closed(err: String) -> String {
    if err.contains("Orca CLI is unavailable") || err.contains("No such file") {
        "Orca не запущена — открой её, и я запущу".into()
    } else {
        err
    }
}

const SHOWN_QUERY: &str = "select json_extract(payload,'$.activeWorktreeId') as worktree, json_extract(payload,'$.activeTabIdByWorktree') as tabs from profile_state_documents where domain='workspaceSession'";

/// Ответ базы Orca: открыт ли сейчас worktree агента и в нём — его вкладка.
fn tab_shown(out: &str, worktree: &str, tab: &str) -> Option<bool> {
    let rows: Value = serde_json::from_str(out).ok()?;
    let row = rows.get(0)?;
    let tabs: Value = serde_json::from_str(row["tabs"].as_str()?).ok()?;
    Some(row["worktree"] == worktree && tabs[worktree] == tab)
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
    fn shown_tab_from_workspace_session() {
        // так отвечает sqlite3 -json на запрос к profile-state.db: вкладка, открытая в окне Orca
        let out = r#"[{"worktree":"w2::/p/b","tabs":"{\"w1::/p/a\":\"tab-a\",\"w2::/p/b\":\"tab-b\"}"}]"#;
        assert_eq!(tab_shown(out, "w2::/p/b", "tab-b"), Some(true));
        assert_eq!(tab_shown(out, "w1::/p/a", "tab-a"), Some(false), "вкладка своего worktree, но открыт другой");
        assert_eq!(tab_shown(out, "w2::/p/b", "tab-x"), Some(false));
        assert_eq!(tab_shown("[]", "w2::/p/b", "tab-b"), None, "нет сессии — фокус неизвестен");
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

    #[test]
    fn closed_orca_is_said_plainly() {
        let shim = "exit status: 1: Orca CLI is unavailable; reopen Orca or register the CLI again.";
        assert!(closed(shim.into()).starts_with("Orca не запущена"), "закрытая Orca — не «нет такого проекта»");
        assert!(closed("No such file or directory (os error 2)".into()).starts_with("Orca не запущена"));
        assert_eq!(closed("таймаут 10 с".into()), "таймаут 10 с");
    }
}
