//! Команды клиента → сервис: одна JSON-строка в unix-сокет.

use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Msg {
    Hook { kind: String, payload: serde_json::Value },
    Read,
    Stop,
    /// Ому перебили: оборвать звучащее, выбросить её ответ; новости и чтение остаются.
    Hush,
    Pause,
    /// Соединение остаётся открытым: сервис пишет строку состояния сразу и при каждом изменении.
    Subscribe,
    /// Настройка из плагина: сервис проверяет, сохраняет config.toml и рассылает состояние.
    Set { key: String, value: serde_json::Value },
    /// Прочитать текст как ручное чтение (образец голоса).
    Say { text: String },
    /// Ответ Омы голосом.
    Voice { text: String },
    /// Короткий звук «услышала»: реплика ушла мозгу, ответа ещё нет.
    Chime,
    /// Пауза Омы на время перебива (on) и продолжение (off): ручную паузу пользователя не трогает.
    OmaPause { on: bool },
    /// Состояние Омы (idle | loading | listening | thinking | speaking | paused) и talking — идёт ли разговор.
    VoiceState { state: String, #[serde(default)] talking: bool },
    /// Команда Оме от плагина и CLI: action уходит строкой в её stdin.
    Oma { action: serde_json::Value },
    /// Строка от Омы подписчикам как есть.
    Emit { line: serde_json::Value },
    /// Голос пользователя от Омы: here | busy («не сейчас») | away («я отойду»).
    Presence { state: String },
    /// Окно Омы показывает чат агента (id) или нет (пусто): лента его перед глазами — новости его не вслух.
    View { agent: String },
    /// Ответ агенту от Омы (`agent-speak tell`): запрос с ответом `{"ok": метка нажатого пункта}` или `{"err": …}`;
    /// идёт в очередь потока действий, той же, что у сторожа. expect — метка варианта, пусто — не сверять.
    Tell { session: String, option: usize, text: String, #[serde(default)] expect: String },
}

pub fn socket_path() -> PathBuf {
    PathBuf::from(std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into())).join("agent-speak.sock")
}

pub fn send(msg: &Msg) -> bool {
    let Ok(mut s) = UnixStream::connect(socket_path()) else { return false };
    s.write_all(format!("{}\n", serde_json::to_string(msg).unwrap()).as_bytes()).is_ok()
}

/// Команда с ответом (tell): строка туда, строка обратно — `{"ok": …}` или `{"err": …}`.
pub fn request(msg: &Msg) -> Result<String, String> {
    let mut s = UnixStream::connect(socket_path()).map_err(|_| "служба Омы не запущена".to_string())?;
    s.write_all(format!("{}\n", serde_json::to_string(msg).unwrap()).as_bytes()).map_err(|e| e.to_string())?;
    let mut line = String::new();
    BufReader::new(s).read_line(&mut line).map_err(|e| e.to_string())?;
    let v: serde_json::Value = serde_json::from_str(&line).unwrap_or_default();
    match v["ok"].as_str() {
        Some(ok) => Ok(ok.to_string()),
        None => Err(v["err"].as_str().unwrap_or("служба не ответила — перезапусти её после обновления").to_string()),
    }
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
        assert!(matches!(serde_json::from_str::<Msg>("{\"cmd\":\"hush\"}").unwrap(), Msg::Hush));
        assert!(matches!(serde_json::from_str::<Msg>("{\"cmd\":\"subscribe\"}").unwrap(), Msg::Subscribe));
        let set = serde_json::from_str::<Msg>("{\"cmd\":\"set\",\"key\":\"read_intermediate\",\"value\":false}").unwrap();
        assert!(matches!(set, Msg::Set { ref key, ref value } if key == "read_intermediate" && *value == serde_json::json!(false)));
        assert!(matches!(serde_json::from_str::<Msg>("{\"cmd\":\"say\",\"text\":\"т\"}").unwrap(), Msg::Say { .. }));
        assert!(matches!(serde_json::from_str::<Msg>("{\"cmd\":\"voice\",\"text\":\"т\"}").unwrap(), Msg::Voice { .. }));
        assert!(matches!(serde_json::from_str::<Msg>("{\"cmd\":\"chime\"}").unwrap(), Msg::Chime));
        assert!(matches!(serde_json::from_str::<Msg>("{\"cmd\":\"oma_pause\",\"on\":true}").unwrap(), Msg::OmaPause { on: true }));
        assert!(matches!(serde_json::from_str::<Msg>("{\"cmd\":\"voice_state\",\"state\":\"thinking\"}").unwrap(), Msg::VoiceState { ref state, talking: false } if state == "thinking"));
        assert!(matches!(serde_json::from_str::<Msg>("{\"cmd\":\"voice_state\",\"state\":\"idle\",\"talking\":true}").unwrap(), Msg::VoiceState { talking: true, .. }));
        let oma = serde_json::from_str::<Msg>("{\"cmd\":\"oma\",\"action\":{\"do\":\"ask\",\"text\":\"т\",\"key\":\"new\"}}").unwrap();
        assert!(matches!(oma, Msg::Oma { ref action } if action["do"] == "ask" && action["key"] == "new"));
        let emit = serde_json::from_str::<Msg>("{\"cmd\":\"emit\",\"line\":{\"chats\":[]}}").unwrap();
        assert!(matches!(emit, Msg::Emit { ref line } if *line == serde_json::json!({"chats": []})));
        let view = serde_json::from_str::<Msg>("{\"cmd\":\"view\",\"agent\":\"s\"}").unwrap();
        assert!(matches!(view, Msg::View { ref agent } if agent == "s"));
        let presence = serde_json::from_str::<Msg>("{\"cmd\":\"presence\",\"state\":\"busy\"}").unwrap();
        assert!(matches!(presence, Msg::Presence { ref state } if state == "busy"));
        let tell = serde_json::from_str::<Msg>("{\"cmd\":\"tell\",\"session\":\"s\",\"option\":1,\"text\":\"\"}").unwrap();
        assert!(matches!(tell, Msg::Tell { ref session, option: 1, ref expect, .. } if session == "s" && expect.is_empty()), "метка — по желанию");
    }

    #[test]
    fn send_without_daemon_is_false() {
        unsafe { std::env::set_var("XDG_RUNTIME_DIR", "/nonexistent-agent-speak") };
        assert!(!send(&Msg::Stop));
        assert!(request(&Msg::Stop).unwrap_err().contains("не запущена"));
    }
}
