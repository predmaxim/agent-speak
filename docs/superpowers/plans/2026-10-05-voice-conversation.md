# Голосовой разговор — план реализации

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** живой голосовой разговор с Claude или Codex: включил хоткеем, выбрал агента, говоришь, перебиваешь голосом.

**Architecture:** демон agent-speak (Rust) получает команды `voice_*`, запускает дочерний Python-процесс `agent_voice` и проигрывает его фразы вне логики активных окон; терминальные агенты на это время глушатся. `agent_voice`: микрофон (`pw-record`) → Silero VAD → сегментатор → faster-whisper → адаптер агента (Claude Agent SDK / `codex app-server`) → нарезка ответа на предложения → `voice` в демон; речь во время ответа → `stop` + `interrupt`.

**Tech Stack:** Rust (существующий демон), Python 3.12 (отдельный venv через `uv`), faster-whisper + nvidia-cublas/cudnn из pip, silero-vad + torch CPU, claude-agent-sdk, codex app-server (JSON-RPC по stdio), QML-плагин, PipeWire echo-cancel.

**Spec:** `docs/superpowers/specs/2026-10-05-voice-conversation-design.md`

## Global Constraints

- Права агента — только чтение: Claude `permission_mode="plan"`, Codex `sandbox: "read-only"`, `approvalPolicy: "never"`.
- Конец фразы — пауза ≥ 700 мс; начало речи — ≥ 250 мс речи подряд (кадр VAD 32 мс: 8 кадров на старт, 22 на конец).
- Фраза короче 2 символов (после strip) агенту не отправляется.
- Предложение для озвучки — не короче 20 символов, иначе клеится к следующему (кроме финального хвоста).
- Голос ответа — голос по умолчанию (`cfg.speaker`), как у образца.
- Распознавание: `large-v3-turbo`, `language="ru"`, CUDA float16; при ошибке — `small` на CPU int8.
- Добавка к промпту (дословно): «Это голосовой разговор. Отвечай коротко, разговорным языком, без markdown, списков и кода вслух; код и пути упоминай словами.»
- Дочерние процессы `agent_voice` получают `AGENT_SPEAK=1` — хуки самих голосовых сессий не озвучиваются (`main.rs` уже выходит при этой переменной).
- Комментарии и сообщения в коде — по-русски, как в остальном проекте; плагин — английские ключи + `I18n.js`.
- Не пушить ветку `impl`; коммиты — в текущую ветку `main` локально.
- Деплой (`install.sh`) перезапускает демон и обрывает озвучку — перед запуском спросить пользователя.

## Review Focus

1. Перебивание во время «думает» (ответа ещё нет) — `interrupt` уходит агенту, звук не трогается зря; старые дельты прерванного хода не озвучиваются (Task 3, Task 4).
2. Демон перезапущен или упал — `agent_voice` завершается сам по EOF stdin, не остаётся сиротой с открытым микрофоном (Task 2, Task 5).
3. Codex присылает дельты/`turn/completed` прерванного хода после нового `turn/start` — игнорируются по `turnId` (Task 4).
4. Нет модуля echo-cancel — `pw-record` берёт микрофон по умолчанию, `pw-cat` играет по умолчанию, режим работает с предупреждением (Task 5, Task 7).
5. Хоткей «включить», когда окно настроек уже открыто — окно не закрывается, курсор встаёт на строку разговора (Task 6).

---

## Карта файлов

| Файл | Что |
|---|---|
| `src/queue.rs` | `Kind::Voice`: играет при любой активной сессии |
| `src/ipc.rs` | `Msg::Voice`, `VoiceStart`, `VoiceStop`, `VoiceToggle`, `VoiceState` |
| `src/config.rs` | `voice_last_agent` |
| `src/audio.rs`, `src/speaker.rs` | цель воспроизведения `pw-cat --target` из `Shared.sink` |
| `src/daemon.rs` | голосовой режим: глушение, дочерний процесс, состояние, cwd |
| `src/main.rs` | `agent-speak voice` |
| `python/agent_voice/segment.py` | сегментатор по вероятностям VAD (чистая логика) |
| `python/agent_voice/chunker.py` | нарезка дельт на предложения |
| `python/agent_voice/conversation.py` | машина состояний разговора → действия |
| `python/agent_voice/agents.py` | `ClaudeAgent`, `CodexAgent`, `CodexProtocol` |
| `python/agent_voice/link.py` | сокет демона: команды и подписка на `speaking` |
| `python/agent_voice/ear.py` | `pw-record` + VAD + whisper |
| `python/agent_voice/__main__.py` | сборка всего вместе |
| `python/test_agent_voice.py` | тесты чистой логики |
| `install.sh` | voice-venv, ссылка на пакет |
| `plugin/Model.js`, `Panel.qml`, `I18n.js`, `Indicator.qml`, `test.js` | строка «Разговор» в окне, фазы в значке |
| `~/omarchy-dotfiles/home/.config/pipewire/pipewire.conf.d/echo-cancel.conf` | модуль эхоподавления |
| `~/omarchy-dotfiles/home/.config/hypr/hyprland.lua` | хоткей `SUPER + ALT + S` |

---

### Task 1: Демон — голосовой режим без процесса (Kind::Voice, глушение, состояние)

**Files:**
- Modify: `src/queue.rs` (enum `Kind`, `pop`, `ready`)
- Modify: `src/ipc.rs` (enum `Msg`, тест `msg_roundtrip`)
- Modify: `src/config.rs` (поле `voice_last_agent`)
- Modify: `src/daemon.rs` (`Status`, `State`, `status_line`, `enqueue_msg`, `on_msg`, `on_hook`, `test_state`, тест `status_line_shape`)

**Interfaces:**
- Produces: `Kind::Voice`; `Msg::Voice { text: String }`, `Msg::VoiceStart { agent: String }`, `Msg::VoiceStop`, `Msg::VoiceToggle`, `Msg::VoiceState { state: String }`; `State.voice: Option<VoiceSession>` где `struct VoiceSession { agent: String, state: String, child: Option<std::process::Child> }`; `State.last_cwd: String`; поля строки состояния `voice` (`"off"|"starting"|"listening"|"thinking"|"speaking"`), `voice_agent`, `voice_last_agent`; `Config.voice_last_agent: String` (по умолчанию `"claude"`).

- [ ] **Step 1: Тесты (падают)**

В `src/ipc.rs`, в конец `msg_roundtrip`:

```rust
        assert!(matches!(serde_json::from_str::<Msg>("{\"cmd\":\"voice\",\"text\":\"т\"}").unwrap(), Msg::Voice { .. }));
        assert!(matches!(serde_json::from_str::<Msg>("{\"cmd\":\"voice_start\",\"agent\":\"codex\"}").unwrap(), Msg::VoiceStart { ref agent } if agent == "codex"));
        assert!(matches!(serde_json::from_str::<Msg>("{\"cmd\":\"voice_stop\"}").unwrap(), Msg::VoiceStop));
        assert!(matches!(serde_json::from_str::<Msg>("{\"cmd\":\"voice_toggle\"}").unwrap(), Msg::VoiceToggle));
        assert!(matches!(serde_json::from_str::<Msg>("{\"cmd\":\"voice_state\",\"state\":\"thinking\"}").unwrap(), Msg::VoiceState { .. }));
```

В `src/queue.rs` в `mod tests` (помощник создания `Item` там уже есть — если нет, создать `Item` литералом как в `src/speaker.rs:246`):

```rust
    #[test]
    fn voice_plays_whatever_session_is_active() {
        let mut q = Queue::new(Duration::from_secs(30));
        q.set_active("a");
        q.push(Item { session: "voice".into(), text: "ответ".into(), kind: Kind::Voice, born: Instant::now(), msg: "m".into(), speaker: String::new() });
        assert!(q.ready());
        assert_eq!(q.pop(Instant::now()).unwrap().kind, Kind::Voice);
    }
```

В `src/daemon.rs` в `mod tests`:

```rust
    fn voice_on(st: &mut State) {
        st.voice = Some(VoiceSession { agent: "claude".into(), state: "listening".into(), child: None });
    }

    #[test]
    fn voice_text_queued_with_default_voice() {
        let mut st = test_state();
        voice_on(&mut st);
        st.on_msg(Msg::Voice { text: "Привет, слушаю.".into() });
        let it = st.shared.queue.lock().unwrap().pop(Instant::now()).unwrap();
        assert_eq!(it.kind, Kind::Voice);
        assert_eq!(it.speaker, "");
    }

    #[test]
    fn voice_text_ignored_when_mode_off() {
        let mut st = test_state();
        st.on_msg(Msg::Voice { text: "Опоздавшая фраза.".into() });
        assert!(st.shared.queue.lock().unwrap().is_empty());
    }

    #[test]
    fn terminal_agents_muted_in_voice_mode() {
        let mut st = test_state();
        voice_on(&mut st);
        st.on_hook("notification", &serde_json::json!({"session_id": "b", "cwd": "/p/x", "message": "Нужно разрешение"}));
        st.enqueue(Agent::Claude, "a", "Финал.", Kind::Manual);
        assert!(st.shared.queue.lock().unwrap().is_empty());
    }

    #[test]
    fn voice_state_broadcast_and_hook_cwd_remembered() {
        let mut st = test_state();
        let (ours, theirs) = UnixStream::pair().unwrap();
        let mut r = reader(ours);
        st.subscribe(theirs);
        assert_eq!(next(&mut r)["voice"], "off");
        voice_on(&mut st);
        st.on_msg(Msg::VoiceState { state: "thinking".into() });
        let s = next(&mut r);
        assert_eq!(s["voice"], "thinking");
        assert_eq!(s["voice_agent"], "claude");
        st.on_hook("user-prompt-submit", &serde_json::json!({"session_id": "s", "cwd": "/home/u/p"}));
        assert_eq!(st.last_cwd, "/home/u/p");
    }
```

Заменить ожидание в `status_line_shape`:

```rust
            "{\"running\":true,\"speaking\":false,\"paused\":false,\"mode\":\"manual\",\"read_intermediate\":true,\"speaker\":\"xenia\",\"rate\":\"medium\",\"project\":\"\",\"voice\":\"off\",\"voice_agent\":\"\",\"voice_last_agent\":\"claude\"}\n"
```

- [ ] **Step 2: Запустить — падают**

Run: `cd ~/Projects/agent-speak && cargo test 2>&1 | tail -20`
Expected: ошибки компиляции (`Kind::Voice`, `Msg::Voice`, `VoiceSession`, `last_cwd` не определены).

- [ ] **Step 3: Реализация**

`src/queue.rs` — в enum `Kind` после `Preview`:

```rust
    Voice,   // голосовой разговор: играет при любой активной сессии
```

В `pop` и `ready` расширить условие:

```rust
matches!(i.kind, Kind::Urgent | Kind::Preview | Kind::Read | Kind::Voice)
```

`src/ipc.rs` — в enum `Msg`:

```rust
    /// Фраза голосового разговора (от agent_voice).
    Voice { text: String },
    /// Включить голосовой разговор с агентом (claude | codex).
    VoiceStart { agent: String },
    VoiceStop,
    /// Хоткей: выключен — открыть окно выбора агента, включён — выключить.
    VoiceToggle,
    /// Фаза разговора от agent_voice: listening | thinking | speaking.
    VoiceState { state: String },
```

`src/config.rs` — поле в `Config` (после `read_intermediate`) и значение по умолчанию:

```rust
    /// агент последнего голосового разговора: первым в окне выбора
    pub voice_last_agent: String,
```

```rust
        Config { mode: "manual".into(), speaker: "xenia".into(), rate: "medium".into(), max_age_secs: 30, read_intermediate: true, voice_last_agent: "claude".into() }
```

`src/daemon.rs`:

В `struct Status` добавить поля после `project`:

```rust
    voice: &'a str,
    voice_agent: &'a str,
    voice_last_agent: &'a str,
```

Рядом со `struct State`:

```rust
/// Идущий голосовой разговор; child — процесс agent_voice (в тестах нет).
struct VoiceSession {
    agent: String,
    state: String,
    child: Option<std::process::Child>,
}
```

В `struct State` (и в `run()`, и в `test_state()` — `voice: None, last_cwd: String::new()`):

```rust
    voice: Option<VoiceSession>,
    last_cwd: String, // cwd последнего хука — рабочая папка голосового разговора
```

В `status_line` в литерал `Status`:

```rust
            voice: self.voice.as_ref().map_or("off", |v| v.state.as_str()),
            voice_agent: self.voice.as_ref().map_or("", |v| v.agent.as_str()),
            voice_last_agent: &self.cfg.voice_last_agent,
```

В `enqueue_msg` первой строкой (до `prepare`):

```rust
        // голосовой разговор: терминальные агенты молчат; Voice без разговора — опоздавшая фраза
        if self.voice.is_some() != (kind == Kind::Voice) && kind != Kind::Preview {
            eprintln!("agent-speak: пропущено (голосовой режим: {}): {}", self.voice.is_some(), raw.chars().take(40).collect::<String>());
            return 0;
        }
```

Там же голос: заменить строку `let speaker = if kind == Kind::Preview ...` на:

```rust
        let speaker = if matches!(kind, Kind::Preview | Kind::Voice) { String::new() } else { self.voice_of(session) };
```

В `on_msg` — ветки (`changes` дополнить `Msg::VoiceState { .. }`):

```rust
            Msg::Voice { text } => {
                self.enqueue(Agent::Claude, "voice", &text, Kind::Voice);
            }
            Msg::VoiceState { state } => {
                if let Some(v) = self.voice.as_mut() {
                    v.state = state;
                }
            }
            Msg::VoiceStart { .. } | Msg::VoiceStop | Msg::VoiceToggle => {} // Task 2
```

```rust
        let changes = matches!(m, Msg::Stop | Msg::Pause | Msg::Mode | Msg::VoiceState { .. });
```

В `on_hook` после блока с `projects.insert`:

```rust
        if let Some(cwd) = p["cwd"].as_str().filter(|c| !c.is_empty()) {
            self.last_cwd = cwd.to_string();
        }
```

- [ ] **Step 4: Тесты проходят**

Run: `cargo test 2>&1 | tail -5`
Expected: `test result: ok`, все тесты.

- [ ] **Step 5: Commit**

```bash
git add src/ && git commit -m "daemon: voice mode state, Kind::Voice, terminal agents muted while talking"
```

---

### Task 2: Демон — процесс agent_voice, вывод в эхоподавление, CLI

**Files:**
- Modify: `src/speaker.rs` (`Shared.sink`, `Shared::new`, `run`)
- Modify: `src/audio.rs` (`Player::new(sink)`, `--target`)
- Modify: `src/daemon.rs` (`State.voice_cmd`, `voice_start`, `voice_stop`, `tick`, `on_msg`, `Event::Tick`)
- Modify: `src/main.rs` (`voice`)

**Interfaces:**
- Consumes: Task 1 (`VoiceSession`, `Msg::Voice*`, `last_cwd`, `cfg.voice_last_agent`).
- Produces: команда `agent-speak voice` → `Msg::VoiceToggle`; дочерний процесс запускается как `<voice_cmd...> --agent <claude|codex> --cwd <dir>` с `AGENT_SPEAK=1`, stdin — pipe (EOF = демон умер); `Shared.sink: Arc<Mutex<String>>` — имя узла PipeWire для `pw-cat --target`, пусто — по умолчанию.

- [ ] **Step 1: Тесты (падают)**

В `src/daemon.rs` `mod tests`; в `test_state()` добавить поле `voice_cmd: vec!["sh".into(), "-c".into(), "sleep 30".into(), "x".into()]` (лишние аргументы уйдут в `$1…`):

```rust
    #[test]
    fn voice_start_spawns_child_remembers_agent_and_routes_sink() {
        let home = std::env::temp_dir().join(format!("agent-speak-voice-{}", std::process::id()));
        unsafe { std::env::set_var("HOME", &home) };
        let mut st = test_state();
        st.on_msg(Msg::VoiceStart { agent: "codex".into() });
        let v = st.voice.as_ref().unwrap();
        assert_eq!((v.agent.as_str(), v.state.as_str()), ("codex", "starting"));
        assert!(v.child.is_some());
        assert_eq!(st.cfg.voice_last_agent, "codex");
        assert_eq!(*st.shared.sink.lock().unwrap(), "echo-cancel-sink");
        st.on_msg(Msg::VoiceStart { agent: "claude".into() }); // уже идёт — не второй процесс
        assert_eq!(st.voice.as_ref().unwrap().agent, "codex");
        st.on_msg(Msg::VoiceStop);
        assert!(st.voice.is_none());
        assert_eq!(*st.shared.sink.lock().unwrap(), "");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn voice_start_rejects_unknown_agent() {
        let mut st = test_state();
        st.on_msg(Msg::VoiceStart { agent: "rm -rf".into() });
        assert!(st.voice.is_none());
    }

    #[test]
    fn dead_child_turns_voice_off() {
        let mut st = test_state();
        st.voice_cmd = vec!["true".into()];
        st.on_msg(Msg::VoiceStart { agent: "claude".into() });
        std::thread::sleep(Duration::from_millis(200));
        st.tick();
        assert!(st.voice.is_none());
    }
```

Тест `set_applies_saves_broadcasts_and_rejects_bad` единственный трогает HOME — новый тест тоже трогает (сохраняет config). Чтобы тесты не гонялись, оба держать под общим `static HOME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());` — `let _g = HOME_LOCK.lock().unwrap();` первой строкой каждого из двух.

- [ ] **Step 2: Падают**

Run: `cargo test 2>&1 | tail -20`
Expected: ошибки компиляции (`voice_cmd`, `sink`, `tick`).

- [ ] **Step 3: Реализация**

`src/speaker.rs` — в `Shared`:

```rust
    pub sink: Arc<Mutex<String>>, // узел PipeWire для pw-cat --target; пусто — вывод по умолчанию
```

в `Shared::new` — `sink: Arc::new(Mutex::new(String::new())),`; в `run`: `run_with(shared.clone(), tts, Player::new(shared.sink.clone()))` (подогнать под фактическую сигнатуру `run`: `shared` до вызова клонировать).

`src/audio.rs`:

```rust
use std::sync::{Arc, Mutex};

pub struct Player {
    child: Option<Child>,
    sink: Arc<Mutex<String>>,
}

impl Player {
    pub fn new(sink: Arc<Mutex<String>>) -> Player {
        Player { child: None, sink }
    }
```

в `write` при запуске `pw-cat`:

```rust
        if self.child.is_none() {
            let mut cmd = Command::new("pw-cat");
            cmd.args(["--playback", "--raw", "--format", "s16", "--rate", "48000", "--channels", "1"]);
            let sink = self.sink.lock().unwrap().clone();
            if !sink.is_empty() {
                cmd.args(["--target", &sink]); // нет такого узла — PipeWire подключит к выходу по умолчанию
            }
            self.child = cmd.arg("-").stdin(Stdio::piped()).spawn().ok();
        }
```

`src/daemon.rs`:

В `State` поле `voice_cmd: Vec<String>, // запуск agent_voice; в тестах — заглушка`. В `run()`:

```rust
        voice_cmd: vec![
            data.join("voice-venv/bin/python").to_string_lossy().into(),
            "-m".into(),
            "agent_voice".into(),
        ],
```

Цикл: `Event::Tick => st.tick(),`.

Методы `State`:

```rust
    /// Голосовой разговор: agent_voice — дочерний процесс; вывод — в узел эхоподавления.
    fn voice_start(&mut self, agent: String) {
        if self.voice.is_some() || !["claude", "codex"].contains(&agent.as_str()) {
            eprintln!("agent-speak: voice_start пропущен: {agent}");
            return;
        }
        let cwd = if self.last_cwd.is_empty() { home().to_string_lossy().into_owned() } else { self.last_cwd.clone() };
        let child = std::process::Command::new(&self.voice_cmd[0])
            .args(&self.voice_cmd[1..])
            .args(["--agent", &agent, "--cwd", &cwd])
            .current_dir(home().join(".local/share/agent-speak"))
            .env("AGENT_SPEAK", "1")
            .stdin(std::process::Stdio::piped()) // EOF — демон умер, agent_voice выходит
            .spawn();
        let child = match child {
            Ok(c) => c,
            Err(e) => {
                eprintln!("agent-speak: agent_voice не запустился: {e}");
                notice("Голос: не запустился", &e.to_string());
                return;
            }
        };
        self.cfg.voice_last_agent = agent.clone();
        self.cfg.save();
        *self.shared.sink.lock().unwrap() = "echo-cancel-sink".into();
        self.stop(); // терминальное чтение обрывается, pw-cat перезапустится с новым выводом
        self.voice = Some(VoiceSession { agent, state: "starting".into(), child: Some(child) });
    }

    fn voice_stop(&mut self) {
        if let Some(mut v) = self.voice.take() {
            if let Some(c) = v.child.as_mut() {
                let _ = c.kill();
                let _ = c.wait();
            }
        }
        *self.shared.sink.lock().unwrap() = String::new();
        self.stop();
    }

    /// Раз в секунду: agent_voice завершился сам (ошибка — он уже уведомил) — режим выключен.
    fn tick(&mut self) {
        let dead = self.voice.as_mut().and_then(|v| v.child.as_mut()).is_some_and(|c| !matches!(c.try_wait(), Ok(None)));
        if dead {
            eprintln!("agent-speak: agent_voice завершился");
            self.voice_stop();
            self.broadcast();
        }
    }
```

В `on_msg` заменить заглушку Task 1:

```rust
            Msg::VoiceStart { agent } => self.voice_start(agent),
            Msg::VoiceStop => self.voice_stop(),
            Msg::VoiceToggle => {
                if self.voice.is_some() {
                    self.voice_stop();
                } else {
                    // окно плагина: первая строка — выбор агента
                    let _ = std::process::Command::new("omarchy-shell").args(["predmaxim.agent-speak", "voice"]).spawn();
                }
            }
```

и `changes` дополнить: `| Msg::VoiceStart { .. } | Msg::VoiceStop | Msg::VoiceToggle`.

Зомби от `spawn` `omarchy-shell`: обернуть как в `notice.rs` — `std::thread::spawn(move || { let _ = cmd.status(); })`.

`src/main.rs`: `Some("voice") => Msg::VoiceToggle,` и в строку использования добавить `| voice`.

- [ ] **Step 4: Тесты проходят**

Run: `cargo test 2>&1 | tail -5`
Expected: `test result: ok`.

- [ ] **Step 5: Commit**

```bash
git add src/ && git commit -m "daemon: spawn agent_voice, route playback to echo-cancel sink, agent-speak voice"
```

---

### Task 3: agent_voice — чистая логика (сегментатор, нарезка, разговор)

**Files:**
- Create: `python/agent_voice/__init__.py` (пустой)
- Create: `python/agent_voice/segment.py`
- Create: `python/agent_voice/chunker.py`
- Create: `python/agent_voice/conversation.py`
- Test: `python/test_agent_voice.py`

**Interfaces:**
- Produces:
  - `Segmenter(start_frames=8, end_frames=22, preroll=8)`, `.feed(prob: float, frame: bytes) -> list[tuple]` — события `("start",)` и `("phrase", bytes)`.
  - `Chunker(min_len=20)`, `.feed(delta: str) -> list[str]`, `.flush() -> list[str]`, `.reset()`.
  - `Conversation()`, методы `speech_start()`, `phrase(text)`, `delta(text)`, `done()`, `audio(busy: bool)` → `list[tuple]` действий: `("stop",)`, `("interrupt",)`, `("send", str)`, `("say", str)`, `("state", "listening"|"thinking"|"speaking")`.

- [ ] **Step 1: Тесты (падают)**

`python/test_agent_voice.py`:

```python
# python -m pytest python/test_agent_voice.py  (из voice-venv или любого python3 с pytest)
import sys, os
sys.path.insert(0, os.path.dirname(__file__))
from agent_voice.segment import Segmenter
from agent_voice.chunker import Chunker
from agent_voice.conversation import Conversation

F = b"\x00\x00" * 512


def feed(seg, probs):
    out = []
    for i, p in enumerate(probs):
        out += seg.feed(p, bytes([i % 256]) * 1024)
    return out


def test_short_blip_is_not_speech():
    assert feed(Segmenter(), [0.9] * 7 + [0.1] * 40) == []


def test_phrase_after_pause_with_preroll():
    seg = Segmenter()
    ev = feed(seg, [0.1] * 10 + [0.9] * 20 + [0.1] * 22)
    assert ev[0] == ("start",)
    assert ev[1][0] == "phrase"
    # предзахват: 8 кадров до старта + 20 речи + 22 тишины
    assert len(ev[1][1]) == (8 + 20 + 22) * 1024
    assert len(ev) == 2


def test_short_pause_keeps_one_phrase():
    ev = feed(Segmenter(), [0.9] * 10 + [0.1] * 10 + [0.9] * 10 + [0.1] * 22)
    assert [e[0] for e in ev] == ["start", "phrase"]


def test_chunker_sentences_and_min_len():
    c = Chunker()
    assert c.feed("Да. Сейчас посмотрю файл конфигурации") == []  # «Да.» короче 20 — ждёт
    assert c.feed(" сервиса. Ещё") == ["Да. Сейчас посмотрю файл конфигурации сервиса."]
    assert c.flush() == ["Ещё"]
    assert c.flush() == []


def test_chunker_newline_ends_sentence():
    c = Chunker()
    assert c.feed("Первая строка достаточно длинная\nвторая") == ["Первая строка достаточно длинная"]


def test_conversation_turn():
    cv = Conversation()
    assert cv.phrase(" а ") == []  # короче 2 символов после strip — ничего
    assert cv.phrase("Что в этом проекте?") == [("send", "Что в этом проекте?"), ("state", "thinking")]
    assert cv.delta("Это демон озвучки агентов, написан на Rust. И") == [
        ("say", "Это демон озвучки агентов, написан на Rust."), ("state", "speaking")]
    cv.audio(True)
    assert cv.done() == [("say", "И")]
    assert cv.audio(False) == [("state", "listening")]


def test_barge_in_while_speaking_stops_and_interrupts():
    cv = Conversation()
    cv.phrase("Расскажи подробно")
    cv.delta("Длинный ответ про устройство демона. Дальше")
    cv.audio(True)
    assert cv.speech_start() == [("stop",), ("interrupt",), ("state", "listening")]
    assert cv.delta(" хвост старого хода.") == []  # прерванный ход не озвучивается
    assert cv.done() == []


def test_barge_in_while_thinking_only_interrupts():
    cv = Conversation()
    cv.phrase("Подумай")
    assert cv.speech_start() == [("interrupt",), ("state", "listening")]


def test_speech_while_listening_does_nothing():
    assert Conversation().speech_start() == []


def test_barge_in_after_turn_done_only_stops_audio():
    cv = Conversation()
    cv.phrase("Скажи")
    cv.delta("Готово, всё проверил полностью.")
    cv.audio(True)  # ответ ещё звучит, когда ход закончился
    assert cv.done() == []
    assert cv.speech_start() == [("stop",), ("state", "listening")]
```

- [ ] **Step 2: Падают**

Run: `cd ~/Projects/agent-speak && python3 -m pytest python/test_agent_voice.py -q 2>&1 | tail -5`
Expected: `ModuleNotFoundError: No module named 'agent_voice'` (если нет pytest: `uv run --with pytest python -m pytest …`).

- [ ] **Step 3: Реализация**

`python/agent_voice/segment.py`:

```python
"""Фразы из потока вероятностей Silero VAD: старт после start_frames речи подряд,
конец после end_frames тишины. Кадр — 32 мс."""
from collections import deque


class Segmenter:
    def __init__(self, start_frames=8, end_frames=22, preroll=8, threshold=0.5):
        self.start_frames, self.end_frames, self.threshold = start_frames, end_frames, threshold
        self.pre = deque(maxlen=preroll)   # тишина до речи: начало слова не обрезается
        self.run = []                      # кадры кандидата в речь
        self.buf = None                    # идущая фраза
        self.silence = 0

    def feed(self, prob, frame):
        speech = prob >= self.threshold
        if self.buf is None:
            if not speech:
                self.pre.extend(self.run + [frame]) if self.run else self.pre.append(frame)
                self.run = []
                return []
            self.run.append(frame)
            if len(self.run) < self.start_frames:
                return []
            self.buf = list(self.pre) + self.run
            self.pre.clear()
            self.run, self.silence = [], 0
            return [("start",)]
        self.buf.append(frame)
        self.silence = 0 if speech else self.silence + 1
        if self.silence < self.end_frames:
            return []
        audio, self.buf = b"".join(self.buf), None
        return [("phrase", audio)]
```

Проверить ожидание `test_phrase_after_pause_with_preroll`: `pre` держит 8 последних кадров тишины, `run` — 8 кадров, после старта ещё 12 речи + 22 тишины → 8 + 20 + 22. Если считается иначе — править код, не тест.

`python/agent_voice/chunker.py`:

```python
"""Поток дельт ответа → предложения для озвучки."""
import re

END = re.compile(r"(?<=[.!?…])\s+|\n+")


class Chunker:
    def __init__(self, min_len=20):
        self.min_len, self.buf = min_len, ""

    def feed(self, delta):
        self.buf += delta
        parts = END.split(self.buf)
        self.buf = parts.pop()  # недописанное — ждёт
        out, acc = [], ""
        for p in parts:
            acc = f"{acc} {p.strip()}".strip()
            if len(acc) >= self.min_len:
                out.append(acc)
                acc = ""
        if acc:
            self.buf = f"{acc} {self.buf}" if self.buf else acc
        return out

    def flush(self):
        rest, self.buf = self.buf.strip(), ""
        return [rest] if rest else []

    def reset(self):
        self.buf = ""
```

Проверить `test_chunker_newline_ends_sentence`: строка «Первая строка достаточно длинная» (32 символа) уходит, «вторая» ждёт.

`python/agent_voice/conversation.py`:

```python
"""Машина состояний разговора: события слуха, агента и звука → действия."""
from .chunker import Chunker


class Conversation:
    def __init__(self):
        self.state = "listening"
        self.turn = False      # агент отвечает на нашу реплику
        self.audio_busy = False
        self.chunker = Chunker()

    def _to(self, state):
        if state == self.state:
            return []
        self.state = state
        return [("state", state)]

    def speech_start(self):
        acts = []
        if self.audio_busy or self.state == "speaking":
            acts.append(("stop",))
        if self.turn:
            acts.append(("interrupt",))
        if not acts:
            return []
        self.turn = False
        self.chunker.reset()
        return acts + self._to("listening")

    def phrase(self, text):
        t = text.strip()
        if len(t) < 2:
            return []
        self.turn = True
        self.chunker.reset()
        return [("send", t)] + self._to("thinking")

    def delta(self, text):
        if not self.turn:
            return []
        said = [("say", s) for s in self.chunker.feed(text)]
        return said + (self._to("speaking") if said else [])

    def done(self):
        if not self.turn:
            return []
        self.turn = False
        said = [("say", s) for s in self.chunker.flush()]
        if said:
            return said + self._to("speaking")
        return [] if self.audio_busy else self._to("listening")

    def audio(self, busy):
        self.audio_busy = busy
        if not busy and not self.turn:
            return self._to("listening")
        return []
```

- [ ] **Step 4: Тесты проходят**

Run: `python3 -m pytest python/test_agent_voice.py -q 2>&1 | tail -3`
Expected: `10 passed`.

- [ ] **Step 5: Commit**

```bash
git add python/agent_voice python/test_agent_voice.py && git commit -m "agent_voice: VAD segmenter, sentence chunker, conversation state machine"
```

---

### Task 4: agent_voice — адаптеры Claude и Codex, связь с демоном

**Files:**
- Create: `python/agent_voice/agents.py`
- Create: `python/agent_voice/link.py`
- Test: `python/test_agent_voice.py` (дописать)

**Interfaces:**
- Consumes: ничего из Task 3 (адаптеры независимы).
- Produces:
  - `VOICE_PROMPT: str` (дословно из Global Constraints).
  - Адаптер: `await a.start(cwd: str)`, `await a.send(text: str)`, `await a.interrupt()`, `await a.close()`, `a.events: asyncio.Queue` с `("delta", str)`, `("done", None)`, `("error", str)`.
  - `ClaudeAgent(events)`, `CodexAgent(events)`, `make_agent(name: str, events) -> ClaudeAgent | CodexAgent`.
  - `CodexProtocol` (чистый): `.request(method, params) -> (id, line)`, `.handle(line: str) -> list[tuple]` — события адаптера плюс `("response", id, result)`; `.turn_id: str | None`, `.thread_id: str | None`.
  - `claude_events(msg, st: dict) -> list[tuple]` (чистая; `st["skip"]` — глотать до конца прерванного хода).
  - `link.send(cmd: dict) -> bool`, `async link.watch_speaking(cb)` — вызывает `cb(bool)` при смене `speaking` в строке состояния.

- [ ] **Step 1: Тесты (падают)**

Дописать в `python/test_agent_voice.py`:

```python
import json
from types import SimpleNamespace
from agent_voice.agents import CodexProtocol, claude_events


def test_codex_requests_and_turn_filter():
    p = CodexProtocol()
    rid, line = p.request("turn/start", {"threadId": "t1", "input": [{"type": "text", "text": "привет"}]})
    assert json.loads(line) == {"id": rid, "method": "turn/start", "params": {"threadId": "t1", "input": [{"type": "text", "text": "привет"}]}}
    assert line.endswith("\n")
    assert p.handle(json.dumps({"id": rid, "result": {"turn": {"id": "u1", "items": [], "status": "inProgress"}}})) == [("response", rid, {"turn": {"id": "u1", "items": [], "status": "inProgress"}})]
    p.turn_id = "u1"
    d = lambda turn, text: json.dumps({"method": "item/agentMessage/delta", "params": {"delta": text, "itemId": "i", "threadId": "t1", "turnId": turn}})
    assert p.handle(d("u1", "Привет")) == [("delta", "Привет")]
    assert p.handle(d("u0", "старое")) == []  # дельта прерванного хода
    done = lambda turn: json.dumps({"method": "turn/completed", "params": {"threadId": "t1", "turn": {"id": turn, "items": [], "status": "completed"}}})
    assert p.handle(done("u0")) == []
    assert p.handle(done("u1")) == [("done", None)]
    assert p.turn_id is None
    assert p.handle("не json") == []


def test_codex_error_response():
    p = CodexProtocol()
    rid, _ = p.request("thread/start", {})
    assert p.handle(json.dumps({"id": rid, "error": {"code": -1, "message": "not logged in"}})) == [("error", "not logged in")]


def test_claude_events_text_and_skip():
    st = {"skip": False}
    delta = lambda t: SimpleNamespace(event={"type": "content_block_delta", "delta": {"type": "text_delta", "text": t}})
    ResultMessage = type("ResultMessage", (), {})
    assert claude_events(delta("Да"), st) == [("delta", "Да")]
    assert claude_events(SimpleNamespace(event={"type": "message_start"}), st) == []
    assert claude_events(ResultMessage(), st) == [("done", None)]
    st["skip"] = True  # после interrupt: хвост прерванного хода не нужен
    assert claude_events(delta("хвост"), st) == []
    assert claude_events(ResultMessage(), st) == []
    assert st["skip"] is False
```

- [ ] **Step 2: Падают**

Run: `python3 -m pytest python/test_agent_voice.py -q 2>&1 | tail -3`
Expected: `ModuleNotFoundError: No module named 'agent_voice.agents'`.

- [ ] **Step 3: Реализация**

`python/agent_voice/agents.py`:

```python
"""Агенты голосового разговора: общий вид — start/send/interrupt/close и очередь events
с ("delta", текст), ("done", None), ("error", причина). Права — только чтение."""
import asyncio
import itertools
import json

VOICE_PROMPT = ("Это голосовой разговор. Отвечай коротко, разговорным языком, без markdown, "
                "списков и кода вслух; код и пути упоминай словами.")


def claude_events(msg, st):
    """Сообщение Claude Agent SDK → события. st["skip"]: глотать до конца прерванного хода."""
    if type(msg).__name__ == "ResultMessage":
        if st["skip"]:
            st["skip"] = False
            return []
        return [("done", None)]
    ev = getattr(msg, "event", None)
    if st["skip"] or not isinstance(ev, dict) or ev.get("type") != "content_block_delta":
        return []
    d = ev.get("delta", {})
    return [("delta", d["text"])] if d.get("type") == "text_delta" and d.get("text") else []


class ClaudeAgent:
    def __init__(self, events):
        self.events, self.st, self.client, self.reader = events, {"skip": False}, None, None

    async def start(self, cwd):
        from claude_agent_sdk import ClaudeAgentOptions, ClaudeSDKClient
        opts = ClaudeAgentOptions(
            cwd=cwd, permission_mode="plan", include_partial_messages=True,
            system_prompt={"type": "preset", "preset": "claude_code", "append": VOICE_PROMPT})
        self.client = ClaudeSDKClient(opts)
        await self.client.connect()
        self.reader = asyncio.create_task(self._read())

    async def _read(self):
        try:
            async for m in self.client.receive_messages():
                for e in claude_events(m, self.st):
                    await self.events.put(e)
        except Exception as e:  # SDK упал — разговор кончается
            await self.events.put(("error", f"Claude: {e}"))

    async def send(self, text):
        await self.client.query(text)

    async def interrupt(self):
        # ponytail: interrupt сразу после ResultMessage съест следующий ход; счётчик ходов, если это всплывёт
        self.st["skip"] = True
        await self.client.interrupt()

    async def close(self):
        if self.client:
            await self.client.disconnect()


class CodexProtocol:
    """JSON-RPC codex app-server (строка = сообщение) без ввода-вывода."""
    def __init__(self):
        self.ids = itertools.count(1)
        self.thread_id = None
        self.turn_id = None

    def request(self, method, params):
        rid = next(self.ids)
        return rid, json.dumps({"id": rid, "method": method, "params": params}, ensure_ascii=False) + "\n"

    def handle(self, line):
        try:
            m = json.loads(line)
        except ValueError:
            return []
        if "id" in m and "method" not in m:
            if "error" in m:
                return [("error", m["error"].get("message", str(m["error"])))]
            return [("response", m["id"], m.get("result"))]
        p = m.get("params") or {}
        if m.get("method") == "item/agentMessage/delta" and p.get("turnId") == self.turn_id:
            return [("delta", p.get("delta", ""))]
        if m.get("method") == "turn/completed" and (p.get("turn") or {}).get("id") == self.turn_id:
            self.turn_id = None
            return [("done", None)]
        return []


class CodexAgent:
    def __init__(self, events):
        self.events, self.p, self.proc, self.waiting = events, CodexProtocol(), None, {}

    async def _call(self, method, params):
        rid, line = self.p.request(method, params)
        fut = asyncio.get_running_loop().create_future()
        self.waiting[rid] = fut
        self.proc.stdin.write(line.encode())
        await self.proc.stdin.drain()
        return await fut

    async def start(self, cwd):
        self.proc = await asyncio.create_subprocess_exec(
            "codex", "app-server", stdin=asyncio.subprocess.PIPE, stdout=asyncio.subprocess.PIPE)
        self.reader = asyncio.create_task(self._read())
        await self._call("initialize", {"clientInfo": {"name": "agent-speak", "version": "1"}})
        self.proc.stdin.write(b'{"method":"initialized"}\n')
        r = await self._call("thread/start", {
            "cwd": cwd, "sandbox": "read-only", "approvalPolicy": "never",
            "developerInstructions": VOICE_PROMPT, "ephemeral": True})
        self.p.thread_id = r["thread"]["id"]

    async def _read(self):
        async for raw in self.proc.stdout:
            for e in self.p.handle(raw.decode(errors="replace")):
                if e[0] == "response":
                    fut = self.waiting.pop(e[1], None)
                    if fut and not fut.done():
                        fut.set_result(e[2])
                else:
                    await self.events.put(e)
        await self.events.put(("error", "Codex: app-server завершился"))

    async def send(self, text):
        r = await self._call("turn/start", {"threadId": self.p.thread_id, "input": [{"type": "text", "text": text}]})
        self.p.turn_id = r["turn"]["id"]

    async def interrupt(self):
        turn, self.p.turn_id = self.p.turn_id, None  # дальнейшие дельты этого хода отсекаются
        if turn:
            await self._call("turn/interrupt", {"threadId": self.p.thread_id, "turnId": turn})

    async def close(self):
        if self.proc and self.proc.returncode is None:
            self.proc.kill()
            await self.proc.wait()


def make_agent(name, events):
    return {"claude": ClaudeAgent, "codex": CodexAgent}[name](events)
```

Гонка в `CodexAgent.send`: дельты нового хода могут прийти раньше ответа на `turn/start` (тогда `turn_id` ещё `None` и они отсекутся). Проверить вживую в Task 7; если теряется начало — запоминать дельты при `turn_id is None` и отдавать после ответа.

`python/agent_voice/link.py`:

```python
"""Сокет демона agent-speak: команды — строка JSON; подписка — строки состояния."""
import asyncio
import json
import os
import socket

SOCK = os.path.join(os.environ.get("XDG_RUNTIME_DIR", "/tmp"), "agent-speak.sock")


def send(cmd):
    try:
        with socket.socket(socket.AF_UNIX) as s:
            s.connect(SOCK)
            s.sendall((json.dumps(cmd, ensure_ascii=False) + "\n").encode())
        return True
    except OSError:
        return False


async def watch_speaking(cb):
    """cb(bool) при каждой смене «говорит» у демона."""
    r, w = await asyncio.open_unix_connection(SOCK)
    w.write(b'{"cmd":"subscribe"}\n')
    await w.drain()
    last = None
    async for line in r:
        try:
            speaking = bool(json.loads(line).get("speaking"))
        except ValueError:
            continue
        if speaking != last:
            last = speaking
            cb(speaking)
```

- [ ] **Step 4: Тесты проходят**

Run: `python3 -m pytest python/test_agent_voice.py -q 2>&1 | tail -3`
Expected: `13 passed`.

- [ ] **Step 5: Commit**

```bash
git add python/agent_voice python/test_agent_voice.py && git commit -m "agent_voice: Claude SDK and codex app-server adapters, daemon link"
```

---

### Task 5: agent_voice — слух, главный цикл, установка

**Files:**
- Create: `python/agent_voice/ear.py`
- Create: `python/agent_voice/__main__.py`
- Modify: `install.sh`

**Interfaces:**
- Consumes: `Segmenter`, `Conversation` (Task 3); `make_agent`, `link.send`, `link.watch_speaking` (Task 4); запуск `python -m agent_voice --agent X --cwd D` из демона (Task 2), рабочая папка — `~/.local/share/agent-speak`.
- Produces: процесс, который шлёт демону `voice`, `voice_state`, `stop`; при ошибке — `notify-send "Голос: …"` и выход с кодом 1.

- [ ] **Step 1: Установка окружения**

В `install.sh` после блока Silero (до `systemctl`):

```bash
# Голосовой разговор: отдельный venv на Python 3.12 (колёса faster-whisper/ctranslate2), whisper на GPU
ln -sfn "$PWD/python/agent_voice" "$DATA/agent_voice"
if ! "$DATA/voice-venv/bin/python" -c 'import faster_whisper, silero_vad, claude_agent_sdk' 2>/dev/null; then
  uv venv -q --python 3.12 "$DATA/voice-venv"
  uv pip install -q --python "$DATA/voice-venv/bin/python" torch --index-url https://download.pytorch.org/whl/cpu
  uv pip install -q --python "$DATA/voice-venv/bin/python" faster-whisper silero-vad claude-agent-sdk \
    nvidia-cublas-cu12 'nvidia-cudnn-cu12==9.*' pytest
fi
```

Поставить окружение без перезапуска демона (деплой — с разрешения пользователя):

Run: `cd ~/Projects/agent-speak && DATA=~/.local/share/agent-speak && ln -sfn "$PWD/python/agent_voice" "$DATA/agent_voice" && uv venv -q --python 3.12 "$DATA/voice-venv" && uv pip install -q --python "$DATA/voice-venv/bin/python" torch --index-url https://download.pytorch.org/whl/cpu && uv pip install -q --python "$DATA/voice-venv/bin/python" faster-whisper silero-vad claude-agent-sdk nvidia-cublas-cu12 'nvidia-cudnn-cu12==9.*' pytest`
Expected: без ошибок. Затем тесты в нём: `~/.local/share/agent-speak/voice-venv/bin/python -m pytest python/test_agent_voice.py -q` → `13 passed`.

- [ ] **Step 2: ear.py**

```python
"""Слух: pw-record → Silero VAD (кадры 32 мс) → Segmenter → faster-whisper.
В очередь out: ("start",) — начало речи, ("phrase", текст) — распознанная фраза."""
import asyncio
import subprocess

import numpy as np

from .segment import Segmenter

RATE, FRAME = 16000, 512  # 512 отсчётов = 32 мс


def load_whisper():
    from faster_whisper import WhisperModel
    try:
        return WhisperModel("large-v3-turbo", device="cuda", compute_type="float16")
    except Exception as e:
        print(f"agent_voice: whisper на GPU не загрузился ({e}), CPU small", flush=True)
        return WhisperModel("small", device="cpu", compute_type="int8")


def transcribe(model, pcm):
    audio = np.frombuffer(pcm, np.int16).astype(np.float32) / 32768
    segs, _ = model.transcribe(audio, language="ru", beam_size=1)
    return " ".join(s.text.strip() for s in segs).strip()


def source():
    """Источник эхоподавления, если модуль загружен; иначе — микрофон по умолчанию."""
    names = subprocess.run(["pactl", "list", "short", "sources"], capture_output=True, text=True).stdout
    return "echo-cancel-source" if "echo-cancel-source" in names else None


async def listen(out, model):
    import torch
    from silero_vad import load_silero_vad
    vad = load_silero_vad()
    src = source()
    if not src:
        await out.put(("warn", "нет эхоподавления: без наушников перебивание сработает на свой голос"))
    cmd = ["pw-record", "--rate", str(RATE), "--channels", "1", "--format", "s16"]
    cmd += ["--target", src] if src else []
    proc = await asyncio.create_subprocess_exec(*cmd, "-", stdout=asyncio.subprocess.PIPE)
    phrases = asyncio.Queue()

    async def recognize():  # отдельно: распознавание не задерживает чтение микрофона (перебивание)
        while True:
            pcm = await phrases.get()
            await out.put(("phrase", await asyncio.to_thread(transcribe, model, pcm)))

    task = asyncio.create_task(recognize())
    seg = Segmenter()
    try:
        while True:
            b = await proc.stdout.readexactly(FRAME * 2)
            x = torch.from_numpy(np.frombuffer(b, np.int16).astype(np.float32) / 32768)
            for ev in seg.feed(vad(x, RATE).item(), b):
                if ev[0] == "start":
                    await out.put(ev)
                else:
                    phrases.put_nowait(ev[1])
    finally:
        task.cancel()
        proc.kill()
```

- [ ] **Step 3: __main__.py**

```python
"""agent_voice: голосовой разговор с агентом. Запускает демон agent-speak:
python -m agent_voice --agent claude|codex --cwd DIR"""
import argparse
import asyncio
import glob
import os
import subprocess
import sys


def cuda_env():
    """CUDA-библиотеки из pip (nvidia-*) видны ctranslate2 только через LD_LIBRARY_PATH при старте."""
    libs = glob.glob(os.path.join(sys.prefix, "lib/python3*/site-packages/nvidia/*/lib"))
    have = os.environ.get("LD_LIBRARY_PATH", "")
    if libs and libs[0] not in have:
        os.environ["LD_LIBRARY_PATH"] = ":".join(libs + ([have] if have else []))
        os.execv(sys.executable, [sys.executable, "-m", "agent_voice", *sys.argv[1:]])


def notify(text):
    subprocess.run(["notify-send", "-t", "4000", "-a", "Озвучка", "Голос", text])


async def main(agent_name, cwd):
    from . import link
    from .agents import make_agent
    from .conversation import Conversation
    from .ear import listen, load_whisper

    loop = asyncio.get_running_loop()
    # EOF на stdin — демон умер: выходим, микрофон не остаётся открытым
    loop.add_reader(sys.stdin.fileno(), lambda: sys.stdin.buffer.read1(1) or os._exit(0))

    events = asyncio.Queue()
    agent = make_agent(agent_name, events)
    cv = Conversation()
    model = await asyncio.to_thread(load_whisper)
    await agent.start(cwd)

    async def act(actions):
        for a in actions:
            if a[0] == "send":
                await agent.send(a[1])
            elif a[0] == "interrupt":
                await agent.interrupt()
            elif a[0] == "stop":
                link.send({"cmd": "stop"})
            elif a[0] == "say":
                link.send({"cmd": "voice", "text": a[1]})
            elif a[0] == "state":
                link.send({"cmd": "voice_state", "state": a[1]})

    asyncio.create_task(link.watch_speaking(lambda busy: events.put_nowait(("audio", busy))))
    asyncio.create_task(listen(events, model))
    link.send({"cmd": "voice_state", "state": "listening"})
    while True:
        ev = await events.get()
        kind = ev[0]
        if kind == "error":
            raise RuntimeError(ev[1])
        if kind == "warn":
            notify(ev[1])
        elif kind == "start":
            await act(cv.speech_start())
        elif kind == "phrase":
            print(f"agent_voice: > {ev[1]}", flush=True)
            await act(cv.phrase(ev[1]))
        elif kind == "delta":
            await act(cv.delta(ev[1]))
        elif kind == "done":
            await act(cv.done())
        elif kind == "audio":
            await act(cv.audio(ev[1]))


if __name__ == "__main__":
    cuda_env()
    ap = argparse.ArgumentParser()
    ap.add_argument("--agent", choices=["claude", "codex"], required=True)
    ap.add_argument("--cwd", required=True)
    a = ap.parse_args()
    try:
        asyncio.run(main(a.agent, a.cwd))
    except Exception as e:
        print(f"agent_voice: {e!r}", file=sys.stderr, flush=True)
        notify(str(e)[:200])
        sys.exit(1)
```

Исключение внутри фоновых задач (`listen`, `watch_speaking`) не дойдёт до цикла само: обернуть обе в `async def guard(coro, name)` которая при исключении кладёт `("error", f"{name}: {e}")` в `events`, и создавать задачи через неё.

- [ ] **Step 4: Ручная проверка процесса без демона-перезапуска**

Демон ещё старый (без `voice`), поэтому проверяется только слух и агент: запустить с выводом в терминал и сказать фразу.

Run: `cd ~/.local/share/agent-speak && ./voice-venv/bin/python -m agent_voice --agent claude --cwd ~/Projects/agent-speak < /dev/tty`
Expected: в выводе `agent_voice: > <твоя фраза>` в течение ~1 с после паузы; Ctrl+C завершает. Попросить пользователя сказать фразу (микрофон — только с его участием).

- [ ] **Step 5: Commit**

```bash
git add python/agent_voice install.sh && git commit -m "agent_voice: microphone, whisper, main loop; install voice venv"
```

---

### Task 6: Плагин — строка «Разговор» и фазы в значке

**Files:**
- Modify: `plugin/Model.js` (`OFFLINE`, `ICONS`, `view`, `voiceSection`)
- Modify: `plugin/Panel.qml` (секция голоса первой, IPC-функция `voice`)
- Modify: `plugin/I18n.js`
- Test: `plugin/test.js`

**Interfaces:**
- Consumes: поля состояния `voice`, `voice_agent`, `voice_last_agent` (Task 1); команды `voice_start {agent}`, `voice_stop` (Task 2); `omarchy-shell predmaxim.agent-speak voice` (Task 2 `VoiceToggle`).
- Produces: `Model.voiceSection(st, tr) -> { caption, options: [{ value, label, cmd }] }` — `cmd` — готовая строка для `link.send`; `Model.view` при `voice !== "off"` показывает микрофон.

- [ ] **Step 1: Тесты (падают)**

В `plugin/test.js` в конец (перед финальным выводом, если он есть):

```js
// Голосовой разговор
const M2 = load("Model.js", "OFFLINE, ICONS, view, voiceSection")
const vs = M2.voiceSection(at({ voice: "off", voice_last_agent: "codex" }), ru)
assert.deepStrictEqual(vs.options.map(o => o.value), ["codex", "claude"]) // последний — первым
assert.strictEqual(vs.options[0].cmd, JSON.stringify({ cmd: "voice_start", agent: "codex" }) + "\n")
const on = M2.voiceSection(at({ voice: "listening", voice_agent: "claude", voice_last_agent: "claude" }), ru)
assert.deepStrictEqual(on.options.map(o => o.value), ["stop"])
assert.strictEqual(on.options[0].cmd, JSON.stringify({ cmd: "voice_stop" }) + "\n")
assert.strictEqual(on.options[0].label, "Закончить разговор с Claude")
assert.deepStrictEqual(v(at({ voice: "thinking", voice_agent: "codex" })), [M2.ICONS.voice, true, "Разговор с Codex: думает"])
assert.deepStrictEqual(v(at({ voice: "listening", voice_agent: "claude" })), [M2.ICONS.voice, true, "Разговор с Claude: слушаю"])
assert.strictEqual(M2.OFFLINE.voice, "off")
```

Существующая проверка `new Set(Object.values(M.ICONS)).size` станет 5 — поменять на `5`. Если `v` в тесте вызывает `M.view`, а не `M2.view` — оба загружают один файл, результат одинаков.

- [ ] **Step 2: Падают**

Run: `cd ~/Projects/agent-speak/plugin && node test.js`
Expected: `TypeError: M2.voiceSection is not a function` (или AssertionError).

- [ ] **Step 3: Реализация**

`plugin/Model.js`:

В `ICONS` добавить `voice: String.fromCodePoint(0xF036C)` (mdi microphone) и в комментарий — «microphone».

В `OFFLINE` добавить `voice: "off", voice_agent: "", voice_last_agent: ""`.

В `view` сразу после проверки `!st.running`:

```js
  if (st.voice && st.voice !== "off") return { icon: ICONS.voice, lit: true, tip: "Talking to %1: " + st.voice, arg: AGENTS[st.voice_agent] || st.voice_agent }
```

Над `view`:

```js
var AGENTS = { claude: "Claude", codex: "Codex" }

// The voice row: off — agents to start (the last one first); on — finish.
function voiceSection(st, tr) {
  if (st.voice && st.voice !== "off")
    return { caption: tr("VOICE CHAT"), options: [{ value: "stop", label: tr("Finish talking to %1", AGENTS[st.voice_agent] || st.voice_agent),
      cmd: JSON.stringify({ cmd: "voice_stop" }) + "\n" }] }
  var order = st.voice_last_agent === "codex" ? ["codex", "claude"] : ["claude", "codex"]
  return { caption: tr("VOICE CHAT"), options: order.map(function(a) {
    return { value: a, label: AGENTS[a], cmd: JSON.stringify({ cmd: "voice_start", agent: a }) + "\n" } }) }
}
```

`plugin/I18n.js` — в таблицу `ru`:

```js
    "VOICE CHAT": "РАЗГОВОР", "Finish talking to %1": "Закончить разговор с %1",
    "Talking to %1: starting": "Разговор с %1: запуск", "Talking to %1: listening": "Разговор с %1: слушаю",
    "Talking to %1: thinking": "Разговор с %1: думает", "Talking to %1: speaking": "Разговор с %1: говорит",
```

`plugin/Panel.qml`:

1. Свойство секций — голос первым:

```qml
  readonly property var sections: [Model.voiceSection(root.speech, root.tr)].concat(Model.sections(root.tr))
```

2. Чипы голосовой строки несут готовую команду `cmd` — отправить её вместо `set`. В `activate()` перед `root.choose(...)`:

```qml
    if (opt && opt.cmd) { link.send(opt.cmd); if (opt.value !== "stop") root.close(); return }
```

и в `onClicked` чипа:

```qml
                    onClicked: {
                      if (chip.modelData.cmd) { link.send(chip.modelData.cmd); if (chip.modelData.value !== "stop") root.close() }
                      else root.choose(group.modelData.key, chip.modelData.value, group.modelData.sample)
                    }
```

`current` чипа для голосовой строки: у секции нет `key` → `root.speech[undefined] === value` ложно — подсветки нет, это правильно.

3. IPC-функция для хоткея (`omarchy-shell predmaxim.agent-speak voice`). Посмотреть, как `Panel` (qs.Ui) объявляет `toggle` для `ipcTarget` — в образце плагина или `/usr/share/omarchy/shell/` (`grep -rn "ipcTarget" /usr/share/omarchy/shell | head`). Добавить по тому же образцу функцию `voice()`:

```qml
  // Hotkey: open (not toggle) with the cursor on the first agent of the voice row.
  function voice() {
    if (!root.opened) root.open()
    Qt.callLater(function() { root.setCursor(0, 0) })
  }
```

Если `Panel` не пробрасывает произвольные функции в IPC — добавить `IpcHandler { target: "predmaxim.agent-speak"; function voice(): void { root.voice() } }` (Quickshell.Io), проверив, что имя цели не конфликтует с уже зарегистрированным `ipcTarget`; при конфликте — `target: "predmaxim.agent-speak.voice"` и поправить команду в `src/daemon.rs` (`VoiceToggle`) на `["predmaxim.agent-speak.voice", "voice"]`.

4. В `onOpenedChanged` курсор сбрасывается (`cursorActive = false`) — `voice()` ставит его после через `callLater`, так что порядок верный.

- [ ] **Step 4: Тесты и вживую**

Run: `node test.js && omarchy-shell shell rescanPlugins`
Expected: тест молча проходит (или печатает свой «ok»). Затем `omarchy-shell shell toggle predmaxim.agent-speak` → в окне сверху строка «РАЗГОВОР: Claude Codex» — скриншот `grim -g "$(hyprctl monitors -j | jq -r '.[0] | "\(.x),\(.y) \(.width)x\(.height)"')" /tmp/as.png` и посмотреть. Закрыть Esc программно (rules.md §2).

- [ ] **Step 5: Commit**

```bash
git add plugin/ && git commit -m "plugin: voice chat row (pick agent / finish), voice phases in the indicator"
```

---

### Task 7: Система — эхоподавление, хоткей, деплой, живая проверка

**Files:**
- Create: `~/omarchy-dotfiles/home/.config/pipewire/pipewire.conf.d/echo-cancel.conf`
- Modify: `~/omarchy-dotfiles/home/.config/hypr/hyprland.lua` (блок agent-speak, ~строка 99)
- Modify: `~/omarchy-dotfiles/watch.tsv`, `docs/changelog.md`, `docs/rules.md` (§ про agent-speak), `README.md` (таблица)
- Modify: `~/Projects/agent-speak/README.md` (раздел «Голосовой разговор»)

**Interfaces:**
- Consumes: всё выше.
- Produces: узлы PipeWire `echo-cancel-source` и `echo-cancel-sink`; хоткей `SUPER + ALT + S` → `agent-speak voice`.

- [ ] **Step 1: Прочитать правила**

`~/omarchy-dotfiles/docs/rules.md` — §1, §2 и раздел про реестр (`watch.tsv`). Проверить ветку: `git -C ~/omarchy-dotfiles branch --show-current` — не `main` → работать в worktree, как велят правила.

- [ ] **Step 2: Эхоподавление**

`home/.config/pipewire/pipewire.conf.d/echo-cancel.conf`:

```
# agent-speak: эхоподавление для голосового разговора. Демон в голосовом режиме
# играет в echo-cancel-sink, agent_voice слушает echo-cancel-source; вне режима никто
# ими не пользуется — устройства по умолчанию не меняются.
context.modules = [
  { name = libpipewire-module-echo-cancel
    args = {
      library.name  = aec/libspa-aec-webrtc
      monitor.mode  = false
      capture.props  = { node.name = "echo-cancel-capture"  node.passive = true }
      source.props   = { node.name = "echo-cancel-source"   node.description = "Микрофон без эха (agent-speak)" }
      sink.props     = { node.name = "echo-cancel-sink"     node.description = "Озвучка с эхоподавлением (agent-speak)" }
      playback.props = { node.name = "echo-cancel-playback" node.passive = true }
    }
  }
]
```

Связать (по §1 — ссылкой на файл, не на папку): `mkdir -p ~/.config/pipewire/pipewire.conf.d && ln -sfn ~/omarchy-dotfiles/home/.config/pipewire/pipewire.conf.d/echo-cancel.conf ~/.config/pipewire/pipewire.conf.d/echo-cancel.conf`

Перезапуск PipeWire обрывает весь звук — спросить пользователя, затем: `systemctl --user restart pipewire pipewire-pulse wireplumber && sleep 2 && pactl list short sources | grep echo-cancel && pactl info | grep -E "Default (Source|Sink)"`
Expected: `echo-cancel-source` есть; источник и выход по умолчанию — прежние (`alsa_…`). Если по умолчанию стал echo-cancel — вернуть `pactl set-default-source/sink` и добавить в конфиг `node.passive`/приоритеты так, чтобы WirePlumber их не выбирал.

- [ ] **Step 3: Хоткей**

Проверить, что комбинация свободна: `grep -n '"SUPER + ALT + S"' ~/omarchy-dotfiles/home/.config/hypr/*.lua /usr/share/omarchy/default/hypr/*.lua 2>/dev/null` и `hyprctl binds -j | jq -r '.[] | select(.key=="S") | "\(.modmask) \(.description)"'` (modmask SUPER+ALT = 72). Занята — выбрать свободную в семействе S и согласовать с пользователем.

В блок agent-speak `hyprland.lua` (после строки `Pause agent reading`), комментарий блока дополнить «voice chat»:

```lua
o.bind("SUPER + ALT + S", "Voice chat with agent", "agent-speak voice")
```

Run: `hyprctl reload && hyprctl configerrors`
Expected: пусто.

- [ ] **Step 4: Реестр, журнал, правила, README**

`watch.tsv` (по образцу строк 39–46):

```
agent-speak	ours	~/.config/pipewire/pipewire.conf.d/echo-cancel.conf
agent-speak	ours	~/.local/share/agent-speak/voice-venv
agent-speak	ours	~/omarchy-dotfiles/home/.config/hypr/hyprland.lua#agent-speak-voice
```

`changelog.md` — запись за 2026-10-05: голосовой разговор, хоткей, echo-cancel. `rules.md` — в абзац про `predmaxim.agent-speak` одно предложение: строка «Разговор» в окне, хоткей, эхоподавление только в голосовом режиме. README репозитория dotfiles — строка таблицы. `~/Projects/agent-speak/README.md` — раздел «Голосовой разговор» (как включить, что нужно: GPU желательно, наушники или echo-cancel; права — только чтение).

- [ ] **Step 5: Деплой и живая проверка**

Спросить пользователя (деплой обрывает озвучку). Затем:

Run: `cd ~/Projects/agent-speak && ./install.sh && cargo test 2>&1 | tail -2 && node plugin/test.js && ~/.local/share/agent-speak/voice-venv/bin/python -m pytest python/test_agent_voice.py -q | tail -1`
Expected: всё зелёное, `systemctl --user is-active agent-speakd` → `active`.

Живая проверка — вместе с пользователем (он говорит):
1. `SUPER + ALT + S` → окно, курсор на агенте; скриншот подтверждает.
2. Enter (Claude) → значок микрофона, «слушаю»; `journalctl --user -u agent-speakd -f` показывает `agent_voice: > …` после фразы.
3. Вопрос → ответ голосом, фаза «думает» → «говорит» → «слушаю».
4. Перебить посреди ответа → звук обрывается < 300 мс, агент отвечает на новую фразу, хвост старого не звучит.
5. То же для Codex; проверить, что начало ответа Codex не теряется (гонка `turn/start`, Task 4).
6. Через колонки: ответ агента не вызывает самоперебивания (echo-cancel). Если вызывает — поднять порог VAD (`Segmenter(threshold=0.7)`) и проверить снова.
7. `SUPER + ALT + S` во время разговора → режим выключен, `pgrep -f agent_voice` пусто, терминальная озвучка снова работает.
8. `systemctl --user restart agent-speakd` во время разговора → `pgrep -f agent_voice` пусто через ≤ 2 с.

- [ ] **Step 6: Commit**

```bash
cd ~/Projects/agent-speak && git add README.md && git commit -m "README: voice chat"
cd ~/omarchy-dotfiles && git add -A home/.config/pipewire home/.config/hypr/hyprland.lua watch.tsv docs README.md && git commit -m "agent-speak voice chat: echo-cancel, Super+Alt+S" && git push origin main
```
