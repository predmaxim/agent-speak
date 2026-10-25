# Ома-прораб, план 3: задания и ответы агентам голосом, запуск агентов, подтверждение опасного — план реализации

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** пользователь говорит Оме коротко — она разворачивает это в полное задание или ответ и передаёт агенту в его терминал (Orca или herdr), отвечает за пользователя на меню вопросов и одобрение плана, запускает нового агента Claude в проекте Orca; опасное, сказанное голосом, уходит только после дословной фразы «подтверждаю …», совпадение проверяет код.

**Architecture:** решает и пишет текст мозг Омы (Sonnet, `brain.py`): новые инструменты `tell`, `projects`, `launch`. Инструменты идут через `Orders` (`python/agent_voice/orders.py`). Это код без модели: он ловит опасное в реплике, сказанной голосом, держит его до фразы подтверждения, а потом зовёт CLI `agent-speak tell|launch|projects`. CLI — Rust (`src/orders.rs`): по живой сессии находит терминал (`mux::locate`). Если агент ждёт меню (`AskUserQuestion`, `ExitPlanMode` в полёте по транскрипту), CLI выбирает клавиши, иначе передаёт текст. Пишет CLI через адаптер. Запуск и список проектов — новые методы трейта `Mux`, реализованы у Orca. Демон даёт Оме только две вещи: путь к своему бинарнику и поле `ask` (вопрос с вариантами) в снимке агентов.

**Tech Stack:** Rust (serde_json — уже в Cargo.toml), CLI `orca` 1.4.x и `herdr` 0.9.1, Python 3 (voice-venv, Claude Agent SDK, asyncio subprocess).

**Spec:** `docs/superpowers/specs/2026-10-24-oma-foreman-design.md` — «Задания агентам», «Мои решения» (ответ на вопрос — кому), анти-цели, «Оставлено строителю» (меню `AskUserQuestion` и одобрение плана через клавиши). План 2 (сторож) — `2026-10-25-oma-watch.md`.

## Итог (2026-10-25)

**Выполнен в ветке `oma-orders`.** Все шесть задач сделаны. Финальная проверка ветки (Opus) нашла 2 критические и 5 важных ошибок; ещё две мелкие подняты до важных. Все девять закрыты тестами RED→GREEN. Наборы: Rust 349, Python 107, плагин — зелёные.

Проверено вживую:
- **herdr** (пробный агент на Haiku без `--dangerously-skip-permissions`): вариант цифрой, свой ответ, два вопроса (отказ от меню и ответ репликой), правка плана и одобрение — `agent-speak tell`.
- **Orca**: задание текстом и свой ответ в меню.
- **Мозг набором**: «передай агенту в omaprobe…» → агент получил развёрнутое задание, Ома ответила «Передала проекту omaprobe: …».
- `agent-speak projects` — 18 проектов Orca. Служба видит Orca и без обёртки в PATH.
- **Запуск** (запустил пользователь): `agent-speak launch <agent-speak> "Скажи «привет» и закончи"` открыл вкладку Orca «Приветствие и завершение». Команда в ней — `claude '--dangerously-skip-permissions' '<задание>'` (флаги из настроек Orca), агент ответил «привет».

Не проверено мной:
- Голосовой путь с подтверждением.
- Задание агенту, у которого Bash в полёте (пробный Bash заблокировал авто-режим).
Эти пункты — за пользователем.

Финальная проверка, закрыто:
- «Не подтверждаю пуш» подтверждал. Теперь «не» и «нет» во фразе — отказ.
- Ответ на меню разрешения уходил текстом с Enter, а Enter выбирает «Yes». Теперь при вызове в полёте Ома смотрит экран и с меню отказывает: «ответь там». herdr не шлёт текст командой агенту в диалоге.
- При ответе вариантом проверялся текст, а уходит вариант. Теперь проверяется вариант. Одобрение плана отказом не считается.
- Отказ прикрывал опасное, дописанное мозгом («нет, лучше удали ветку»). Теперь не прикрывает.
- В вопросе агента учитывалась вся его реплика («всё запушено»). Теперь — только часть «Ждёт ответа на …».
- Мозг видел план только до 3000 знаков. Теперь до 20 000.
- В словаре опасного не было «убери», `clean`, «откати», `rebase`, «залей», `update`, «в базе». Добавлены.
- Перефраз опасного в том же ходе уходил без подтверждения. После опасного ход больше ничего не передаёт.
- Таймаут Python 40 с был не больше худшего случая Rust. Теперь 90 с, а при таймауте — «не знаю, дошло ли».

Отложенные мелочи:
- набранная реплика не снимает ожидание подтверждения;
- называется только первый вид опасного;
- ложные срабатывания словаря («удалённый репозиторий»);
- текст с «--» в начале Orca разберёт как флаг;
- диспетчер ждёт CLI во время подтверждения;
- закрытая Orca выглядит как «нет такого проекта»;
- пустой `AGENT_SPEAK_BIN`;
- точное название агента проигрывает подстроке;
- повтор «ждёт ответа: Ждёт ответа на» в выводе `agents`.

## Решения этого плана

Их спека оставила строителю, или их решил пользователь 2026-10-25.

- **Запуск — в Orca, не в herdr** (решение пользователя). Агенты пользователя живут в Orca, herdr не запущен. Запуск: `orca agent launch --agent claude --worktree path:<папка> --surface terminal --prompt <задание> --json`. Флаги Orca подставляет сам из своих настроек (`agentDefaultArgs.claude` = `--dangerously-skip-permissions`) — это «те же флаги, с какими пользователь запускает сам».
- **Известные проекты — worktree Orca** (`orca worktree list --json`, сейчас 18, все из `~/Projects`). Папку вне Orca Orca не запускает (`selector_not_found`). Поэтому список спеки «папки прошлых агентов и `~/Projects`» сужен до проектов Orca. Проекта нет в списке — Ома говорит: «Этого проекта нет в Orca — добавь его, и я запущу».
- **Меню в TUI Claude Code** — проверено вживую 2026-10-25 (Claude Code 2.1.310, herdr, агент на Haiku):
  - `AskUserQuestion`, один вопрос, один выбор: цифра варианта сразу выбирает и отправляет. Свой ответ: цифра N+1 («Type something.»), затем текст без Enter, затем Enter. Последний пункт N+2 «Chat about this» отклоняет вопрос («User declined to answer questions»), агент кончает ход.
  - Несколько вопросов: цифра на каждый с переходом к следующему, затем экран «Submit answers» (1). Выбор нескольких (`multiSelect`): цифры ставят галочки, «Submit» без номера. Такие формы Ома не набирает клавишами. Она жмёт «Chat about this» первого вопроса и передаёт ответ текстом: агент получит его следующей репликой.
  - `ExitPlanMode`: «1. Yes, and use auto mode», «2. Yes, manually approve edits», «3. Tell Claude what to change». Одобрить — `1`. Правки — номер строки «Tell Claude what to change» с экрана, текст, Enter: агент переписал план и спросил снова. Номер берётся с экрана, потому что набор пунктов зависит от режима и версии.
  - herdr `agent prompt` агенту, который ждёт меню, отвечает `agent_blocked`. Поэтому меню — только клавишами `pane send-keys` и текстом `pane send-text`.
- **Где решается, что опасно.** Слова — у кода (`orders.danger`), по основам слов на русском и английском. Проверяются три текста: реплика пользователя, текст агенту и вопрос агента, на который отвечают: поле `ask` или последнее предложение с «?» в последней реплике агента. Отказ («нет», «не …») не требует подтверждения, если и реплика, и текст агенту начинаются с отказа, а в самой реплике опасного слова нет.
- **Фраза подтверждения** — «подтверждаю <слово>» в 2–4 словах: пуш, удаление, деплой, миграцию, сброс, форс, запись (в базу). Подходит только следующая реплика голосом, не позже 60 с (`CONFIRM_WAIT`). Другая реплика снимает ожидание. Подтверждённое передаёт код, без модели, и говорит готовую фразу «Передала проекту X: <смысл>». Мозг узнаёт об этом пометкой в начале следующей реплики.
- **Набранное в окне** (`ask` из плагина) подтверждения не требует: там ослышки нет (спека).
- **Кому.** Окно ответа после вопроса уже открывает чат спросившего агента (план 1). Промпт чата агента говорит: реплика после его вопроса — обычно ответ ему, через `tell`. Вопрос к самой Оме («а что он имеет в виду?») передавать не надо. Ответ инструмента велит назвать, кому и что передано.
- **Два агента подходят под имя** — `tell` ничего не передаёт, а просит мозг спросить пользователя, какому.
- **Текст агенту — одной строкой** (переводы строк → пробелы). Иначе Enter внутри текста отправил бы задание по частям в Orca и в поле меню. shortcut: структура длинного задания теряется. Если агенты начнут путаться — проверить вставку многострочного текста (bracketed paste) в Orca.
- **CLI, а не демон.** `tell`/`launch` выполняет отдельный процесс `agent-speak`, и мозг сразу знает исход: «передано» или почему нет. shortcut: CLI и поток действий сторожа могут писать в один терминал одновременно. Если это начнёт мешать — вести задания через демон.

## Global Constraints

- Новых зависимостей нет ни в `Cargo.toml`, ни в voice-venv, ни в плагине.
- Команды и форматы Orca и herdr — только в `src/mux/orca.rs` и `src/mux/herdr.rs`; ядро (`orders.rs`, `daemon.rs`, Python) знает только трейт `Mux` и CLI `agent-speak`.
- Вызовы CLI мультиплексоров — с таймаутом (`mux::cli`, 10 с; запуск — 30 с), никогда в цикле событий демона.
- Опасное действие по голосу уходит только после фразы подтверждения, которую проверил код, а не модель (анти-цель спеки «опасное по ослышке»).
- Агенту не уходит текст, о котором пользователь не узнал хотя бы смыслом. После каждой передачи Ома говорит, кому и что передано, или это говорит готовая фраза кода.
- Ома отправляет сразу, без «точно отправить?». Исключение — опасное по голосу.
- Пользователю — смысл, не текст задания. В речи нет имён переменных, функций, путей, команд.
- Тихий режим: ни звука, пузыри и окно остаются (`Conversation` уже так делает).
- Комментарии: в Rust и Python — по-русски, в QML и JS — по-английски; одна строка «почему».
- Коммиты — без ИИ-соавторов и подписей «Generated with…».
- Проверка: `cargo test`; Ома — `~/.local/share/agent-speak/voice-venv/bin/python -m pytest python/test_agent_voice.py`; плагин — `node plugin/test.js`. `cargo build` без предупреждений.

## Review Focus

1. Агент спросил «Запушить?», пользователь голосом говорит «нет». Ответ уходит без подтверждения (тест `negative_answer_needs_no_confirmation`, задача 4).
2. Ома ждёт «подтверждаю пуш», а распознано «подтверждаю удаление», или фраза пришла через 61 с, или это длинная фраза со словом «подтверждаю». Ничего не уходит (тест `confirm_only_same_kind_in_time`, задача 4).
3. Агент стоит на меню с вариантами, пользователь отвечает своими словами. Текст попадает в поле «Type something», а не цифрами или мусором в меню (тест `free_text_answer_goes_to_type_something`, задача 2).
4. В проекте два агента, пользователь называет проект. Задание не уходит случайному агенту, Ома спрашивает, какому (тест `tell_refuses_ambiguous_agent`, задача 5).
5. Пользователь набрал в окне «удали старую ветку». Уходит сразу, без фразы подтверждения (тест `typed_order_needs_no_confirmation`, задача 5).

---

### Task 1: Адаптеры — клавиши меню, набор без Enter, проекты и запуск

**Files:**
- Modify: `src/mux/mod.rs`, `src/mux/orca.rs`, `src/mux/herdr.rs`

**Interfaces:**
- Produces:
  - `Key::Enter`, `Key::Digit(u8)` (1–9) рядом с `Esc`, `CtrlC`, `CtrlU`.
  - `Step::Type(String)` — текст без Enter и без Ctrl+U перед ним (поле меню).
  - В трейте `Mux`: `fn type_text(&self, pane: &Pane, text: &str) -> Result<(), String>` (обязательный); `fn projects(&self) -> Result<Vec<String>, String> { Ok(Vec::new()) }`; `fn launch(&self, cwd: &str, prompt: &str) -> Result<(), String> { Err(format!("{} не запускает агентов", self.name())) }`.
  - `pub fn perform(pane: &Pane, steps: &[Step]) -> Result<(), String>` — шаги через адаптер панели. Поток действий (`spawn`) зовёт его же. `perform_on` принимает `(m, pane, steps)`.

- [ ] **Step 1: тесты.** В `mux/mod.rs`: `Rec` пишет `type_text` как `"type:<t>"`; новый тест `type_step_has_no_line_clear`: шаги `[Key(Digit(3)), Type("зелёный"), Key(Enter)]` → `["Digit(3)", "type:зелёный", "Enter"]` (без `CtrlU`). Старый `text_goes_after_clearing_the_line` — через `perform_on(&m, &pane, &steps)`.
  В `orca.rs`: `key_text(&Key::Enter) == Some("\r")`, `key_text(&Key::Digit(3)) == Some("3")`, `key_text(&Key::Esc) == Some("\x1b")`, `key_text(&Key::CtrlU) == Some("\x15")`, `key_text(&Key::CtrlC) == None` (у него `--interrupt`). `worktree_paths(r#"{"ok":true,"result":{"worktrees":[{"path":"/p/a"},{"path":"/p/b"}]}}"#) == ["/p/a", "/p/b"]`, на мусоре — пусто. `succeeded(r#"{"ok":true,"result":{}}"#) == Ok(())`; `succeeded(r#"{"ok":false,"error":{"message":"selector_not_found"}}"#)` — `Err` с `selector_not_found`.
  В `herdr.rs`: `key_name(&Key::Digit(2)) == "2"`, `key_name(&Key::Enter) == "enter"`, `key_name(&Key::Esc) == "esc"`.
- [ ] **Step 2:** `cargo test mux` — падает (нет `Digit`, `Type`, `key_text`, …).
- [ ] **Step 3: код.**
  - Orca: клавиши — `send --text <key_text>`, у `CtrlC` по-прежнему `send --interrupt`. `type_text` — `send --text <t>` без `--enter`, ответ через `accepted`. `projects` — `worktree list --json` → `worktree_paths`. `launch` — `agent launch --agent claude --worktree path:<cwd> --surface terminal --prompt <prompt> --json` → `succeeded`, через `mux::run` с таймаутом 30 с: запуск дольше обычного вызова.
  - Orca без терминала агента (проекты, запуск) зовёт `orca_bin()`: `$XDG_CONFIG_HOME` (иначе `~/.config`) + `/orca/linux-orca-cli-shim/orca`, если файл есть, иначе `orca` из PATH. У служб systemd PATH бывает без обёртки Orca.
  - herdr: `key_name` для всех клавиш, `type_text` — `pane send-text <id> <t>` → `ok`.
- [ ] **Step 4:** `cargo test` — зелёные все, `cargo build` без предупреждений.
- [ ] **Step 5:** `git commit -m "адаптеры: клавиши меню и набор без Enter, проекты и запуск агента в Orca"`

### Task 2: `agent-speak tell | launch | projects` — как передать агенту

**Files:**
- Create: `src/orders.rs`
- Modify: `src/main.rs` (подкоманды, `mod orders`), `src/source/tail.rs` (сюда переезжает `tail_of` из `daemon.rs`, `pub fn tail_of(path: &str) -> String`; демон берёт его оттуда)

**Interfaces:**
- Consumes: `Key::{Digit, Enter}`, `Step::Type`, `mux::perform`, `Mux::{projects, launch}` (задача 1); `claude::in_flight`, `agents::scan_all`, `mux::{locate, screen, adapters}`.
- Produces (CLI, его зовёт Python, задача 4): `agent-speak tell <session> <option> <text>` (`option` 0 — свой ответ), `agent-speak launch <path> <text>`, `agent-speak projects` (папки по строке). Успех — код 0. Ошибка — код 1 и одна строка по-русски в stderr, её дословно получает мозг. Нет аргументов — код 2 и подсказка.
- Produces (Rust): `pub fn steps(call: Option<&Value>, text: &str, option: usize, screen: impl FnOnce() -> Result<String, String>) -> Result<Vec<Step>, String>`; `pub fn one_line(text: &str) -> String`; `pub fn tell(session: &str, option: usize, text: &str) -> Result<(), String>`; `pub fn launch(path: &str, text: &str) -> Result<(), String>`; `pub fn projects() -> Vec<String>`.

- [ ] **Step 1: тесты `steps`** (вызов — `json!`; `ask(n, multi)` — `AskUserQuestion` с одним вопросом из n вариантов; экран плана — строки из живой проверки, см. «Решения»):

```rust
#[test]
fn option_picks_digit_of_single_question() {
    assert_eq!(steps(Some(&ask(2, false)), "", 2, no_screen).unwrap(), [Step::Key(Key::Digit(2))]);
    assert!(steps(Some(&ask(2, false)), "", 3, no_screen).unwrap_err().contains("нет варианта 3"));
}

#[test]
fn free_text_answer_goes_to_type_something() {
    assert_eq!(steps(Some(&ask(2, false)), "Зелёный, как трава", 0, no_screen).unwrap(),
        [Step::Key(Key::Digit(3)), Step::Pause(500), Step::Type("Зелёный, как трава".into()), Step::Key(Key::Enter)]);
}

#[test]
fn several_questions_or_multiselect_decline_then_text() {
    // «Chat about this» первого вопроса (N+2), затем ответ обычной репликой
    let want = [Step::Key(Key::Digit(5)), Step::Pause(1500), Step::Text("яблоко и слива".into())];
    assert_eq!(steps(Some(&ask(3, true)), "яблоко и слива", 0, no_screen).unwrap(), want);
    assert!(steps(Some(&ask(3, true)), "", 1, no_screen).is_err(), "такой ответ — только текстом");
}

#[test]
fn plan_approve_and_feedback_by_screen() {
    let plan = json!({"name": "ExitPlanMode", "input": {"plan": "создать файл"}});
    assert_eq!(steps(Some(&plan), "", 1, || Ok(PLAN_SCREEN.into())).unwrap(), [Step::Key(Key::Digit(1))]);
    assert_eq!(steps(Some(&plan), "Назови файл b.txt", 0, || Ok(PLAN_SCREEN.into())).unwrap(),
        [Step::Key(Key::Digit(3)), Step::Pause(500), Step::Type("Назови файл b.txt".into()), Step::Key(Key::Enter)]);
    assert!(steps(Some(&plan), "", 1, || Ok("❯ ".into())).unwrap_err().contains("меню"), "меню нет на экране — не жмём вслепую");
}

#[test]
fn no_menu_is_plain_text_and_empty_is_error() {
    let bash = json!({"name": "Bash", "input": {"command": "cargo test"}});
    assert_eq!(steps(Some(&bash), "займись тестами", 2, no_screen).unwrap(), [Step::Text("займись тестами".into())]);
    assert_eq!(steps(None, "займись тестами", 0, no_screen).unwrap(), [Step::Text("займись тестами".into())]);
    assert!(steps(None, " ", 0, no_screen).is_err());
}

#[test]
fn task_text_is_one_line() {
    assert_eq!(one_line("Сделай так:\n1. тесты\n\n2. доки  "), "Сделай так: 1. тесты 2. доки");
}

#[test]
fn tell_unknown_session_is_error() {
    assert!(tell("нет-такой-сессии", 0, "привет").unwrap_err().contains("нет среди живых"));
}
```

  `fn no_screen() -> Result<String, String> { Err("экран не нужен".into()) }`: без меню и в `AskUserQuestion` экран не читается.
- [ ] **Step 2:** `cargo test orders` — падает.
- [ ] **Step 3: код** (`src/orders.rs`).
  - `steps`, правила по вызову в полёте:
    - `AskUserQuestion`, один вопрос, `multiSelect` не `true`, N вариантов:
      - `option` 1…N — цифра;
      - `option` больше N — ошибка «нет варианта N»;
      - `option` 0 и текст — N+1, пауза 500, `Type`, Enter;
      - иначе — «пустой ответ».
    - Другие формы `AskUserQuestion`: текст есть — N+2 первого вопроса, пауза 1500, `Text`; текста нет — ошибка «на такой вопрос — ответ текстом».
    - `ExitPlanMode`:
      - `option` 1 — на экране должна быть строка с «1. Yes», тогда `Digit(1)`;
      - `option` 0 и текст — номер строки с «Tell Claude what to change» (цифры в начале строки после «❯» и пробелов), пауза 500, `Type`, Enter;
      - нет нужной строки — «не вижу меню одобрения плана».
    - Прочее: непустой текст — `Text`. `option` тут не значит ничего.
    - Цифра больше 9 — ошибка.
  - `tell`:
    1. Живая сессия по id из `agents::scan_all()`. Нет — «агента нет среди живых».
    2. Терминал — `mux::locate(pid)`. Нет — «агент не в Orca и не в herdr — передать не могу».
    3. Вызов в полёте — `claude::in_flight(&tail_of(transcript))`, только у Claude. У Codex меню не разбираем — всегда текст.
    4. `steps(call, &one_line(text), option, || mux::screen(&pane))`, затем `mux::perform`.
    shortcut из «Решений» (CLI и сторож в одном терминале) — комментарием у `tell`.
  - `projects` — объединение `projects()` всех адаптеров, по алфавиту, без повторов; ошибка адаптера — его проектов нет.
  - `launch` — адаптер, в чьих `projects()` есть ровно `path`. Нет такого — «этого проекта нет в Orca — добавь его, и я запущу». Затем `launch(path, &one_line(text))`.
  - `main.rs`: `Some("tell" | "launch" | "projects")` — разбор аргументов, вызов, вывод, код выхода. Строка подсказки дополняется ими.
- [ ] **Step 4:** `cargo test` — зелёные; `cargo build` без предупреждений. Проба: `cargo run -q -- projects` печатает папки Orca, а `cargo run -q -- tell x 0 привет; echo $?` — ошибку и `1`.
- [ ] **Step 5:** `git commit -m "agent-speak tell/launch/projects: ответ агенту текстом или клавишами меню, запуск в проекте Orca"`

### Task 3: Демон даёт Оме вопрос с вариантами и путь к себе

**Files:**
- Modify: `src/agents.rs`, `src/daemon.rs`

**Interfaces:**
- Produces: `Entry.ask: String` (в JSON снимка агентов — поле `ask`) — текст `claude::question_context` для агента, который ждёт ответа на уведомление. `Registry::asking(&mut self, id: &str, text: &str)`. Любое событие агента, кроме `question`, поле очищает. Ома запускается с окружением `AGENT_SPEAK_BIN` = `std::env::current_exe()` демона.

- [ ] **Step 1: тесты.** `agents.rs`, `ask_set_on_question_cleared_by_next_event`: `question` + `asking(id, "Ждёт ответа на AskUserQuestion: …")` → `ask` непустой и есть в `line()`. После `status` — пусто, после `asking` и `prompt` — тоже пусто.
  `daemon.rs`: в тесте уведомления (рядом с `question_job_carries_agent_text_and_call`) `registry.get(id).ask` содержит «Варианты:».
- [ ] **Step 2:** `cargo test ask_` — падает.
- [ ] **Step 3: код.** В хуке `notification` `claude::question_context(&tail)` считается один раз: и для `asking`, и для задания пересказа. В `oma_spawn` — `.env("AGENT_SPEAK_BIN", …)`.
- [ ] **Step 4:** `cargo test` — зелёные.
- [ ] **Step 5:** `git commit -m "снимок агентов: вопрос с вариантами (ask); Ома знает путь к agent-speak"`

### Task 4: `Orders` — опасное по голосу ждёт «подтверждаю …»

**Files:**
- Create: `python/agent_voice/orders.py`
- Test: `python/test_agent_voice.py`

**Interfaces:**
- Consumes: CLI задачи 2 (`agent-speak tell|launch|projects`), путь — `os.environ.get("AGENT_SPEAK_BIN", "agent-speak")`.
- Produces:
  - `CONFIRM_WAIT = 60`.
  - `DANGER` — кортежи `(что, слово фразы, основы слов в тексте, основы в подтверждении)`:

```python
DANGER = (
    ("пуш", "пуш", ("push", "пуш", "запуш", "отпуш"), ("пуш", "push")),
    ("удаление", "удаление", ("удал", "delete", "rm", "сотри", "стери", "снеси"), ("удален",)),
    ("деплой", "деплой", ("deploy", "депло", "задепло", "выкат", "release", "релиз", "publish", "опублик"), ("депло", "deploy")),
    ("миграция", "миграцию", ("migrat", "миграц"), ("миграц",)),
    ("сброс", "сброс", ("reset", "сброс", "сбрось"), ("сброс",)),
    ("force", "форс", ("force", "форс"), ("форс", "force")),
    ("запись в базу", "запись", ("drop", "truncate", "insert", "базу", "бд"), ("запис",)),
)
```

  - Функции. Слова — `conversation.words`, основа сравнивается с началом слова.
    - `danger(text) -> tuple | None`.
    - `negative(text) -> bool`: первое слово — «нет», «не», «отмена» или «стоп».
    - `confirms(text, d) -> bool`: 2–4 слова, есть слово на «подтвержд» и слово на основу подтверждения из `d`.
    - `asked(entry) -> str`: `entry["ask"]`, иначе последнее предложение с «?» из последней строки ленты вида `question` или `final`.
    - `async def cli(*args) -> tuple[bool, str]`: таймаут 40 с. Код 0 — `(True, stdout)`, иначе `(False, stderr)`. Таймаут — `(False, "терминал не ответил")`.
  - `class Orders(run=cli, clock=time.monotonic)`. Поля: `voiced`, `heard`, `pending`, `note`. Методы:
    - `turn(voiced, heard)`;
    - `gate(sent, asked="") -> tuple | None`;
    - `async tell(entry, text, gist, option=0) -> str`;
    - `async launch(path, task, gist) -> str`;
    - `async projects() -> str`;
    - `async confirm(text) -> str | None`.
  - Ответы инструментов (текст для мозга) и фразы кода:
    - передано — `f"Передано проекту {p}. Скажи пользователю одной фразой, кому и что: «Передала проекту {p}: …» — смысл, не текст задания."`;
    - запущено — `f"Агент запущен в проекте {name}. Скажи пользователю одной фразой: «Запустила агента в проекте {name}: …» — смысл задания."`. Здесь `name` — последняя часть пути;
    - нужно подтверждение — `f"Не передано: это {что}, такое уходит только после подтверждения голосом. Скажи пользователю ровно: «Это {что}. Скажи: подтверждаю {слово}»."`;
    - ошибка CLI — `f"Не передано: {err}. Скажи пользователю коротко, что не вышло."`;
    - `confirm` успех — `f"Передала проекту {p}: {gist}."` / `f"Запустила агента в проекте {name}: {gist}."`, сбой — `f"Не вышло: {err}."`; пометка мозгу `note` — `f"(Пользователь подтвердил голосом, Ома сама: {фраза}) "`.

- [ ] **Step 1: тесты.** Обвязка: `_orders(clk=None) -> (Orders, calls)` — у `Orders` фейковый CLI, он пишет аргументы в `calls` и отвечает `(True, "")`. Часы — `Clock` из тестов, `run = asyncio.run`, `E` — агенты из тестов `format_agents`.

```python
def test_danger_words_both_scripts():
    assert danger("запушь ветку")[0] == "пуш" and danger("git push")[0] == "пуш"
    assert danger("удали старые логи")[0] == "удаление" and danger("rm -rf build")[0] == "удаление"
    assert danger("займись тестами") is None and danger("сотрудник написал") is None

def test_voice_danger_waits_for_phrase_typed_goes():
    o, calls = _orders()
    o.turn(True, "пусть запушит")
    r = run(o.tell(E[0], "Запушь изменения", "запушить"))
    assert "подтверждаю пуш" in r and calls == []
    o.turn(False, "удали старую ветку")
    assert "Передано проекту omarchy-mail" in run(o.tell(E[0], "Удали старую ветку", "удалить ветку")) and calls

def test_negative_answer_needs_no_confirmation():
    o, calls = _orders()
    agent = dict(E[0], ask="Ждёт ответа на AskUserQuestion: Запушить? Варианты: Да; Нет")
    o.turn(True, "нет")
    assert run(o.tell(agent, "Нет, не пушь", "не пушить")).startswith("Передано") and calls
    o.turn(True, "да")
    assert "подтверждаю пуш" in run(o.tell(agent, "Да", "пушить")), "опасность в вопросе агента"

def test_confirm_only_same_kind_in_time():
    clk = Clock()
    for heard, wait, sent in (("подтверждаю пуш", 1, True), ("Подтверждаю, push.", 1, True),
                              ("подтверждаю удаление", 1, False), ("подтверждаю пуш", 61, False),
                              ("ну я вроде подтверждаю этот пуш наверное", 1, False)):
        o, calls = _orders(clk)
        o.turn(True, "запушь")
        run(o.tell(E[0], "Запушь", "запушить"))
        clk.t += wait
        said = run(o.confirm(heard))
        assert bool(calls) == sent and (said == "Передала проекту omarchy-mail: запушить.") == sent, heard
        assert o.pending is None
    assert run(o.confirm("подтверждаю пуш")) is None, "ожидание одно: после ответа его нет"
```

  Плюс короткие тесты:
  - `asked` берёт последнее предложение с «?» из итога: «Всё запушено. Взяться за тесты?» → «Взяться за тесты?» (и в нём нет опасного);
  - ошибка CLI приходит мозгу текстом «Не передано: …»;
  - `projects` превращает строки CLI в «имя — папка».
- [ ] **Step 2:** pytest по новым тестам — падает (нет модуля).
- [ ] **Step 3: код.** `gate`: `voiced` ложно — `None`. Иначе `d = danger(heard) or danger(sent) or danger(asked)`. Если `d`, но `negative(heard) and negative(sent) and not danger(heard)` — `None`. `pending` — `{"args", "what", "said", "until"}`. `confirm`:
  - нет ожидания или срок вышел — `None`;
  - фраза совпала — `run(*args)`, фраза, `note`;
  - не совпала — ожидание снимается, `None`.
  В любом случае `pending = None`.
- [ ] **Step 4:** весь pytest — зелёный.
- [ ] **Step 5:** `git commit -m "Ома: задания агентам — опасное голосом уходит только после «подтверждаю …», проверяет код"`

### Task 5: Мозг передаёт и запускает; голос или окно; подтверждение мимо модели

**Files:**
- Modify: `python/agent_voice/brain.py`, `python/agent_voice/conversation.py`, `python/agent_voice/__main__.py`
- Test: `python/test_agent_voice.py`

**Interfaces:**
- Consumes: `Orders` (задача 4), поле `ask` снимка (задача 3).
- Produces:
  - `oma_tools(snapshot, send, orders=None)`, `Brain(events, snapshot, send=None, orders=None)`, `Oma(..., orders=None)`. По умолчанию — `Orders()`.
  - Новые инструменты `tell`, `projects`, `launch`; `OMA_TOOLS` дополнен `mcp__oma__tell`, `mcp__oma__projects`, `mcp__oma__launch`.
  - `Conversation.voiced: bool`: `_send(text, ctx="", voiced=True)`, `ask()` передаёт `voiced=False`. Кортеж `("send", text, ctx)` не меняется.

- [ ] **Step 1: тесты.**
  - `tell_refuses_ambiguous_agent`: два агента с проектом `omarchy-mail` и `tell(agent="mail")` → «несколько агентов» и оба id, фейковые `orders` не вызваны. Точный id — вызваны с этим агентом. Пустой `agent` — агент в фокусе.
  - `typed_order_needs_no_confirmation`: `oma.command({"do": "ask", "text": "удали старую ветку"})` → `orders.voiced` ложно, тот же текст голосом — истинно.
  - `oma_confirmation_skips_brain`: в `Orders` ждёт пуш, голосом «подтверждаю пуш». Тогда `brain.texts` пуст, пузырь Омы «Передала проекту …», разговор после фразы в покое (`voice_state` не застрял в `thinking`). Следующая реплика уходит мозгу с пометкой `note` в начале.
  - `format_agents` с `ask` → строка кончается `; ждёт ответа: …`.
  - Промпт: в `OMA_PROMPT` есть `tell`, `launch`, `projects`, нет «пока не умеешь». `agent_prompt` содержит `tell("<id>"`.
- [ ] **Step 2:** pytest — падает.
- [ ] **Step 3: код.**
  - Инструменты (описания для модели):
    - `tell` — «Передать агенту ответ или задание. agent — id из agents (или часть названия/проекта; пусто — агент в фокусе); text — полное задание или ответ от лица пользователя; gist — смысл для пользователя, несколько слов; option — номер варианта, если агент ждёт ответа с вариантами и ответ ровно один из них (у плана: 1 — одобрить), иначе 0». Схема — объект: `text` и `gist` обязательны.
    - `projects` — «Проекты, где можно запустить нового агента: название и папка».
    - `launch` — «Запустить нового агента Claude в проекте. path — папка из projects; task — полное задание; gist — смысл для пользователя».
  - Агента для `tell` ищет код: точный id, иначе единственный по части названия или проекта.
  - `OMA_PROMPT`: фраза «Ты только читаешь: ничего не меняешь и не передаёшь агентам; если просят передать или сделать — скажи, что пока не умеешь.» заменяется на:

    > Файлы ты только читаешь. Передать агенту ответ или задание — tell: разверни короткую просьбу в полное задание от лица пользователя (что сделать, где искать, что вернуть), фактов не выдумывай; ответ на вопрос агента передай полностью, с нужными деталями. Агент ждёт ответа с вариантами (в agents — «ждёт ответа: … Варианты: …») — option с номером варианта по порядку, если ответ ровно один из них, иначе свой ответ текстом. Новый агент — projects, затем launch с заданием. После tell или launch скажи пользователю одной фразой смысл и кому, не текст задания. Инструмент просит подтверждения — скажи пользователю ровно то, что он велит, и больше ничего не передавай.

    `agent_prompt` дописывает: `Реплика пользователя после вопроса этого агента — обычно ответ ему: tell("<id>", …); вопрос к тебе самой передавать не надо.`
  - `format_agents` — `+ (f'; ждёт ответа: {e["ask"]}' if e.get("ask") else "")`.
  - `Oma.act`, действие `send`:
    1. `orders.turn(conv.voiced, text)`.
    2. Голосом — `said = await orders.confirm(text)`.
    3. `said` есть — мозг не зовём. Пузырь «Ты» как обычно. После всех действий, как `failed`, `event(("delta", said))` и `event(("done", None))`: в списке действий за `send` ещё идёт `("state", "thinking")`.
    4. Иначе `brain.send(orders.note + ctx + text)`, `note` очищается.
  - `main()` создаёт один `Orders()` и отдаёт его `Brain` и `Oma`.
- [ ] **Step 4:** весь pytest — зелёный; `cargo test` и `node plugin/test.js` не задеты.
- [ ] **Step 5:** `git commit -m "мозг Омы: tell, projects, launch — задания и ответы агентам, запуск агента; подтверждение опасного мимо модели"`

### Task 6: Документация, установка, живая проверка

**Files:**
- Modify: `README.md`, `docs/superpowers/specs/2026-10-24-oma-foreman-design.md` (в «Задания агентам» — решение «запуск в Orca, проекты — worktree Orca»), `~/omarchy-dotfiles/docs/rules.md` (абзац `predmaxim.agent-speak`: Ома передаёт агентам задания и ответы и запускает агентов в Orca; опасное голосом — после «подтверждаю …»), `~/omarchy-dotfiles/docs/changelog.md`, этот план («Итог»), память `oma-foreman-stage0.md`.

- [ ] **Step 1: README.**
  - Новый раздел «Задания и ответы агентам»: что сказать, что Ома делает сама (разворачивает задание, говорит смысл и кому); меню вариантов и одобрение плана; опасное и фраза «подтверждаю …» (7 слов); набранное в окне — без подтверждения; запуск в проекте Orca; агенты вне Orca и herdr.
  - «Устройство» — `src/orders.rs`, `orders.py`, CLI `tell|launch|projects`, поле `ask`.
  - Строки о том, что Ома только читает, исправить.
- [ ] **Step 2: установка.** `./install.sh`, сервис перезапущен, `journalctl --user -u agent-speakd -n 50` без ошибок.
- [ ] **Step 3: живая проверка.** Классификатор Claude Code не даёт агенту запускать агентов с `--dangerously-skip-permissions`, поэтому пробные агенты — `claude --model haiku` без этого флага, в herdr через `env -i`, как в плане 2.
  1. **herdr, меню.** Сессия `omaprobe`, агент задаёт `AskUserQuestion`. Три пробы, каждая — `agent-speak tell <id> …`: вариант (`2`), свой ответ (`0 "Зелёный"`), два вопроса текстом. В транскрипте — нужный ответ. Затем `ExitPlanMode`: правка и одобрение.
  2. **Orca, текст и меню.** `orca terminal create --worktree path:<agent-speak> --title oma-probe --command 'claude --model haiku'` — агент задаёт вопрос, затем `agent-speak tell … 1` (цифра через `send --text`). Затем — обычное задание текстом. Терминал закрыть.
  3. **Мозг, набором.** В окне Омы набрать «передай агенту в omaprobe: скажи, сколько будет два плюс два». Агент получил развёрнутое задание, Ома ответила «Передала проекту …».
  4. **Запуск.** `agent-speak projects` — 18 папок. Запуск настоящего агента (флаги Orca) — попросить пользователя: `! agent-speak launch <папка agent-speak> "Скажи «привет» и закончи"`. Ожидание: вкладка в Orca, агент ответил.
  5. **Голосом — пользователь.** «Пусть агент в omaprobe запушит» → «Это пуш. Скажи: подтверждаю пуш» → «подтверждаю пуш» → в агенте задание. Ответ «нет» на его «запушить?» уходит сразу. В пробной папке нет remote, push безвреден.
  6. Остановить `omaprobe`, удалить сессию и `/tmp/omaprobe`.
- [ ] **Step 4:** «Итог» в этом плане, память проекта, `git commit` и `git push` вместе с документацией; в `~/omarchy-dotfiles` — свой коммит и push.
