# Ома-прораб, план 1: сжатие и приоритет речи — план реализации

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** агенты больше не звучат дословно: Ома пересказывает их ходы лёгкой моделью и говорит новости одной фразой по приоритету, после чего на 5 секунд слушает ответ.

**Architecture:** демон (Rust) по хукам собирает текст хода, вопроса и промежуточного; фоновые воркеры `digest` отдают его `claude -p --model haiku` и получают `{"say", "important"}`; доска `news` хранит пересказы и по правилам приоритета склеивает фразу, которая уходит в очередь речи новым видом `Kind::News`. Старая озвучка «по фокусу» (статусы, финалы, голоса окон, dedup) удаляется. Ома (Python) получает команды `warm` и `listen`.

**Tech Stack:** Rust (serde_json, notify — уже в Cargo.toml), Python 3 (voice-venv), QML/JS плагин.

**Spec:** `docs/superpowers/specs/2026-10-24-oma-foreman-design.md` — этот план покрывает разделы «Сжатие», «Приоритет» (кроме восстановления работы — план 2), «Микрофон после речи», решения про пересказ, настройки и сводную фразу. Сторож, herdr, действия, задания агентам — планы 2 и 3.

## Global Constraints

- Новых зависимостей нет (ни в `Cargo.toml`, ни в voice-venv, ни в плагине).
- Модель пересказа — `timeout 60 claude -p <промпт> --model haiku --no-session-persistence --setting-sources "" --strict-mcp-config --tools ""`, env `MAX_THINKING_TOKENS=0`, `CLAUDE_CODE_DISABLE_AUTO_MEMORY=1`, `AGENT_SPEAK=1` — ровно как сейчас в `src/learn.rs`.
- В речи нет имён переменных, функций, файлов, путей, команд: это правило промпта, а не постобработки.
- Тихий режим: ничего не звучит; новости, пришедшие в тишину, не копятся и не звучат после неё.
- После новостей Омы микрофон открывается на 5 секунд (`LISTEN_SECS = 5`).
- Комментарии в коде — по-русски, одна строка «почему», в стиле окружающего кода.
- Коммиты — без ИИ-соавторов и подписей «Generated with…».
- Проверка: `cargo test`; Ома — `~/.local/share/agent-speak/voice-venv/bin/python -m pytest python/test_agent_voice.py`; плагин — `node plugin/test.js`. `cargo build` без предупреждений о мёртвом коде.

## Review Focus

1. Модель ответила не JSON-ом, по-английски или с markdown → звучит запасная фраза, а не сырой текст агента (тест `digest_falls_back_on_prose`, задача 1).
2. Пять агентов закончили ход одновременно → одновременно идут не больше трёх процессов `claude` (тест `spawn_runs_at_most_three_at_once`, задача 1).
3. Ночь: новости тихого режима не прорываются утром пачкой (тест `quiet_digest_bubbles_but_never_speaks_later`, задача 5).
4. На вопрос ответили в терминале раньше, чем Ома его озвучила → вопрос не звучит (тесты `answered_drops_pending_question`, задача 2, и `prompt_submit_answers_question`, задача 5).
5. Пересказ промежуточного пришёл, когда ход уже закончился → выброшен, не звучит поверх итога (тест `stale_progress_digest_dropped`, задача 5).

---

### Task 1: Пересказ — модуль `digest`

**Files:**
- Create: `src/digest.rs`
- Modify: `src/learn.rs` (ветка `Agent::Claude` в `ask` → общая функция), `src/main.rs` (`mod digest;`)
- Test: `src/digest.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: —
- Produces:
  ```rust
  // src/learn.rs
  pub fn ask_claude(prompt: &str) -> Option<String> // лёгкий Claude, как сейчас в ask; ask зовёт её для Agent::Claude

  // src/digest.rs
  #[derive(Clone, Copy, Debug, PartialEq)]
  pub enum Ask { Progress, Final, Question }
  #[derive(Clone, Debug)]
  pub struct Job { pub session: String, pub project: String, pub ask: Ask, pub text: String,
                   pub context: String,            // последний промпт пользователя этой сессии
                   pub fallback: Option<String> }  // фраза без модели; None — без модели молчим
  #[derive(Clone, Debug, PartialEq)]
  pub struct Digest { pub session: String, pub project: String, pub ask: Ask, pub important: bool, pub say: String }
  pub const MAX_INPUT: usize = 6000; // символов текста агента в промпте — хвост
  pub const WORKERS: usize = 3;
  pub fn prompt(job: &Job) -> String;
  pub fn parse(out: &str) -> Option<(bool, String)>;               // (important, say)
  pub fn digest(job: Job, ask: impl Fn(&str) -> Option<String>) -> Option<Digest>;
  pub fn spawn(out: Sender<Digest>) -> Sender<Job>;               // = spawn_with(out, learn::ask_claude)
  pub fn spawn_with(out: Sender<Digest>, ask: fn(&str) -> Option<String>) -> Sender<Job>;
  ```

- [ ] **Step 1: Write the failing tests**

```rust
fn job(ask: Ask, text: &str, fallback: Option<&str>) -> Job {
    Job { session: "s".into(), project: "plug".into(), ask, text: text.into(), context: "почини орб".into(), fallback: fallback.map(String::from) }
}

#[test]
fn prompt_has_rules_context_and_tail() {
    let long = format!("{}КОНЕЦ", "а".repeat(MAX_INPUT * 2));
    let p = prompt(&job(Ask::Final, &long, None));
    assert!(p.contains("Не называй переменные"));
    assert!(p.contains("\"say\"") && p.contains("\"important\""));
    assert!(p.contains("почини орб"));
    assert!(p.contains("КОНЕЦ"));                       // хвост, а не начало
    assert!(p.chars().count() < MAX_INPUT + 2000);
    assert!(prompt(&job(Ask::Question, "x", None)).contains("ждёт ответа"));
    assert!(prompt(&job(Ask::Progress, "x", None)).contains("какой шаг закрыт"));
}

#[test]
fn parse_accepts_fenced_json_rejects_prose_and_empty() {
    assert_eq!(parse("```json\n{\"say\": \"Экран готов.\", \"important\": false}\n```"), Some((false, "Экран готов.".into())));
    assert_eq!(parse("{\"say\":\"Вопрос.\",\"important\":true}"), Some((true, "Вопрос.".into())));
    assert_eq!(parse("{\"say\":\"Без важности.\"}"), Some((false, "Без важности.".into())));
    assert_eq!(parse("Sure! Here is the summary."), None);
    assert_eq!(parse("{\"say\": \"  \", \"important\": true}"), None);
}

#[test]
fn digest_uses_model_answer() {
    let d = digest(job(Ask::Final, "Сделал.", Some("готово")), |_| Some("{\"say\":\"Фича готова.\",\"important\":true}".into())).unwrap();
    assert_eq!(d, Digest { session: "s".into(), project: "plug".into(), ask: Ask::Final, important: true, say: "Фича готова.".into() });
}

#[test]
fn digest_falls_back_on_prose() {
    let d = digest(job(Ask::Question, "Удалить?", Some("Клод задал вопрос")), |_| Some("Sure!".into())).unwrap();
    assert_eq!((d.say.as_str(), d.important), ("Клод задал вопрос", true)); // вопрос без модели — всё равно важен
    let d = digest(job(Ask::Final, "Сделал.", Some("готово")), |_| None).unwrap();
    assert_eq!((d.say.as_str(), d.important), ("готово", false));
    assert_eq!(digest(job(Ask::Progress, "шаг", None), |_| None), None);
    assert_eq!(digest(job(Ask::Final, "  ", Some("готово")), |_| panic!("пустой текст — модель не зовём")).unwrap().say, "готово");
}

#[test]
fn spawn_runs_at_most_three_at_once() {
    static NOW: AtomicUsize = AtomicUsize::new(0);
    static MAX: AtomicUsize = AtomicUsize::new(0);
    fn slow(_: &str) -> Option<String> {
        let n = NOW.fetch_add(1, Ordering::SeqCst) + 1;
        MAX.fetch_max(n, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(50));
        NOW.fetch_sub(1, Ordering::SeqCst);
        Some("{\"say\":\"ок\"}".into())
    }
    let (tx, rx) = mpsc::channel();
    let jobs = spawn_with(tx, slow);
    for _ in 0..6 { jobs.send(job(Ask::Final, "т", None)).unwrap(); }
    for _ in 0..6 { rx.recv_timeout(Duration::from_secs(5)).unwrap(); }
    assert!(MAX.load(Ordering::SeqCst) <= WORKERS);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test digest::`
Expected: FAIL — модуля `digest` нет.

- [ ] **Step 3: Extract `ask_claude` in `src/learn.rs`**

Тело ветки `Agent::Claude` из `ask` плюс общий хвост (`AGENT_SPEAK=1`, `stdin`/`stderr` в null, успех → stdout) переезжают в `pub fn ask_claude(prompt: &str) -> Option<String>`; `ask` для Claude зовёт её. Поведение `learn` не меняется.

- [ ] **Step 4: Implement `src/digest.rs`**

Промпт — ровно этот текст (общая часть, затем часть вида, затем данные):

```text
Ты — Ома, помощница, которая пересказывает пользователю голосом работу ИИ-агента. Пиши по-русски, коротко, простыми словами. Не называй переменные, функции, классы, файлы, пути, команды и код — только смысл: что сделано, каким способом в общих словах, что дальше. Без markdown и списков. Ответ — строго одна строка JSON: {"say": "…", "important": true или false}.
```

- `Final`: `Агент закончил ход. Одной-двумя фразами: что сделано и что требует внимания пользователя — оставшиеся проблемы, советы агента, что не запушено или не проверено. important — true, если пользователю нужно что-то решить или сделать.`
- `Question`: `Агент ждёт ответа пользователя. Перескажи суть вопроса общими словами и совет агента, если он есть. Если вопрос важный (архитектура, данные, деньги, необратимые действия), добавь в общих словах, как агент предлагает это сделать. important — true.`
- `Progress`: `Агент ещё работает; ниже его сообщения за последнюю минуту. Одной фразой: какой шаг закрыт и что дальше. important — true, только если пользователю стоит узнать это сейчас: проблема, смена плана, неожиданный результат.`
- Данные: `\n\nЗадача пользователя: {context}\n\nТекст агента:\n{последние MAX_INPUT символов text}`.

`parse`: от первой `{` до последней `}`, `serde_json`; `say` — непустая после trim строка, `important` — bool, по умолчанию false. `digest`: пустой текст или неудача модели/разбора → `fallback` (important = `ask == Ask::Question`), без `fallback` — `None`. `spawn_with`: `WORKERS` потоков делят `Arc<Mutex<Receiver<Job>>>`, результат `Some` — в `out`.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test digest:: && cargo test learn::`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src/digest.rs src/learn.rs src/main.rs
git commit -m "digest: пересказ хода агента лёгкой моделью, запасная фраза без модели"
```

---

### Task 2: Доска новостей — модуль `news`

**Files:**
- Create: `src/news.rs`
- Modify: `src/main.rs` (`mod news;`)
- Test: `src/news.rs`

**Interfaces:**
- Consumes: —
- Produces:
  ```rust
  #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
  pub enum Class { Restore, Question, Final, Progress } // порядок — приоритет; Restore наполнит план 2
  #[derive(Clone, Debug)]
  pub struct News { pub session: String, pub class: Class, pub say: String, pub born: u64 } // say — уже с «проект: »
  pub const ANSWER_WAIT: u64 = 120; // с: после озвученного вопроса итоги ждут ответа не дольше
  #[derive(Default)]
  pub struct Board { /* items: Vec<News>, asked: Option<(String, u64)> */ }
  impl Board {
      pub fn push(&mut self, n: News);                       // та же сессия и класс — новое вместо старого
      pub fn answered(&mut self, session: &str);             // пользователь ответил: снять ожидание и вопросы сессии
      pub fn retain_sessions(&mut self, alive: impl Fn(&str) -> bool);
      pub fn retain_classes(&mut self, keep: &[Class]);
      pub fn take(&mut self, now: u64) -> Option<String>;    // фраза следующей пачки
  }
  pub fn more_questions(n: usize) -> String;
  ```

- [ ] **Step 1: Write the failing tests**

```rust
fn n(session: &str, class: Class, say: &str, born: u64) -> News { News { session: session.into(), class, say: say.into(), born } }

#[test]
fn five_agents_example() {
    let mut b = Board::default();
    b.push(n("e", Class::Progress, "E: нашёл утечку.", 1));
    b.push(n("d", Class::Final, "D: фича готова.", 2));
    b.push(n("c", Class::Question, "C: что делаем с мёртвым кодом?", 3));
    b.push(n("a", Class::Restore, "A: агент повис — перезапустила.", 4));
    b.push(n("b", Class::Restore, "B: сабагент повис — перезапустила.", 5));
    assert_eq!(b.take(10).unwrap(), "A: агент повис — перезапустила. B: сабагент повис — перезапустила. C: что делаем с мёртвым кодом?");
    assert_eq!(b.take(20), None);                 // ждём ответа на вопрос
    b.answered("c");
    assert_eq!(b.take(21).unwrap(), "D: фича готова.");
    assert_eq!(b.take(22).unwrap(), "E: нашёл утечку."); // промежуточное — когда больше нечего
    assert_eq!(b.take(23), None);
}

#[test]
fn questions_one_at_a_time_oldest_first_with_count() {
    let mut b = Board::default();
    b.push(n("y", Class::Question, "Y?", 2));
    b.push(n("x", Class::Question, "X?", 1));
    b.push(n("z", Class::Question, "Z?", 3));
    assert_eq!(b.take(10).unwrap(), "X? После этого ещё 2 вопроса.");
    b.answered("x");
    assert_eq!(b.take(11).unwrap(), "Y? После этого ещё один вопрос.");
}

#[test]
fn answer_wait_expires() {
    let mut b = Board::default();
    b.push(n("c", Class::Question, "C?", 1));
    b.push(n("d", Class::Final, "D готов.", 1));
    assert_eq!(b.take(10).unwrap(), "C?");
    assert_eq!(b.take(10 + ANSWER_WAIT - 1), None);
    assert_eq!(b.take(10 + ANSWER_WAIT).unwrap(), "D готов.");
}

#[test]
fn answered_drops_pending_question() {
    let mut b = Board::default();
    b.push(n("c", Class::Question, "C?", 1));
    b.answered("c");                               // ответили в терминале раньше Омы
    assert_eq!(b.take(10), None);
}

#[test]
fn push_replaces_same_session_and_class() {
    let mut b = Board::default();
    b.push(n("a", Class::Progress, "старое", 1));
    b.push(n("a", Class::Progress, "новое", 2));
    assert_eq!(b.take(10).unwrap(), "новое");
}

#[test]
fn retain_drops_gone_sessions_and_muted_classes() {
    let mut b = Board::default();
    b.push(n("gone", Class::Final, "G.", 1));
    b.push(n("a", Class::Final, "A.", 1));
    b.push(n("q", Class::Question, "Q?", 1));
    b.retain_sessions(|s| s != "gone");
    b.retain_classes(&[Class::Restore, Class::Question]); // agents_voice выключен
    assert_eq!(b.take(10).unwrap(), "Q?");
    b.answered("q");
    assert_eq!(b.take(11), None);
}

#[test]
fn more_questions_plural() {
    assert_eq!(more_questions(1), "После этого ещё один вопрос.");
    assert_eq!(more_questions(2), "После этого ещё 2 вопроса.");
    assert_eq!(more_questions(5), "После этого ещё 5 вопросов.");
    assert_eq!(more_questions(11), "После этого ещё 11 вопросов.");
    assert_eq!(more_questions(21), "После этого ещё 21 вопрос.");
    assert_eq!(more_questions(22), "После этого ещё 22 вопроса.");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test news::`
Expected: FAIL — модуля `news` нет.

- [ ] **Step 3: Implement `src/news.rs`**

`take(now)`:

```text
waiting = asked есть и now < asked.1 + ANSWER_WAIT; иначе asked = None
parts = все Restore по born
если !waiting и есть Question: самый старый по born → в parts; остальных вопросов k > 0 → more_questions(k); asked = (его сессия, now); waiting = true
если !waiting: parts += все Final по born; если parts пусто — parts += все Progress по born
parts пусто → None, иначе parts.join(" ")
```

`answered(s)`: если `asked` — эта сессия, снять; убрать Question этой сессии.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test news::`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/news.rs src/main.rs
git commit -m "news: доска новостей Омы — приоритет, вопросы по одному, ожидание ответа"
```

---

### Task 3: Убрать дословную озвучку агентов

Агенты перестают звучать сами: уходят статусы и финалы в очереди, активное окно по фокусу, голоса окон, `dedup`, настройки `read_intermediate` и `max_age_secs`, сырые пузыри статусов и «готово». Лента агента в окне, вопрос с `Kind::Urgent`, ручное чтение, образец голоса и ответы Омы остаются как есть.

**Files:**
- Modify: `src/queue.rs`, `src/speaker.rs`, `src/daemon.rs`, `src/config.rs`, `src/main.rs`
- Delete: `src/dedup.rs`
- Modify: `plugin/Model.js`, `plugin/I18n.js`, `plugin/test.js`, `README.md` (таблица настроек)

**Interfaces:**
- Consumes: —
- Produces (очередь после задачи):
  ```rust
  pub enum Kind { Urgent, Read, Preview, Voice }
  pub struct Item { pub session: String, pub text: String, pub kind: Kind }
  impl Queue {
      pub fn new() -> Queue;
      pub fn push(&mut self, item: Item);        // Preview — один, впереди; Urgent — за срочными; прочее — в конец
      pub fn push_front(&mut self, item: Item);  // как сейчас: за образцом
      pub fn set_hold(&mut self, on: bool);
      pub fn held(&self) -> bool;                 // больше не только для тестов
      pub fn retain_audible(&mut self, quiet: bool);
      pub fn pop(&mut self) -> Option<Item>;     // на паузе — только Preview; на удержании — не Urgent
      pub fn ready(&self) -> bool;
      pub fn pause(&mut self); pub fn resume(&mut self); pub fn paused(&self) -> bool;
      pub fn clear(&mut self);                   // agent-speak stop: всё
      pub fn hush(&mut self);                    // Ому перебили: выбросить Voice
  }
  // src/daemon.rs
  pub fn audible(kind: &Kind, quiet: bool) -> bool; // Read, Preview — всегда; иначе — не в тишине
  ```
  Из `State` уходят `dedup`, `active`, `candidate`, `voices`; из `Status` (строка состояния) — `read_intermediate`. `focused()` остаётся: он нужен пузырям и `registry.set_focused`.

- [ ] **Step 1: Write the failing tests**

В `src/daemon.rs`:

```rust
#[test]
fn agent_text_is_not_spoken_but_reaches_feed() {
    let mut st = test_state();
    st.cfg.agents_voice = true;
    // агент «s» — в реестре (как в соседних тестах реестра)
    md(&mut st, "s", "Делаю экран настроек. ");
    st.on_hook("stop", &serde_json::json!({"session_id": "s", "transcript_path": "/nonexistent", "last_assistant_message": "Готово."}));
    assert!(texts(&st).is_empty());
    assert!(st.registry.get("s").unwrap().feed.iter().any(|l| l.text.contains("Делаю экран настроек")));
}
```

В `src/config.rs`:

```rust
#[test]
fn old_config_keys_ignored() {
    let c: Config = toml::from_str("read_intermediate = false\nmax_age_secs = 5\nspeaker = \"baya\"\n").unwrap();
    assert_eq!(c.speaker, "baya");
}
```
(Если `Config` грузится иначе, чем `toml::from_str`, тест повторяет путь `Config::load`.)

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test agent_text_is_not_spoken old_config_keys_ignored`
Expected: FAIL — финал попадает в очередь; поля ещё есть (второй тест может пройти сразу — тогда он фиксирует совместимость).

- [ ] **Step 3: Remove the live-speech path**

- `queue.rs` — к API выше: уходят `Status`, `Manual`, `FINAL_MAX_AGE`, `max_age`, `born`, `msg`, `speaker`, `active`, `started`, `popped`, `paused_total`, `fill`, `new_turn`, `set_active`, `has_final`, `popped_final`, `reading`, `clear_active`, `clear_session`. Тесты очереди — под новый API; тесты устаревания, фокуса и `fill` удаляются.
- `speaker.rs` — голос всегда из `shared.voice`; `pop()` без времени.
- `daemon.rs` — `message-display` и строки Codex пишут только в реестр (`registry.status`); `stop`/`codex-notify` — только `registry.finished` + `registry.reload`; `notification` — как сейчас (`Kind::Urgent`, `registry.question`, пузырь вопроса). Уходят `poll_focus`/`set_active`/`is_active`/`reading`/`final_pending`/`auto`/`live`/`voice_of`/`reassign_default`/`final_answer`/`enqueue_new`, параметр `only_new`, пузыри статусов и «готово» (`turn_done` → прямой `registry.finished`). `read()` больше не трогает активную сессию. Хелперы тестов `env_state`/`at`/`active` уходят вместе с `poll_focus` (или сводятся к подмене `clock`, если она ещё нужна оставшимся тестам). Тесты статусов, финалов, фокуса, голосов и dedup удаляются; тесты тишины, срочного, чтения, образца, Омы, подписчиков — остаются под новый API.
- `config.rs` — без `read_intermediate` и `max_age_secs` (в `set`, по умолчанию, тестах).
- Плагин: из `Model.js` — `read_intermediate` в `OFFLINE`, строка настроек и `SETTINGS_ROWS`; из `I18n.js` — `INTERMEDIATE STATUSES`; `test.js` — те же проверки на `speaker`.
- `README.md` — из таблицы настроек строки `read_intermediate` и `max_age_secs`.

- [ ] **Step 4: Run all checks**

Run: `cargo build 2>&1 | grep -c warning; cargo test && node plugin/test.js`
Expected: `0`, затем все тесты PASS.

- [ ] **Step 5: Commit**

```bash
git add -A src plugin README.md
git commit -m "агенты больше не звучат дословно: без статусов, финалов, фокуса и голосов окон"
```

---

### Task 4: Ома — окно ответа после новостей

**Files:**
- Modify: `python/agent_voice/conversation.py`, `python/agent_voice/__main__.py`
- Test: `python/test_agent_voice.py`

**Interfaces:**
- Consumes: —
- Produces: команды Оме от демона `{"do": "warm"}` (загрузить whisper заранее) и `{"do": "listen", "secs": 5}`; `Conversation.listen(secs: float) -> list`.

- [ ] **Step 1: Write the failing tests**

```python
def test_listen_window_ends_without_speech():
    clk, cv = _conv()
    assert cv.listen(5) == [("ear", True), ("state", "listening")] and cv.active
    clk.t += 4.9; assert cv.tick() == []
    clk.t += 0.2; assert cv.tick() == [("ear", False), ("state", "idle")]


def test_listen_window_speech_becomes_conversation():
    clk, cv = _conv()
    cv.listen(5); clk.t += 3
    cv.speech_start(1); cv.phrase("да, удаляй", 1)
    clk.t += SEND_PAUSE
    assert ("send", "да, удаляй") in cv.tick()


def test_listen_noop_when_quiet_mic_busy_or_talking():
    clk, cv = _conv()
    cv.set_quiet(True); assert cv.listen(5) == []
    cv.set_quiet(False); cv.mic_busy(True); assert cv.listen(5) == []
    cv.mic_busy(False); cv.talk(); assert cv.listen(5) == []
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `~/.local/share/agent-speak/voice-venv/bin/python -m pytest python/test_agent_voice.py -k listen -v`
Expected: FAIL — `listen` нет.

- [ ] **Step 3: Implement**

`Conversation.listen(secs)`: идёт разговор, тихий режим или занят микрофон → `[]`; иначе `active = True`, `[("ear", True)] + self._to("listening")`, затем `quiet_at = clock() + secs` (после `_to`, который ставит `SILENCE_END`). Дальше работает обычная машина: `speech_start` снимает срок, фраза ставит `SILENCE_END`.
`__main__.command`: `"listen"` → если `await asyncio.to_thread(locked)` — ничего, иначе `await self.act(c.listen(float(cmd.get("secs", 5))))`; `"warm"` → `self.warm()`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `~/.local/share/agent-speak/voice-venv/bin/python -m pytest python/test_agent_voice.py`
Expected: PASS (все).

- [ ] **Step 5: Commit**

```bash
git add python/agent_voice/conversation.py python/agent_voice/__main__.py python/test_agent_voice.py
git commit -m "Ома: окно ответа на 5 секунд после новостей, заранее загруженный слух"
```

---

### Task 5: Демон — пересказ, доска и речь новостей

**Files:**
- Modify: `src/daemon.rs`, `src/queue.rs` (`Kind::News`), `src/agents.rs` (`summary`, `last_prompt`)
- Modify: `README.md`, `~/omarchy-dotfiles/docs/changelog.md` (сначала прочитать `~/omarchy-dotfiles/docs/rules.md`)
- Test: `src/daemon.rs`, `src/agents.rs`

**Interfaces:**
- Consumes: `digest::{Ask, Job, Digest, spawn}` (задача 1); `news::{Board, News, Class}` (задача 2); очередь задачи 3; команды Омы `warm`/`listen` (задача 4).
- Produces:
  ```rust
  // src/queue.rs
  pub enum Kind { Read, Preview, Voice, News } // News заменяет Urgent: голос Омы, держится hold, вперёд Voice не лезет, звучит не в тишине
  // src/agents.rs
  impl Registry {
      pub fn summary(&mut self, id: &str, text: &str) -> bool; // строка «что сейчас» в окне, без строки ленты
      pub fn last_prompt(&self, id: &str) -> String;           // последняя строка ленты вида prompt, иначе ""
  }
  // src/daemon.rs
  const PROGRESS_EVERY: Duration = Duration::from_secs(60);
  const LISTEN_SECS: u64 = 5;
  enum Event { …, Digest(Digest) }
  // State: digest: Sender<Job>, board: Board, progress: HashMap<String, (String, Instant)>, listen_after: bool
  fn ask(&mut self, session: &str, ask: Ask, text: &str, fallback: Option<String>); // сессии не из реестра — пропуск
  fn on_digest(&mut self, d: Digest);
  fn speak_news(&mut self);   // из tick
  fn after_speech(&mut self); // из Event::Busy
  ```

Поведение:
- `stop`, `codex-notify`: как после задачи 3, плюс `progress.remove(session)` и `ask(Final, last, Some("готово"))`.
- `notification` (кроме `""` и `Claude is waiting for your input`): `registry.question` как сейчас; **вместо** `Kind::Urgent` и сырого пузыря — `ask(Question, last_question или msg, Some(msg))`, где `msg` — нынешний перевод `notification_text`. `Kind::Urgent` больше никто не ставит — удалить его из очереди, `audible` и тестов (готовые фразы сторожа в плане 2 пойдут через доску).
- `message-display`, строки Codex: кроме `registry.status` — дописать предложение в `progress[session]` (время — первого предложения).
- `user-prompt-submit`: `board.answered(session)`, `progress.remove(session)`.
- `tick`: накопленное старше `PROGRESS_EVERY` → `ask(Progress, текст, None)`; затем `speak_news()`.
- `ask`: `Job { project: entry.project, context: registry.last_prompt(..), .. }` → `self.digest`.
- `on_digest`: сессии нет в реестре → выбросить; `Progress`, а агент уже не `Working` → выбросить; иначе `registry.summary`. Пузырь (текст — `say` без проекта, `who` — заголовок агента): `Final` → `"final"`, `Question` → `"question"`, `Progress` — только `important`, `"status"`. На доску (`say` = `"{project}: {say}"`, `born` = `now_secs`) — только не в тишине: `Question` — всегда; `Final` — при `agents_voice`; `Progress` — при `agents_voice` и `important`.
- `speak_news`: не тишина, не `busy`, `!queue.ready()`, `!queue.held()` → `board.take(now_secs)` → `enqueue(.., "news", &text, Kind::News)`, `oma_send({"do":"warm"})`, `listen_after = true`.
- `after_speech`: `listen_after`, не `busy`, `!queue.ready()` → `listen_after = false`; не тишина → `oma_send({"do":"listen","secs":LISTEN_SECS})`.
- `apply()`: `agents_voice` выключен → `board.retain_classes(&[Restore, Question])`; тишина → `board.retain_classes(&[])`.
- `rescan`: `board.retain_sessions` и `progress.retain` — по реестру.

- [ ] **Step 1: Write the failing tests**

Фикстура: в `test_state()` — `digest: mpsc::channel().0`; тесты, которым нужны задания, подменяют `st.digest` своим каналом и читают `rx.try_recv()`. Агент «s» с проектом `plug` добавляется в реестр так же, как в соседних тестах реестра (`scan`/`Live`). Время — через `env_state()`; секунды реестра — `now_secs`.

```rust
#[test]
fn stop_asks_final_digest_with_last_prompt() {
    // промпт «сделай фичу», затем stop с last_assistant_message «Сделал фичу.»
    let job = rx.try_recv().unwrap();
    assert_eq!((job.ask, job.text.as_str(), job.context.as_str(), job.fallback.as_deref()), (Ask::Final, "Сделал фичу.", "сделай фичу", Some("готово")));
}

#[test]
fn final_digest_bubbles_then_speaks_when_idle() {
    st.on_digest(Digest { session: "s".into(), project: "plug".into(), ask: Ask::Final, important: false, say: "Фича готова.".into() });
    // подписчик получил {"bubble":{… "kind":"final", "text":"Фича готова." …}}
    st.tick();
    assert_eq!(pop1(&st), "plug: Фича готова."); // текст после prepare — сравнивать так же, как соседние тесты
}

#[test]
fn question_notification_asks_digest_not_urgent() {
    st.on_hook("notification", &json!({"session_id": "s", "message": "Claude needs your permission to use Bash", "transcript_path": "/nonexistent", "cwd": "/x/plug"}));
    let job = rx.try_recv().unwrap();
    assert_eq!(job.ask, Ask::Question);
    assert_eq!(job.fallback.as_deref(), Some("Клоду нужно разрешение на команду"));
    assert!(texts(&st).is_empty());
}

#[test]
fn agents_voice_off_speaks_questions_not_finals() { /* agents_voice=false; digest Final и Question → tick → звучит только вопрос */ }

#[test]
fn quiet_digest_bubbles_but_never_speaks_later() {
    // тишина: digest Final и Question → пузыри есть; тишина снята → tick → очередь пуста
}

#[test]
fn progress_flushes_after_a_minute_and_stop_drops_it() {
    // md «Шаг один. » → +59 с tick → заданий нет; +2 с tick → Progress с текстом «Шаг один.»
    // md «Шаг два. » → stop → задание Final, Progress нет
}

#[test]
fn unimportant_progress_only_updates_summary() { /* digest Progress important=false → нет пузыря, доска пуста, entry.status == say */ }

#[test]
fn stale_progress_digest_dropped() { /* после stop пришёл digest Progress important=true → нет пузыря, нет речи */ }

#[test]
fn prompt_submit_answers_question() {
    // digest Question(s) и Final(t) → tick: звучит вопрос; pop; tick → тишина;
    // user-prompt-submit от s → tick → звучит итог t
}

#[test]
fn news_waits_while_oma_talks() { /* hold (voice_state thinking) → tick → очередь пуста; idle → tick → новость */ }

#[test]
fn listen_after_news_played() {
    // Ома — заглушка, как в oma_actions_go_to_child_stdin; tick с новостью → в stdin {"do":"warm"};
    // pop, busy=false → after_speech → в stdin {"do":"listen","secs":5}; второй after_speech — ничего
}
```

В `src/agents.rs`: `summary_sets_status_without_feed_line`, `last_prompt_is_latest_prompt_line`.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test daemon:: agents::`
Expected: FAIL — нет `on_digest`, `Kind::News`, `summary`.

- [ ] **Step 3: Implement the wiring above**

`run()`: `let (dtx, drx) = mpsc::channel(); let digest = digest::spawn(dtx);` и поток-пересыльщик `drx → Event::Digest`, как у `Busy`. `Event::Busy` → `st.broadcast(); st.after_speech();`.

- [ ] **Step 4: Run all checks**

Run: `cargo build 2>&1 | grep -c warning; cargo test && node plugin/test.js && ~/.local/share/agent-speak/voice-venv/bin/python -m pytest python/test_agent_voice.py`
Expected: `0`, затем все PASS.

- [ ] **Step 5: Docs**

`README.md`: раздел «Как звучат агенты» — заново: Ома пересказывает (итог, вопрос, важное промежуточное), приоритет и вопросы по одному с «ещё N», окно ответа 5 с, тишина; «Пузыри» — пересказы вместо сырых статусов; «Устройство» — `digest.rs` (Haiku, 3 воркера, запасные фразы), `news.rs`, команды Оме `warm`/`listen`. Прочитать `~/omarchy-dotfiles/docs/rules.md`, дописать запись в `~/omarchy-dotfiles/docs/changelog.md` в его формате.

- [ ] **Step 6: Live check**

Run: `./install.sh && systemctl --user restart agent-speakd`, затем в Claude-сессии дать короткое задание и смотреть `journalctl --user -u agent-speakd -f`.
Expected: после конца хода — пузырь с пересказом без имён файлов; при `agents_voice` — фраза «<проект>: …» голосом Омы и орб «слушаю» на 5 секунд.

- [ ] **Step 7: Commit**

```bash
git add src README.md
git commit -m "Ома пересказывает агентов: доска новостей, речь по приоритету, окно ответа"
git -C ~/omarchy-dotfiles add docs/changelog.md && git -C ~/omarchy-dotfiles commit -m "agent-speak: Ома пересказывает агентов"
```
