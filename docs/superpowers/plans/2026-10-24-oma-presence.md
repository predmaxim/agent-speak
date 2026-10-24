# Ома видит, слушает ли пользователь — план реализации

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ома не говорит в пустоту: знает, слушает ли пользователь (ввод, блокировка, звонок, видео, наушники, его слова), копит новости, пока его нет, и вводит в курс, когда он вернулся; «Ок» после новости закрывает микрофон; речь — с верными ударениями; конец хода, пока агент ждёт своих фоновых помощников, не пересказывается как «ждёт тебя».

**Architecture:** демон (Rust) держит состояние присутствия (`src/presence.rs`) рядом с доской новостей (`src/news.rs`) и по нему решает, говорить ли сейчас, спросить «Ты тут?» или копить. Сигналы приходят новыми сообщениями сокета: простой ввода и MPRIS — от плагина (Quickshell `IdleMonitor`, `Mpris`), блокировка, чужой микрофон и наушники — от Омы (Python), смысл фраз «не сейчас / я отойду / я тут» — от мозга Омы инструментом `presence`. Закрывающие «Ок» ловит код разговора (`conversation.py`).

**Tech Stack:** Rust (serde_json — уже в Cargo.toml), Python 3 (voice-venv), QML/JS плагин (Quickshell 0.3.2), OpenFreebuds (системная программа из AUR, необязательная — за адаптером).

**Spec:** `docs/superpowers/specs/2026-10-24-oma-presence-design.md`

## Global Constraints

- Новых зависимостей нет ни в `Cargo.toml`, ни в voice-venv, ни в плагине. OpenFreebuds — внешняя программа; нет её — «датчика нет», всё остальное работает.
- Пороги — константы в коде, без настроек: «не уверена» 90 с простоя, «нет на месте» 10 мин, отсутствие для приветствия и повтора — от 3 мин, «не сейчас» — 15 мин, «я отойду» — 60 с ввод не считается возвращением, сводка Haiku — итогов больше 3, «возможно, не услышано» — нет ввода от 60 с до начала речи.
- Нет сигнала от плагина (не прислал, молчит дольше 150 с) — поведение как до этого плана.
- Тихий режим: ничего не звучит; пришедшее в тишину не копится; накопленное до тишины, пока пользователя нет, ждёт утра.
- В речи нет имён переменных, функций, файлов, путей, команд.
- Комментарии: в Rust и Python — по-русски, в QML и JS плагина — по-английски, как в окружающем коде; одна строка «почему».
- Коммиты — без ИИ-соавторов и подписей «Generated with…».
- Проверка: `cargo test`; Ома — `~/.local/share/agent-speak/voice-venv/bin/python -m pytest python/test_agent_voice.py`; плагин — `node plugin/test.js`. `cargo build` без предупреждений о мёртвом коде.

## Review Focus

1. На вопрос ответили в терминале, пока пользователя не было → на возвращении он не повторяется (тест `answered_drops_spoken_question`, задача 3).
2. Агент закрыт, пока пользователя не было → его новости не повторяются (тест `closed_session_not_replayed`, задача 3).
3. Пока пользователя не было, тот же агент закончил ещё раз → повтор старого итога не вытесняет новый (тест `replay_never_overrides_newer_news`, задача 3).
4. Ноутбук спал час, открыли крышку и шевельнули мышью → это возвращение со сводкой, а не «Ты тут?» в пустую комнату до сна (тест `wake_from_sleep_is_return`, задача 2).
5. Оболочка перезапущена или упала посреди простоя → Ома не замолкает навсегда: устаревший сигнал — как «слушает» (тест `stale_signal_is_here`, задача 2).

---

### Task 0: Пересказ не приписывает пользователю ожидание агента

Не из спеки — ошибка, о которой сообщил пользователь (2026-10-24). Агент-дирижёр (рабочий проект, сессия `xxxxxxxx…`) кончает ход фразой «Жду ревью 7.» или «Жду правки обеих спек и исполнителей партии 1.», а ждёт он своих фоновых помощников. Стоп-хук отдаёт это на пересказ итога, промпт итога просит «что требует внимания пользователя», и Haiku говорит «агент ждёт твоего ревью». Исправление: пока помощники агента в фоне, конец хода — не итог: модель не зовётся, ничего не звучит, в строке статуса — сам текст агента, агент «работает». Промпт тоже не даёт приписывать пользователю чужое ожидание.

**Files:**
- Modify: `src/source/claude.rs`, `src/daemon.rs` (ветка `"stop"` в `on_hook`), `src/digest.rs` (`COMMON`)
- Test: `src/source/claude.rs`, `src/daemon.rs`, `src/digest.rs`

**Interfaces:**
- Produces: `pub fn pending_background(content: &str) -> usize` в `src/source/claude.rs` — сколько фоновых задач агента запущено и ещё не отчиталось, по хвосту транскрипта (`tail_of`).

Форматы в транскрипте Claude Code (проверены на сессии `xxxxxxxx…`):
- запуск помощника — `tool_result` с текстом `Async agent launched successfully.` и строкой `agentId: <id>`;
- фоновая команда — `tool_result` с `running in background with ID: <id>`;
- продолжение помощника (`SendMessage`) — `tool_result` с JSON `"resumedAgentId":"<id>"`;
- отчёт — реплика `user` с `<task-notification>`, в ней `<task-id><id></task-id>` и `<status>completed|failed|killed</status>`.

Строки идут по порядку: запуск или продолжение добавляет id, отчёт с конечным статусом — снимает.

Ветка `"stop"`: `pending_background(&tail_of(transcript)) > 0` → `registry.status(&session, &last, now)` вместо `finished`, без `ask(.., Ask::Final, ..)`; `asm.flush_session`, `reload`, `board.answered`, `progress.remove` — как сейчас. Комментарий: `shortcut: вопрос текстом в конце такого хода не звучит (AskUserQuestion — звучит), научить, если начнёт теряться`.

В `COMMON` дописать (дословно): «Кого или чего ждёт агент — бери из его текста. Что агент ждёт пользователя, говори, только если агент прямо обращается к пользователю: просит ответить, решить, проверить. «Жду ревью», «жду исполнителей» без обращения — агент ждёт своих помощников. Не приписывай пользователю действий, о которых агент не просит.»

- [ ] **Step 1: Write the failing tests**

```rust
// src/source/claude.rs — строки транскрипта собрать в тесте по форматам выше
#[test] fn pending_counts_launched_minus_reported() { /* 2 запуска агента + 1 фоновая команда, 1 отчёт → 2 */ }
#[test] fn resumed_helper_pending_again() { /* запуск, отчёт, resumedAgentId, → 1 */ }
#[test] fn no_background_is_zero() { /* обычный ход → 0 */ }
// src/daemon.rs
#[test] fn stop_with_pending_helpers_is_not_a_final() {
    // транскрипт во временной папке: запуск помощника без отчёта; stop("Жду ревью 7.") →
    // rx.try_recv() пусто (модель не звана), registry.get(s).state == Working, status == "Жду ревью 7."
}
// src/digest.rs
#[test] fn prompt_forbids_inventing_user_wait() { /* prompt(&job) содержит «Не приписывай пользователю действий» */ }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test pending stop_with_pending prompt_forbids`
Expected: FAIL — `pending_background` не найден.

- [ ] **Step 3: Implement** по интерфейсу выше.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test`
Expected: PASS.

- [ ] **Step 5: Живая проба промпта.** Тест `#[ignore] probe_waiting_for_helpers` в `src/digest.rs`: три прогона `digest(job, learn::ask_claude)` по каждому тексту — `Ask::Final`, текст «Исполнитель на проверке запущен. Лишний временный файл удалён. Жду ревью 7.» и «Жду правки обеих спек и исполнителей партии 1.»; печатает `say`. Run: `cargo test probe_waiting_for_helpers -- --ignored --nocapture`. Expected: ни в одном из шести ответов нет «твоего», «твоих», «тебя», «ты» как того, кого ждут. Иначе — уточнить фразу в `COMMON` и прогнать снова; итог проб записать в сообщение коммита.

- [ ] **Step 6: Commit**

```bash
git add src/source/claude.rs src/daemon.rs src/digest.rs
git commit -m "Ома: конец хода при фоновых помощниках — не итог; пересказ не приписывает пользователю ожидание агента"
```

---

### Task 1: Проверка на живой системе

Без кода в репозитории: результат — раздел «Результаты проверок» в конце спеки. Нужен пользователь (поставить пакет, вынуть наушник, пошевелить мышью).

**Files:**
- Modify: `docs/superpowers/specs/2026-10-24-oma-presence-design.md` (новый раздел в конце)

- [ ] **Step 1: `IdleMonitor`.** Записать `/tmp/idle.qml`:

```qml
import QtQuick
import Quickshell
import Quickshell.Wayland
ShellRoot { IdleMonitor { timeout: 5; respectInhibitors: false; onIsIdleChanged: console.log("idle", isIdle, Date.now()) } }
```

Запустить `quickshell -p /tmp/idle.qml` в фоне, попросить пользователя: 10 с не трогать ничего, потом двинуть мышью, ещё 10 с покоя, нажать клавишу. Ожидаемо: `idle true` через ~5 с покоя, `idle false` и на мышь, и на клавишу. Ещё раз — с видео во весь экран в Chromium: `idle true` всё равно приходит (запрет на сон не учитывается).

- [ ] **Step 2: MPRIS.** `busctl --user list | grep mpris` и `quickshell` с `Mpris.players.values.map(p => p.identity + ":" + p.isPlaying)` по таймеру: видео в Chromium, mpv, пауза. Ожидаемо: `isPlaying` true только пока играет. Записать, есть ли свойство `isPlaying` (иначе — `playbackState`).

- [ ] **Step 3: выход звука.** `pactl get-default-sink` с наушниками в ушах (сейчас `bluez_output.00_11_22_33_44_55.1`) и после того, как пользователь положит их в кейс. Ожидаемо: имя меняется на не-`bluez_output.*`.

- [ ] **Step 4: OpenFreebuds.** Попросить пользователя выполнить `! yay -S openfreebuds` (нужен sudo). Найти, как прочитать «в ушах» у HUAWEI FreeBuds Pro 4 без окна: CLI, встроенный HTTP-сервер, D-Bus или Python-модуль пакета (`openfreebuds --help`, файлы пакета `pacman -Ql openfreebuds`). Пользователь вынимает и вставляет наушник — значение меняется? Звук в наушниках при этом не рвётся? Телефон (AI Life) не теряет наушники?

- [ ] **Step 5: записать результат** в спеку разделом «Результаты проверок»: по каждому шагу — что увидели; для OpenFreebuds — точная команда или запрос, его вывод «в ушах / вынут», время ответа, или «датчик не читается» с причиной. Задача 9 строит адаптер по этой записи.

- [ ] **Step 6: Commit**

```bash
git add docs/superpowers/specs/2026-10-24-oma-presence-design.md
git commit -m "docs: присутствие — результаты проверок IdleMonitor, MPRIS, выхода звука, OpenFreebuds"
```

---

### Task 2: Состояние присутствия — `src/presence.rs`

Чистая машина состояний без ввода-вывода; время — эпоха в секундах, как у доски.

**Files:**
- Create: `src/presence.rs`
- Modify: `src/main.rs` (`mod presence;`)
- Test: `src/presence.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Produces:
  ```rust
  pub const PROBE: u64 = 5;          // таймаут IdleMonitor плагина: простой начался за столько до сигнала
  pub const UNSURE_AFTER: u64 = 90;
  pub const AWAY_AFTER: u64 = 600;
  pub const BACK_AFTER: u64 = 180;   // отсутствие не короче — приветствие и повтор
  pub const BUSY_FOR: u64 = 900;
  pub const AWAY_GRACE: u64 = 60;
  pub const STALE_AFTER: u64 = 150;  // плагин шлёт сигнал раз в 60 с; дольше тишина — сигнала нет
  #[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum Mode { Here, Busy, Unsure, Away }
  #[derive(Clone, Copy, Debug, PartialEq, Eq, Default)] pub enum Buds { #[default] Unknown, In, Out }
  impl Buds { pub fn parse(s: &str) -> Buds } // "in" | "out" | прочее — Unknown
  #[derive(Default)] pub struct Presence { .. }
  impl Presence {
      pub fn signal(&mut self, idle: bool, now: u64) -> Option<u64>; // сообщение плагина; Some(отсутствие, с) — вернулся
      pub fn input(&mut self, now: u64) -> Option<u64>;              // любой ввод; Some — только если до него было Unsure или Away
      pub fn set_locked(&mut self, on: bool, now: u64) -> Option<u64>; // разблокировка — ввод
      pub fn set_call(&mut self, on: bool);
      pub fn set_media(&mut self, on: bool);
      pub fn set_buds(&mut self, b: Buds);                           // переход в Out — «не уверена» до ввода
      pub fn said(&mut self, m: Mode, now: u64) -> Option<u64>;      // слова пользователя; Some — только для Here
      pub fn unanswered(&mut self);                                  // «Ты тут?» без ответа — Away до ввода
      pub fn mode(&self, now: u64) -> Mode;
      pub fn last_input(&self, now: u64) -> u64;
      pub fn sure(&self) -> bool;                                    // наушники в ушах: слышит наверняка
  }
  ```

Порядок в `mode` — приоритет: заблокировано → Away; «я отойду» / «Ты тут?» без ответа (до ввода после `AWAY_GRACE`) → Away; звонок, медиа, `busy_until > now` → Busy; `Buds::In` → Here; сигнала нет (`signal` не приходил или последний старше `STALE_AFTER`) → Here; `Buds::Out` после последнего ввода → Unsure; простой ≥ `AWAY_AFTER` → Away; ≥ `UNSURE_AFTER` → Unsure; иначе Here. `signal(true, now)` ставит начало простоя `now - PROBE`, только если простоя ещё нет (повтор раз в минуту его не сдвигает); `signal(false, now)` — это `input(now)`. `said(Here)` — ввод, снимает busy и away; `said(Busy)` — ввод без возврата (`None`) и `busy_until = now + BUSY_FOR`; `said(Away)` — away с отсрочкой `AWAY_GRACE`: ввод в отсрочке away не снимает.

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn idle_thresholds() {
    let mut p = Presence::default();
    p.signal(true, 1005); // простой с 1000
    assert_eq!([p.mode(1089), p.mode(1090), p.mode(1599), p.mode(1600)], [Mode::Here, Mode::Unsure, Mode::Unsure, Mode::Away]);
    p.signal(true, 1065); // повтор раз в минуту начало не сдвигает
    assert_eq!(p.mode(1090), Mode::Unsure);
}
#[test] fn input_returns_absence_only_after_unsure_or_away() {
    // простой с 1000: ввод в 1060 → None; простой снова с 1060, ввод в 1260 → Some(200)
}
#[test] fn busy_beats_idle_but_not_lock() {
    // set_media(true), простой 700 с → Busy; set_locked(true) → Away; set_call(true) без медиа → Busy
}
#[test] fn buds_in_beats_idle() { /* Buds::In, простой 700 с → Here; sure() == true */ }
#[test] fn buds_out_unsure_until_input() { /* In → Out при простое 10 с → Unsure; input → Here */ }
#[test] fn said_away_has_grace() {
    // said(Away, 1000); input(1030) → Away; input(1061) → Here
}
#[test] fn said_busy_for_fifteen_minutes() { /* said(Busy, 1000) → Busy до 1899, Here в 1900; возврат None */ }
#[test] fn said_here_clears_busy_and_away() {}
#[test] fn unanswered_is_away_until_input() {}
#[test] fn no_signal_is_here() { /* signal не приходил: простоя не знаем → Here при любом now */ }
#[test] fn stale_signal_is_here() {
    // signal(true, 1005), потом ничего: mode(1005 + STALE_AFTER + 1) == Here, хотя простой > 90 с
}
#[test] fn wake_from_sleep_is_return() {
    // signal(true, 1005); ноутбук спал; signal(false, 4600) → Some(3600)
}
```

Тела тестов — по комментариям; значения точные.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test presence`
Expected: FAIL — `presence` не найден.

- [ ] **Step 3: Implement `src/presence.rs`** по интерфейсу и порядку приоритетов выше; `mod presence;` в `src/main.rs`.

Внимание: `stale_signal_is_here` и `wake_from_sleep_is_return` противоречат друг другу только на вид — устаревший сигнал даёт Here в `mode`, а `signal(false)` после сна возвращает отсутствие от начала простоя: `input` считает его по `last_input`, а не по `mode`. Возврат — `Some`, если до ввода было Unsure или Away **без учёта устаревания**.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test presence`
Expected: PASS (12 тестов).

- [ ] **Step 5: Commit**

```bash
git add src/presence.rs src/main.rs
git commit -m "presence: состояние присутствия — простой, блокировка, звонок, видео, наушники, слова пользователя"
```

---

### Task 3: Доска помнит сказанное — `src/news.rs`

**Files:**
- Modify: `src/news.rs`
- Test: `src/news.rs` (`mod tests`; все прежние вызовы `take(now)` → `take(now, None, true)`)

**Interfaces:**
- Consumes: —
- Produces:
  ```rust
  pub const SUMMARY: &str = "summary";   // сессия сводки возвращения: retain_sessions её не снимает
  pub const UNHEARD_AFTER: u64 = 60;
  impl Board {
      pub fn take(&mut self, now: u64, skip: Option<&str>, sure: bool) -> Option<String>;
      pub fn returned(&mut self, last_input: u64, long: bool);
      pub fn unask(&mut self);
      pub fn overview(&self) -> Option<String>;
      pub fn finals(&self) -> usize;
      pub fn take_finals(&mut self) -> Vec<News>;
      pub fn is_empty(&self) -> bool;
  }
  fn plural(n: usize, forms: [&'static str; 3]) -> &'static str; // один / два-четыре / пять; more_questions — через неё
  ```

Поведение:
- `asked` хранит саму новость: `Option<(News, u64)>`; `asked()` по-прежнему отдаёт сессию.
- `take(now, skip, sure)`: `skip` — сессия агента в фокусе: её итог и промежуточное снимаются с доски без речи, её вопрос остаётся на доске и не берётся. `sure == false` — всё взятое пишется в журнал `spoken: Vec<(News, u64)>` со временем `now`.
- `returned(last, long)`: `long` — новости журнала со временем ≥ `last + UNHEARD_AFTER` возвращаются на доску, но только если на доске нет новости той же сессии и класса (новое главнее); вернувшийся вопрос снимает `asked` своей сессии. Журнал очищается всегда.
- `unask()`: озвученный вопрос — снова на доску, `asked = None` (итоги его не ждут).
- `answered(session)` и `retain_sessions` чистят и журнал; `retain_sessions` не снимает `SUMMARY`.
- `overview()`: из счётчиков доски (итоги — по сессиям, вопросы — на доске и `asked`): `"Пока тебя не было: 2 проекта закончили, 1 ждёт ответа."`; часть с нулём опускается; оба нуля — `None`. Формы: «проект / проекта / проектов», «закончил / закончили», «ждёт / ждут» — по `plural` (цифры словами сделает `prepare`).

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn take_logs_spoken_unless_sure() {}
#[test] fn returned_long_replays_only_unheard() {
    // вопрос сказан в 1060 при last_input 1000 → returned(1000, true) — снова на доске;
    // итог сказан в 1050 (< 1000 + 60) — нет
}
#[test] fn returned_short_forgets_spoken() { /* returned(1000, false) → журнал пуст, take → None */ }
#[test] fn replay_never_overrides_newer_news() {
    // итог s сказан, потом новый итог s на доске; returned(.., true) → take даёт только новый
}
#[test] fn answered_drops_spoken_question() {}
#[test] fn closed_session_not_replayed() {}
#[test] fn summary_survives_retain_sessions() {}
#[test] fn focused_final_dropped_question_kept() {
    // итог и вопрос сессии f: take(now, Some("f"), true) → None; take(now, None, true) → вопрос f
}
#[test] fn unask_puts_question_back_and_frees_finals() {}
#[test] fn overview_counts_and_plurals() {
    // 1 итог: "Пока тебя не было: 1 проект закончил."; 2 итога + 1 вопрос: "Пока тебя не было: 2 проекта закончили, 1 ждёт ответа.";
    // 5 итогов + 2 вопроса: "… 5 проектов закончили, 2 ждут ответа."; пусто — None
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test news`
Expected: FAIL — нет `returned`, `unask`, `overview`; прежние тесты — не та сигнатура `take`.

- [ ] **Step 3: Implement** в `src/news.rs`; вызов `board.take(now)` в `src/daemon.rs` временно → `take(now, None, true)` (прежнее поведение: журнал не ведётся), чтобы сборка шла.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/news.rs src/daemon.rs
git commit -m "news: доска помнит сказанное — повтор неуслышанного, вопрос обратно на доску, агент в фокусе, обзор"
```

---

### Task 4: Сводка Haiku — `Ask::Summary`

**Files:**
- Modify: `src/digest.rs`
- Test: `src/digest.rs` (`mod tests`)

**Interfaces:**
- Produces: `Ask::Summary`; `pub const MAX_SUMMARY_WORDS: usize = 45;`; `pub fn parse(out: &str, max: usize) -> Option<(bool, String)>` (был `parse(out)`; `digest` передаёт `MAX_SUMMARY_WORDS` для `Summary`, иначе `MAX_SAY_WORDS`).

Промпт `Ask::Summary` (дословно): «Ниже итоги нескольких агентов, по строке на проект: «проект: итог». Сожми их в две-три фразы, не длиннее 40 слов: назови проекты и главное, что требует внимания пользователя. important — true.» Текст агента в промпте — строки итогов.

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn summary_prompt_lists_finals() { /* prompt(&job) содержит «Сожми их в две-три фразы» и обе строки итогов */ }
#[test] fn summary_clipped_at_45_words() { /* parse(ответ на 60 слов, MAX_SUMMARY_WORDS) — не длиннее 45 слов */ }
#[test] fn summary_falls_back_to_joined_finals() { /* ask → None: Digest.say == job.fallback */ }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test digest`
Expected: FAIL — нет `Ask::Summary`.

- [ ] **Step 3: Implement**; прежние вызовы `parse(out)` в тестах → `parse(out, MAX_SAY_WORDS)`. В `src/daemon.rs` у `match d.ask` ветки `Ask::Summary` пока нет — добавить `Ask::Summary => None` в оба `match` в `on_digest` (задача 7 заменит).

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/digest.rs src/daemon.rs
git commit -m "digest: сводка итогов одним вызовом Haiku — до 45 слов, без модели — склейка"
```

---

### Task 5: Ударения от модели

**Files:**
- Modify: `src/text/mod.rs`, `src/focus.rs` (`plus_stress` переезжает), `src/digest.rs` (`COMMON`)
- Test: `src/text/mod.rs`, `src/digest.rs`

**Interfaces:**
- Produces: `pub fn plus_stress(p: &str) -> String` в `src/text/mod.rs` (тело — из `src/focus.rs`, там — вызов `crate::text::plus_stress`); `prepare` применяет её первой.

В `COMMON` дописать (дословно): «В словах, где ударение легко прочесть неверно, — омографы (замо́к и за́мок), редкие слова, имена и названия — ставь знак ударения U+0301 сразу после ударной гласной.»

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn prepare_keeps_model_stress() {
    let (s, _) = prepare("Открой замо\u{301}к и все\u{301}.", &Terms::from_str(""));
    assert_eq!(s, vec!["Открой зам+ок и вс+е."]);
}
#[test] fn prompt_asks_for_stress_marks() { /* prompt(&job) содержит "U+0301" */ }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test stress`
Expected: FAIL.

- [ ] **Step 3: Implement.** Если «+» теряется дальше по `prepare` (`clean`, `abbr`, `terms`, `numbers`) — чинить там, где теряется; тест `prepare_end_to_end` не меняется.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/text/mod.rs src/focus.rs src/digest.rs
git commit -m "ударения: модель ставит знак в омографах и редких словах, Silero получает «+»"
```

---

### Task 6: Демон — сигналы, молчание и «Ты тут?»

**Files:**
- Modify: `src/ipc.rs`, `src/daemon.rs`
- Test: `src/ipc.rs` (`msg_roundtrip`), `src/daemon.rs` (`mod tests`)

**Interfaces:**
- Consumes: `Presence`, `Mode`, `Buds` (задача 2); `Board::take(now, skip, sure)`, `unask`, `is_empty` (задача 3).
- Produces:
  ```rust
  // ipc::Msg
  Input { idle: bool },                       // плагин: IdleMonitor; и раз в 60 с — как есть
  Media { playing: bool },                    // плагин: играет ли какой-нибудь MPRIS-плеер
  Signals { #[serde(default)] locked: bool, #[serde(default)] call: bool, #[serde(default)] buds: String }, // Ома: снимок
  Presence { state: String },                 // Ома: "here" | "busy" | "away" — голос пользователя
  // daemon
  const HERE_WAIT: u64 = 20;                  // с от «Ты тут?» до вывода «нет на месте»
  const ARE_YOU_THERE: &str = "Ты тут? Есть новости.";
  // State: presence: Presence, here_ask: Option<u64>,
  //        listen_after: Option<(&'static str, String)> вместо bool — (kind, сказанное)
  // команда Оме: {"do":"listen","secs":5,"kind":"news"|"question"|"here","said":<текст>[,"key":"agent:<s>"]}
  ```

Поведение `speak_news` (прежние условия молчания остаются первыми):
- `Mode::Busy | Mode::Away` — ничего.
- `Mode::Unsure` — доска не пуста и `here_ask` пуст: в очередь `ARE_YOU_THERE` (`Kind::News`), `warm`, `listen_after = Some(("here", ARE_YOU_THERE))`, `here_ask = Some(now)`. Иначе ничего.
- `Mode::Here` — `board.take(now, focused, presence.sure())`, где `focused` — id сессии в фокусе (`self.focused()`); `listen_after = Some((kind, text))`, kind — `"question"`, если после `take` есть `board.asked()`, иначе `"news"`.

`tick`: `here_ask = Some(t)`, `now ≥ t + HERE_WAIT`, `!oma.talking` → если `presence.last_input(now) < t` — `presence.unanswered()`; `here_ask = None` в любом случае.

`on_msg`: `Input` → `presence.signal`; `Media` → `set_media`; `Signals` → `set_locked`, `set_call`, `set_buds(Buds::parse(..))`; `Presence` → `said`; при `"busy"` ещё `board.unask()` (звучащую новость обрывать не нужно: перебив уже оборвал её на начале речи). Возврат (`Some`) из `signal` / `set_locked` / `said` — в `on_back(Option<u64>)`; в этой задаче он только снимает `here_ask` и зовёт `board.returned(now - a, false)`; задача 7 добавит приветствие.

- [ ] **Step 1: Write the failing tests** (в `daemon.rs`; время — подменой `now_secs` на значение под `Arc<Mutex<u64>>`)

```rust
#[test] fn msg_roundtrip() /* ipc.rs: + input, media, signals (без полей — false/""), presence */
#[test] fn away_user_hears_nothing() {
    // Input{idle:true} в 1000, now 1700; итог на доске; tick → texts пусто
}
#[test] fn unsure_asks_are_you_there_once() {
    // Input{idle:true} в 1000, now 1100; итог; tick → texts == ["Ты тут? Есть новости."]; pop; tick → пусто
}
#[test] fn are_you_there_unanswered_is_away() {
    // после вопроса now += HERE_WAIT; tick → presence.mode(now) == Mode::Away
}
#[test] fn voice_here_after_are_you_there_speaks_news() {
    // Msg::Presence{state:"here"}; tick → texts == ["плуг: Фича готова."]
}
#[test] fn busy_holds_news_and_releases_without_greeting() {
    // Media{playing:true}; итог; tick → пусто; Media{playing:false}; tick → ["плуг: Фича готова."]
}
#[test] fn not_now_after_question_requeues_it() {
    // вопрос озвучен; Msg::Presence{state:"busy"}; итог s2 на доске; tick → пусто (busy);
    // now += BUSY_FOR; tick → первым звучит вопрос
}
#[test] fn buds_in_ear_speaks_despite_idle() { /* Signals{buds:"in"}, простой 700 с → новость звучит */ }
#[test] fn focused_agent_final_not_spoken() { /* focus_src → s; итог s; tick → пусто; вопрос s ждёт, пока s в фокусе */ }
#[test] fn no_plugin_signal_behaves_as_before() { /* без Input: итог звучит, как в final_digest_bubbles_then_speaks_when_idle */ }
```

Прежние тесты окна ответа (`listen_after` как bool, ожидаемый JSON `{"do":"listen","secs":5}` и `{"do":"listen","secs":5,"key":"agent:s"}`) — обновить под `kind` и `said`.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test`
Expected: FAIL — нет `Msg::Input` и т. д.

- [ ] **Step 3: Implement** в `src/ipc.rs` и `src/daemon.rs` по поведению выше.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test && cargo build 2>&1 | grep -c warning`
Expected: PASS; `0`.

- [ ] **Step 5: Commit**

```bash
git add src/ipc.rs src/daemon.rs
git commit -m "Ома молчит, когда её не слушают: сигналы присутствия, «Ты тут?», агент в фокусе, «не сейчас»"
```

---

### Task 7: Демон — возвращение, сводка, ночь

**Files:**
- Modify: `src/daemon.rs`
- Test: `src/daemon.rs` (`mod tests`)

**Interfaces:**
- Consumes: `on_back` (задача 6), `Board::returned`, `overview`, `finals`, `take_finals`, `SUMMARY` (задача 3), `Ask::Summary` (задача 4), `presence::BACK_AFTER`.
- Produces: `State.welcome: bool`, `State.night_keep: bool`; `const SUMMARY_OVER: usize = 3;`; `const WELCOME: &str = "С возвращением.";`

Поведение:
- `on_back(Some(a))`, `a ≥ BACK_AFTER` → `board.returned(now - a, true)`, `welcome = true`; иначе `board.returned(.., false)`.
- `speak_news` в `Mode::Here` при `welcome`: доска пуста — `welcome = false`, молчим; иначе `over = board.overview()`; `board.finals() > SUMMARY_OVER` — `take_finals()` уходят одним `Job { session: SUMMARY, project: "", ask: Ask::Summary, text: строки итогов через "\n", context: "", fallback: Some(они же через пробел) }`; фраза = `WELCOME` + `over` + `board.take(..)` (что есть); `welcome = false`.
- `on_digest` с `Ask::Summary`: без реестра — пузырь (`who` «Ома», kind `final`) и `board.push(News { session: SUMMARY, class: Final, say, born: now })` (вне тишины).
- Ночь: на границе тишины в `tick` (`self.quiet() != before`) `night_keep = quiet && mode ∈ {Unsure, Away}`; `purge` в тишине чистит доску только при `!night_keep`; конец тишины — `night_keep = false`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn return_after_long_absence_greets_with_overview() {
    // простой с 1000; итог в 1300 (Away → не звучит); Input{idle:false} в 1700; tick →
    // texts == ["С возвращением.", "Пока тебя не было: один проект закончил.", "плуг: Фича готова."]
}
#[test] fn news_spoken_into_void_repeated_on_return() {
    // ввод в 1000; итог звучит в 1060 (Here); Input{idle:true} в 1065; Input{idle:false} в 1300 → итог звучит снова с приветствием
}
#[test] fn listener_sitting_still_hears_no_repeat() {
    // итог звучит в 1070 при простое с 1000; Input{idle:false} в 1150 → tick → пусто
}
#[test] fn short_absence_no_greeting() { /* отсутствие 120 с, итог на доске → звучит без «С возвращением.» */ }
#[test] fn long_absence_summarizes_finals_once() {
    // 4 итога, возвращение → Job Ask::Summary с 4 строками; on_digest(Summary) → следующей звучит сводка
}
#[test] fn locked_then_unlocked_is_return() { /* Signals{locked:true} → молчит; Signals{locked:false} через 400 с → приветствие */ }
#[test] fn night_keeps_board_when_away_until_morning() {
    // Away, итог на доске; minutes → 23:00, tick (тишина) → доска цела; minutes → 08:00, tick; Input{idle:false} → приветствие и итог
}
#[test] fn night_clears_board_when_here() { /* как сейчас: muting_clears_board */ }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test`
Expected: FAIL.

- [ ] **Step 3: Implement** в `src/daemon.rs`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test && cargo build 2>&1 | grep -c warning`
Expected: PASS; `0`.

- [ ] **Step 5: Commit**

```bash
git add src/daemon.rs
git commit -m "Ома вводит в курс по возвращении: приветствие, обзор, повтор неуслышанного, сводка Haiku, ночь не стирает"
```

---

### Task 8: Плагин — простой ввода и MPRIS

**Files:**
- Modify: `plugin/Model.js`, `plugin/Oma.qml`, `plugin/test.js`

**Interfaces:**
- Consumes: `Msg::Input`, `Msg::Media` (задача 6).
- Produces: `Model.input(idle) -> string` (`{"cmd":"input","idle":idle}\n`), `Model.media(playing) -> string`, `Model.anyPlaying(players) -> bool` (массив объектов с `isPlaying`; поле — по записи задачи 1).

В `Oma.qml` (один экземпляр `Link`; `Indicator.qml` не трогать): `IdleMonitor { timeout: 5; respectInhibitors: false }` — `onIsIdleChanged` шлёт `Model.input(isIdle)`; `readonly property bool playing: Model.anyPlaying(Mpris.players.values)` — `onPlayingChanged` шлёт `Model.media(playing)`; `Timer` 60 с шлёт оба как есть (сигнал жив); при `link.up` — сразу оба. Комментарий у `timeout`: должен совпадать с `presence::PROBE`.

- [ ] **Step 1: Write the failing test** (в `plugin/test.js`, имена — в список `load`)

```js
assert.strictEqual(M.input(true), '{"cmd":"input","idle":true}\n')
assert.strictEqual(M.media(false), '{"cmd":"media","playing":false}\n')
assert.strictEqual(M.anyPlaying([{ isPlaying: false }, { isPlaying: true }]), true)
assert.strictEqual(M.anyPlaying([]), false)
```

- [ ] **Step 2: Run test to verify it fails**

Run: `node plugin/test.js`
Expected: FAIL — `M.input is not a function`.

- [ ] **Step 3: Implement** в `Model.js` и `Oma.qml`.

- [ ] **Step 4: Run tests and load the shell**

Run: `node plugin/test.js && omarchy-shell restart 2>&1 | tail -3; sleep 3; journalctl --user -b --since "-1min" | grep -i -E "predmaxim.oma|qml" | grep -i -E "error|warn" | head`
Expected: `ok`; ошибок QML нет. В журнале демона (`journalctl --user -u agent-speak --since "-1min"`) ошибок разбора `input`/`media` нет.

- [ ] **Step 5: Commit**

```bash
git add plugin/Model.js plugin/Oma.qml plugin/test.js
git commit -m "plugin: простой ввода (IdleMonitor) и играющий MPRIS — демону, раз в минуту — как есть"
```

---

### Task 9: Ома — блокировка, звонок, наушники

**Files:**
- Create: `python/agent_voice/buds.py`
- Modify: `python/agent_voice/__main__.py` (цикл `signals` в `keep`)
- Test: `python/test_agent_voice.py`

**Interfaces:**
- Consumes: `Msg::Signals` (задача 6); `locked`, `mic_outputs`, `foreign_capture` из `context.py`; запись задачи 1 об OpenFreebuds.
- Produces:
  ```python
  def default_sink(run=subprocess.run) -> str            # pactl get-default-sink, сбой — ""
  def in_ear(run=subprocess.run) -> bool | None          # датчик «в ушах»; None — датчика нет
  def buds_state(sink: str, sensor: bool | None, was_on_buds: bool) -> str  # "in" | "out" | "unknown"
  SIGNALS_EVERY = 3  # с, __main__.py
  ```

`buds_state`: выход `bluez_output.*` → `"in"` при `sensor is True`, `"out"` при `False`, `"unknown"` при `None`; выход не наушники → `"out"`, если прошлый опрос был на наушниках, иначе `"unknown"`. `in_ear` — по записи задачи 1 (команда/запрос с таймаутом 2 с, сбой — `None`); если задача 1 записала «датчик не читается» — `in_ear` возвращает `None` без вызовов, с комментарием `shortcut: датчика нет, вернуть, когда OpenFreebuds научится читать Pro 4`.

Цикл `signals` в `__main__.py`: раз в `SIGNALS_EVERY` — `locked()`, `foreign_capture(mic_outputs(), {oma.ear.pid} if pid else set())`, `buds_state(...)` (блокирующее — `asyncio.to_thread`); первый снимок и каждый изменившийся — `link.send({"cmd": "signals", "locked": …, "call": …, "buds": …})`.

- [ ] **Step 1: Write the failing tests**

```python
def test_buds_state_table():
    assert buds_state("bluez_output.00_11_22_33_44_55.1", True, False) == "in"
    assert buds_state("bluez_output.00_11_22_33_44_55.1", False, True) == "out"
    assert buds_state("bluez_output.00_11_22_33_44_55.1", None, True) == "unknown"
    assert buds_state("alsa_output.pci-0000_00_1f.3.analog-stereo", None, True) == "out"
    assert buds_state("alsa_output.pci-0000_00_1f.3.analog-stereo", None, False) == "unknown"

def test_default_sink_failure_is_empty():
    def boom(*a, **k): raise OSError
    assert default_sink(boom) == ""
```

Плюс тест `in_ear` на подделке `run` по формату вывода из задачи 1 (если датчик читается).

- [ ] **Step 2: Run tests to verify they fail**

Run: `~/.local/share/agent-speak/voice-venv/bin/python -m pytest python/test_agent_voice.py -k buds -q`
Expected: FAIL — `buds` не найден.

- [ ] **Step 3: Implement** `buds.py` и цикл `signals`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `~/.local/share/agent-speak/voice-venv/bin/python -m pytest python/test_agent_voice.py -q`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add python/agent_voice/buds.py python/agent_voice/__main__.py python/test_agent_voice.py
git commit -m "Ома: блокировка, звонок и наушники «в ушах» — демону снимком при изменении"
```

---

### Task 10: Разговор из окна ответа — «Ок», «да», короткий хвост

**Files:**
- Modify: `python/agent_voice/conversation.py`, `python/agent_voice/__main__.py` (`Oma.command` для `listen`, `Oma.act` для `presence` и `send`)
- Test: `python/test_agent_voice.py`

**Interfaces:**
- Consumes: команда `listen` с `kind` и `said` (задача 6); `Msg::Presence` (задача 6).
- Produces:
  ```python
  WINDOW_END = 5   # с тишины после ответа Омы в разговоре из окна ответа
  ACKS = {"ок", "окей", "ok", "okay", "понял", "поняла", "ясно", "спасибо", "хорошо", "угу", "ага", "принято"}
  YES = {"да", "тут", "здесь", "я", "ага", "угу", "слушаю"}
  def words(text: str) -> list[str]        # нижний регистр, ё→е, только буквы
  def is_ack(text: str) -> bool            # 1–3 слова, все из ACKS
  def is_yes(text: str) -> bool            # 1–3 слова, все из YES
  Conversation.listen(secs, kind="news", said="")
  # действия: ("presence", "here"), ("send", текст, пометка_для_мозга)
  ```

Поведение (только в разговоре, открытом `listen`; клик — как сейчас): первая реплика — `("presence", "here")`. `kind == "here"`: `is_yes` — `end()` без `send`; иначе `send` с пометкой `"Ома спросила: «Ты тут? Есть новости». "`. `kind == "news"` и первая реплика `is_ack` — `end()` без `send`. Иначе `send`; у первой реплики пометка `f"Ома только что сказала: «{said}». "`; реплика `is_ack` — после ответа Омы (`done` без звука или конец звука) сразу `end()`. Тишина после ответа Омы — `WINDOW_END` вместо `SILENCE_END`. `end()` сбрасывает окно. `Oma.act`: `("send", text, ctx)` — мозгу `ctx + text`, в пузырь и чат — `text`; `("presence", s)` — `send({"cmd": "presence", "state": s})`.

- [ ] **Step 1: Write the failing tests** (фальшивые часы — как в соседних тестах `Conversation`)

```python
def test_ack_after_news_closes_without_send(): ...      # listen(5,"news","плуг: готово"); "Ок." → ("presence","here"), ("ear", False), нет "send"
def test_ack_after_question_sent_then_closes(): ...     # listen(5,"question",…); "Ок" → ("send","Ок","Ома только что сказала: «…». "); done() → ("ear", False)
def test_window_silence_after_reply_is_five_seconds(): ... # ответ «А что за баг?» → done(); +4.9 с — слушает; +5.1 с — end
def test_are_you_there_yes_closes(): ...                # listen(5,"here"); "да, я тут" → presence here, ear False, нет send
def test_are_you_there_other_goes_to_brain(): ...       # "через минуту" → send с пометкой «Ома спросила: …»
def test_click_conversation_ack_still_sent(): ...       # talk(); "ок" → send без пометки и без presence; тишина — SILENCE_END
def test_is_ack_table(): ...                            # "Ок, спасибо." True; "OK" True; "Окей!" True; "ок давай пушь" False; "" False
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `~/.local/share/agent-speak/voice-venv/bin/python -m pytest python/test_agent_voice.py -q`
Expected: FAIL.

- [ ] **Step 3: Implement** в `conversation.py` и `__main__.py`; прежние тесты, где `("send", text)` двухэлементный, — под трёхэлементный.

- [ ] **Step 4: Run tests to verify they pass**

Run: `~/.local/share/agent-speak/voice-venv/bin/python -m pytest python/test_agent_voice.py -q`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add python/agent_voice/conversation.py python/agent_voice/__main__.py python/test_agent_voice.py
git commit -m "Ома: «Ок» после новости закрывает микрофон, «да» на «Ты тут?», тишина после ответа — 5 секунд"
```

---

### Task 11: Мозг понимает «не сейчас» — инструмент `presence`, ударения

**Files:**
- Modify: `python/agent_voice/brain.py`, `python/agent_voice/__main__.py` (`Brain(..., send=link.send)`)
- Test: `python/test_agent_voice.py`

**Interfaces:**
- Consumes: `Msg::Presence` (задача 6).
- Produces: `oma_tools(snapshot, send)`; инструмент `presence` с `{"state": str}` (`here` | `busy` | `away`), шлёт `send({"cmd": "presence", "state": s})`, иное — ответ «state: here | busy | away» без отправки; `"mcp__oma__presence"` в `OMA_TOOLS`; `Brain(events, snapshot, send=link.send)`.

Дописать в `OMA_PROMPT` (дословно):
«Если пользователь просит сейчас не говорить или отложить («не сейчас», «давай потом», «замолчи» и т. п.) — вызови presence с busy; говорит, что уходит, — away; просит новости или говорит, что вернулся, — here. После presence ответь одним-двумя словами или ничего. Реплика может начинаться с пометки «Ома только что сказала: …» или «Ома спросила: …» — это твои же слова вслух; «потом» и «не сейчас» после вопроса агента — presence busy, а не ответ агенту. В словах, где ударение легко прочесть неверно, — омографы, редкие слова, имена — ставь знак ударения U+0301 сразу после ударной гласной.»

- [ ] **Step 1: Write the failing tests**

```python
def test_presence_tool_sends_state(): ...      # вызвать обработчик инструмента с {"state":"busy"} → send получил {"cmd":"presence","state":"busy"}
def test_presence_tool_rejects_unknown(): ...  # {"state":"sleep"} → send не вызван
def test_prompt_mentions_presence_and_stress(): ...  # "presence" и "U+0301" в OMA_PROMPT; "mcp__oma__presence" в OMA_TOOLS
```

Как вызвать обработчик `@tool` в тесте — так же, как соседние тесты инструментов в `test_agent_voice.py` (если их нет — через атрибут обработчика объекта SDK, `.handler`).

- [ ] **Step 2: Run tests to verify they fail**

Run: `~/.local/share/agent-speak/voice-venv/bin/python -m pytest python/test_agent_voice.py -k presence -q`
Expected: FAIL.

- [ ] **Step 3: Implement** в `brain.py` и `__main__.py`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `~/.local/share/agent-speak/voice-venv/bin/python -m pytest python/test_agent_voice.py -q`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add python/agent_voice/brain.py python/agent_voice/__main__.py python/test_agent_voice.py
git commit -m "мозг Омы: «не сейчас», «я отойду», «рассказывай» любыми словами — инструмент presence; ударения в ответах"
```

---

### Task 12: Установка, документация, живая проверка

**Files:**
- Modify: `README.md` (раздел о присутствии: состояния, «Ты тут?», возвращение, «Ок», наушники и OpenFreebuds — необязательно), `~/omarchy-dotfiles/docs/changelog.md` (запись по `rules.md`, «Порядок работы»)

- [ ] **Step 1: Все проверки**

Run: `cargo test && cargo build --release 2>&1 | grep -c warning; node plugin/test.js; ~/.local/share/agent-speak/voice-venv/bin/python -m pytest python/test_agent_voice.py -q`
Expected: всё PASS; `0`.

- [ ] **Step 2: Установить** `./install.sh` (как в прошлых планах), `omarchy-shell restart`. Ожидаемо: `systemctl --user is-active agent-speak` → `active`; в журнале демона через минуту есть принятые `input`/`media`/`signals` без ошибок разбора.

- [ ] **Step 3: Живая проверка с пользователем** — у него «Следить за агентами» выключено: попросить включить. По каждому пункту — что услышал пользователь:
  1. 2 мин не трогать ноутбук, агент заканчивает ход → «Ты тут? Есть новости.»; «да» → итог.
  2. Заблокировать экран, агент заканчивает ход → тишина; через 4 мин разблокировать → «С возвращением. Пока тебя не было: …».
  3. Видео в Chromium, агент заканчивает ход → тишина, пузырь; пауза → итог без приветствия.
  4. Посреди новости «давай потом» → смолкла; через 15 мин или по «рассказывай» → новости.
  5. После новости «Ок» → микрофон закрыт сразу (орб не слушает).
  6. Вынуть наушник (если датчик читается) или положить в кейс → следующая новость начинается с «Ты тут?».
  7. Ударения: итог с омографом звучит верно (по журналу видно «+»).

- [ ] **Step 4: Документация и коммит**

```bash
git add README.md
git commit -m "README: Ома видит, слушает ли пользователь"
git -C ~/omarchy-dotfiles add docs/changelog.md && git -C ~/omarchy-dotfiles commit -m "changelog: agent-speak — присутствие пользователя"
```
