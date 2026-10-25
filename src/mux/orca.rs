//! Orca (MIT): терминалы — через его CLI `orca terminal …`.

use super::{cli, Key, Mux, Pane};
use serde_json::Value;
use std::collections::HashMap;
use std::process::Command;

pub struct Orca;

impl Mux for Orca {
    fn name(&self) -> &'static str {
        "orca"
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
        let out = match key {
            Key::Esc => cli(orca(pane, &["send", "--text", "\x1b"]))?,
            Key::CtrlC => cli(orca(pane, &["send", "--interrupt"]))?,
        };
        accepted(&out)
    }
    fn screen(&self, pane: &Pane) -> Result<String, String> {
        let out = cli(orca(pane, &["read", "--screen"]))?;
        screen_text(&out).ok_or_else(|| error_of(&out))
    }
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
}
