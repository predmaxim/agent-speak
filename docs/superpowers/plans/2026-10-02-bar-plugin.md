# Плагин панели Omarchy для agent-speak — план реализации

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Значок озвучки в центральной группе индикаторов и окно настроек, которые говорят с `agent-speakd` через его unix-сокет: состояние в реальном времени, пауза/стоп, режим, промежуточные статусы, голос и скорость.

**Architecture:** Сервис получает три команды сокета: `subscribe` (соединение остаётся открытым, сервис пишет строку состояния сразу и при каждом изменении), `set` (проверить, применить, сохранить `config.toml`, разослать), `say` (ручное чтение образца). Каждое соединение — свой поток; поток воспроизведения сообщает о смене «говорит/молчит» через канал в основной цикл. Плагин `plugin/` (id `predmaxim.agent-speak`): `Link.qml` — общая подписка на сокет (`Quickshell.Io` `Socket`), `Model.js` — чистая логика (состояние → значок/подсказка, строки команд, разделы окна), `Indicator.qml` — значок (`BarIndicator`, копируется хуком в клон индикаторов), `Panel.qml` — скрытый виджет с IPC `toggle` и модалкой.

**Tech Stack:** Rust 2024 (std, `serde`/`serde_json`, `toml` — уже в `Cargo.toml`), QML Quickshell (`Quickshell.Io` `Socket`/`SplitParser`, `Quickshell.Wayland`), компоненты оболочки `qs.Ui` (`Panel`, `BarIndicator`, `PanelHero`, `PanelSeparator`, `PanelSectionHeader`, `CursorSurface`, `Button`, `BorderSurface`), Node для `test.js`, bash-хук в `omarchy-dotfiles`.

**Spec:** `docs/superpowers/specs/2026-10-02-bar-plugin-design.md`

## Global Constraints

- Сокет: `$XDG_RUNTIME_DIR/agent-speak.sock`, сообщения — JSON-строки, разделитель `\n`.
- Строка состояния — ровно эти поля в этом порядке: `{"running":true,"speaking":…,"paused":…,"mode":…,"read_intermediate":…,"speaker":…,"rate":…,"project":…}`; `project` — имя папки проекта читаемой сессии, пустая строка, если не читает.
- `set`: `mode` — `auto`/`manual`; `read_intermediate` — `true`/`false` (JSON-булево); `speaker` — `xenia`/`baya`/`kseniya`/`aidar`/`eugene`; `rate` — `x-slow`/`slow`/`medium`/`fast`/`x-fast`. Неверное — игнорировать, строка в журнал.
- Состояние рассылается при: начале и конце речи, паузе/продолжении, стопе, смене настроек (`set`, `mode`, правка файла). Длина очереди не рассылается.
- Подписчик, запись которому не удалась, удаляется молча.
- Настройки — только в `~/.config/agent-speak/config.toml` сервиса, не в `shell.json` (исключение из `rules.md` §9 — записать).
- Id плагина `predmaxim.agent-speak`; код — `plugin/` репозитория; `install.sh` ставит ссылку `plugin/` → `~/.config/omarchy/plugins/predmaxim.agent-speak`.
- Значок — в центральной группе (клон `predmaxim.indicators`), файл `indicators/AgentSpeak.qml`, копирует хук `patch_indicators`.
- Клики по значку: **левый** — открыть/закрыть окно (`omarchy-shell predmaxim.agent-speak toggle`); **правый** — `pause` только если говорит или на паузе, иначе ничего.
- Окно показывает подтверждённое сервисом (следующую строку состояния), а не нажатое.
- Нет соединения — состояние «недоступен», переподключение раз в 5 с.
- Фраза образца голоса и скорости: «Так звучит этот голос.»
- Надписи плагина — английские в `tr("…")`, русские в `I18n.js`; комментарии в QML/JS — по-английски (как в остальных плагинах), в Rust — по-русски.
- Дизайн окна — одноцветно (`root.bar.foreground`, приглушённое `Qt.darker(…, 1.4)`), `PanelHero`, `PanelSeparator`, подписи капсом.
- Нельзя: `sed -i` по пути-ссылке (править файлы по пути в `~/omarchy-dotfiles/...`); коммитить `.superpowers/`.
- Живая проверка — снимками `grim -o eDP-2`, просмотр снимка инструментом Read.
- Коммиты оканчиваются пустой строкой и `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Review Focus

1. Сервис перезапущен или не запущен, пока оболочка работает — значок показывает «недоступен» и сам возвращается в течение ~5 с после старта сервиса. → живая проверка в Task 7 (пробник с `systemctl --user restart`) и Task 9; `Model.parse` на мусоре не меняет состояние → `test.js` в Task 6.
2. Два подписчика (значок и окно), окно закрыли — сервис удаляет мёртвого и продолжает слать живому. → тест `dead_subscriber_dropped_live_one_kept` в Task 4.
3. В соединение пришла битая строка или неизвестная команда (старый плагин, ручной `socat`) — строка в журнал, соединение и следующие команды живы. → тест `serve_reads_every_line_and_survives_garbage` в Task 4.
4. `read_intermediate` пришёл строкой `"false"` вместо булева — сервис отвергает, плагин всегда шлёт булево. → тест `set_validates_keys_and_values` в Task 1 и проверка типов значений разделов в `test.js` Task 6.
5. Первая настройка из окна, когда `~/.config/agent-speak/` ещё нет — файл создаётся, состояние рассылается. → тест `set_applies_saves_broadcasts_and_rejects_bad` в Task 5 (чистый `HOME`).

---

## File Structure

```
src/config.rs        + SPEAKERS, RATES, Config::set(key, value) -> bool
src/speaker.rs       + Shared.current, Shared.changed, Shared::set_busy(Option<&str>)
src/focus.rs         + SessionRef.project, project_of(pid)
src/ipc.rs           + Msg::Subscribe, Msg::Set { key, value }, Msg::Say { text }
src/daemon.rs        + serve() (поток на соединение), Event::Subscribe/Busy, Status, подписчики,
                       broadcast(), apply(), set/say, карта сессия → проект
install.sh           + ссылка plugin/ → ~/.config/omarchy/plugins/predmaxim.agent-speak
README.md            + строка про плагин
plugin/manifest.json виджет панели predmaxim.agent-speak
plugin/Model.js      состояние → значок/подсказка, строки команд, разделы окна
plugin/I18n.js       русские надписи
plugin/test.js       node-тесты Model.js, переводов, запрещённых имён свойств
plugin/Link.qml      подписка на сокет, переподключение, send()
plugin/Panel.qml     скрытый виджет: IPC toggle + модалка настроек
plugin/Indicator.qml значок для клона predmaxim.indicators
~/omarchy-dotfiles/home/.config/omarchy/hooks/post-update.d/keep-custom-widgets.sh  patch_indicators
~/omarchy-dotfiles/docs/rules.md, docs/changelog.md, home/.config/omarchy/shell.json
```

---

### Task 1: `Config::set` — проверка и применение настройки

**Files:**
- Modify: `src/config.rs`

**Interfaces:**
- Consumes: `Config { mode, speaker, rate, max_age_secs, read_intermediate }` (есть).
- Produces: `pub const SPEAKERS: [&str; 5]`, `pub const RATES: [&str; 5]`, `impl Config { pub fn set(&mut self, key: &str, value: &serde_json::Value) -> bool }` — `true`, если применено; при `false` конфиг не меняется.

- [ ] **Step 1: Написать падающий тест**

В `mod tests` файла `src/config.rs` добавить:

```rust
    #[test]
    fn set_validates_keys_and_values() {
        use serde_json::json;
        let mut c = Config::default();
        assert!(c.set("mode", &json!("auto")));
        assert!(c.set("speaker", &json!("eugene")));
        assert!(c.set("rate", &json!("x-fast")));
        assert!(c.set("read_intermediate", &json!(false)));
        assert!(!c.set("read_intermediate", &json!("true"))); // строка вместо булева
        assert!(!c.set("mode", &json!("loud")));
        assert!(!c.set("speaker", &json!(1)));
        assert!(!c.set("rate", &json!("warp")));
        assert!(!c.set("max_age_secs", &json!(5))); // не настраивается из интерфейса
        assert_eq!(
            (c.mode.as_str(), c.speaker.as_str(), c.rate.as_str(), c.read_intermediate, c.max_age_secs),
            ("auto", "eugene", "x-fast", false, 30)
        );
    }
```

- [ ] **Step 2: Убедиться, что падает**

Run: `cargo test set_validates_keys_and_values`
Expected: ошибка компиляции `no method named `set` found for struct `Config``.

- [ ] **Step 3: Реализация**

В `src/config.rs` после `pub fn path()` добавить константы, в `impl Config` — метод:

```rust
pub const SPEAKERS: [&str; 5] = ["xenia", "baya", "kseniya", "aidar", "eugene"];
pub const RATES: [&str; 5] = ["x-slow", "slow", "medium", "fast", "x-fast"];
```

```rust
    /// Настройка из интерфейса (команда `set`); неверный ключ или значение — false, ничего не меняется.
    pub fn set(&mut self, key: &str, value: &serde_json::Value) -> bool {
        let s = value.as_str().unwrap_or_default();
        match key {
            "mode" if ["auto", "manual"].contains(&s) => self.mode = s.into(),
            "speaker" if SPEAKERS.contains(&s) => self.speaker = s.into(),
            "rate" if RATES.contains(&s) => self.rate = s.into(),
            "read_intermediate" if value.is_boolean() => self.read_intermediate = value.as_bool().unwrap(),
            _ => return false,
        }
        true
    }
```

- [ ] **Step 4: Убедиться, что проходит**

Run: `cargo test config::`
Expected: `test result: ok. 2 passed`.

- [ ] **Step 5: Коммит**

```bash
git add src/config.rs
git commit -m "config: validated set() for the bar plugin

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: Поток воспроизведения сообщает «говорит/молчит»

**Files:**
- Modify: `src/speaker.rs`

**Interfaces:**
- Consumes: `Shared` (есть), `run_with` (есть).
- Produces: поля `pub current: Mutex<String>` (сессия звучащей фразы), `pub changed: Mutex<Option<std::sync::mpsc::Sender<()>>>`; метод `pub fn set_busy(&self, session: Option<&str>)` — ставит `busy`, запоминает сессию, шлёт `()` в `changed`, если изменилось «говорит/молчит» или сессия. `Shared::new` — без изменения сигнатуры (`changed` = `None`).

- [ ] **Step 1: Написать падающие тесты**

В `mod tests` файла `src/speaker.rs` добавить:

```rust
    #[test]
    fn busy_changes_are_reported_once() {
        let (tx, rx) = std::sync::mpsc::channel();
        let shared = Shared::new(Queue::new(Duration::from_secs(30)), "x".into(), "m".into());
        *shared.changed.lock().unwrap() = Some(tx);
        shared.set_busy(Some("a"));
        shared.set_busy(Some("a")); // без изменений — молчит
        shared.set_busy(Some("b")); // другая сессия — сообщает
        shared.set_busy(None);
        shared.set_busy(None);
        assert_eq!(rx.try_iter().count(), 3);
        assert_eq!(*shared.current.lock().unwrap(), "b");
        assert!(!shared.busy.load(Ordering::SeqCst));
    }

    #[test]
    fn speaker_reports_speaking_and_silence() {
        let (shared, ev) = start(0, 0);
        let (tx, rx) = std::sync::mpsc::channel();
        *shared.changed.lock().unwrap() = Some(tx);
        say(&shared);
        wait(|| count(&ev, "write") == 2);
        rx.recv_timeout(Duration::from_secs(2)).unwrap(); // начал говорить
        rx.recv_timeout(Duration::from_secs(3)).unwrap(); // замолчал
        assert!(!shared.busy.load(Ordering::SeqCst));
        assert_eq!(*shared.current.lock().unwrap(), "s");
    }
```

- [ ] **Step 2: Убедиться, что падают**

Run: `cargo test speaker::`
Expected: ошибка компиляции `no field `changed` on type `Shared``.

- [ ] **Step 3: Реализация**

В `struct Shared` после `pub busy: AtomicBool,` добавить:

```rust
    pub current: Mutex<String>, // сессия звучащей фразы — для «Читаю: <проект>»
    pub changed: Mutex<Option<std::sync::mpsc::Sender<()>>>, // смена «говорит/молчит» → основной цикл
```

В `Shared::new` после `busy: AtomicBool::new(false),`:

```rust
            current: Mutex::new(String::new()),
            changed: Mutex::new(None),
```

В `impl Shared` (после `stop`):

```rust
    /// Some(сессия) — говорит, None — молчит. Сообщает основному циклу только об изменении.
    pub fn set_busy(&self, session: Option<&str>) {
        let was = self.busy.swap(session.is_some(), Ordering::SeqCst);
        let mut cur = self.current.lock().unwrap();
        let changed = was != session.is_some() || session.is_some_and(|s| *cur != s);
        if let Some(s) = session {
            *cur = s.to_string();
        }
        drop(cur);
        if changed {
            if let Some(tx) = &*self.changed.lock().unwrap() {
                let _ = tx.send(());
            }
        }
    }
```

В `run_with` заменить три записи `busy`:
- `shared.busy.store(false, Ordering::SeqCst);` перед `let to = …` → `shared.set_busy(None);`
- `shared.busy.store(true, Ordering::SeqCst);` после блока выбора фразы → `shared.set_busy(Some(&item.session));`
- `shared.busy.store(false, Ordering::SeqCst);` в ветке «синтез упал» → `shared.set_busy(None);`

- [ ] **Step 4: Убедиться, что проходят**

Run: `cargo test speaker::`
Expected: `test result: ok. 6 passed`.

- [ ] **Step 5: Коммит**

```bash
git add src/speaker.rs
git commit -m "speaker: report speaking/silent changes and the spoken session

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: Проект сессии в фокусе (`SessionRef.project`)

**Files:**
- Modify: `src/focus.rs`

**Interfaces:**
- Consumes: `claude_under`, `codex_under` (есть).
- Produces: `SessionRef { agent, id, transcript, pub project: String }` — имя папки `cwd` процесса агента; `fn project_of(pid: u32) -> String`.

- [ ] **Step 1: Написать падающий тест**

В `mod tests` файла `src/focus.rs`:

```rust
    #[test]
    fn project_is_process_cwd_dir_name() {
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(project_of(std::process::id()), cwd.file_name().unwrap().to_str().unwrap());
        assert_eq!(project_of(u32::MAX), "");
    }
```

- [ ] **Step 2: Убедиться, что падает**

Run: `cargo test project_is_process_cwd_dir_name`
Expected: ошибка компиляции `cannot find function `project_of``.

- [ ] **Step 3: Реализация**

В `struct SessionRef` после `pub transcript: PathBuf,`:

```rust
    pub project: String, // имя папки, где запущен агент — для «Читаю: <проект>»
```

После `fn under(...)`:

```rust
/// Имя папки рабочего каталога процесса; нет процесса — пустая строка.
fn project_of(pid: u32) -> String {
    std::fs::read_link(format!("/proc/{pid}/cwd"))
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_default()
}
```

В `claude_under` заменить `return Some(SessionRef { agent: Agent::Claude, id: sid.to_string(), transcript: p });` на:

```rust
                return Some(SessionRef { agent: Agent::Claude, id: sid.to_string(), transcript: p, project: project_of(pid as u32) });
```

В `codex_under` заменить `return Some(SessionRef { agent: Agent::Codex, id, transcript: target });` на:

```rust
                return Some(SessionRef { agent: Agent::Codex, id, transcript: target, project: project_of(pid) });
```

- [ ] **Step 4: Убедиться, что проходит**

Run: `cargo test focus::`
Expected: `test result: ok. 4 passed`.

- [ ] **Step 5: Коммит**

```bash
git add src/focus.rs
git commit -m "focus: project name of the focused agent session

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: Поток на соединение, подписка и рассылка состояния

**Files:**
- Modify: `src/ipc.rs`, `src/daemon.rs`

**Interfaces:**
- Consumes: `Shared::set_busy`, `Shared.changed`, `Shared.current` (Task 2).
- Produces:
  - `Msg::Subscribe` (`{"cmd":"subscribe"}`).
  - `fn serve(conn: UnixStream, tx: Sender<Event>)` — читает строки до закрытия; `subscribe` → `Event::Subscribe(клон потока)`, прочее → `Event::Msg`.
  - `enum Event { Msg(Msg), File(PathBuf), Config, Subscribe(UnixStream), Busy }`.
  - `State.subs: Vec<UnixStream>`; `State::status_line(&self) -> String` (JSON + `\n`); `State::subscribe(&mut self, UnixStream)`; `State::broadcast(&mut self)`; `State::apply(&mut self)` (голос, возраст очереди, рассылка); `fn test_state() -> State` в тестах.

- [ ] **Step 1: Написать падающие тесты**

В `src/ipc.rs`, тест `msg_roundtrip`, добавить строку перед закрывающей скобкой:

```rust
        assert!(matches!(serde_json::from_str::<Msg>("{\"cmd\":\"subscribe\"}").unwrap(), Msg::Subscribe));
```

В `mod tests` файла `src/daemon.rs` (после `use super::*;`):

```rust
    use crate::text::terms::Terms;

    fn test_state() -> State {
        let shared = Arc::new(Shared::new(Queue::new(Duration::from_secs(30)), "xenia".into(), "medium".into()));
        State {
            cfg: Config::default(),
            shared,
            terms: Arc::new(Mutex::new(Terms::from_str(""))),
            learn: mpsc::channel().0,
            dedup: Dedup::new(),
            asm: Assembler::new(),
            tail: Tailer::new(),
            focus_cache: (Instant::now(), None), // свежий кэш: hyprctl в тестах не зовётся
            subs: Vec::new(),
        }
    }

    fn next(r: &mut BufReader<UnixStream>) -> serde_json::Value {
        let mut l = String::new();
        r.read_line(&mut l).unwrap();
        serde_json::from_str(&l).unwrap()
    }

    fn reader(s: UnixStream) -> BufReader<UnixStream> {
        s.set_read_timeout(Some(Duration::from_millis(500))).unwrap();
        BufReader::new(s)
    }

    #[test]
    fn serve_reads_every_line_and_survives_garbage() {
        let (mut client, server) = UnixStream::pair().unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || serve(server, tx));
        client.write_all(b"{\"cmd\":\"subscribe\"}\n\xd0\xbc\xd1\x83\xd1\x81\xd0\xbe\xd1\x80\n{\"cmd\":\"nope\"}\n{\"cmd\":\"pause\"}\n").unwrap();
        assert!(matches!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), Event::Subscribe(_)));
        assert!(matches!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), Event::Msg(Msg::Pause)));
    }

    #[test]
    fn status_line_shape() {
        let st = test_state();
        assert_eq!(
            st.status_line(),
            "{\"running\":true,\"speaking\":false,\"paused\":false,\"mode\":\"manual\",\"read_intermediate\":true,\"speaker\":\"xenia\",\"rate\":\"medium\",\"project\":\"\"}\n"
        );
    }

    #[test]
    fn subscriber_gets_state_now_and_on_change() {
        let mut st = test_state();
        let (ours, theirs) = UnixStream::pair().unwrap();
        let mut r = reader(ours);
        st.subscribe(theirs);
        assert_eq!(next(&mut r)["paused"], false);
        st.on_msg(Msg::Pause);
        assert_eq!(next(&mut r)["paused"], true);
        st.shared.set_busy(Some("s"));
        st.broadcast(); // так цикл отвечает на Event::Busy
        assert_eq!(next(&mut r)["speaking"], true);
    }

    #[test]
    fn dead_subscriber_dropped_live_one_kept() {
        let mut st = test_state();
        let (dead, a) = UnixStream::pair().unwrap();
        let (live, b) = UnixStream::pair().unwrap();
        let mut r = reader(live);
        st.subscribe(a);
        st.subscribe(b);
        drop(dead);
        st.on_msg(Msg::Stop);
        assert_eq!(st.subs.len(), 1);
        next(&mut r); // при подписке
        next(&mut r); // после стопа
    }
```

- [ ] **Step 2: Убедиться, что падают**

Run: `cargo test daemon:: ipc::`
Expected: ошибки компиляции (`no variant named `Subscribe``, `cannot find function `serve``, `struct `State` has no field named `subs``).

- [ ] **Step 3: `Msg::Subscribe`**

В `src/ipc.rs`, в `enum Msg` после `Mode,`:

```rust
    /// Соединение остаётся открытым: сервис пишет строку состояния сразу и при каждом изменении.
    Subscribe,
```

- [ ] **Step 4: Поток на соединение, `Event`, подписчики**

В `src/daemon.rs` заменить строки `use std::io::BufRead;` и `use std::os::unix::net::UnixListener;` на:

```rust
use serde::Serialize;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
```

Заменить `enum Event` на:

```rust
enum Event {
    Msg(Msg),
    File(PathBuf),
    Config,
    Subscribe(UnixStream),
    Busy, // поток воспроизведения: начал или закончил говорить
}

/// Строка состояния для подписчиков (плагин панели). Порядок полей — протокол.
#[derive(Serialize)]
struct Status<'a> {
    running: bool,
    speaking: bool,
    paused: bool,
    mode: &'a str,
    read_intermediate: bool,
    speaker: &'a str,
    rate: &'a str,
    project: &'a str,
}
```

В `struct State` после `focus_cache: …,`:

```rust
    subs: Vec<UnixStream>,
```

В `run()` заменить весь блок от `let (tx, rx) = mpsc::channel::<Event>();` до конца блока с `listener.incoming()` (включая его закрывающую `}`) на:

```rust
    let (tx, rx) = mpsc::channel::<Event>();
    {
        // «говорит/молчит» из потока воспроизведения — в основной цикл
        let (btx, brx) = mpsc::channel::<()>();
        *shared.changed.lock().unwrap() = Some(btx);
        let tx = tx.clone();
        std::thread::spawn(move || {
            for () in brx {
                let _ = tx.send(Event::Busy);
            }
        });
    }
    let sock = ipc::socket_path();
    let _ = std::fs::remove_file(&sock);
    let listener = UnixListener::bind(&sock).expect("bind agent-speak.sock");
    {
        let tx = tx.clone();
        std::thread::spawn(move || {
            for conn in listener.incoming().flatten() {
                let tx = tx.clone();
                std::thread::spawn(move || serve(conn, tx));
            }
        });
    }
```

Внимание: `shared` к этому месту уже клонирован в поток воспроизведения выше (`let shared = shared.clone();` в своём блоке), внешний `shared` доступен — менять порядок не нужно.

В инициализации `let mut st = State { … }` после `focus_cache: …,` добавить `subs: Vec::new(),`. Цикл событий:

```rust
    for ev in rx {
        match ev {
            Event::Msg(m) => st.on_msg(m),
            Event::File(p) => st.on_file(&p),
            Event::Config => st.reload(),
            Event::Subscribe(s) => st.subscribe(s),
            Event::Busy => st.broadcast(),
        }
    }
```

После `fn final_message` добавить:

```rust
/// Одно соединение: команды построчно до закрытия (CLI и хуки — одна строка).
/// subscribe — копия потока уходит в основной цикл подписчиком, чтение команд продолжается.
// ponytail: поток на соединение без лимита — клиенты только свои (CLI, хуки, плагин)
fn serve(conn: UnixStream, tx: Sender<Event>) {
    let Ok(input) = conn.try_clone() else { return };
    for line in BufReader::new(input).lines() {
        let Ok(line) = line else { break };
        match serde_json::from_str::<Msg>(&line) {
            Ok(Msg::Subscribe) => match conn.try_clone() {
                Ok(w) => {
                    let _ = tx.send(Event::Subscribe(w));
                }
                Err(_) => break,
            },
            Ok(m) => {
                let _ = tx.send(Event::Msg(m));
            }
            Err(e) => eprintln!("agent-speak: плохое сообщение: {e}"),
        }
    }
}
```

В `impl State` заменить `fn reload` на:

```rust
    /// config.toml изменён (интерфейсом или руками) — применить без перезапуска.
    fn reload(&mut self) {
        self.cfg = Config::load();
        self.apply();
    }

    /// Настройки → поток воспроизведения и очередь, затем — подписчикам.
    fn apply(&mut self) {
        *self.shared.voice.lock().unwrap() = (self.cfg.speaker.clone(), self.cfg.rate.clone());
        self.shared.queue.lock().unwrap().set_max_age(Duration::from_secs(self.cfg.max_age_secs));
        self.broadcast();
    }

    fn status_line(&self) -> String {
        let speaking = self.shared.busy.load(Ordering::SeqCst);
        let paused = self.shared.queue.lock().unwrap().paused();
        let project = String::new();
        let st = Status {
            running: true,
            speaking,
            paused,
            mode: &self.cfg.mode,
            read_intermediate: self.cfg.read_intermediate,
            speaker: &self.cfg.speaker,
            rate: &self.cfg.rate,
            project: &project,
        };
        format!("{}\n", serde_json::to_string(&st).unwrap())
    }

    fn subscribe(&mut self, mut s: UnixStream) {
        let _ = s.set_write_timeout(Some(Duration::from_secs(1))); // зависший подписчик не держит цикл
        if s.write_all(self.status_line().as_bytes()).is_ok() {
            self.subs.push(s);
        }
    }

    /// Состояние всем подписчикам; отвалившиеся удаляются молча.
    fn broadcast(&mut self) {
        let line = self.status_line();
        self.subs.retain_mut(|s| s.write_all(line.as_bytes()).is_ok());
    }
```

В `fn on_msg` первой строкой добавить `let changes = matches!(m, Msg::Stop | Msg::Pause | Msg::Mode);`, в `match m` добавить ветку `Msg::Subscribe => {} // приходит как Event::Subscribe`, а после `match` — `if changes { self.broadcast(); }`.

- [ ] **Step 5: Убедиться, что проходят**

Run: `cargo test`
Expected: `0 failed` (прежние тесты и 4 новых в `daemon`).

- [ ] **Step 6: Коммит**

```bash
git add src/ipc.rs src/daemon.rs
git commit -m "daemon: thread per connection, subscribe and state broadcast

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: Команды `set` и `say`, проект в состоянии, выкладка сервиса

**Files:**
- Modify: `src/ipc.rs`, `src/daemon.rs`

**Interfaces:**
- Consumes: `Config::set` (Task 1), `SessionRef.project` (Task 3), `State::apply/broadcast/status_line`, `test_state()` (Task 4), `Shared.current` (Task 2).
- Produces: `Msg::Set { key: String, value: serde_json::Value }` (`{"cmd":"set","key":K,"value":V}`), `Msg::Say { text: String }` (`{"cmd":"say","text":T}`); `State.projects: HashMap<String, String>` (сессия → имя папки проекта); `project` в строке состояния, пока говорит.

- [ ] **Step 1: Написать падающие тесты**

В `src/ipc.rs`, тест `msg_roundtrip`, добавить:

```rust
        let set = serde_json::from_str::<Msg>("{\"cmd\":\"set\",\"key\":\"read_intermediate\",\"value\":false}").unwrap();
        assert!(matches!(set, Msg::Set { ref key, ref value } if key == "read_intermediate" && *value == serde_json::json!(false)));
        assert!(matches!(serde_json::from_str::<Msg>("{\"cmd\":\"say\",\"text\":\"т\"}").unwrap(), Msg::Say { .. }));
```

В `mod tests` файла `src/daemon.rs`: в `test_state()` после `subs: Vec::new(),` добавить `projects: HashMap::new(),` и тесты:

```rust
    #[test]
    fn set_applies_saves_broadcasts_and_rejects_bad() {
        // единственный тест, трогающий HOME: config::path() читает его при каждом вызове
        let home = std::env::temp_dir().join(format!("agent-speak-set-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        unsafe { std::env::set_var("HOME", &home) };
        let mut st = test_state();
        let (ours, theirs) = UnixStream::pair().unwrap();
        let mut r = reader(ours);
        st.subscribe(theirs);
        next(&mut r);
        st.on_msg(Msg::Set { key: "speaker".into(), value: serde_json::json!("baya") });
        assert_eq!(next(&mut r)["speaker"], "baya");
        assert_eq!(st.shared.voice.lock().unwrap().0, "baya");
        let saved = std::fs::read_to_string(home.join(".config/agent-speak/config.toml")).unwrap();
        assert!(saved.contains("speaker = \"baya\""), "{saved}");
        st.on_msg(Msg::Set { key: "speaker".into(), value: serde_json::json!("nobody") });
        st.on_msg(Msg::Set { key: "read_intermediate".into(), value: serde_json::json!("false") });
        st.on_msg(Msg::Set { key: "volume".into(), value: serde_json::json!("loud") });
        let mut l = String::new();
        assert!(r.read_line(&mut l).is_err(), "неверный set ничего не рассылает: {l}");
        assert_eq!((st.cfg.speaker.as_str(), st.cfg.read_intermediate), ("baya", true));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn say_queues_manual_reading() {
        let mut st = test_state();
        st.on_msg(Msg::Say { text: "Так звучит этот голос.".into() });
        let it = st.shared.queue.lock().unwrap().pop(Instant::now()).unwrap();
        assert_eq!(it.kind, Kind::Manual);
        assert!(it.text.contains("голос"), "{}", it.text);
    }

    #[test]
    fn project_of_spoken_session_from_hook_cwd() {
        let mut st = test_state();
        st.on_msg(Msg::Hook {
            kind: "user-prompt-submit".into(),
            payload: serde_json::json!({"session_id": "s1", "cwd": "/home/u/Projects/agent-speak"}),
        });
        st.shared.set_busy(Some("s1"));
        let v: serde_json::Value = serde_json::from_str(&st.status_line()).unwrap();
        assert_eq!((v["speaking"].clone(), v["project"].clone()), (serde_json::json!(true), serde_json::json!("agent-speak")));
        st.shared.set_busy(None);
        let v: serde_json::Value = serde_json::from_str(&st.status_line()).unwrap();
        assert_eq!(v["project"], "");
    }
```

- [ ] **Step 2: Убедиться, что падают**

Run: `cargo test daemon:: ipc::`
Expected: ошибки компиляции `no variant named `Set``, `no field `projects``.

- [ ] **Step 3: Варианты `Msg`**

В `src/ipc.rs`, в `enum Msg` после `Subscribe,`:

```rust
    /// Настройка из плагина: сервис проверяет, сохраняет config.toml и рассылает состояние.
    Set { key: String, value: serde_json::Value },
    /// Прочитать текст как ручное чтение (образец голоса).
    Say { text: String },
```

- [ ] **Step 4: `set`, `say`, проекты в `daemon.rs`**

Добавить импорт `use std::collections::HashMap;`. В `struct State` после `subs: Vec<UnixStream>,`:

```rust
    projects: HashMap<String, String>, // сессия → имя папки проекта (из хуков и окна в фокусе)
```

В `run()` при создании `State` добавить `projects: HashMap::new(),`.

`fn focused` заменить на:

```rust
    fn focused(&mut self) -> Option<SessionRef> {
        if self.focus_cache.0.elapsed() > Duration::from_millis(500) {
            self.focus_cache = (Instant::now(), focus::focused());
            if let Some(f) = &self.focus_cache.1 {
                self.projects.insert(f.id.clone(), f.project.clone());
            }
        }
        self.focus_cache.1.clone()
    }
```

В `status_line` заменить `let project = String::new();` на:

```rust
        let project = if speaking {
            self.projects.get(&*self.shared.current.lock().unwrap()).cloned().unwrap_or_default()
        } else {
            String::new()
        };
```

В `fn on_hook` после строки `let session = …;`:

```rust
        if let Some(cwd) = p["cwd"].as_str().filter(|_| !session.is_empty()) {
            let name = Path::new(cwd).file_name().and_then(|n| n.to_str()).unwrap_or_default();
            self.projects.insert(session.clone(), name.to_string());
        }
```

В `match m` метода `on_msg` перед `Msg::Subscribe => {}` добавить:

```rust
            Msg::Set { key, value } => {
                if self.cfg.set(&key, &value) {
                    self.cfg.save();
                    self.apply();
                } else {
                    eprintln!("agent-speak: set: неверно {key} = {value}");
                }
            }
            Msg::Say { text } => {
                self.enqueue(Agent::Claude, "say", &text, Kind::Manual);
            }
```

- [ ] **Step 5: Убедиться, что проходят**

Run: `cargo test`
Expected: `0 failed`.

- [ ] **Step 6: Выложить сервис и проверить вживую**

Run: `cd ~/Projects/agent-speak && cp ~/.config/agent-speak/config.toml /tmp/agent-speak-config.bak 2>/dev/null; ./install.sh && sleep 2 && systemctl --user is-active agent-speakd`
Expected: `active`.

Run:
```bash
S=$XDG_RUNTIME_DIR/agent-speak.sock
(printf '{"cmd":"subscribe"}\n'; sleep 1; printf '{"cmd":"set","key":"rate","value":"fast"}\n'; sleep 1; printf '{"cmd":"set","key":"rate","value":"warp"}\n'; sleep 1) | socat - UNIX-CONNECT:$S
grep rate ~/.config/agent-speak/config.toml
journalctl --user -u agent-speakd --since -1min | grep 'set: неверно'
```
Expected: первая строка состояния с `"running":true`, затем строки с `"rate":"fast"` (одна от `set`, возможно вторая от перечитывания файла); `rate = "fast"` в файле; в журнале `set: неверно rate = "warp"`.

Run: `(printf '{"cmd":"subscribe"}\n'; sleep 0.5; printf '{"cmd":"say","text":"Так звучит этот голос."}\n'; sleep 6) | socat - UNIX-CONNECT:$XDG_RUNTIME_DIR/agent-speak.sock`
Expected: звучит фраза; приходят строки `"speaking":true,…,"project":""` и затем `"speaking":false`.

Вернуть настройки: `cp /tmp/agent-speak-config.bak ~/.config/agent-speak/config.toml` (если файла не было — `rm ~/.config/agent-speak/config.toml`).

- [ ] **Step 7: Коммит**

```bash
git add src/ipc.rs src/daemon.rs
git commit -m "daemon: set and say commands, project of the spoken session

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: Модель плагина, переводы, тесты, манифест

**Files:**
- Create: `plugin/manifest.json`, `plugin/Model.js`, `plugin/I18n.js`, `plugin/test.js`

**Interfaces:**
- Consumes: протокол из Global Constraints.
- Produces (`Model.js`, `.pragma library`):
  - `SAMPLE` — `"Так звучит этот голос."`; `ICONS = { speaking, paused, idle, off }`; `OFFLINE` — состояние «сервис недоступен» (все поля, `running: false`).
  - `parse(line) -> object|null` — `null` на всём, что не строка состояния.
  - `view(st) -> { icon, lit, tip, arg }` — `tip` — английский ключ для `tr(tip, arg)`.
  - `busy(st) -> bool` — говорит или на паузе (правый клик и кнопки «Пауза»/«Стоп»).
  - `cmd(name)`, `set(key, value)`, `say(text)` — строки команд с `\n`.
  - `sections(tr) -> [{ key, caption, sample, options: [{ value, label }] }]` — разделы окна.
- `I18n.js`: `TABLES`, `textLanguage(env)`, `translator(lang)` — та же схема, что у `predmaxim.todo`.

- [ ] **Step 1: Написать падающий тест**

`plugin/test.js`:

```js
// node test.js
const fs = require("fs")
const assert = require("assert")
const load = (file, names) =>
  new Function(fs.readFileSync(__dirname + "/" + file, "utf8").replace(".pragma library", "") + "; return { " + names + " }")()
const M = load("Model.js", "OFFLINE, ICONS, SAMPLE, parse, view, busy, cmd, set, say, sections")
const I = load("I18n.js", "TABLES, translator")
const ru = I.translator("ru")

// The daemon's state line
const st = M.parse('{"running":true,"speaking":true,"paused":false,"mode":"auto","read_intermediate":true,"speaker":"xenia","rate":"medium","project":"agent-speak"}')
assert.strictEqual(st.speaker, "xenia")
// Anything else from the socket never replaces the state
assert.strictEqual(M.parse("{broken"), null)
assert.strictEqual(M.parse('{"speaking":true}'), null)
assert.strictEqual(M.parse(""), null)
assert.strictEqual(M.parse("null"), null)

// Icon, lit, tooltip per state (spec table)
const at = patch => Object.assign({}, st, patch)
const v = s => { const x = M.view(s); return [x.icon, x.lit, ru(x.tip, x.arg)] }
assert.deepStrictEqual(v(st), [M.ICONS.speaking, true, "Читаю: agent-speak"])
assert.deepStrictEqual(v(at({ project: "" })), [M.ICONS.speaking, true, "Читаю…"])
assert.deepStrictEqual(v(at({ speaking: false, paused: true })), [M.ICONS.paused, true, "Пауза — правый клик продолжит"])
assert.deepStrictEqual(v(at({ speaking: false })), [M.ICONS.idle, true, "Авто: агент в фокусе читается сам"])
assert.deepStrictEqual(v(at({ speaking: false, mode: "manual" })), [M.ICONS.idle, false, "Вручную: Super+Alt+R"])
assert.deepStrictEqual(v(M.OFFLINE), [M.ICONS.off, false, "Сервис озвучки не запущен"])
assert.deepStrictEqual(v(null), [M.ICONS.off, false, "Сервис озвучки не запущен"])
assert.strictEqual(new Set(Object.values(M.ICONS)).size, 4)
assert.strictEqual(M.view(st).tip, "Reading: %1") // English without a table
assert.strictEqual(I.translator("en")(M.view(st).tip, "x"), "Reading: x")

// Right click and Pause/Stop: only while speaking or paused
assert.strictEqual(M.busy(st), true)
assert.strictEqual(M.busy(at({ speaking: false, paused: true })), true)
assert.strictEqual(M.busy(at({ speaking: false })), false)
assert.strictEqual(M.busy(at({ running: false })), false)
assert.strictEqual(M.busy(M.OFFLINE), false)
assert.strictEqual(M.busy(null), false)

// Command lines
assert.strictEqual(M.cmd("pause"), '{"cmd":"pause"}\n')
assert.strictEqual(M.cmd("subscribe"), '{"cmd":"subscribe"}\n')
assert.strictEqual(M.set("read_intermediate", false), '{"cmd":"set","key":"read_intermediate","value":false}\n')
assert.deepStrictEqual(JSON.parse(M.set("speaker", "baya")), { cmd: "set", key: "speaker", value: "baya" })
assert.strictEqual(M.say(M.SAMPLE), '{"cmd":"say","text":"Так звучит этот голос."}\n')

// Window sections: keys, values the daemon accepts, value types as in the state line
const secs = M.sections(ru)
assert.deepStrictEqual(secs.map(s => s.key), ["mode", "read_intermediate", "speaker", "rate"])
assert.deepStrictEqual(secs.map(s => s.sample), [false, false, true, true])
assert.deepStrictEqual(secs.map(s => s.caption), ["РЕЖИМ", "ПРОМЕЖУТОЧНЫЕ СТАТУСЫ", "ГОЛОС", "СКОРОСТЬ"])
assert.deepStrictEqual(secs[0].options, [{ value: "auto", label: "Авто" }, { value: "manual", label: "Вручную" }])
assert.deepStrictEqual(secs[1].options, [{ value: true, label: "Читать" }, { value: false, label: "Только итог" }])
assert.deepStrictEqual(secs[2].options.map(o => o.value), ["xenia", "baya", "kseniya", "aidar", "eugene"])
assert.deepStrictEqual(secs[2].options.map(o => o.label), ["Xenia", "Baya", "Kseniya", "Aidar", "Eugene"])
assert.deepStrictEqual(secs[3].options.map(o => o.value), ["x-slow", "slow", "medium", "fast", "x-fast"])
assert.deepStrictEqual(secs[3].options.map(o => o.label), ["Очень медленно", "Медленно", "Обычно", "Быстро", "Очень быстро"])
for (const s of secs) for (const o of s.options) assert.strictEqual(typeof o.value, typeof st[s.key], s.key)

// Every tr("…") has a Russian line
for (const f of ["Panel.qml", "Indicator.qml", "Link.qml", "Model.js"]) {
  if (!fs.existsSync(__dirname + "/" + f)) continue
  for (const m of fs.readFileSync(__dirname + "/" + f, "utf8").matchAll(/\btr\("((?:[^"\\]|\\.)*)"/g))
    assert.ok(Object.prototype.hasOwnProperty.call(I.TABLES.ru, JSON.parse(`"${m[1]}"`)), f + ": no ru for " + m[1])
}

// QML: no own property or id may reuse a built-in name (rules.md §9, test of predmaxim.vpn)
const builtins = ["state", "status", "data", "children", "visible", "enabled", "opacity", "parent", "left", "right", "top", "bottom", "x", "y", "width", "height", "states", "focus", "settings", "opened", "bar"]
for (const f of ["Panel.qml", "Indicator.qml", "Link.qml"]) {
  if (!fs.existsSync(__dirname + "/" + f)) continue
  const src = fs.readFileSync(__dirname + "/" + f, "utf8")
  for (const m of src.matchAll(/^\s*(?:readonly\s+|required\s+)?property\s+\S+\s+(\w+)/gm))
    assert.ok(!builtins.includes(m[1]), f + ": property '" + m[1] + "' shadows a built-in")
  for (const m of src.matchAll(/\bid:\s*(\w+)/g))
    assert.ok(!builtins.includes(m[1]), f + ": id '" + m[1] + "' shadows a built-in")
}

console.log("ok")
```

- [ ] **Step 2: Убедиться, что падает**

Run: `node plugin/test.js`
Expected: `Error: ENOENT: no such file or directory, open '…/plugin/Model.js'`.

- [ ] **Step 3: `Model.js`**

```js
.pragma library

// agent-speakd's state line -> what the bar icon and the window show, and the
// command lines the plugin writes to the daemon's socket.

var SAMPLE = "Так звучит этот голос."

// Nerd Font glyphs: volume-high, pause, volume-low, volume-off.
var ICONS = {
  speaking: String.fromCodePoint(0xF057E),
  paused: String.fromCodePoint(0xF03E4),
  idle: String.fromCodePoint(0xF057F),
  off: String.fromCodePoint(0xF0581)
}

// No connection to the daemon.
var OFFLINE = { running: false, speaking: false, paused: false, mode: "", read_intermediate: false, speaker: "", rate: "", project: "" }

// A state line, or null for anything else (a broken line must not blank the icon).
function parse(line) {
  var s = null
  try { s = JSON.parse(line) } catch (e) { return null }
  return s && typeof s.running === "boolean" ? s : null
}

// Icon, whether it is lit in the indicator group, and the tooltip key for tr(tip, arg).
function view(st) {
  if (!st || !st.running) return { icon: ICONS.off, lit: false, tip: "Speech service is not running", arg: "" }
  if (st.paused) return { icon: ICONS.paused, lit: true, tip: "Paused — right click resumes", arg: "" }
  if (st.speaking) return st.project
    ? { icon: ICONS.speaking, lit: true, tip: "Reading: %1", arg: st.project }
    : { icon: ICONS.speaking, lit: true, tip: "Reading…", arg: "" }
  if (st.mode === "auto") return { icon: ICONS.idle, lit: true, tip: "Auto: the focused agent is read aloud", arg: "" }
  return { icon: ICONS.idle, lit: false, tip: "Manual: Super+Alt+R", arg: "" }
}

// Something to pause, resume or stop.
function busy(st) {
  return !!st && st.running && (st.speaking || st.paused)
}

function cmd(name) {
  return JSON.stringify({ cmd: name }) + "\n"
}

function set(key, value) {
  return JSON.stringify({ cmd: "set", key: key, value: value }) + "\n"
}

function say(text) {
  return JSON.stringify({ cmd: "say", text: text }) + "\n"
}

// The window's selectors. Values are exactly what the daemon's `set` accepts;
// sample: play SAMPLE after the choice, to compare by ear.
function sections(tr) {
  var voices = ["xenia", "baya", "kseniya", "aidar", "eugene"]
  return [
    { key: "mode", caption: tr("MODE"), sample: false,
      options: [{ value: "auto", label: tr("Auto") }, { value: "manual", label: tr("Manual") }] },
    { key: "read_intermediate", caption: tr("INTERMEDIATE STATUSES"), sample: false,
      options: [{ value: true, label: tr("Read") }, { value: false, label: tr("Final answer only") }] },
    { key: "speaker", caption: tr("VOICE"), sample: true,
      options: voices.map(function(v) { return { value: v, label: v.charAt(0).toUpperCase() + v.slice(1) } }) },
    { key: "rate", caption: tr("SPEED"), sample: true,
      options: [
        { value: "x-slow", label: tr("Very slow") }, { value: "slow", label: tr("Slow") },
        { value: "medium", label: tr("Normal") }, { value: "fast", label: tr("Fast") },
        { value: "x-fast", label: tr("Very fast") }
      ] }
  ]
}
```

- [ ] **Step 4: `I18n.js`**

```js
.pragma library
// Interface text in other languages, keyed by the English text itself, so a
// string missing from a table shows in English. tr("Reading: %1", x) fills in
// %1, %2… after the lookup. The same scheme as predmaxim.todo: Quickshell
// plugins get no .qm catalogues, so Qt's qsTr isn't used.
var TABLES = {
  ru: {
    "Speech": "Озвучка",
    "Reading: %1": "Читаю: %1", "Reading…": "Читаю…",
    "Paused — right click resumes": "Пауза — правый клик продолжит",
    "Auto: the focused agent is read aloud": "Авто: агент в фокусе читается сам",
    "Manual: Super+Alt+R": "Вручную: Super+Alt+R",
    "Speech service is not running": "Сервис озвучки не запущен",
    "Pause": "Пауза", "Resume": "Продолжить", "Stop": "Стоп",
    "MODE": "РЕЖИМ", "Auto": "Авто", "Manual": "Вручную",
    "INTERMEDIATE STATUSES": "ПРОМЕЖУТОЧНЫЕ СТАТУСЫ", "Read": "Читать", "Final answer only": "Только итог",
    "VOICE": "ГОЛОС", "SPEED": "СКОРОСТЬ",
    "Very slow": "Очень медленно", "Slow": "Медленно", "Normal": "Обычно", "Fast": "Быстро", "Very fast": "Очень быстро"
  }
}

// Text follows LC_MESSAGES, overridden by LC_ALL and defaulting to LANG, as
// the system splits it. env is name -> value (Quickshell.env in QML).
function textLanguage(env) {
  var names = ["LC_ALL", "LC_MESSAGES", "LANG"]
  for (var i = 0; i < names.length; i++) {
    var v = String(env(names[i]) || "").split(".")[0].split("@")[0]
    if (v && v !== "C" && v !== "POSIX") {
      var l = v.slice(0, 2).toLowerCase()
      return TABLES[l] ? l : "en"
    }
  }
  return "en"
}

function translator(lang) {
  var table = TABLES[lang] || {}
  return function(text) {
    var out = Object.prototype.hasOwnProperty.call(table, text) ? table[text] : text
    for (var i = 1; i < arguments.length; i++) out = out.split("%" + i).join(String(arguments[i]))
    return out
  }
}
```

- [ ] **Step 5: `manifest.json`**

```json
{
  "schemaVersion": 1,
  "id": "predmaxim.agent-speak",
  "name": "Agent speech",
  "version": "1.0.0",
  "author": "predmaxim",
  "description": "agent-speakd control: state, pause, mode, intermediate statuses, voice and speed",
  "kinds": ["bar-widget"],
  "entryPoints": { "barWidget": "Panel.qml" },
  "barWidget": {
    "displayName": "Agent speech",
    "description": "Read-aloud of AI agents: state and settings",
    "category": "Audio",
    "allowMultiple": false,
    "defaultSection": "right"
  }
}
```

- [ ] **Step 6: Убедиться, что проходит**

Run: `node plugin/test.js`
Expected: `ok`.

- [ ] **Step 7: Коммит**

```bash
git add plugin/manifest.json plugin/Model.js plugin/I18n.js plugin/test.js
git commit -m "plugin: model, translations and tests for the bar plugin

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: `Link.qml` — подписка на сокет и пробник API `Socket`

API `Socket` сверено по `/usr/lib/qt6/qml/Quickshell/Io/quickshell-io.qmltypes`: свойства `path`, `connected` (notify `connectionStateChanged`), `parser` (от `DataStream`), методы `write(string)`, `flush()`, сигнал `error`; `SplitParser` — сигнал `read(data)`, по умолчанию режет по `\n`. Не сверено и проверяется пробником: срабатывает ли `onConnectedChanged`, переподключает ли `connected = false; connected = true` после отказа, работает ли импорт папки плагина как пространства имён (`import ".." as Plugin` → `Plugin.Link`), на который опирается `Indicator.qml`.

**Files:**
- Create: `plugin/Link.qml`
- Временно (не коммитить): `plugin/probe/shell.qml`

**Interfaces:**
- Consumes: `Model.OFFLINE`, `Model.parse`, `Model.cmd` (Task 6).
- Produces: тип `Link` (`Item`, невидимый): `property bool wanted` (по умолчанию `true`; `false` — отключиться), `property var speech` (последнее состояние или `Model.OFFLINE`), `function send(line)` (пишет, только если соединение есть).

- [ ] **Step 1: `Link.qml`**

```qml
import QtQuick
import Quickshell
import Quickshell.Io
import "Model.js" as Model

// The connection to agent-speakd: subscribes on connect and keeps the latest
// state line in `speech`; commands go out with send(). No daemon (or it
// restarted) — `speech` is Model.OFFLINE and a retry runs every 5 s.
// Shared by Indicator.qml (in the predmaxim.indicators clone) and Panel.qml.
Item {
  id: link

  property bool wanted: true
  property var speech: Model.OFFLINE

  function send(line) {
    if (!sock.connected) return
    sock.write(line)
    sock.flush()
  }

  visible: false
  onWantedChanged: if (!wanted) sock.connected = false

  Socket {
    id: sock
    path: Quickshell.env("XDG_RUNTIME_DIR") + "/agent-speak.sock"
    onConnectedChanged: {
      if (connected) link.send(Model.cmd("subscribe"))
      else link.speech = Model.OFFLINE
    }
    parser: SplitParser {
      onRead: function(line) {
        var s = Model.parse(line)
        if (s) link.speech = s
      }
    }
  }

  Timer {
    interval: 5000
    repeat: true
    triggeredOnStart: true
    running: link.wanted && !sock.connected
    onTriggered: { sock.connected = false; sock.connected = true }
  }
}
```

- [ ] **Step 2: Пробник**

`plugin/probe/shell.qml` (временный, удаляется в Step 5):

```qml
import QtQuick
import Quickshell
import ".." as Plugin

ShellRoot {
  Plugin.Link {
    id: probe
    onSpeechChanged: console.log("PROBE speech", JSON.stringify(probe.speech))
  }
}
```

- [ ] **Step 3: Прогнать пробник с перезапуском сервиса**

Run:
```bash
cd ~/Projects/agent-speak
timeout 20 qs -p plugin/probe/shell.qml >/tmp/agent-speak-probe.log 2>&1 &
sleep 4; systemctl --user restart agent-speakd; sleep 14
grep -E 'PROBE|ERROR|error|WARN' /tmp/agent-speak-probe.log
```
Expected: сначала `PROBE speech {"running":true,…}` (значит: импорт папки, соединение, `write`, `SplitParser.onRead` работают), после перезапуска — `PROBE speech {"running":false,…}`, затем в течение ~5 с снова `{"running":true,…}`; ошибок QML нет.

Если `running:false` после перезапуска не появляется — `onConnectedChanged` не срабатывает: заменить в `Link.qml` `onConnectedChanged:` на `onConnectionStateChanged:` и прогнать снова. Если нет второго `running:true` — переподключение переключением `connected` не работает: в `onTriggered` заменить тело на `{ sock.path = ""; sock.path = Quickshell.env("XDG_RUNTIME_DIR") + "/agent-speak.sock"; sock.connected = true }` и прогнать снова. Если `Plugin.Link` не находится (`Plugin.Link is not a type`) — остановиться и сообщить: `Indicator.qml` (Task 9) придётся держать сокет у себя, не через `Link`.

- [ ] **Step 4: Тесты плагина**

Run: `node plugin/test.js`
Expected: `ok` (проверка имён свойств и `id` теперь охватывает `Link.qml`).

- [ ] **Step 5: Убрать пробник и закоммитить**

```bash
rm -r plugin/probe
git add plugin/Link.qml
git status --short   # нет plugin/probe и .superpowers/
git commit -m "plugin: Link — socket subscription with reconnect

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 8: `Panel.qml` — скрытый виджет и окно настроек, установка

**Files:**
- Create: `plugin/Panel.qml`
- Modify: `install.sh`, `README.md`

**Interfaces:**
- Consumes: `Link { wanted, speech, send() }` (Task 7); `Model.view/busy/cmd/set/say/sections/SAMPLE`, `I18n` (Task 6).
- Produces: виджет `predmaxim.agent-speak` с IPC `omarchy-shell predmaxim.agent-speak toggle|open|close` (даёт базовый `Panel` через `ipcTarget`); ссылка `~/.config/omarchy/plugins/predmaxim.agent-speak` → `plugin/`.

- [ ] **Step 1: `Panel.qml`**

```qml
import QtQuick
import Quickshell
import Quickshell.Wayland
import qs.Commons
import qs.Ui
import "Model.js" as Model
import "I18n.js" as I18n

// predmaxim.agent-speak: the settings window of agent-speakd. The bar icon is
// Indicator.qml in the predmaxim.indicators clone; this widget stays in the
// layout hidden, for the IPC toggle. Settings live in the daemon's
// config.toml: a choice goes out as `set`, and the window lights what the
// daemon confirms in its next state line, not what was clicked.
Panel {
  id: root
  moduleName: "predmaxim.agent-speak"
  ipcTarget: "predmaxim.agent-speak"

  readonly property var tr: I18n.translator(I18n.textLanguage(function(name) { return Quickshell.env(name) }))
  readonly property var speech: link.speech
  readonly property var look: Model.view(root.speech)
  readonly property var sections: Model.sections(root.tr)

  function choose(key, value, sample) {
    link.send(Model.set(key, value))
    if (sample) link.send(Model.say(Model.SAMPLE))
  }

  visible: false
  implicitWidth: 0
  implicitHeight: 0

  onOpenedChanged: if (opened) Qt.callLater(function() { escCatcher.forceActiveFocus() })

  // Own subscription while the window is open.
  Link { id: link; wanted: root.opened }

  // A modal in the middle of the screen, like predmaxim.todo: a click on the
  // dimmed screen or Esc closes it.
  PanelWindow {
    id: modal
    screen: root.QsWindow.window ? root.QsWindow.window.screen : null
    visible: root.opened
    color: Color.menu.scrim
    exclusionMode: ExclusionMode.Ignore
    anchors { top: true; bottom: true; left: true; right: true }
    WlrLayershell.namespace: "predmaxim-agent-speak"
    WlrLayershell.layer: WlrLayer.Overlay
    WlrLayershell.keyboardFocus: visible ? WlrKeyboardFocus.Exclusive : WlrKeyboardFocus.None

    MouseArea { anchors.fill: parent; onClicked: root.close() }

    Item {
      id: escCatcher
      focus: true
      Keys.onEscapePressed: root.close()
    }

    BorderSurface {
      id: card
      anchors.centerIn: parent
      width: Math.min(Style.space(480), modal.width - Style.space(80))
      height: Math.min(column.implicitHeight + card.contentTopInset + card.contentBottomInset, modal.height * 0.85)
      color: Color.popups.background
      borderSpec: Border.surfaceSpec("popups", "border", Color.popups.border, Math.max(1, Style.space(2)))
      padding: Style.spacing.panelPadding
      radius: Style.cornerRadius

      MouseArea { anchors.fill: parent }   // clicks on the card stay on it

      Column {
        id: column
        anchors.fill: parent
        anchors.topMargin: card.contentTopInset
        anchors.rightMargin: card.contentRightInset
        anchors.bottomMargin: card.contentBottomInset
        anchors.leftMargin: card.contentLeftInset
        spacing: Style.space(14)

        PanelHero {
          title: root.tr("Speech")
          meta: root.tr(root.look.tip, root.look.arg)
          foreground: root.bar.foreground
          fontFamily: root.bar.fontFamily
          iconComponent: Text {
            text: root.look.icon
            color: root.bar.foreground
            font.family: root.bar.fontFamily
            font.pixelSize: Style.font.display
          }
        }

        Row {
          visible: root.speech.running
          spacing: Style.space(8)

          Button {
            text: root.speech.paused ? root.tr("Resume") : root.tr("Pause")
            enabled: Model.busy(root.speech)
            bordered: true
            foreground: root.bar.foreground
            fontFamily: root.bar.fontFamily
            onClicked: link.send(Model.cmd("pause"))
          }

          Button {
            text: root.tr("Stop")
            enabled: Model.busy(root.speech)
            bordered: true
            foreground: root.bar.foreground
            fontFamily: root.bar.fontFamily
            onClicked: link.send(Model.cmd("stop"))
          }
        }

        PanelSeparator { foreground: root.bar.foreground }

        // No daemon: what to run instead of the selectors.
        Column {
          visible: !root.speech.running
          width: parent.width
          spacing: Style.space(6)

          Text {
            textFormat: Text.PlainText
            text: root.tr("Speech service is not running")
            color: root.bar.foreground
            font.family: root.bar.fontFamily
            font.pixelSize: Style.font.body
            font.bold: true
          }

          Text {
            textFormat: Text.PlainText
            text: "systemctl --user start agent-speakd"
            color: Qt.darker(root.bar.foreground, 1.4)
            font.family: root.bar.fontFamily
            font.pixelSize: Style.font.caption
          }
        }

        // Selectors: the current option lit like the headphones mode in predmaxim.audio.
        Repeater {
          model: root.speech.running ? root.sections : []

          Column {
            id: group
            required property var modelData
            width: column.width
            spacing: Style.space(6)

            PanelSectionHeader {
              text: group.modelData.caption
              foreground: root.bar.foreground
              fontFamily: root.bar.fontFamily
            }

            Flow {
              width: parent.width
              spacing: Style.space(6)

              Repeater {
                model: group.modelData.options

                CursorSurface {
                  id: chip
                  required property var modelData
                  width: chipLabel.implicitWidth + Style.spacing.rowPaddingX * 2
                  height: chipLabel.implicitHeight + Style.spacing.xl
                  current: root.speech[group.modelData.key] === chip.modelData.value
                  hasCursor: chipMouse.containsMouse
                  foreground: root.bar.foreground

                  Text {
                    id: chipLabel
                    anchors.centerIn: parent
                    textFormat: Text.PlainText
                    text: chip.modelData.label
                    color: root.bar.foreground
                    font.family: root.bar.fontFamily
                    font.pixelSize: Style.font.body
                    font.bold: chip.current
                  }

                  MouseArea {
                    id: chipMouse
                    anchors.fill: parent
                    hoverEnabled: true
                    cursorShape: Qt.PointingHandCursor
                    onClicked: root.choose(group.modelData.key, chip.modelData.value, group.modelData.sample)
                  }
                }
              }
            }
          }
        }
      }
    }
  }
}
```

- [ ] **Step 2: Тесты плагина**

Run: `node plugin/test.js`
Expected: `ok` (все `tr("…")` из `Panel.qml` есть в `I18n.js`, запрещённых имён нет).

- [ ] **Step 3: Ссылка в `install.sh`, строка в `README.md`**

В `install.sh` после строки `ln -sfn "$PWD/systemd/agent-speakd.service" ~/.config/systemd/user/agent-speakd.service` добавить:

```bash
# Плагин панели Omarchy; значок в группе индикаторов копирует хук patch_indicators (omarchy-dotfiles)
mkdir -p ~/.config/omarchy/plugins
ln -sfn "$PWD/plugin" ~/.config/omarchy/plugins/predmaxim.agent-speak
```

В `README.md` после строки `- Настройки: …` добавить:

```markdown
- Плагин панели Omarchy `predmaxim.agent-speak` (`plugin/`, ставится ссылкой `install.sh`): значок в центральной группе индикаторов (левый клик — окно настроек, правый — пауза/продолжение), окно меняет настройки командой `set` в сокет сервиса. Тесты: `node plugin/test.js`.
```

- [ ] **Step 4: Установить и поставить скрытый виджет на панель**

Run:
```bash
cd ~/Projects/agent-speak && ./install.sh
omarchy-shell shell rescanPlugins
omarchy-plugin-list --json | jq '.[] | select(.id == "predmaxim.agent-speak") | {id, enabled}'
omarchy bar put predmaxim.agent-speak --section right --after predmaxim.todo
omarchy restart shell
sleep 3; jq -c '.bar.layout.right | map(.id)' ~/.config/omarchy/shell.json
```
Expected: плагин в списке; `predmaxim.agent-speak` в правом разделе после `predmaxim.todo`; на панели ничего нового не видно (виджет скрыт).

- [ ] **Step 5: Окно вживую**

Run:
```bash
omarchy-shell predmaxim.agent-speak toggle; sleep 1
grim -o eDP-2 /tmp/agent-speak-window.png
```
Посмотреть `/tmp/agent-speak-window.png` (Read). Expected: модалка по центру на затемнении: значок, «Озвучка», строка состояния; «Пауза» и «Стоп» неактивны; четыре раздела капсом (РЕЖИМ, ПРОМЕЖУТОЧНЫЕ СТАТУСЫ, ГОЛОС, СКОРОСТЬ), в каждом подсвечен текущий пункт из `config.toml`; карточка не вылезает за экран.

Run: `hyprctl dispatch 'hl.dsp.send_shortcut({ mods = "", key = "Escape" })'; sleep 0.5; grim -o eDP-2 /tmp/agent-speak-closed.png`
Expected: на снимке окна нет.

Run: `systemctl --user stop agent-speakd; omarchy-shell predmaxim.agent-speak toggle; sleep 1; grim -o eDP-2 /tmp/agent-speak-offline.png; omarchy-shell predmaxim.agent-speak toggle; systemctl --user start agent-speakd`
Expected: в окне вместо разделов «Сервис озвучки не запущен» и `systemctl --user start agent-speakd`, кнопок нет.

- [ ] **Step 6: Коммит**

```bash
git add plugin/Panel.qml install.sh README.md
git commit -m "plugin: settings window and install link

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 9: `Indicator.qml` и хук `patch_indicators`

**Files:**
- Create: `plugin/Indicator.qml`
- Modify: `~/omarchy-dotfiles/home/.config/omarchy/hooks/post-update.d/keep-custom-widgets.sh` (функция `patch_indicators`, строки ~390–425) — править **по пути в репозитории**, не через `~/.config/...`

**Interfaces:**
- Consumes: `Link`, `Model.view/busy/cmd`, `I18n` (Tasks 6–7); IPC `predmaxim.agent-speak toggle` (Task 8); `BarIndicator` (`active`, `activeText`, `inactiveText`, `activeTooltipText`, `inactiveTooltipText`, сигнал `pressed(int button)`).
- Produces: `indicators/AgentSpeak.qml` в клоне `predmaxim.indicators`, запись `"AgentSpeak"` в `defaultIndicatorEntries`.

- [ ] **Step 1: `Indicator.qml`**

```qml
import QtQuick
import Quickshell
import qs.Commons
import qs.Ui
import "@PLUGIN_DIR@" as Plugin
import "@PLUGIN_DIR@/Model.js" as Model
import "@PLUGIN_DIR@/I18n.js" as I18n

// predmaxim.agent-speak among the bar's indicators: lit while speaking, paused
// or in auto mode, otherwise only when the group is hovered. Left click
// toggles the settings window (the hidden widget, Panel.qml); right click
// pauses or resumes, and only while there is something to pause.
// keep-custom-widgets.sh copies this file into the predmaxim.indicators clone
// as indicators/AgentSpeak.qml and fills in @PLUGIN_DIR@.
BarIndicator {
  id: root

  readonly property var tr: I18n.translator(I18n.textLanguage(function(name) { return Quickshell.env(name) }))
  readonly property var look: Model.view(link.speech)

  active: look.lit
  activeText: look.icon
  inactiveText: look.icon
  activeTooltipText: root.tr(look.tip, look.arg)
  inactiveTooltipText: root.tr(look.tip, look.arg)

  onPressed: function(button) {
    if (button === Qt.RightButton) {
      if (Model.busy(link.speech)) link.send(Model.cmd("pause"))
    } else if (button === Qt.LeftButton) {
      Quickshell.execDetached(["omarchy-shell", "predmaxim.agent-speak", "toggle"])
    }
  }

  Plugin.Link { id: link }
}
```

- [ ] **Step 2: Тесты плагина**

Run: `node ~/Projects/agent-speak/plugin/test.js`
Expected: `ok`.

- [ ] **Step 3: Хук**

В `~/omarchy-dotfiles/home/.config/omarchy/hooks/post-update.d/keep-custom-widgets.sh` заменить:

```bash
# My plugins' icons among the indicators (<plugin>/Indicator.qml of annotate,
# todo): the widget loads indicators only from its own folder, so the
# clone gets copies.
patch_indicators() {
  local name
  for name in annotate todo; do
    local source=$plugins/$user.$name/Indicator.qml
    [[ -f $source ]] || return 1
    sed "s|@PLUGIN_DIR@|../../$user.$name|" "$source" >"$1/indicators/${name^}.qml" || return 1
  done
```

на:

```bash
# My plugins' icons among the indicators (<plugin>/Indicator.qml of annotate,
# todo, agent-speak): the widget loads indicators only from its own folder, so
# the clone gets copies.
patch_indicators() {
  local name
  for name in annotate todo; do
    local source=$plugins/$user.$name/Indicator.qml
    [[ -f $source ]] || return 1
    sed "s|@PLUGIN_DIR@|../../$user.$name|" "$source" >"$1/indicators/${name^}.qml" || return 1
  done
  # agent-speak lives in its own repo and its install.sh links the plugin in;
  # until then the group builds without its icon.
  local speak=$plugins/$user.agent-speak/Indicator.qml
  if [[ -f $speak ]]; then
    sed "s|@PLUGIN_DIR@|../../$user.agent-speak|" "$speak" >"$1/indicators/AgentSpeak.qml" || return 1
  fi
```

И в той же функции заменить строку-замену списка:

```bash
'  readonly property var defaultIndicatorEntries: [ "Dictation", "ScreenRecording", "Reminder", "NightLight", "StayAwake", "Mic", "Notifications", "Todo", "Annotate" ]'
```

на:

```bash
'  readonly property var defaultIndicatorEntries: [ "Dictation", "ScreenRecording", "Reminder", "NightLight", "StayAwake", "Mic", "Notifications", "Todo", "Annotate", "AgentSpeak" ]'
```

(Якорь — первая строка `patch_once` со стоковым списком — не меняется. Без файла `AgentSpeak.qml` запись в списке только даёт предупреждение загрузчика, группа работает.)

- [ ] **Step 4: Прогнать хук**

Run:
```bash
bash -n ~/omarchy-dotfiles/home/.config/omarchy/hooks/post-update.d/keep-custom-widgets.sh
~/.config/omarchy/hooks/post-update.d/keep-custom-widgets.sh; echo "exit $?"
grep -n '@PLUGIN_DIR@\|predmaxim.agent-speak' ~/.config/omarchy/plugins/predmaxim.indicators/indicators/AgentSpeak.qml | head -3
grep -c AgentSpeak ~/.config/omarchy/plugins/predmaxim.indicators/Indicators.qml
```
Expected: `exit 0`; в `AgentSpeak.qml` импорты `../../predmaxim.agent-speak`, строк с `@PLUGIN_DIR@` нет; `AgentSpeak` в `Indicators.qml` — 1. Хук сам делает `omarchy restart shell`, если клон изменился; если нет — `omarchy restart shell`.

- [ ] **Step 5: Значок вживую — состояния и размер**

Сохранить настройки: `cp ~/.config/agent-speak/config.toml /tmp/agent-speak-config.bak`. Высота панели для обрезки не нужна — смотреть снимок целиком (Read), значок в центральной группе рядом с колокольчиком/todo.

Run (авто, тишина):
```bash
S=$XDG_RUNTIME_DIR/agent-speak.sock
printf '{"cmd":"set","key":"mode","value":"auto"}\n' | socat - UNIX-CONNECT:$S; sleep 1
grim -o eDP-2 /tmp/as-auto.png
```
Expected: горит динамик (`volume-low`), того же размера и базовой линии, что соседи.

Run (говорит и пауза):
```bash
printf '{"cmd":"say","text":"Проверка значка на панели, раз, два, три, четыре, пять, шесть, семь."}\n' | socat - UNIX-CONNECT:$S
sleep 1.5; grim -o eDP-2 /tmp/as-speaking.png
printf '{"cmd":"pause"}\n' | socat - UNIX-CONNECT:$S; sleep 1; grim -o eDP-2 /tmp/as-paused.png
printf '{"cmd":"stop"}\n' | socat - UNIX-CONNECT:$S
```
Expected: `/tmp/as-speaking.png` — динамик с волнами; `/tmp/as-paused.png` — значок паузы.

Run (вручную и недоступен):
```bash
printf '{"cmd":"set","key":"mode","value":"manual"}\n' | socat - UNIX-CONNECT:$S; sleep 1; grim -o eDP-2 /tmp/as-manual.png
systemctl --user stop agent-speakd; sleep 1; grim -o eDP-2 /tmp/as-off.png
systemctl --user start agent-speakd; sleep 7; grim -o eDP-2 /tmp/as-back.png
cp /tmp/agent-speak-config.bak ~/.config/agent-speak/config.toml
```
Expected: `as-manual`, `as-off` — значок не горит (виден только при наведении на группу); `as-back` — состояние вернулось без перезапуска оболочки (переподключение раз в 5 с).

Если на снимках значок мельче соседей — в `plugin/Indicator.qml` после `inactiveText: look.icon` добавить строку `  fontSize: Style.font.body` (как микрофон, `rules.md` §9), снова прогнать хук (Step 4) и повторить снимок `as-auto`.

- [ ] **Step 6: Клики вживую (руками пользователя)**

Попросить пользователя: левый клик по значку — окно открывается, повторный — закрывается; правый клик при тишине — ничего; запустить чтение (`Super+Alt+R` на окне агента), правый клик — пауза, ещё раз — продолжение. В окне выбрать голос Baya — звучит «Так звучит этот голос.», подсветка переходит на Baya после ответа сервиса, `grep speaker ~/.config/agent-speak/config.toml` → `baya`; вернуть прежний голос в окне.

- [ ] **Step 7: Коммиты**

```bash
cd ~/Projects/agent-speak
git add plugin/Indicator.qml
git commit -m "plugin: bar indicator for the central indicators group

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

(Хук коммитится в `omarchy-dotfiles` вместе с документацией в Task 10.)

---

### Task 10: `omarchy-dotfiles` — правила, журнал, раскладка панели

**Files:**
- Modify: `~/omarchy-dotfiles/docs/rules.md` (§9), `~/omarchy-dotfiles/docs/changelog.md`, `~/omarchy-dotfiles/home/.config/omarchy/shell.json` (через `save.sh`); хук из Task 9.

**Interfaces:**
- Consumes: результат Tasks 8–9 (виджет на панели, значок в группе).
- Produces: задокументированное исключение и запись в журнале.

- [ ] **Step 1: `rules.md` §9 — список значков группы**

В `~/omarchy-dotfiles/docs/rules.md` заменить:

```
флаг-файл `$XDG_RUNTIME_DIR/predmaxim-annotate`). Стокового
```

на:

```
флаг-файл `$XDG_RUNTIME_DIR/predmaxim-annotate`), озвучка агентов (`predmaxim.agent-speak`: горит, пока говорит, на паузе или в режиме «авто»; правый клик — пауза/продолжение, только когда есть что ставить на паузу). Стокового
```

- [ ] **Step 2: `rules.md` §9 — исключение про настройки**

Заменить:

```
читать — `setting(name, fallback)`. Своих файлов настроек не заводить.
```

на:

```
читать — `setting(name, fallback)`. Своих файлов настроек не заводить.
- **Исключение — `predmaxim.agent-speak`**: настройки озвучки живут в `~/.config/agent-speak/config.toml` сервиса `agent-speakd`, а не в `shell.json` — это настройки сервиса, он читает их без оболочки. Окно плагина меняет их командой `set` в сокет сервиса (`$XDG_RUNTIME_DIR/agent-speak.sock`), сервис проверяет значение, сохраняет файл и рассылает состояние подписчикам (значок и окно). Код плагина — в репозитории `predmaxim/agent-speak` (`plugin/`), ставится ссылкой его `install.sh`, как `predmaxim.vpn`; значок хук `patch_indicators` копирует, только если плагин установлен.
```

- [ ] **Step 3: Журнал**

В `~/omarchy-dotfiles/docs/changelog.md` заменить:

```
## 2026-10-02

- **Озвучка агентов — свой сервис
```

на:

```
## 2026-10-02

- **Плагин панели для озвучки `predmaxim.agent-speak`** (код в [`predmaxim/agent-speak`](https://github.com/predmaxim/agent-speak), `plugin/`; спецификация там же, `docs/superpowers/specs/2026-10-02-bar-plugin-design.md`). Значок в центральной группе индикаторов (`patch_indicators` копирует `Indicator.qml` как `AgentSpeak.qml`): говорит / пауза / авто / вручную / сервис не запущен; левый клик — окно, правый — пауза/продолжение. Окно: пауза и стоп, режим, чтение промежуточных статусов, голос и скорость с образцом на слух. Плагин говорит с сервисом через его unix-сокет (`subscribe`/`set`/`say`); настройки — в `config.toml` сервиса (исключение в `rules.md` §9). Скрытый виджет справа на панели после todo держит IPC `toggle`.
- **Озвучка агентов — свой сервис
```

- [ ] **Step 4: Раскладка панели в репозиторий**

Run: `~/omarchy-dotfiles/save.sh`
Expected: в списке изменений `home/.config/omarchy/shell.json` (добавлен `predmaxim.agent-speak`), правленые `docs/` и хук; других неожиданных файлов нет (если есть — не добавлять их в коммит).

Run: `cd ~/omarchy-dotfiles && git diff home/.config/omarchy/shell.json`
Expected: только запись `{"id": "predmaxim.agent-speak"}` в `right`.

- [ ] **Step 5: Коммит в `omarchy-dotfiles`**

```bash
cd ~/omarchy-dotfiles
git add home/.config/omarchy/hooks/post-update.d/keep-custom-widgets.sh docs/rules.md docs/changelog.md home/.config/omarchy/shell.json
git commit -m "agent-speak bar plugin: indicator in the group, hidden widget, rules exception

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

- [ ] **Step 6: Итоговая проверка**

Run: `cd ~/Projects/agent-speak && cargo test 2>&1 | tail -2 && node plugin/test.js && git status --short`
Expected: `0 failed`, `ok`, рабочее дерево чистое (нет `plugin/probe`, `.superpowers/` не в индексе).

---

## Self-review

- Покрытие спецификации: Часть 1 п.1 — Task 4 (`serve`); п.2 `subscribe` и строка состояния, `project` — Tasks 2, 3, 4, 5; п.3 `set` — Tasks 1, 5; п.4 `say` — Task 5; п.5 моменты рассылки — Task 4 (пауза, стоп, режим, перечитывание файла, начало/конец речи), Task 5 (`set`); п.6 устройство (событие из потока воспроизведения, удаление подписчика) — Tasks 2, 4. Часть 2 — Tasks 7, 9 (таблица состояний — `Model.view` в Task 6). Часть 3 — Task 8. Часть 4 — тесты в Tasks 1–6, `install.sh` — Task 8, хук/`bar put`/`rules.md`/журнал/`omarchy restart shell` — Tasks 8–10.
- Заглушек нет; условные шаги (Task 7 Step 3, Task 9 Step 5) содержат точный код замены.
- Имена сквозные: `set_busy(Option<&str>)`, `current`, `changed`, `project_of`, `serve`, `status_line`, `subscribe`, `broadcast`, `apply`, `projects`, `Link { wanted, speech, send }`, `Model.{parse, view, busy, cmd, set, say, sections, SAMPLE, ICONS, OFFLINE}`.
