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
    /// Настройка из плагина: сервис проверяет, сохраняет config.toml и рассылает состояние.
    Set { key: String, value: serde_json::Value },
    /// Прочитать текст как ручное чтение (образец голоса).
    Say { text: String },
    /// Фраза голосового разговора (от agent_voice).
    Voice { text: String },
    /// Включить голосовой разговор с агентом (claude | codex).
    VoiceStart { agent: String },
    VoiceStop,
    /// Хоткей: выключен — открыть окно выбора агента, включён — выключить.
    VoiceToggle,
    /// Фаза разговора от agent_voice: listening | thinking | speaking.
    VoiceState { state: String },
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
        let set = serde_json::from_str::<Msg>("{\"cmd\":\"set\",\"key\":\"read_intermediate\",\"value\":false}").unwrap();
        assert!(matches!(set, Msg::Set { ref key, ref value } if key == "read_intermediate" && *value == serde_json::json!(false)));
        assert!(matches!(serde_json::from_str::<Msg>("{\"cmd\":\"say\",\"text\":\"т\"}").unwrap(), Msg::Say { .. }));
        assert!(matches!(serde_json::from_str::<Msg>("{\"cmd\":\"voice\",\"text\":\"т\"}").unwrap(), Msg::Voice { .. }));
        assert!(matches!(serde_json::from_str::<Msg>("{\"cmd\":\"voice_start\",\"agent\":\"codex\"}").unwrap(), Msg::VoiceStart { ref agent } if agent == "codex"));
        assert!(matches!(serde_json::from_str::<Msg>("{\"cmd\":\"voice_stop\"}").unwrap(), Msg::VoiceStop));
        assert!(matches!(serde_json::from_str::<Msg>("{\"cmd\":\"voice_toggle\"}").unwrap(), Msg::VoiceToggle));
        assert!(matches!(serde_json::from_str::<Msg>("{\"cmd\":\"voice_state\",\"state\":\"thinking\"}").unwrap(), Msg::VoiceState { .. }));
    }

    #[test]
    fn send_without_daemon_is_false() {
        unsafe { std::env::set_var("XDG_RUNTIME_DIR", "/nonexistent-agent-speak") };
        assert!(!send(&Msg::Stop));
    }
}
