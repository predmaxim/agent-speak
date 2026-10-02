//! Команды клиента → сервис: одна JSON-строка в unix-сокет.

use serde::{Deserialize, Serialize};
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Msg {
    Hook { kind: String, payload: serde_json::Value },
    Read,
    Stop,
    Pause,
    Mode,
    /// Соединение остаётся открытым: сервис пишет строку состояния сразу и при каждом изменении.
    Subscribe,
}

pub fn socket_path() -> PathBuf {
    PathBuf::from(std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into())).join("agent-speak.sock")
}

pub fn send(msg: &Msg) -> bool {
    let Ok(mut s) = UnixStream::connect(socket_path()) else { return false };
    s.write_all(format!("{}\n", serde_json::to_string(msg).unwrap()).as_bytes()).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn msg_roundtrip() {
        let m = Msg::Hook { kind: "stop".into(), payload: serde_json::json!({"session_id": "s"}) };
        let s = serde_json::to_string(&m).unwrap();
        assert!(s.contains("\"cmd\":\"hook\""));
        assert!(matches!(serde_json::from_str::<Msg>(&s).unwrap(), Msg::Hook { .. }));
        assert!(matches!(serde_json::from_str::<Msg>("{\"cmd\":\"pause\"}").unwrap(), Msg::Pause));
        assert!(matches!(serde_json::from_str::<Msg>("{\"cmd\":\"subscribe\"}").unwrap(), Msg::Subscribe));
    }

    #[test]
    fn send_without_daemon_is_false() {
        unsafe { std::env::set_var("XDG_RUNTIME_DIR", "/nonexistent-agent-speak") };
        assert!(!send(&Msg::Stop));
    }
}
