//! herdr (Apache-2.0): панели — через его CLI, сессия выбирается сокетом `HERDR_SOCKET_PATH`.

use super::{cli, Key, Mux, Pane};
use serde_json::Value;
use std::collections::HashMap;
use std::process::Command;

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
    fn screen(&self, pane: &Pane) -> Result<String, String> {
        cli(herdr(pane, &["pane", "read", &pane.id, "--source", "visible", "--lines", "40"]))
    }
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
    fn key_names() {
        assert_eq!(key_name(&Key::Digit(2)), "2");
        assert_eq!(key_name(&Key::Enter), "enter");
        assert_eq!(key_name(&Key::Esc), "esc");
    }
}
