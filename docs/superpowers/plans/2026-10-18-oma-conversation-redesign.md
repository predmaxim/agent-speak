# Ома: разговор одним кликом, свободный орб — план реализации

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Один режим разговора с Омой: клик — слушает, пауза — отправила сама, ответила, снова слушает. Esc отменяет фразу. Текст идёт в чат открытого окна или в пузыри разных оттенков. Агенты звучат по своему тумблеру. Орб ставится куда угодно и магнитится к углам.

**Architecture:**
- **Машина разговора** (`python/agent_voice/conversation.py`) — чистая, с внедрёнными часами:
  - сроки отправки и конца разговора — поля-дедлайны;
  - процесс Омы шлёт ей `tick` раз в 0,2 с, пока разговор идёт.
- **Отмена фразы, которая ещё распознаётся.** Слух нумерует речь: `("start", n)` и `("phrase", text, n)`. Машина помнит номер, до которого всё отменено.
- **Демон:**
  - `dialog` распадается на `talking` (идёт разговор, от Омы) и `agents_voice` (настройка, решает, звучат ли агенты);
  - `orb_corner` заменяется на `orb_pos`.
- **Плагин:**
  - черновик хранится отдельно от стопки пузырей и показывается там, где пользователь смотрит;
  - Esc — бинд Hyprland, который включается и выключается через `Keybind:set_enabled` (`hyprctl eval`).

**Tech Stack:** Rust 2024 (serde, toml); Python 3.12 (voice-venv, asyncio); Quickshell QML (`qs.Commons`, `qs.Ui`, `Quickshell.Wayland`, `Quickshell.Hyprland`, `Quickshell.Io`); node для `plugin/test.js`; Hyprland 0.56 с Lua-конфигом.

**Spec:** `docs/superpowers/specs/2026-10-18-oma-conversation-redesign-design.md` (поверх `2026-10-17-oma-stage1-design.md`)

## Global Constraints

- **Рабочее место.** Работа идёт в worktree `~/Projects/agent-speak-talk`, ветка `oma-talk` от `main`.
  - Почему не в `~/Projects/agent-speak`: оттуда живыми ссылками работают плагин и `agent_voice`.
  - В `main` ветка вливается только в Task 8, с пользователем.
  - Создать worktree: `git -C ~/Projects/agent-speak worktree add ~/Projects/agent-speak-talk -b oma-talk`.
- **Коммиты.** Без ИИ-соавторов, по-русски, в форме `область: что сделано`. Ничего личного в коде и тестах.
- **Язык.** В Rust и Python комментарии по-русски. В QML комментарии по-английски, как сейчас; каждая новая строка интерфейса — английский ключ плюс русский перевод в `plugin/I18n.js`.
- **Цвета** только из темы (`Color.*`), без захардкоженных.
- **Без обходов Omarchy:** только стандартные механизмы Hyprland и Quickshell.
- **Константы:**

  | Что | Значение | Где |
  |---|---|---|
  | `SEND_PAUSE` | 1.3 с от прихода распознанной фразы (VAD уже ждал 0,7 с, итого около 2 с после конца речи) | `conversation.py` |
  | `SILENCE_END` | 30 с | `conversation.py` |
  | Тик разговора | 0.2 с | `__main__.py` |
  | Проверка блокировки | раз в 2 с, пока разговор идёт | `__main__.py` |
  | Удержание Esc после отмены | 1000 мс | `Oma.qml` |
  | Магнит к углу | `Style.space(64)` | `Orb.qml` |
  | Оттенки | «ты» — акцент с альфой 0.18 поверх фона; Ома — передний цвет с альфой 0.07; агенты — фон как сейчас | `Model.TONE_ALPHA` |

- **`orb_pos`.** Формат `"<corner>,<dx>,<dy>"`: угол из `top-left|top-right|bottom-left|bottom-right`, целые `0..10000`. По умолчанию `"bottom-right,0,0"`. `orb_corner` удаляется, старое значение не переносится.
- **`agents_voice`** — bool в `config.toml`, по умолчанию `false`: сейчас без диалога агенты молчат, пусть так и остаётся, пока пользователь не включит.
- **Esc-бинд — Lua через `hyprctl eval`:**

  ```
  on:  local b = _G.predmaxim_oma_esc or hl.bind("ESCAPE", hl.dsp.exec_cmd("omarchy-shell predmaxim.oma cancel")); _G.predmaxim_oma_esc = b; b:set_enabled(true)
  off: if _G.predmaxim_oma_esc then _G.predmaxim_oma_esc:set_enabled(false) end
  ```

  `hl.unbind` и `:remove()` запрещены: они снимают клавишу во всех сабмапах, и сломался бы Esc в `switcher`.
- **Тесты:**
  - `cargo test`;
  - `~/.local/share/agent-speak/voice-venv/bin/python -m pytest python/test_agent_voice.py`;
  - `node plugin/test.js`.

  Все три — зелёные в конце каждой задачи, которая их трогает.
- **Деплой и живые проверки.** `install.sh` и перезапуск оболочки обрывают звук — только с согласия пользователя. Живые проверки окон и хоткеев — после вопроса «руки убрал?».

## Review Focus

1. **Esc остаётся включённым после сбоя.** Сценарии: падает Ома, рестартует демон или оболочка посреди фразы. Ожидание: Esc снова доходит до программ. Сторожит `Model.escWanted`: тест в Task 5 и строка в Task 7 — при старте плагина бинд выключается.
2. **Фраза приходит после конца разговора.** whisper ещё распознавал, когда пользователь кликнул «закончить». Ожидание: фраза выброшена, черновик не появляется. Тест `test_phrase_after_end_dropped` в Task 3.
3. **Речь началась, а фраза так и не пришла.** Микрофон занял voxtype или созвон, и сегментатор сброшен. Ожидание: «слышу» не залипает, отправка и счёт тишины не встают навсегда. Тест `test_mic_busy_clears_hearing` в Task 3.
4. **Орб на мониторе меньше того, где его поставили.** Ожидание: орб на экране, отступ урезан. Тест `parsePos` с обрезкой в Task 5.
5. **Ома упала с недосказанной фразой.** Ожидание: «…» не висит у орба и в чате вечно. Строка в Task 7: при `oma: off` черновик сбрасывается.

---

### Task 1: Демон — голос агентов, флаг разговора, позиция орба

**Files:**
- Modify: `src/config.rs` (поля, `Default`, `set`, тесты)
- Modify: `src/daemon.rs` (`audible`, `Oma`, `auto`, `live`, `purge`, `status_line`, `on_msg`, `oma_down`, тесты)
- Modify: `src/ipc.rs` (`Msg::VoiceState`, тесты)
- Modify: `src/main.rs` (`voice`)

**Interfaces:**
- Produces:
  - строка состояния: поле `dialog` → `talking: bool`; `orb_corner` → `orb_pos: string`; новое поле `agents_voice: bool`;
  - `set` принимает `agents_voice` (bool) и `orb_pos` (строка формата из Global Constraints);
  - от Омы: `{"cmd":"voice_state","state":…,"talking":bool}`;
  - CLI: `agent-speak voice` шлёт Оме `{"do":"talk"}`.

- [ ] **Step 1: Write the failing tests**

  `config.rs`:
  - `defaults_and_partial_file`: проверять `(c.agents_voice, c.orb_pos.as_str()) == (false, "bottom-right,0,0")` вместо `orb_corner`.
  - Новый `orb_pos_and_agents_voice_set`:

    ```rust
    let mut c = Config::default();
    assert!(c.set("orb_pos", &json!("top-left,120,340")) && c.orb_pos == "top-left,120,340");
    for bad in ["middle,1,1", "top-left,1", "top-left,-1,0", "top-left,1,x", "top-left,10001,0", ""] {
        assert!(!c.set("orb_pos", &json!(bad)), "{bad}");
    }
    assert!(c.set("agents_voice", &json!(true)) && c.agents_voice);
    assert!(!c.set("agents_voice", &json!("true")));
    assert!(!c.set("orb_corner", &json!("top-left")));
    ```

  `daemon.rs`:
  - `statuses_spoken_only_in_dialog` → `statuses_spoken_by_agents_voice`. Та же схема, но переключается `st.cfg.agents_voice`, а не `st.oma.dialog`. Плюс проверка: при `st.oma.talking = true` и `agents_voice = false` статус не звучит.
  - Все тесты с `st.oma.dialog = true // финалы звучат только в диалоге` → `st.cfg.agents_voice = true`.
  - `Msg::VoiceState { …, dialog }` → `talking`.
  - `dialog_off_drops_held_final` → `agents_voice_off_drops_held_final`. Финал удержан, затем `on_msg(Msg::Set { key: "agents_voice", value: json!(false) })` — финал выброшен.
  - `status_line_shape`: эталонная строка с `"talking":false`, `"agents_voice":false`, `"orb_pos":"bottom-right,0,0"`, без `dialog` и `orb_corner`.
  - `dead_oma_respawned_after_backoff`: `!st.oma.talking`.

  `ipc.rs`: тесты `voice_state` — с `talking`, по умолчанию `false`.

- [ ] **Step 2: Run tests to verify they fail**

  Run: `cargo test`
  Expected: ошибки компиляции — нет `agents_voice`, `orb_pos`, `talking`.

- [ ] **Step 3: Implement**

  - `config.rs`:
    - удалить `orb_corner`;
    - добавить `pub agents_voice: bool` и `pub orb_pos: String`;
    - валидация `orb_pos` — `fn valid_pos(s: &str) -> bool`: три части через запятую, угол из `CORNERS`, два `u32 <= 10000`.
  - `daemon.rs`:
    - `pub fn audible(kind: &Kind, agents: bool, quiet: bool)`: `Status | Manual => agents`;
    - `auto()` → `self.cfg.agents_voice`;
    - `live()` → `self.cfg.agents_voice && self.cfg.read_intermediate`;
    - `purge()` и пропуск в `enqueue` — по `self.cfg.agents_voice`;
    - `Oma.dialog` → `talking`, со строкой-комментарием «идёт разговор с Омой (для окна)»;
    - `VoiceState` пишет `talking` и по-прежнему ставит `hold` и `purge`;
    - `oma_down` сбрасывает `talking`;
    - `Status`: поля `talking`, `agents_voice`, `orb_pos`;
    - `Msg::Set` уже зовёт `apply()` → `purge()`, отдельно не нужно.
  - `queue.rs`: `retain_audible(agents, quiet)` — переименовать параметр.
  - `ipc.rs`: `VoiceState { state: String, #[serde(default)] talking: bool }`.
  - `main.rs`: `Some("voice") => Msg::Oma { action: json!({"do": "talk"}) }`.

- [ ] **Step 4: Run tests to verify they pass**

  Run: `cargo test`
  Expected: всё зелёное.

- [ ] **Step 5: Commit**

  `git commit -am "daemon: агенты звучат по настройке agents_voice, разговор — talking, орб — orb_pos"`

---

### Task 2: Демон — агенты названы как их окна

**Files:**
- Modify: `src/agents.rs` (`scan_claude`, новая `fn session_title(transcript: &Path) -> Option<String>`, тесты)

**Interfaces:**
- Produces: `Live.title` и строка агентов `title`. Порядок выбора названия:
  1. имя сессии, если `nameSource != "derived"`;
  2. иначе последняя `customTitle` или `aiTitle` из транскрипта;
  3. иначе папка.

  Плагин и мозг Омы берут `title` как есть, менять их не нужно.

- [ ] **Step 1: Write the failing test**

  В `agents.rs`, тест `title_from_transcript`. Транскрипт во временной папке:
  - строки `{"type":"ai-title","aiTitle":"Старое"}`, затем 300 КиБ строк `{"type":"assistant","x":"…"}`, затем `{"type":"ai-title","aiTitle":"Скилл GitGuardian"}` и ещё 100 КиБ мусора → `session_title(&p) == Some("Скилл GitGuardian")`;
  - добавить в конец `{"type":"custom-title","customTitle":"мой"}` → `Some("мой")` (что записано позже, то и главнее);
  - файл без заголовков → `None`; нет файла → `None`.

  В `scan_keeps_only_live_user_sessions` для сессии с `nameSource: "derived"` положить транскрипт с `ai-title` и ждать название из него, а не папку.

- [ ] **Step 2: Run test to verify it fails**

  Run: `cargo test title_from_transcript`
  Expected: ошибка компиляции — нет `session_title`.

- [ ] **Step 3: Implement**

  `session_title` читает файл с конца кусками по 64 КиБ, не больше 1 МиБ, и берёт последнюю строку с `"type":"custom-title"` или `"type":"ai-title"` (парсить JSON строки, `customTitle`/`aiTitle`, пустую пропускать).
  - Строка на стыке кусков — склеить с началом следующего куска.
  - Лимит: заголовок повторяется в транскрипте примерно каждые 15 строк (50–230 КиБ), и мегабайта хватает с запасом. Цена: при скане раз в 2 с читается не больше 1 МиБ на сессию из page cache.
  - Отметить `// ponytail: …`: кэш по размеру файла — если скан станет заметен.

- [ ] **Step 4: Run tests to verify they pass**

  Run: `cargo test`
  Expected: всё зелёное.

- [ ] **Step 5: Commit**

  `git commit -am "agents: название агента — как у его окна (ai-title из транскрипта), а не папка"`

---

### Task 3: Машина разговора — один режим

**Files:**
- Modify: `python/agent_voice/conversation.py` (переписать класс)
- Test: `python/test_agent_voice.py`:
  - заменить тесты машины: `test_talk_draft_send` … `test_short_phrases_dropped`, `test_ask_from_talk_listening_closes_ear`, `test_click_stops_status_audio_and_keeps_draft`;
  - тесты `Segmenter` и `Chunker` не трогать.

**Interfaces:**
- Produces: `Conversation(clock=time.monotonic)`.

  Константы модуля: `SEND_PAUSE = 1.3`, `SILENCE_END = 30`.

  Поля: `state` (`idle|listening|thinking|speaking|paused`), `active: bool`, `draft: str`, `quiet`, `mic`, `audio_busy`, `turn`.

  Методы — каждый возвращает список действий:

  | Метод | Что делает |
  |---|---|
  | `talk()` | клик |
  | `cancel()` | Esc |
  | `end()` | конец разговора: блокировка, отказ слуха |
  | `ask(text)` | набранный текст |
  | `speech_start(n)` | слух услышал начало речи |
  | `phrase(text, n)` | пришла распознанная фраза |
  | `tick()` | проверка дедлайнов |
  | `delta(text)`, `done()`, `audio(busy)`, `mic_busy(on)`, `set_quiet(on)` | как сейчас |

  Действия: `("ear", bool)`, `("state", s)`, `("draft", text, hearing: bool)`, `("drop_draft",)`, `("send", text)`, `("stop",)`, `("interrupt",)`, `("show", s)`, `("say", s)`.

  Удаляются: `set_dialog`, `send`, `dialog`.

- [ ] **Step 1: Write the failing tests**

  Общий фейк часов: `class Clock: t = 0.0; def __call__(self): return self.t`. Каждый тест: `clk = Clock(); cv = Conversation(clock=clk)`.

  ```python
  def test_talk_listens_and_pause_sends():
      assert cv.talk() == [("ear", True), ("state", "listening")] and cv.active
      assert cv.speech_start(1) == [("draft", "", True)]
      assert cv.phrase("какая погода", 1) == [("draft", "какая погода", False)]
      clk.t += 1.0; assert cv.tick() == []
      assert cv.speech_start(2) == [("draft", "какая погода", True)]
      clk.t += 5; assert cv.tick() == []                      # пока слышит — не отправляет
      assert cv.phrase("в Москве", 2) == [("draft", "какая погода в Москве", False)]
      clk.t += SEND_PAUSE
      assert cv.tick() == [("drop_draft",), ("send", "какая погода в Москве"), ("state", "thinking")]

  def test_answer_then_listens_again():
      # talk, phrase, tick → thinking; delta → show+say+speaking; audio(True); done() == []
      assert cv.audio(False) == [("state", "listening")] and cv.active

  def test_click_while_listening_ends_and_drops_draft():
      cv.talk(); cv.speech_start(1); cv.phrase("черновик тут", 1)
      assert cv.talk() == [("drop_draft",), ("ear", False), ("state", "idle")] and not cv.active

  def test_click_while_speaking_interrupts_and_listens():
      # в разговоре: talk, phrase, tick, delta, audio(True)
      assert cv.talk() == [("stop",), ("interrupt",), ("state", "listening")]

  def test_click_while_speaking_typed_answer_starts_conversation():
      cv.ask("расскажи"); cv.delta("Длинный ответ про всё сразу. "); cv.audio(True)
      assert cv.talk() == [("stop",), ("interrupt",), ("ear", True), ("state", "listening")] and cv.active

  def test_cancel_drops_phrase_in_flight():
      cv.talk(); cv.speech_start(1); cv.phrase("раз два", 1); cv.speech_start(2)
      assert cv.cancel() == [("drop_draft",)]
      assert cv.phrase("три четыре", 2) == []                 # распознано после отмены
      assert cv.speech_start(3) == [("draft", "", True)]
      assert cv.phrase("пять шесть", 3) == [("draft", "пять шесть", False)]
      cv.cancel(); assert cv.cancel() == []                    # отменять нечего

  def test_silence_ends_conversation():
      cv.talk(); clk.t += SILENCE_END - 0.1; assert cv.tick() == []
      clk.t += 0.2; assert cv.tick() == [("ear", False), ("state", "idle")]
      assert cv.end() == []

  def test_silence_not_counted_while_hearing_or_answering():
      cv.talk(); cv.speech_start(1); clk.t += 60; assert cv.tick() == []
      cv.phrase("вопрос тут", 1); clk.t += SEND_PAUSE; cv.tick()  # → thinking
      clk.t += 60; assert cv.tick() == []
      cv.done(); clk.t += SILENCE_END - 1; assert cv.tick() == []  # счёт — заново с «слушаю»

  def test_end_stops_turn():
      # talk, phrase, tick → thinking
      assert cv.end() == [("interrupt",), ("ear", False), ("state", "idle")]

  def test_phrase_after_end_dropped():
      cv.talk(); cv.speech_start(1); cv.talk()
      assert cv.phrase("поздно уже", 1) == [] and cv.draft == ""

  def test_voice_barge_in():
      # talk, phrase(…, 1), tick, delta, audio(True)
      assert cv.speech_start(2) == [("stop",), ("interrupt",), ("state", "listening"), ("draft", "", True)]

  def test_mic_busy_clears_hearing():
      cv.talk(); cv.speech_start(1)
      assert cv.mic_busy(True) == [("state", "paused"), ("drop_draft",)]
      assert cv.phrase("это для voxtype", 1) == []
      assert cv.mic_busy(False) == [("state", "listening")]
      clk.t += SILENCE_END; assert cv.tick() == [("ear", False), ("state", "idle")]

  def test_ask_in_conversation_keeps_listening():
      cv.talk(); cv.speech_start(1); cv.phrase("черновик тут", 1)
      assert cv.ask("который час") == [("drop_draft",), ("send", "который час"), ("state", "thinking")]
      # delta, audio(True), done(), audio(False)
      assert cv.state == "listening" and cv.active

  def test_quiet_shows_without_saying():  # как сейчас; done() вне разговора → [("state", "idle")]

  def test_short_phrases_dropped():
      cv.talk(); cv.speech_start(1)
      assert cv.phrase(" а ", 1) == [("drop_draft",)] and cv.ask("ж") == []

  def test_status_audio_does_not_end_draft():
      cv.talk(); cv.phrase("подожди", 1)
      assert cv.audio(True) == [] and cv.audio(False) == []
      assert (cv.state, cv.draft) == ("listening", "подожди")
  ```

  Комментарии «# talk, phrase, …» в тестах — развернуть в вызовы.

- [ ] **Step 2: Run tests to verify they fail**

  Run: `~/.local/share/agent-speak/voice-venv/bin/python -m pytest python/test_agent_voice.py -q`
  Expected: FAIL — `Conversation()` без `clock`, нет `tick`/`end`.

- [ ] **Step 3: Implement `Conversation`**

  Правила, которые тесты не определяют до конца:

  - **«Слышу».** Ведутся номера `started` (последний `speech_start`), `heard` (последняя пришедшая фраза) и `cut` (до какого номера всё отменено). `hearing = started > max(heard, cut)`. `cancel`, `end` и `mic_busy(True)` ставят `cut = started`. Фраза с `n <= cut` выбрасывается.
  - **Дедлайны.**
    - `send_at` ставится фразой и снимается при `speech_start`.
    - `quiet_at` ставится при входе в `listening`/`paused` в разговоре и каждой фразой; снимается при `speech_start` и при уходе в `thinking`/`speaking`.
    - `tick()`: сначала отправка (`state == "listening"`, не `hearing`, есть черновик), потом конец разговора (`quiet_at` истёк, не `hearing`, `state` ∈ {`listening`, `paused`}).
  - **Отправка.** Если звучит статус агента (`audio_busy`), впереди `("stop",)` — как сейчас. Слух в разговоре не выключается.
  - **Короткая фраза** (меньше 2 символов) засчитывается как пришедшая. «…» гасится: `("draft", draft, False)`, если черновик есть, иначе `("drop_draft",)`.
  - **`_rest()`** — `paused`/`listening`, если `active`, иначе `idle`.
  - **Докстринг модуля** переписать под один режим: клик слушает, пауза отправляет, Esc отменяет, 30 с тишины заканчивают.

- [ ] **Step 4: Run tests to verify they pass**

  Run: тот же pytest. Expected: тесты машины зелёные. Тест `test_oma_routes_…` падает до Task 4 — это допустимо, отметить в отчёте.

- [ ] **Step 5: Commit**

  `git commit -am "conversation: один режим — пауза отправляет, Esc отменяет фразу, 30 с тишины заканчивают"`

---

### Task 4: Процесс Омы — нумерация речи, тики, блокировка, пузыри

**Files:**
- Modify: `python/agent_voice/ear.py` (`Ear._read`, `Ear._recognize`)
- Modify: `python/agent_voice/__main__.py` (`Oma.command`, `Oma.event`, `Oma.act`, `main`)
- Modify: `python/agent_voice/context.py` (`locked`)
- Test: `python/test_agent_voice.py`

**Interfaces:**
- Consumes: `Conversation` из Task 3.
- Produces — события слуха:
  - `("start", n)` и `("phrase", text, n)`;
  - `n` — `Ear.seq`, растёт на каждое начало речи и не сбрасывается при перезапуске слуха.
- Produces — команды Оме: `talk`, `cancel`, `ask`, `open`, `new`, `chats`. Команды `send` и `dialog` удаляются.
- Produces — внутренние события `("tick",)` и `("locked",)`.
- Produces — пузыри (в `emit`):
  - черновик: `{"id":"draft","who":"Ты","text":t,"kind":"draft","hearing":bool,"append":False}`;
  - удаление черновика — как сейчас, `drop: True`;
  - отправленная фраза: `{"id":f"me-{pid}-{n}","who":"Ты","text":t,"kind":"me","append":False}` — тот же `n`, что у ответа `oma-{pid}-{n}`.
- Produces: `voice_state` с `"talking": conv.active`.
- Produces: `context.locked() -> bool` — `omarchy-hyprland-session-locked` вернул 0. Таймаут 3 с; ошибка или отсутствие скрипта — `False`.

- [ ] **Step 1: Write the failing tests**

  ```python
  def test_oma_routes_talk_draft_tick_and_quiet_answer():  # заменяет test_oma_routes_talk_draft_send_and_quiet_answer
      # Fake*-классы — как в старом тесте; clk = Clock(); Oma(Conversation(clock=clk), …)
      await oma.command({"do": "talk"}); assert oma.ear.open
      assert sent[-1] == {"cmd": "voice_state", "state": "listening", "talking": True}
      await oma.event(("start", 1))
      assert emitted[-1]["line"]["bubble"] == {"id": "draft", "who": "Ты", "text": "", "kind": "draft", "hearing": True, "append": False}
      await oma.event(("phrase", "как дела у почты", 1))
      clk.t += SEND_PAUSE; await oma.event(("tick",))
      assert oma.brain.texts == ["как дела у почты"] and oma.ear.open
      b = [e["line"]["bubble"] for e in emitted if "bubble" in e["line"]]
      assert b[-2]["drop"] and b[-1]["kind"] == "me" and b[-1]["text"] == "как дела у почты"
      # тихий ответ — как раньше: нет voice, есть пузырь kind "oma"

  def test_oma_locked_ends_conversation():
      await oma.command({"do": "talk"}); await oma.event(("locked",))
      assert not oma.ear.open and sent[-1]["talking"] is False

  def test_oma_cancel_command():
      # talk, ("start", 1), {"do": "cancel"} → последний пузырь — drop черновика

  def test_ear_numbers_speech():
      # _ear() с записью, где две фразы речи
      # события: ("start", 1), ("phrase", …, 1), ("start", 2), ("phrase", …, 2)

  def test_locked_false_without_helper(monkeypatch):
      monkeypatch.setenv("PATH", "/nonexistent")
      assert locked() is False
  ```

  Для `test_ear_numbers_speech` взять за основу `_ear(cmd)` и `test_real_vad_one_phrase_from_synthetic_speech`. Если с настоящим VAD две фразы не выходят стабильно, подменить `self._vad` функцией, которая возвращает вероятности по заранее заданному списку.

- [ ] **Step 2: Run tests to verify they fail**

  Run: pytest. Expected: FAIL на новых тестах.

- [ ] **Step 3: Implement**

  - **`ear.py`.** `self.seq = 0` в `__init__`. В `_read` на `start`: `self.seq += 1`, наружу `("start", self.seq)`, в очередь распознавания `(self.seq, pcm)`. `_recognize` отдаёт `("phrase", text, n)`.
  - **`__main__.py`, `Oma`:**
    - `event`: `start` → `speech_start(ev[1])`; `phrase` → `phrase(ev[1], ev[2])`; `tick` → `tick()`; `locked` → `end()`;
    - в `error` — `c.end()` вместо `_dialog(False)`/`cancel()`;
    - удалить `_dialog`;
    - `act`: `draft` с флагом `hearing`; на `send` после `brain.send` — пузырь `me` с `self.n` (после `self.n += 1`);
    - `state` шлёт `talking`.
  - **`main()`.** Ещё два цикла под `keep`:
    - `ticker`: спит 0.2 с, если `oma.conv.active`, иначе 1 с; при активном разговоре кладёт `("tick",)`;
    - `lock`: раз в 2 с, при `oma.conv.active` и `await asyncio.to_thread(locked)` кладёт `("locked",)`.

- [ ] **Step 4: Run tests to verify they pass**

  Run: pytest. Expected: всё зелёное.

- [ ] **Step 5: Commit**

  `git commit -am "agent_voice: речь с номерами, тик разговора, конец по блокировке, пузырь отправленной фразы"`

---

### Task 5: Плагин — модель

**Files:**
- Modify: `plugin/Model.js`, `plugin/I18n.js`
- Test: `plugin/test.js`

**Interfaces:**
- Consumes: поля строки состояния из Task 1; пузыри из Task 4.
- Produces (в `Model.js`):

  | Функция / значение | Что делает |
  |---|---|
  | `OFFLINE` | поля `talking:false`, `agents_voice:false`, `orb_pos:""`; без `dialog` и `orb_corner` |
  | `parsePos(s, w, h, slot) -> {atTop, atLeft, dx, dy}` | неверная строка — `bottom-right,0,0`; `dx <= max(0, w - slot)`, `dy <= max(0, h - slot)` |
  | `place(x, y, w, h, slot, snap) -> string` | `x, y` — левый верх слота орба в области перетаскивания `w×h`. Угол — `corner()` по центру слота; `dx, dy` — округлённые отступы от этого угла, не меньше 0; если `Math.hypot(dx, dy) <= snap`, то `0,0` |
  | `withDraft(stack, draft) -> array` | черновик последним, не больше `BUBBLES_MAX`; `draft` null — `stack` как есть |
  | `tone(kind) -> "me" \| "oma" \| "agent"` | `me` и `draft` → `"me"`, `oma` → `"oma"`, остальное → `"agent"`; годится и для роли в чате (`"me"`/`"oma"`) |
  | `TONE_ALPHA = { me: 0.18, oma: 0.07, agent: 0 }` | альфы оттенков |
  | `escWanted(draft, st, holding) -> bool` | `(draft !== null && !!st && st.running && st.oma !== "off") \|\| holding` |
  | `expire` | без особого случая для черновика: черновика в стопке больше нет |

- Produces (в `I18n.js`):
  - добавить `"Read agents aloud": "Озвучивать агентов"` и `"End conversation": "Закончить разговор"`;
  - удалить `"Send"` и `"Continuous dialog"`.

- [ ] **Step 1: Write the failing tests** (в `test.js`; список имён в `load(...)` дополнить)

  ```js
  assert.strictEqual(M.place(0, 0, 1000, 800, 50, 64), "top-left,0,0")
  assert.strictEqual(M.place(940, 740, 1000, 800, 50, 64), "bottom-right,0,0")    // 10,10 — магнит
  assert.strictEqual(M.place(30, 30, 1000, 800, 50, 64), "top-left,0,0")         // 42 px — магнит
  assert.strictEqual(M.place(100, 0, 1000, 800, 50, 64), "top-left,100,0")       // у края, не у угла
  assert.strictEqual(M.place(400, 100, 1000, 800, 50, 64), "top-left,400,100")
  assert.strictEqual(M.place(700, 600, 1000, 800, 50, 64), "bottom-right,250,150")
  assert.deepStrictEqual(M.parsePos("top-right,12,34", 1000, 800, 50), { atTop: true, atLeft: false, dx: 12, dy: 34 })
  assert.deepStrictEqual(M.parsePos("bottom-left,5000,5000", 1000, 800, 50), { atTop: false, atLeft: true, dx: 950, dy: 750 })
  for (const bad of ["", "junk", "top-left,1", "middle,1,1"])
    assert.deepStrictEqual(M.parsePos(bad, 1000, 800, 50), { atTop: false, atLeft: false, dx: 0, dy: 0 })
  const d = { id: "draft", kind: "draft", text: "" }
  assert.deepStrictEqual(M.withDraft([{ id: "a" }, { id: "b" }, { id: "c" }], d).map(b => b.id), ["b", "c", "draft"])
  assert.strictEqual(M.withDraft([{ id: "a" }], null).length, 1)
  assert.deepStrictEqual(["me", "draft", "oma", "status", "final", "question"].map(M.tone), ["me", "me", "oma", "agent", "agent", "agent"])
  assert.strictEqual(M.escWanted(d, st, false), true)
  assert.strictEqual(M.escWanted(d, Object.assign({}, st, { oma: "off" }), false), false)
  assert.strictEqual(M.escWanted(d, M.OFFLINE, false), false)
  assert.strictEqual(M.escWanted(null, st, true), true)
  assert.strictEqual(M.escWanted(null, st, false), false)
  ```

  Старые проверки на черновик в `pushBubble`/`expire` поправить: черновик больше не держит стопку. `corner()` остаётся, её тесты тоже.

- [ ] **Step 2: Run test to verify it fails**

  Run: `node plugin/test.js`
  Expected: `TypeError: M.place is not a function` (или похожая).

- [ ] **Step 3: Implement** в `Model.js` и `I18n.js` по интерфейсам выше.

- [ ] **Step 4: Run test to verify it passes**

  Run: `node plugin/test.js`
  Expected: без вывода, код 0.

- [ ] **Step 5: Commit**

  `git commit -am "plugin: модель — позиция орба с магнитом, черновик отдельно, оттенки, Esc"`

---

### Task 6: Плагин — орб: свободное место, оттенки, без «Отправить»

**Files:**
- Modify: `plugin/Orb.qml`

**Interfaces:**
- Consumes: `Model.parsePos`, `Model.place`, `Model.tone`, `Model.TONE_ALPHA` (Task 5); `link.speech.orb_pos` (Task 1).
- Produces: те же `property var bubbles` (уже с черновиком — его подставит Task 7), `signal dismissed(key)`, `signal windowToggled()`.

- [ ] **Step 1: Position**
  - `saved` = `link.speech.orb_pos || "bottom-right,0,0"`; `dropped` — как сейчас, только строка `orb_pos`.
  - `pos = Model.parsePos(dropped || saved, screen.width - 2*gap, screen.height - 2*gap, slotSize)`; `atTop`/`atLeft` берутся из `pos`.
  - Не во время перетаскивания: поля привязанных сторон — `gap + pos.dx` / `gap + pos.dy`, остальные — `gap`.
  - Во время перетаскивания окно занимает свободную область, как сейчас. База слота — `atLeft ? pos.dx : parent.width - width - pos.dx` (по вертикали так же) плюс `dragDX`/`dragDY`, с прежним ограничением.
  - `drop()`: `Model.place(orbSlot.x, orbSlot.y, win.width, win.height, win.slotSize, Style.space(64))` → `set("orb_pos", …)`.
  - Комментарий в шапке файла поправить: «drop keeps the place, near a corner it snaps».

- [ ] **Step 2: Bubbles**
  - Удалить кнопку `send` и её ширину в `inner`.
  - Цвет карточки — `Qt.tint(Color.popups.background, Qt.rgba(c.r, c.g, c.b, Model.TONE_ALPHA[t]))`, где `t = Model.tone(bubble.kind)`, а `c = t === "me" ? Color.accent : Color.foreground`.
  - Текст черновика: пустой и `hearing` — «…»; непустой и `hearing` — текст + « …».
  - Клик по черновику ничего не делает: черновик убирает только Esc или клик по орбу.

- [ ] **Step 3: Verify**
  - Run: `node plugin/test.js`. Expected: зелёное.
  - Run: `qmllint` по `Orb.qml`, если он доступен в системе (`which qmllint6 qmllint`). Иначе проверка — в Task 8 (журнал оболочки после перезапуска).

- [ ] **Step 4: Commit**

  `git commit -am "plugin: орб стоит где отпустили, у угла магнитится; пузыри своими оттенками, без «Отправить»"`

---

### Task 7: Плагин — черновик и пузыри по месту, Esc, шапка и чат окна

**Files:**
- Modify: `plugin/Oma.qml`, `plugin/Window.qml`

**Interfaces:**
- Consumes: `Model.withDraft`, `Model.escWanted`, `Model.tone`, `Model.TONE_ALPHA` (Task 5); `speech.talking`, `speech.agents_voice` (Task 1); пузыри `draft` с `hearing` и `me` (Task 4).
- Produces:
  - IPC `predmaxim.oma`: `toggle()`, `talk()`, `cancel()`; `dialog()` удаляется;
  - `Window`: `property var draft` (null или пузырь черновика), `readonly property bool shown`.

- [ ] **Step 1: `Oma.qml` — маршрут пузырей**
  - Хранить `property var draft: null`.
  - Пузырь с `id === "draft"` пишет в `draft` (при `drop` — `null`). Остальные пузыри добавляются в `bubbles`, только пока окно не показано.
  - Окно показано — `bubbles = []`.
  - `link.speech.oma === "off"` — `draft = null`.
  - Орбу отдаётся `omaWindow.shown ? [] : Model.withDraft(root.bubbles, root.draft)`; окну — `draft`.
  - Таймер истечения работает по `root.bubbles`, как сейчас.

- [ ] **Step 2: `Oma.qml` — Esc**
  - `Timer { id: escHold; interval: 1000 }`; IPC `cancel()` шлёт `{do:"cancel"}` и делает `escHold.restart()`.
  - `readonly property bool escWanted: Model.escWanted(draft, link.speech, escHold.running)`.
  - Применение — один `Process` (`Quickshell.Io`) с `hyprctl eval <on|off>` из Global Constraints.
    - Команды идут строго по очереди, побеждает последнее желаемое: если `Process` занят, после его выхода запустить снова, пока применённое не совпадёт с `escWanted`.
    - В `Component.onCompleted` — `off`.
  - Комментарий над процессом — почему `set_enabled`, а не `unbind`: снимает во всех сабмапах.

- [ ] **Step 3: `Window.qml` — шапка**
  - Тумблер (столбец шапки 4) теперь «Озвучивать агентов»:
    - `checked: root.speech.agents_voice === true`;
    - `press(4)` → `root.choose("agents_voice", !root.speech.agents_voice, false)`;
    - подсказка `tr("Read agents aloud")`;
    - `id` → `agentsSwitch`.
  - Кнопка-микрофон (столбец 0): `selected: root.speech.talking === true`, подсказка `talking ? tr("End conversation") : tr("Talk")`; действие прежнее — `{do:"talk"}`.
  - Комментарий у `headCount` поправить: «the agents-voice switch».

- [ ] **Step 4: `Window.qml` — чат**
  - Делегат сообщения — карточка с фоном по `Model.tone(role)`: та же формула, что у пузырей; `radius: Style.cornerRadius`; внутренний отступ `Style.space(10)`; метка «ТЫ»/«ОМА» остаётся.
  - `footer` у `chatView`: черновик (`root.draft`), если он не null. Вид «me»; текст как в пузыре («…» при `hearing`), цвет текста `root.dim`.
  - При смене `draft` — `root.scrollDown()`.
  - `back()` не меняется: пока фраза идёт, Esc ловит бинд Hyprland, до окна он не доходит.

- [ ] **Step 5: `Window.qml` — чат держит низ**

  **Гипотеза причины** (подтвердить в живой проверке Task 8): `scrollDown()` зовёт `positionViewAtEnd()` один раз через `Qt.callLater`, сразу после `shown` или `messages`. В этот момент окно слоя ещё не получило размер, переносимые строки не разложены, и рост высоты потом оставляет вид выше конца.

  Исправление:
  - у `chatView` `property bool follow: true`;
  - `onContentHeightChanged` и `onHeightChanged` при `follow` зовут `positionViewAtEnd()`;
  - `onMovementEnded: follow = atYEnd`;
  - `scrollDown()` ставит `follow = true` и прокручивает;
  - `scrollDown()` зовут прежние места плюс смена `draft`.

  Если в живой проверке окно всё равно открывается не внизу — вернуться к диагностике (лог `height`, `contentHeight`, `contentY` при открытии), а не наращивать исправления.

- [ ] **Step 6: Verify**
  - Run: `node plugin/test.js`. Expected: зелёное.
  - Run: `grep -rn '"send"\|do: "dialog"\|dialogSwitch\|orb_corner\|"Send"\|Continuous dialog' plugin/`. Expected: пусто.

- [ ] **Step 7: Commit**

  `git commit -am "plugin: черновик и разговор — в чате открытого окна или в пузырях, Esc отменяет фразу, тумблер «Озвучивать агентов», чат держит низ"`

---

### Task 8: Документация, система, слияние, живая проверка (с пользователем)

**Files:**
- Modify: `README.md` — разделы «Ома» (орб, пузыри, окно), «Как звучат агенты», «Управление», «Настройки»:
  - «Постоянный диалог» → «Разговор»;
  - `agents_voice` и `orb_pos` вместо `orb_corner`;
  - Esc;
  - протокол: `talking`, без `send`/`dialog`.
- Modify: `~/omarchy-dotfiles/home/.config/hypr/hyprland.lua:138-143` — подпись бинда `"Oma: talk"`, комментарий без «continuous dialog».
- Modify: `~/omarchy-dotfiles/docs/rules.md:160` — фраза про «строку Разговор» → клик по орбу или `Super+Ctrl+Alt+S`, Esc отменяет фразу.
- Modify: `~/omarchy-dotfiles/docs/changelog.md` — запись за 2026-10-18.

- [ ] **Step 1: Docs**

  Правки выше. Коммиты отдельно: в agent-speak (ветка `oma-talk`) и в omarchy-dotfiles.

- [ ] **Step 2: Full test run in worktree**

  Run: `cargo test && ~/.local/share/agent-speak/voice-venv/bin/python -m pytest python/test_agent_voice.py -q && node plugin/test.js`
  Expected: всё зелёное.

- [ ] **Step 3: Спросить пользователя, можно ли деплой (обрывает звук). Затем слить и поставить**

  ```bash
  git -C ~/Projects/agent-speak merge --ff-only oma-talk
  cd ~/Projects/agent-speak && ./install.sh
  omarchy restart shell
  ```

  Expected:
  - `systemctl --user is-active agent-speakd` → `active`;
  - в `journalctl --user -u agent-speakd --since -2min` нет ошибок Омы;
  - в журнале оболочки нет ошибок QML от `predmaxim.agent-speak`.

- [ ] **Step 4: Живая проверка — после «руки убрал?»**

  Пункты 1–10 из «Как понять, что готово» в дизайне. Снимки — `grim`, окно — `omarchy-shell predmaxim.oma toggle`, состояние — строка из сокета. Голосовые пункты (1–4) проходит пользователь, ты смотришь журнал и снимки.
  - **Esc (пункт 3).** `hyprctl binds -j | jq '[.[]|select(.key=="ESCAPE" and .submap=="")]'` — бинд есть.
  - **Esc не задел Alt+Tab.** В сабмапе `switcher` `ESCAPE` на месте.
  - **Ома упала (пункт 9).** `pkill -f 'python -m agent_voice'` посреди фразы → `hyprctl repl 'return tostring(_G.predmaxim_oma_esc.enabled)'` → `false`.

- [ ] **Step 5: Push**

  Только с согласия пользователя: `git -C ~/Projects/agent-speak push` и `git -C ~/omarchy-dotfiles push`. После этого убрать worktree: `git worktree remove ~/Projects/agent-speak-talk`.
