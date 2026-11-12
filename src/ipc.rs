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
    /// Ход мозга Омы начинается (перед каждой репликой мозгу, и в тихом режиме): запрос с ответом — служба спрашивает
    /// окно в фокусе и отвечает строкой агентов `{"agents":[…]}` с этим фокусом; её же получают подписчики.
    Turn,
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
    /// Средний клик по орбу: сводка агентов одной фразой (вслух и пузырём Омы); что-то звучит — стоп, как `read`.
    Summary,
    /// `agent-speak show`: агента (session; пусто — следующий ждущий) — на экран, в терминал ничего не пишется. Запрос с
    /// ответом `{"ok": проект}` или `{"err": …}`; ошибка — ещё и пузырём Омы, кроме quiet (зовёт сама Ома — скажет сама).
    Show { #[serde(default)] agent: String, #[serde(default)] quiet: bool },
    /// Колесо над орбом, пока Ома говорит: темп этого ответа на step шагов RATES (+ быстрее); config.toml не пишется.
    RateStep { step: i64 },
    /// Слух Омы для орба: её вход (узел PipeWire; "" — вход по умолчанию), до когда она ещё слушает (эпоха, с; нет —
    /// дуги нет) и длина этого окна, с; wake — слово-побудка у неё: on | off | battery | muted | busy | no_model | no_mic.
    /// Шлёт при смене, отдельно от voice_state: его повтор держал бы новости и чистил очередь.
    Ear { #[serde(default)] ear_in: String, #[serde(default)] until: Option<f64>, #[serde(default)] secs: f64, #[serde(default)] wake: String },
    /// `agent-speak note`: агент пишет Оме, что ему нужно от пользователя. Сессия — из CLAUDE_CODE_SESSION_ID; нет её
    /// (или не агент из реестра) — агент, чей процесс — предок pid CLI. Запрос с ответом `{"ok": …}` или `{"err": …}`.
    Note { #[serde(default)] session: String, #[serde(default)] pid: u32, text: String },
    /// Хук PermissionRequest (`agent-speak hook permission-request`): соединение открыто, пока запрос жив; служба отвечает
    /// одной строкой `{"decision": "allow" | "deny" | ""}` ("" — без решения). EOF хука (его убил Claude Code: «No» или
    /// Esc в терминале) снимает карточку.
    Permission { payload: serde_json::Value },
    /// Кнопка карточки разрешения в окне или пузыре: решение — хуку запроса id. Клик равен набранному: без подтверждения.
    Permit { id: String, allow: bool },
    /// Отложенное поручение: do = tell (session, text, gist — передать агенту, когда закончит ход) | checks (path, pr —
    /// сказать исход проверок PR) | cancel (id) | list. Запрос с ответом `{"ok": …}` или `{"err": …}` (`agent-speak
    /// later …`; голосом tell ставит только код Омы после «да, передай»); с подписанного соединения (окно) — без ответа.
    Later {
        #[serde(rename = "do")]
        act: String,
        #[serde(default)]
        session: String,
        #[serde(default)]
        text: String,
        #[serde(default)]
        gist: String,
        #[serde(default)]
        path: String,
        #[serde(default)]
        pr: u64,
        #[serde(default)]
        id: String,
    },
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
        assert!(matches!(serde_json::from_str::<Msg>("{\"cmd\":\"turn\"}").unwrap(), Msg::Turn));
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
        assert!(matches!(serde_json::from_str::<Msg>("{\"cmd\":\"summary\"}").unwrap(), Msg::Summary));
        let show = serde_json::from_str::<Msg>("{\"cmd\":\"show\",\"agent\":\"s\"}").unwrap();
        assert!(matches!(show, Msg::Show { ref agent, quiet: false } if agent == "s"));
        assert!(matches!(serde_json::from_str::<Msg>("{\"cmd\":\"show\"}").unwrap(), Msg::Show { ref agent, .. } if agent.is_empty()), "без агента — следующий ждущий");
        assert!(matches!(serde_json::from_str::<Msg>("{\"cmd\":\"rate_step\",\"step\":-1}").unwrap(), Msg::RateStep { step: -1 }));
        let ear = serde_json::from_str::<Msg>("{\"cmd\":\"ear\",\"ear_in\":\"alsa_input.x\",\"until\":1794812345.2,\"secs\":20}").unwrap();
        assert!(matches!(ear, Msg::Ear { ref ear_in, until: Some(u), secs, .. } if ear_in == "alsa_input.x" && u == 1794812345.2 && secs == 20.0));
        assert!(matches!(serde_json::from_str::<Msg>("{\"cmd\":\"ear\"}").unwrap(), Msg::Ear { until: None, ref wake, .. } if wake.is_empty()), "поля — по желанию");
        let ear = serde_json::from_str::<Msg>("{\"cmd\":\"ear\",\"wake\":\"battery\"}").unwrap();
        assert!(matches!(ear, Msg::Ear { ref wake, .. } if wake == "battery"));
        let note = serde_json::from_str::<Msg>("{\"cmd\":\"note\",\"session\":\"s\",\"pid\":42,\"text\":\"нужен токен\"}").unwrap();
        assert!(matches!(note, Msg::Note { ref session, pid: 42, ref text } if session == "s" && text == "нужен токен"));
        assert!(matches!(serde_json::from_str::<Msg>("{\"cmd\":\"note\",\"text\":\"т\"}").unwrap(), Msg::Note { pid: 0, .. }), "без сессии — по pid");
        let perm = serde_json::from_str::<Msg>("{\"cmd\":\"permission\",\"payload\":{\"tool_name\":\"Bash\"}}").unwrap();
        assert!(matches!(perm, Msg::Permission { ref payload } if payload["tool_name"] == "Bash"));
        let permit = serde_json::from_str::<Msg>("{\"cmd\":\"permit\",\"id\":\"perm-7\",\"allow\":true}").unwrap();
        assert!(matches!(permit, Msg::Permit { ref id, allow: true } if id == "perm-7"));
        let later = serde_json::from_str::<Msg>("{\"cmd\":\"later\",\"do\":\"checks\",\"path\":\"/p\",\"pr\":12}").unwrap();
        assert!(matches!(later, Msg::Later { ref act, ref path, pr: 12, ref id, .. } if act == "checks" && path == "/p" && id.is_empty()));
        let s = serde_json::to_string(&Msg::Later { act: "cancel".into(), session: String::new(), text: String::new(), gist: String::new(), path: String::new(), pr: 0, id: "later-3".into() }).unwrap();
        assert!(s.contains("\"do\":\"cancel\"") && s.contains("\"id\":\"later-3\""), "{s}");
    }

    #[test]
    fn send_without_daemon_is_false() {
        unsafe { std::env::set_var("XDG_RUNTIME_DIR", "/nonexistent-agent-speak") };
        assert!(!send(&Msg::Stop));
        assert!(request(&Msg::Stop).unwrap_err().contains("не запущена"));
    }
}
