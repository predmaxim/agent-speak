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
        prompted.or_else(|_| cli(herdr(pane, &["pane", "run", &pane.id, text])).and_then(|o| ok(&o)))
    }
    fn key(&self, pane: &Pane, key: &Key) -> Result<(), String> {
        let k = match key {
            Key::Esc => "esc",
            Key::CtrlC => "ctrl+c",
            Key::CtrlU => "ctrl+u",
        };
        ok(&cli(herdr(pane, &["pane", "send-keys", &pane.id, k]))?)
    }
    fn screen(&self, pane: &Pane) -> Result<String, String> {
        cli(herdr(pane, &["pane", "read", &pane.id, "--source", "visible", "--lines", "40"]))
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
}
