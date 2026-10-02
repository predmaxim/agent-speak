# agent-speak Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Сервис, который озвучивает тексты Claude Code и Codex (включая промежуточные статусы) по ходу работы агента, по-русски, через Silero.

**Architecture:** Один Rust-бинарник `agent-speak`: подкоманда `daemon` — сервис (`systemd --user`), остальные подкоманды — клиенты, шлющие JSON-строку в unix-сокет сервиса. Сервис собирает текст из хуков и из транскриптов (inotify), готовит его правилами + словарём терминов (фоновая мини-модель пополняет словарь), ставит в очередь и играет через постоянный `pw-cat`; синтез — дочерний Python-процесс с Silero, модель в памяти, обмен по unix-сокету.

**Tech Stack:** Rust 1.98 (std-потоки, без async), crates `serde`/`serde_json`, `toml`, `notify` 6, `base64`, `libc`; Python 3.14 + torch CPU + Silero v5 (уже стоят в `~/.local/share/agent-speak/venv`, модель `~/.local/share/agent-speak/v5_ru.pt`).

**Spec:** `docs/superpowers/specs/2026-10-02-agent-speak-design.md`

## Global Constraints

- Репозиторий: `~/Projects/agent-speak`; бинарник ставится ссылкой в `~/.local/bin/agent-speak`.
- Сокет сервиса: `$XDG_RUNTIME_DIR/agent-speak.sock`; сокет синтеза: `$XDG_RUNTIME_DIR/agent-speak-tts.sock`.
- Данные: `~/.local/share/agent-speak/` (`venv/`, `v5_ru.pt`, `silero_tts.py`, `terms.tsv`); настройки: `~/.config/agent-speak/config.toml`.
- Звук: PCM s16le, 48000 Гц, моно; `pw-cat --playback --raw --format s16 --rate 48000 --channels 1 -`.
- Настройки и значения по умолчанию: `mode = "manual"`, `speaker = "xenia"`, `rate = "medium"`, `max_age_secs = 30`, `read_intermediate = true`.
- Сервис перечитывает `config.toml` при изменении файла (им будет управлять отдельный интерфейс); перезапуск не нужен.
- Голоса: `xenia`, `baya`, `kseniya`, `aidar`, `eugene`.
- Клиентские подкоманды (`hook`, `stop`, `pause`, `mode`) при недоступном сервисе выходят с кодом 0 молча; `read` — уведомление «Сервис озвучки не запущен».
- Модель словаря Claude: `claude -p --model haiku --no-session-persistence --setting-sources "" --strict-mcp-config --tools ""` c `MAX_THINKING_TOKENS=0`, `CLAUDE_CODE_DISABLE_AUTO_MEMORY=1`, `AGENT_SPEAK=1`; Codex: `codex exec --ephemeral --skip-git-repo-check -m <быстрая модель из ~/.codex/models_cache.json> -c model_reasoning_effort=low`.
- Хуки не должны тормозить агента: клиент только пишет в сокет и выходит.
- Уведомления — `notify-send -t 2500 -a "Озвучка" <заголовок> [текст]`.
- Комментарии в коде — по-русски, коротко, как в остальных проектах пользователя.

## Review Focus

1. Транскрипт дописывается во время чтения (последняя строка без `\n`) — строка не теряется и не читается дважды. → тест в Task 6 (`tail_keeps_partial_line`).
2. Сервис стартует при уже длинных транскриптах — старая история не зачитывается, читается только новое. → тест в Task 6 (`tail_starts_at_end`).
3. Сообщение пришло и из `MessageDisplay`, и из транскрипта (в т. ч. как `narration`) — звучит один раз. → тест в Task 7 (`display_suppresses_transcript_same_message`).
4. Пауза дольше `max_age_secs` — статусы после возобновления не выброшены. → тест в Task 8 (`pause_freezes_age`).
5. Ответ из одного кода/таблицы/путей — не тишина без объяснения и не мусор: строки таблицы читаются, код выброшен, «нечего читать» если пусто. → тесты в Task 3 (`table_rows_read`, `only_code_gives_nothing`) и Task 12 (`read` с пустым результатом шлёт «Нечего читать»).

---

## File Structure

```
Cargo.toml
src/main.rs            разбор подкоманд, запуск daemon / клиента
src/ipc.rs             Msg (serde), путь сокета, send()
src/config.rs          Config: загрузка/сохранение toml
src/notice.rs          notify(): notify-send
src/text/mod.rs        prepare(): весь конвейер подготовки
src/text/clean.rs      clean(): markdown/код/пути/ссылки/таблицы → текст с пометками GAP
src/text/sentences.rs  split(), drop_gutted()
src/text/numbers.rs    numbers_to_words()
src/text/terms.rs      Terms: словарь, транслитерация, аббревиатуры
src/source/tail.rs     Tailer: новые полные строки файлов
src/source/claude.rs   разбор строк транскрипта Claude, last_turn()
src/source/codex.rs    разбор rollout Codex, last_turn()
src/assemble.rs        Assembler: куски MessageDisplay → предложения
src/dedup.rs           Dedup: повторы по message_id и хэшу
src/queue.rs           Queue: возраст, срочные, пауза
src/tts.rs             Tts: запуск/перезапуск silero_tts.py, synth()
src/audio.rs           Player: постоянный pw-cat
src/speaker.rs         поток воспроизведения
src/focus.rs           окно в фокусе → сессия агента
src/learn.rs           фоновое пополнение словаря моделью
src/daemon.rs          цикл событий: ipc, inotify, хуки, команды
python/silero_tts.py   сервер синтеза
data/terms.tsv         стартовый словарь
systemd/agent-speakd.service
install.sh
tests/fixtures/        claude.jsonl, codex.jsonl, message_display.jsonl
```

---

### Task 1: Каркас и запись реальных событий `MessageDisplay`

Формат хука известен только по чужому скрипту — сначала фиксируем настоящий.

**Files:**
- Create: `Cargo.toml`, `src/main.rs`, `.gitignore`, `tests/fixtures/message_display.jsonl`
- Modify (временно): `~/.claude/settings.json`

**Interfaces:**
- Produces: крейт `agent-speak` (бинарник `agent-speak`), фикстура `tests/fixtures/message_display.jsonl` — по одному JSON-событию хука на строку.

- [ ] **Step 1: Каркас крейта**

`Cargo.toml`:
```toml
[package]
name = "agent-speak"
version = "0.1.0"
edition = "2024"

[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
toml = "0.8"
notify = "6"
base64 = "0.22"
libc = "0.2"
```

`src/main.rs`:
```rust
fn main() {
    println!("agent-speak");
}
```

`.gitignore`:
```
/target
```

Run: `cd ~/Projects/agent-speak && cargo build`
Expected: `Finished`.

- [ ] **Step 2: Временный хук-логгер**

```bash
cp ~/.claude/settings.json ~/.claude/settings.json.bak.md-probe
jq '.hooks.MessageDisplay = [{"hooks":[{"type":"command","command":"cat >> /tmp/md-probe.jsonl; echo >> /tmp/md-probe.jsonl"}]}]' \
  ~/.claude/settings.json > /tmp/s.json && mv /tmp/s.json ~/.claude/settings.json && chmod 600 ~/.claude/settings.json
```

- [ ] **Step 3: Вызвать события**

Сначала headless (может не вызывать хук):
```bash
cd /tmp && claude -p --model haiku 'Напиши "Начинаю", выполни `sleep 2` через Bash, потом напиши "Готово".' --allowedTools Bash </dev/null
wc -l /tmp/md-probe.jsonl
```
Если файл пуст — попросить пользователя: «открой новое окно Claude Code и отправь: *напиши "Начинаю", выполни sleep 2, напиши "Готово"*», затем `wc -l /tmp/md-probe.jsonl`.

- [ ] **Step 4: Изучить формат и сохранить фикстуру**

```bash
jq -c 'del(.transcript_path)' /tmp/md-probe.jsonl | head -20
jq -s '[.[]|keys]|unique' /tmp/md-probe.jsonl
grep -v '^$' /tmp/md-probe.jsonl | head -30 > ~/Projects/agent-speak/tests/fixtures/message_display.jsonl
```
Записать в конец этого плана (раздел «Факты из Task 1») реальные имена полей: идентификатор сообщения, кусок текста, признак конца, `session_id`. **Если имена отличаются от `message_id` / `delta` / `final` — заменить их в Task 9 (`MdEvent`) до начала Task 9.** Если хук не даёт текста вообще — в Task 9 оставить только транскрипт (удалить шаги про `Assembler`), отметить в спецификации.

- [ ] **Step 5: Вернуть настройки и закоммитить**

```bash
mv ~/.claude/settings.json.bak.md-probe ~/.claude/settings.json
cd ~/Projects/agent-speak && git add -A && git commit -m "chore: crate skeleton, MessageDisplay fixture"
```

---

### Task 2: Сервер синтеза `silero_tts.py`

**Files:**
- Create: `python/silero_tts.py`, `python/test_silero_tts.py`

**Interfaces:**
- Produces: процесс `python silero_tts.py <model.pt> <socket>`; протокол: клиент шлёт одну JSON-строку `{"text": str, "speaker": str, "rate": str}\n`, сервер отвечает `u32 LE длина` + PCM s16le 48 кГц моно и закрывает соединение. Пустой/нерусский текст → длина 0. Сервер создаёт сокет только после загрузки модели (по появлению сокета клиент понимает, что готово).

- [ ] **Step 1: Тест**

`python/test_silero_tts.py`:
```python
"""Проверка сервера синтеза: python test_silero_tts.py (нужна модель)."""
import json, os, socket, struct, subprocess, sys, time

HERE = os.path.dirname(os.path.abspath(__file__))
DATA = os.path.expanduser("~/.local/share/agent-speak")
SOCK = "/tmp/agent-speak-tts-test.sock"


def ask(req):
    s = socket.socket(socket.AF_UNIX)
    s.connect(SOCK)
    s.sendall((json.dumps(req) + "\n").encode())
    n = struct.unpack("<I", s.recv(4, socket.MSG_WAITALL))[0]
    data = b""
    while len(data) < n:
        data += s.recv(n - len(data))
    return data


if os.path.exists(SOCK):
    os.unlink(SOCK)
p = subprocess.Popen([f"{DATA}/venv/bin/python", f"{HERE}/silero_tts.py", f"{DATA}/v5_ru.pt", SOCK])
try:
    for _ in range(100):
        if os.path.exists(SOCK):
            break
        time.sleep(0.1)
    pcm = ask({"text": "Привет, это проверка.", "speaker": "xenia", "rate": "medium"})
    assert len(pcm) > 48000, len(pcm)  # больше полсекунды звука
    assert len(pcm) % 2 == 0
    fast = ask({"text": "Привет, это проверка.", "speaker": "xenia", "rate": "fast"})
    assert 0 < len(fast) < len(pcm), (len(fast), len(pcm))
    assert ask({"text": "hello 123", "speaker": "xenia", "rate": "medium"}) == b""
    print("ok")
finally:
    p.terminate()
```

- [ ] **Step 2: Запустить — падает**

Run: `~/.local/share/agent-speak/venv/bin/python python/test_silero_tts.py`
Expected: FAIL — `silero_tts.py` не найден (процесс завершится, сокет не появится, `FileNotFoundError` на connect).

- [ ] **Step 3: Реализация**

`python/silero_tts.py`:
```python
"""Сервер синтеза Silero: модель в памяти, запрос — JSON-строка, ответ — u32 длина + PCM s16le 48 кГц моно."""
import json
import os
import re
import socket
import struct
import sys
import warnings

import torch

warnings.filterwarnings("ignore")
torch.set_num_threads(4)
MAX = 800  # Silero не берёт длинный текст за раз


def chunks(text):
    buf = ""
    for s in re.split(r"(?<=[.!?…])\s+", text):
        if buf and len(buf) + len(s) > MAX:
            yield buf
            buf = ""
        buf = f"{buf} {s}".strip()
    if buf:
        yield buf


def synth(model, text, speaker, rate):
    out = []
    for part in chunks(text):
        if not re.search(r"[а-яёА-ЯЁ]", part):
            continue
        kw = dict(speaker=speaker, sample_rate=48000, put_accent=True, put_yo=True)
        if rate == "medium":
            audio = model.apply_tts(text=part, **kw)
        else:
            safe = part.replace("&", " и ").replace("<", " ").replace(">", " ")
            audio = model.apply_tts(ssml_text=f'<speak><prosody rate="{rate}">{safe}</prosody></speak>', **kw)
        out.append((audio * 32767).to(torch.int16).numpy().tobytes())
    return b"".join(out)


def main():
    model_path, sock_path = sys.argv[1], sys.argv[2]
    model = torch.package.PackageImporter(model_path).load_pickle("tts_models", "model")
    if os.path.exists(sock_path):
        os.unlink(sock_path)
    srv = socket.socket(socket.AF_UNIX)
    srv.bind(sock_path)
    srv.listen(4)
    while True:
        conn, _ = srv.accept()
        with conn:
            try:
                req = json.loads(conn.makefile("rb").readline())
                pcm = synth(model, req["text"], req.get("speaker", "xenia"), req.get("rate", "medium"))
            except Exception as e:  # плохой запрос не должен ронять сервер
                print(f"silero_tts: {e}", file=sys.stderr, flush=True)
                pcm = b""
            conn.sendall(struct.pack("<I", len(pcm)) + pcm)


if __name__ == "__main__":
    main()
```

- [ ] **Step 4: Тест проходит**

Run: `~/.local/share/agent-speak/venv/bin/python python/test_silero_tts.py`
Expected: `ok`

- [ ] **Step 5: Commit**

```bash
git add python && git commit -m "feat: Silero synthesis server"
```

---

### Task 3: Очистка текста и разбиение на предложения

**Files:**
- Create: `src/text/mod.rs` (пока только `pub mod`), `src/text/clean.rs`, `src/text/sentences.rs`
- Modify: `src/main.rs` (добавить `mod text;`)

**Interfaces:**
- Produces:
  - `pub const GAP: char = '\u{1}';` (в `clean.rs`) — пометка вырезанного места.
  - `pub fn clean(raw: &str) -> String` — текст без markdown; вырезанное → `GAP`; каждая непустая строка оканчивается `.`/`!`/`?`/`…`/`:`.
  - `pub fn split(text: &str) -> Vec<String>` — предложения (по `.!?…` + пробел и по `\n`), обрезанные, непустые.
  - `pub fn drop_gutted(sentences: Vec<String>) -> Vec<String>` — выбрасывает предложения с ≥2 `GAP` или с `GAP` и <3 словами; из оставшихся `GAP` удаляется.

- [ ] **Step 1: Тесты**

`src/text/clean.rs` (низ файла):
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_block_removed() {
        let t = clean("Смотри:\n```rust\nfn main() {}\n```\nГотово");
        assert!(!t.contains("fn main"));
        assert!(t.contains("Готово."));
    }

    #[test]
    fn path_becomes_file_name() {
        assert_eq!(clean("Правка в `~/.config/hypr/hyprland.lua:42`"), "Правка в hyprland.lua.");
    }

    #[test]
    fn shell_code_is_gap() {
        assert_eq!(clean("Запусти `cargo test --all`"), format!("Запусти {GAP}."));
    }

    #[test]
    fn short_code_kept_as_term() {
        assert_eq!(clean("Используем `serde`"), "Используем serde.");
    }

    #[test]
    fn link_and_url() {
        assert_eq!(clean("Смотри [доку](https://docs.rs/x)"), "Смотри доку.");
        assert_eq!(clean("Тут https://github.com/a/b есть"), "Тут github.com есть.");
    }

    #[test]
    fn table_rows_read() {
        let t = clean("| Голос | Где |\n|---|---|\n| Silero | офлайн |");
        assert_eq!(t, "Голос, Где.\nSilero, офлайн.");
    }

    #[test]
    fn list_heading_bold() {
        assert_eq!(clean("## Итог\n- **первое** дело\n1. второе"), "Итог.\nпервое дело.\nвторое.");
    }

    #[test]
    fn emoji_and_arrow() {
        assert_eq!(clean("Hyprland → хайпрлэнд 🔊"), "Hyprland в хайпрлэнд.");
    }

    #[test]
    fn only_code_gives_nothing() {
        let t = clean("```\nls -la\n```");
        assert!(super::super::sentences::split(&t).is_empty());
    }
}
```

`src/text/sentences.rs` (низ файла):
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::clean::GAP;

    #[test]
    fn splits_on_punctuation_and_lines() {
        assert_eq!(split("Раз. Два! Три?\nЧетыре"), vec!["Раз.", "Два!", "Три?", "Четыре"]);
    }

    #[test]
    fn version_not_split() {
        assert_eq!(split("Версия 2.1.284 стоит."), vec!["Версия 2.1.284 стоит."]);
    }

    #[test]
    fn gutted_dropped_short_whole_kept() {
        let s = vec![
            "Готово.".to_string(),
            format!("Запусти {GAP} и {GAP}."),
            format!("Смотри {GAP}."),
            format!("Поправил {GAP} в конфиге Hyprland."),
        ];
        assert_eq!(drop_gutted(s), vec!["Готово.", "Поправил в конфиге Hyprland."]);
    }
}
```

`src/text/mod.rs`:
```rust
pub mod clean;
pub mod sentences;
```

`src/main.rs`: добавить `mod text;` первой строкой.

- [ ] **Step 2: Тесты падают**

Run: `cargo test text::`
Expected: FAIL — `cannot find function clean`/`split`.

- [ ] **Step 3: Реализация `clean.rs`** (над тестами)

```rust
//! Markdown, код, пути, ссылки, таблицы → читаемый текст. Вырезанное — пометка GAP.

pub const GAP: char = '\u{1}';

pub fn clean(raw: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut in_code = false;
    for line in raw.lines() {
        let l = line.trim();
        if l.starts_with("```") {
            in_code = !in_code;
            continue;
        }
        if in_code || l.is_empty() {
            continue;
        }
        let l = if l.starts_with('|') {
            match table_row(l) {
                Some(r) => r,
                None => continue,
            }
        } else {
            strip_marker(l).to_string()
        };
        let l = inline(&l);
        let l = l.split_whitespace().collect::<Vec<_>>().join(" ");
        if l.is_empty() {
            continue;
        }
        out.push(end_sentence(l));
    }
    out.join("\n")
}

/// Строка таблицы → «a, b, c»; разделитель |---| → None.
fn table_row(l: &str) -> Option<String> {
    let cells: Vec<&str> = l.trim_matches('|').split('|').map(str::trim).collect();
    if cells.iter().all(|c| c.chars().all(|ch| matches!(ch, '-' | ':' | ' '))) {
        return None;
    }
    Some(cells.into_iter().filter(|c| !c.is_empty()).collect::<Vec<_>>().join(", "))
}

/// Заголовок, пункт списка, цитата → без маркера.
fn strip_marker(l: &str) -> &str {
    let l = l.trim_start_matches('#').trim_start_matches('>').trim_start();
    for m in ["- ", "* ", "+ "] {
        if let Some(r) = l.strip_prefix(m) {
            return r;
        }
    }
    let digits = l.chars().take_while(char::is_ascii_digit).count();
    if digits > 0 {
        if let Some(r) = l[digits..].strip_prefix(". ") {
            return r;
        }
    }
    l
}

fn inline(l: &str) -> String {
    let mut s = String::new();
    let mut rest = l;
    while let Some(i) = rest.find('`') {
        s.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        match after.find('`') {
            Some(j) => {
                s.push_str(&code_span(&after[..j]));
                rest = &after[j + 1..];
            }
            None => {
                rest = after;
            }
        }
    }
    s.push_str(rest);
    let s = links(&s);
    let s = s.replace("**", "").replace("__", "").replace('*', "");
    let s = s.replace('→', " в ").replace('←', " из ").replace('·', ",");
    s.chars().filter(|c| !is_emoji(*c)).collect()
}

/// `код`: путь → имя файла; команда/выражение → GAP; одно слово → как есть.
fn code_span(c: &str) -> String {
    let c = c.trim();
    if c.contains('/') && !c.contains(' ') {
        let name = c.rsplit('/').next().unwrap_or(c);
        return name.split(':').next().unwrap_or(name).to_string();
    }
    if c.contains(' ') || c.chars().any(|ch| "=|$;(){}<>&\"'".contains(ch)) {
        return GAP.to_string();
    }
    c.split(':').next().unwrap_or(c).to_string()
}

/// [текст](url) → текст; голый URL → домен.
fn links(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find("](") {
        let open = rest[..i].rfind('[');
        let close = rest[i + 2..].find(')');
        match (open, close) {
            (Some(o), Some(c)) => {
                out.push_str(&rest[..o]);
                out.push_str(&rest[o + 1..i]);
                rest = &rest[i + 2 + c + 1..];
            }
            _ => break,
        }
    }
    out.push_str(rest);
    out.split(' ')
        .map(|w| match w.strip_prefix("https://").or_else(|| w.strip_prefix("http://")) {
            Some(u) => u.split('/').next().unwrap_or(u).to_string(),
            None => w.to_string(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn is_emoji(c: char) -> bool {
    matches!(c as u32, 0x1F000..=0x1FAFF | 0x2600..=0x27BF | 0xFE0F | 0x200D)
}

fn end_sentence(mut l: String) -> String {
    if !l.ends_with(['.', '!', '?', '…', ':']) {
        l.push('.');
    }
    l
}
```

- [ ] **Step 4: Реализация `sentences.rs`** (над тестами)

```rust
//! Разбиение на предложения и выброс «обрубков» после очистки.

use crate::text::clean::GAP;

pub fn split(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        let chars: Vec<char> = line.chars().collect();
        let mut start = 0;
        for i in 0..chars.len() {
            let end = matches!(chars[i], '.' | '!' | '?' | '…');
            let next_space = chars.get(i + 1).is_none_or(|c| c.is_whitespace());
            if end && next_space {
                push(&mut out, &chars[start..=i]);
                start = i + 1;
            }
        }
        push(&mut out, &chars[start..]);
    }
    out
}

fn push(out: &mut Vec<String>, s: &[char]) {
    let s: String = s.iter().collect::<String>().trim().to_string();
    if s.chars().any(|c| c.is_alphanumeric()) {
        out.push(s);
    }
}

pub fn drop_gutted(sentences: Vec<String>) -> Vec<String> {
    sentences
        .into_iter()
        .filter_map(|s| {
            let gaps = s.matches(GAP).count();
            let words = s.split_whitespace().filter(|w| w.chars().any(char::is_alphanumeric) && !w.contains(GAP)).count();
            if gaps >= 2 || (gaps == 1 && words < 3) {
                return None;
            }
            let s = s.replace(GAP, "");
            Some(s.split_whitespace().collect::<Vec<_>>().join(" ").replace(" .", "."))
        })
        .collect()
}
```

- [ ] **Step 5: Тесты проходят**

Run: `cargo test text::`
Expected: все тесты `clean` и `sentences` — ok.

- [ ] **Step 6: Commit**

```bash
git add -A && git commit -m "feat: text cleaning and sentence splitting"
```

---

### Task 4: Числа словами

**Files:**
- Create: `src/text/numbers.rs`
- Modify: `src/text/mod.rs` (`pub mod numbers;`)

**Interfaces:**
- Produces: `pub fn numbers_to_words(s: &str) -> String` — целые до 999 999 999, десятичные и версии через «точка», `N%`/`N %` → «… процентов». Именительный падеж (осознанное ограничение спецификации).

- [ ] **Step 1: Тесты** (низ `numbers.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integers() {
        assert_eq!(numbers_to_words("0"), "ноль");
        assert_eq!(numbers_to_words("42 файла"), "сорок два файла");
        assert_eq!(numbers_to_words("13984"), "тринадцать тысяч девятьсот восемьдесят четыре");
        assert_eq!(numbers_to_words("2001"), "две тысячи один");
        assert_eq!(numbers_to_words("1000000"), "один миллион");
    }

    #[test]
    fn decimals_versions_percent() {
        assert_eq!(numbers_to_words("1,8 с"), "один точка восемь с");
        assert_eq!(numbers_to_words("2.1.284"), "два точка один точка двести восемьдесят четыре");
        assert_eq!(numbers_to_words("50 %"), "пятьдесят процентов");
        assert_eq!(numbers_to_words("7%"), "семь процентов");
    }

    #[test]
    fn sentence_end_dot_kept() {
        assert_eq!(numbers_to_words("Итого 3."), "Итого три.");
    }
}
```

`src/text/mod.rs`: добавить `pub mod numbers;`.

- [ ] **Step 2: Падают**

Run: `cargo test numbers`
Expected: FAIL — `cannot find function numbers_to_words`.

- [ ] **Step 3: Реализация** (над тестами)

```rust
//! Числа → слова, именительный падеж (склонение по контексту правилами ненадёжно).

const ONES: [&str; 20] = [
    "ноль", "один", "два", "три", "четыре", "пять", "шесть", "семь", "восемь", "девять", "десять",
    "одиннадцать", "двенадцать", "тринадцать", "четырнадцать", "пятнадцать", "шестнадцать",
    "семнадцать", "восемнадцать", "девятнадцать",
];
const TENS: [&str; 10] = ["", "", "двадцать", "тридцать", "сорок", "пятьдесят", "шестьдесят", "семьдесят", "восемьдесят", "девяносто"];
const HUNDREDS: [&str; 10] = ["", "сто", "двести", "триста", "четыреста", "пятьсот", "шестьсот", "семьсот", "восемьсот", "девятьсот"];

pub fn numbers_to_words(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if !chars[i].is_ascii_digit() {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        // группа: цифры, разделённые одиночными '.' или ',' (версия/десятичное)
        let mut parts = vec![String::new()];
        while i < chars.len() {
            let c = chars[i];
            if c.is_ascii_digit() {
                parts.last_mut().unwrap().push(c);
            } else if (c == '.' || c == ',') && chars.get(i + 1).is_some_and(char::is_ascii_digit) {
                parts.push(String::new());
            } else {
                break;
            }
            i += 1;
        }
        let words: Vec<String> = parts.iter().map(|p| int_words(p)).collect();
        out.push_str(&words.join(" точка "));
        // процент: "50%" или "50 %"
        let mut j = i;
        if chars.get(j) == Some(&' ') {
            j += 1;
        }
        if chars.get(j) == Some(&'%') {
            out.push_str(" процентов");
            i = j + 1;
        }
    }
    out
}

fn int_words(digits: &str) -> String {
    let n: u64 = match digits.parse() {
        Ok(n) if n < 1_000_000_000 => n,
        _ => return digits.chars().map(|c| ONES[c.to_digit(10).unwrap() as usize]).collect::<Vec<_>>().join(" "),
    };
    if n == 0 {
        return "ноль".into();
    }
    let mut w: Vec<String> = Vec::new();
    let millions = n / 1_000_000;
    let thousands = n / 1000 % 1000;
    let rest = n % 1000;
    if millions > 0 {
        w.push(triple(millions, false));
        w.push(plural(millions, "миллион", "миллиона", "миллионов").into());
    }
    if thousands > 0 {
        w.push(triple(thousands, true));
        w.push(plural(thousands, "тысяча", "тысячи", "тысяч").into());
    }
    if rest > 0 {
        w.push(triple(rest, false));
    }
    w.retain(|s| !s.is_empty());
    w.join(" ")
}

/// 1..999; feminine — для тысяч («одна», «две»).
fn triple(n: u64, feminine: bool) -> String {
    let mut w = Vec::new();
    let h = (n / 100) as usize;
    let t = (n % 100) as usize;
    if h > 0 {
        w.push(HUNDREDS[h].to_string());
    }
    let (tens, ones) = if t < 20 { (0, t) } else { (t / 10, t % 10) };
    if tens > 0 {
        w.push(TENS[tens].to_string());
    }
    if ones > 0 {
        w.push(match (ones, feminine) {
            (1, true) => "одна".into(),
            (2, true) => "две".into(),
            _ => ONES[ones].to_string(),
        });
    }
    w.join(" ")
}

fn plural(n: u64, one: &'static str, few: &'static str, many: &'static str) -> &'static str {
    let (d, h) = (n % 10, n % 100);
    if d == 1 && h != 11 {
        one
    } else if (2..=4).contains(&d) && !(12..=14).contains(&h) {
        few
    } else {
        many
    }
}
```

- [ ] **Step 4: Проходят**

Run: `cargo test numbers`
Expected: ok.

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat: numbers to Russian words"
```

---

### Task 5: Словарь терминов и транслитерация

**Files:**
- Create: `src/text/terms.rs`, `data/terms.tsv`
- Modify: `src/text/mod.rs` (`pub mod terms;` + `prepare`)

**Interfaces:**
- Produces:
  - `pub struct Terms { map: HashMap<String, String>, path: Option<PathBuf> }`
  - `Terms::load(path: &Path) -> Terms` (нет файла → пустой, `path` запомнен), `Terms::from_str(s: &str) -> Terms` (для тестов, `path: None`)
  - `fn apply(&self, sentence: &str) -> (String, Vec<String>)` — латинские слова заменены; второе — неизвестные слова (в нижнем регистре, без повторов).
  - `fn add(&mut self, word: &str, pron: &str)` — в память и дописать строку `word\tpron` в файл (если `path`).
  - `fn contains(&self, word: &str) -> bool`
  - `pub fn transliterate(word: &str) -> String`
  - В `text/mod.rs`: `pub fn prepare(raw: &str, terms: &Terms) -> (Vec<String>, Vec<String>)` — готовые предложения + неизвестные термины.

- [ ] **Step 1: Стартовый словарь `data/terms.tsv`**

```
claude	клод
codex	кодекс
anthropic	энтр+опик
openai	оупен-эй-ай
haiku	хайку
opus	опус
sonnet	сонет
hyprland	хайпрлэнд
omarchy	ом+арчи
wayland	вэйлэнд
pipewire	пайпвайр
silero	сил+еро
voxtype	вокстайп
github	гитхаб
git	гит
commit	коммит
push	пуш
pull	пул
merge	мёрдж
branch	бранч
rust	раст
cargo	карго
python	пайтон
bash	баш
json	джейсон
toml	томл
yaml	ямл
api	апи
cli	си-эл-ай
url	ю-ар-эл
ui	ю-ай
llm	эл-эл-эм
tts	ти-ти-эс
hook	хук
hooks	хуки
daemon	демон
socket	сокет
config	конфиг
linux	линукс
systemd	систем-ди
docker	докер
node	нода
npm	эн-пи-эм
react	риакт
typescript	тайпскрипт
javascript	джаваскрипт
frontend	фронтенд
backend	бэкенд
jira	джира
mcp	эм-си-пи
pr	пи-ар
vpn	ви-пи-эн
```

- [ ] **Step 2: Тесты** (низ `terms.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn t() -> Terms {
        Terms::from_str("hyprland\tхайпрлэнд\nnpm\tэн-пи-эм\n")
    }

    #[test]
    fn known_replaced_case_insensitive() {
        let (s, unknown) = t().apply("Правка в Hyprland через NPM.");
        assert_eq!(s, "Правка в хайпрлэнд через эн-пи-эм.");
        assert!(unknown.is_empty());
    }

    #[test]
    fn unknown_transliterated_and_reported() {
        let (s, unknown) = t().apply("Используем serde и Tokio.");
        assert_eq!(s, "Используем серде и токио.");
        assert_eq!(unknown, vec!["serde", "tokio"]);
    }

    #[test]
    fn abbreviation_spelled() {
        let (s, unknown) = t().apply("Через SSH и gpg.");
        assert_eq!(s, "Через эс-эс-эйч и джи-пи-джи.");
        assert_eq!(unknown, vec!["ssh", "gpg"]);
    }

    #[test]
    fn file_name_parts() {
        let (s, _) = t().apply("Файл hyprland.lua.");
        assert_eq!(s, "Файл хайпрлэнд точка луа.");
    }

    #[test]
    fn translit_digraphs() {
        assert_eq!(transliterate("shell"), "шелл");
        assert_eq!(transliterate("cache"), "каче");
        assert_eq!(transliterate("city"), "сити");
        assert_eq!(transliterate("box"), "бокс");
    }

    #[test]
    fn add_persists() {
        let p = std::env::temp_dir().join(format!("terms-{}.tsv", std::process::id()));
        let _ = std::fs::remove_file(&p);
        let mut a = Terms::load(&p);
        a.add("Tokio", "токио");
        let b = Terms::load(&p);
        assert!(b.contains("tokio"));
        std::fs::remove_file(&p).unwrap();
    }
}
```

- [ ] **Step 3: Падают**

Run: `cargo test terms`
Expected: FAIL — `cannot find type Terms`.

- [ ] **Step 4: Реализация** (над тестами)

```rust
//! Словарь произношений латинских слов + транслитерация для незнакомых.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};

pub struct Terms {
    map: HashMap<String, String>,
    path: Option<PathBuf>,
}

impl Terms {
    pub fn load(path: &Path) -> Terms {
        let s = std::fs::read_to_string(path).unwrap_or_default();
        Terms { path: Some(path.to_path_buf()), ..Terms::from_str(&s) }
    }

    pub fn from_str(s: &str) -> Terms {
        let map = s
            .lines()
            .filter_map(|l| l.split_once('\t'))
            .map(|(w, p)| (w.trim().to_lowercase(), p.trim().to_string()))
            .filter(|(w, p)| !w.is_empty() && !p.is_empty())
            .collect();
        Terms { map, path: None }
    }

    pub fn contains(&self, word: &str) -> bool {
        self.map.contains_key(&word.to_lowercase())
    }

    pub fn add(&mut self, word: &str, pron: &str) {
        let w = word.to_lowercase();
        if let Some(p) = &self.path {
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
                let _ = writeln!(f, "{w}\t{pron}");
            }
        }
        self.map.insert(w, pron.to_string());
    }

    pub fn apply(&self, sentence: &str) -> (String, Vec<String>) {
        let mut out = String::new();
        let mut unknown: Vec<String> = Vec::new();
        let mut word = String::new();
        let flush = |word: &mut String, out: &mut String, unknown: &mut Vec<String>| {
            if word.is_empty() {
                return;
            }
            let w = word.to_lowercase();
            match self.map.get(&w) {
                Some(p) => out.push_str(p),
                None => {
                    out.push_str(&speak_unknown(word));
                    if !unknown.contains(&w) {
                        unknown.push(w);
                    }
                }
            }
            word.clear();
        };
        let chars: Vec<char> = sentence.chars().collect();
        for (i, &c) in chars.iter().enumerate() {
            if c.is_ascii_alphabetic() {
                word.push(c);
                continue;
            }
            // точка между латинскими словами (file.lua) → «точка»
            let between = c == '.' && !word.is_empty() && chars.get(i + 1).is_some_and(char::is_ascii_alphabetic);
            flush(&mut word, &mut out, &mut unknown);
            if between {
                out.push_str(" точка ");
            } else if (c == '-' || c == '_') && chars.get(i + 1).is_some_and(char::is_ascii_alphabetic) {
                out.push(' ');
            } else {
                out.push(c);
            }
        }
        flush(&mut word, &mut out, &mut unknown);
        (out, unknown)
    }
}

/// Аббревиатура (все заглавные ≤5 или без гласных ≤4) — по буквам, иначе транслитерация.
fn speak_unknown(word: &str) -> String {
    let lower = word.to_lowercase();
    let upper = word.len() >= 2 && word.len() <= 5 && word.chars().all(|c| c.is_ascii_uppercase());
    let no_vowels = lower.len() <= 4 && !lower.chars().any(|c| "aeiouy".contains(c));
    if upper || no_vowels {
        lower.chars().map(letter).collect::<Vec<_>>().join("-")
    } else {
        transliterate(&lower)
    }
}

fn letter(c: char) -> &'static str {
    match c {
        'a' => "эй", 'b' => "би", 'c' => "си", 'd' => "ди", 'e' => "и", 'f' => "эф", 'g' => "джи",
        'h' => "эйч", 'i' => "ай", 'j' => "джей", 'k' => "кей", 'l' => "эл", 'm' => "эм", 'n' => "эн",
        'o' => "оу", 'p' => "пи", 'q' => "кью", 'r' => "ар", 's' => "эс", 't' => "ти", 'u' => "ю",
        'v' => "ви", 'w' => "дабл-ю", 'x' => "экс", 'y' => "уай", _ => "зед",
    }
}

// ponytail: побуквенные правила с диграфами — грубо («каче»), словарь и фоновая модель исправляют
pub fn transliterate(word: &str) -> String {
    let w: Vec<char> = word.to_lowercase().chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < w.len() {
        let pair: String = w[i..(i + 2).min(w.len())].iter().collect();
        let di = match pair.as_str() {
            "sh" => Some("ш"), "ch" => Some("ч"), "th" => Some("т"), "ph" => Some("ф"), "zh" => Some("ж"),
            "kh" => Some("х"), "ts" => Some("ц"), "oo" => Some("у"), "ee" => Some("и"), "ya" => Some("я"),
            "yo" => Some("йо"), "yu" => Some("ю"), "ck" => Some("к"), "qu" => Some("кв"),
            _ => None,
        };
        if let Some(d) = di {
            out.push_str(d);
            i += 2;
            continue;
        }
        let next = w.get(i + 1).copied();
        out.push_str(match w[i] {
            'a' => "а", 'b' => "б", 'c' if matches!(next, Some('e' | 'i' | 'y')) => "с", 'c' => "к",
            'd' => "д", 'e' => "е", 'f' => "ф", 'g' => "г", 'h' => "х", 'i' => "и", 'j' => "дж",
            'k' => "к", 'l' => "л", 'm' => "м", 'n' => "н", 'o' => "о", 'p' => "п", 'q' => "к",
            'r' => "р", 's' => "с", 't' => "т", 'u' => "у", 'v' => "в", 'w' => "в", 'x' => "кс",
            'y' if i == 0 => "й", 'y' => "и", 'z' => "з", _ => "",
        });
        i += 1;
    }
    out
}
```

`src/text/mod.rs` целиком:
```rust
pub mod clean;
pub mod numbers;
pub mod sentences;
pub mod terms;

use terms::Terms;

/// Сырой текст агента → предложения для синтеза + незнакомые латинские слова.
pub fn prepare(raw: &str, terms: &Terms) -> (Vec<String>, Vec<String>) {
    let cleaned = clean::clean(raw);
    let mut out = Vec::new();
    let mut unknown = Vec::new();
    for s in sentences::drop_gutted(sentences::split(&cleaned)) {
        let (s, u) = terms.apply(&numbers::numbers_to_words(&s));
        for w in u {
            if !unknown.contains(&w) {
                unknown.push(w);
            }
        }
        out.push(s);
    }
    (out, unknown)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepare_end_to_end() {
        let t = Terms::from_str("hyprland\tхайпрлэнд\n");
        let (s, u) = prepare("## Готово\nПоправил `~/.config/hypr/hyprland.lua`, 3 правки → Hyprland.\n```\nx\n```", &t);
        assert_eq!(s, vec!["Готово.", "Поправил хайпрлэнд точка луа, три правки в хайпрлэнд."]);
        assert_eq!(u, vec!["lua"]);
    }
}
```

- [ ] **Step 5: Проходят**

Run: `cargo test text`
Expected: ok (включая `prepare_end_to_end`). Если `transliterate` в каком-то тесте даёт другой, но разумный вариант — поправить ожидание теста, а не правила, и записать это в коммит.

- [ ] **Step 6: Commit**

```bash
git add -A && git commit -m "feat: term dictionary, transliteration, prepare pipeline"
```

---

### Task 6: Чтение транскриптов: `Tailer`, разбор Claude и Codex

**Files:**
- Create: `src/source/mod.rs`, `src/source/tail.rs`, `src/source/claude.rs`, `src/source/codex.rs`, `tests/fixtures/claude.jsonl`, `tests/fixtures/codex.jsonl`
- Modify: `src/main.rs` (`mod source;`)

**Interfaces:**
- Produces:
  - `pub struct Block { pub message_id: String, pub text: String }`
  - `pub struct Tailer { offsets: HashMap<PathBuf, (u64, String)> }`, `Tailer::new()`, `fn read_new(&mut self, path: &Path) -> Vec<String>` — первая встреча файла: запомнить конец, вернуть пусто; дальше — только новые **полные** строки; недописанный хвост держится до следующего вызова.
  - `claude::parse_line(line: &str) -> Vec<Block>` — блоки `text` и `thinking` с `narration` в декоде `signature` у записей `type == "assistant"`.
  - `claude::last_turn(content: &str) -> Vec<String>` — тексты последнего хода (после последнего настоящего сообщения пользователя; если пусто — предыдущий ход).
  - `codex::parse_line(line: &str) -> Vec<Block>` — `response_item`/`message`/`role == "assistant"`; `message_id` = `payload.id` или пустая строка.
  - `codex::last_turn(content: &str) -> Vec<String>`
  - `codex::is_subagent(first_line: &str) -> bool` — `payload.thread_source` есть и не равен `"user"`/`"cli"` — сабагент.

- [ ] **Step 1: Фикстуры из настоящих файлов**

```bash
cd ~/Projects/agent-speak
T=~/.claude/projects/-home-user-Work/00000000-0000-0000-0000-000000000001.jsonl
# реальное сообщение пользователя, ход с text, thinking-narration и tool_use/tool_result, финальный text
python3 - "$T" > tests/fixtures/claude.jsonl <<'EOF'
import json,sys,base64
lines=[json.loads(l) for l in open(sys.argv[1])]
def narr(c):
    try: return b"narration" in base64.b64decode(c.get("signature","")+"==")[:120]
    except Exception: return False
pick=[]
for e in lines:
    if e.get("type")=="assistant":
        for c in e["message"]["content"]:
            if c["type"]=="thinking" and narr(c) and c.get("thinking"): pick.append(e); break
    if len(pick)>=2: break
user=[e for e in lines if e.get("type")=="user" and isinstance(e["message"]["content"],str)][-1]
tool=[e for e in lines if e.get("type")=="user" and isinstance(e["message"]["content"],list)][-1]
text=[e for e in lines if e.get("type")=="assistant" and any(c["type"]=="text" for c in e["message"]["content"])][-1]
for e in [user]+pick[:1]+[tool]+pick[1:2]+[text]: print(json.dumps(e,ensure_ascii=False))
EOF
wc -l tests/fixtures/claude.jsonl
C=$(ls -S ~/.codex/sessions/2026/*/*/rollout-*.jsonl | head -1)
{ head -1 "$C"; jq -c 'select(.type=="response_item" and .payload.type=="message")' "$C" | tail -6; } > tests/fixtures/codex.jsonl
jq -r '.payload.role // "meta"' tests/fixtures/codex.jsonl
```
Expected: `claude.jsonl` — 5 строк; в `codex.jsonl` есть и `user`, и `assistant`. Проверить, что в фикстурах нет секретов (`grep -iE "token|password|secret" tests/fixtures/*`) — при находке заменить значения на `xxx`.

- [ ] **Step 2: Тесты**

`src/source/tail.rs` (низ):
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn tail_starts_at_end() {
        let p = std::env::temp_dir().join(format!("tail-a-{}", std::process::id()));
        std::fs::write(&p, "old1\nold2\n").unwrap();
        let mut t = Tailer::new();
        assert!(t.read_new(&p).is_empty());
        std::fs::OpenOptions::new().append(true).open(&p).unwrap().write_all(b"new\n").unwrap();
        assert_eq!(t.read_new(&p), vec!["new"]);
        std::fs::remove_file(&p).unwrap();
    }

    #[test]
    fn tail_keeps_partial_line() {
        let p = std::env::temp_dir().join(format!("tail-b-{}", std::process::id()));
        std::fs::write(&p, "").unwrap();
        let mut t = Tailer::new();
        t.read_new(&p);
        let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
        f.write_all(b"{\"a\":").unwrap();
        assert!(t.read_new(&p).is_empty());
        f.write_all(b"1}\nnext\n").unwrap();
        assert_eq!(t.read_new(&p), vec!["{\"a\":1}", "next"]);
        assert!(t.read_new(&p).is_empty());
        std::fs::remove_file(&p).unwrap();
    }
}
```

`src/source/claude.rs` (низ):
```rust
#[cfg(test)]
mod tests {
    use super::*;
    const FX: &str = include_str!("../../tests/fixtures/claude.jsonl");

    #[test]
    fn narration_and_text_parsed() {
        let blocks: Vec<Block> = FX.lines().flat_map(parse_line).collect();
        assert!(blocks.len() >= 3, "{}", blocks.len());
        assert!(blocks.iter().all(|b| !b.text.is_empty() && !b.message_id.is_empty()));
    }

    #[test]
    fn last_turn_after_real_user_message() {
        let turn = last_turn(FX);
        assert!(turn.len() >= 3, "{turn:?}");
        let last = FX.lines().last().unwrap();
        assert_eq!(turn.last().unwrap(), &parse_line(last)[0].text);
    }

    #[test]
    fn garbage_line_ignored() {
        assert!(parse_line("{not json").is_empty());
    }
}
```

`src/source/codex.rs` (низ):
```rust
#[cfg(test)]
mod tests {
    use super::*;
    const FX: &str = include_str!("../../tests/fixtures/codex.jsonl");

    #[test]
    fn assistant_messages_parsed() {
        let n = FX.lines().filter(|l| l.contains("\"assistant\"")).count();
        let blocks: Vec<Block> = FX.lines().flat_map(parse_line).collect();
        assert_eq!(blocks.len(), n);
    }

    #[test]
    fn last_turn_not_empty() {
        assert!(!last_turn(FX).is_empty());
    }

    #[test]
    fn main_session_not_subagent() {
        assert!(!is_subagent(FX.lines().next().unwrap()));
        assert!(is_subagent(r#"{"type":"session_meta","payload":{"thread_source":"subagent"}}"#));
    }
}
```

`src/source/mod.rs`:
```rust
pub mod claude;
pub mod codex;
pub mod tail;

/// Кусок текста агента из транскрипта.
#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub message_id: String,
    pub text: String,
}
```

`src/main.rs`: добавить `mod source;`.

- [ ] **Step 3: Падают**

Run: `cargo test source`
Expected: FAIL — нет `Tailer`, `parse_line`, `last_turn`, `is_subagent`.

- [ ] **Step 4: Реализация `tail.rs`**

```rust
//! Новые полные строки дописываемых файлов; первая встреча — с конца (историю не читаем).

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

pub struct Tailer {
    offsets: HashMap<PathBuf, (u64, String)>,
}

impl Tailer {
    pub fn new() -> Tailer {
        Tailer { offsets: HashMap::new() }
    }

    pub fn read_new(&mut self, path: &Path) -> Vec<String> {
        let Ok(mut f) = std::fs::File::open(path) else { return vec![] };
        let len = f.metadata().map(|m| m.len()).unwrap_or(0);
        let Some((off, partial)) = self.offsets.get_mut(path) else {
            self.offsets.insert(path.to_path_buf(), (len, String::new()));
            return vec![];
        };
        if len < *off {
            *off = 0; // файл пересоздан
            partial.clear();
        }
        let mut buf = String::new();
        if f.seek(SeekFrom::Start(*off)).is_err() || f.read_to_string(&mut buf).is_err() {
            return vec![];
        }
        *off += buf.len() as u64;
        partial.push_str(&buf);
        let mut lines: Vec<String> = partial.split('\n').map(String::from).collect();
        *partial = lines.pop().unwrap_or_default();
        lines.retain(|l| !l.trim().is_empty());
        lines
    }
}
```

- [ ] **Step 5: Реализация `claude.rs`**

```rust
//! Транскрипт Claude Code: тексты агента, включая промежуточные, записанные как thinking с narration.

use super::Block;
use base64::Engine;
use serde_json::Value;

pub fn parse_line(line: &str) -> Vec<Block> {
    let Ok(e) = serde_json::from_str::<Value>(line) else { return vec![] };
    if e["type"] != "assistant" {
        return vec![];
    }
    let id = e["message"]["id"].as_str().unwrap_or_default().to_string();
    let Some(content) = e["message"]["content"].as_array() else { return vec![] };
    content
        .iter()
        .filter_map(|c| {
            let text = match c["type"].as_str()? {
                "text" => c["text"].as_str()?,
                "thinking" if is_narration(c["signature"].as_str().unwrap_or_default()) => c["thinking"].as_str()?,
                _ => return None,
            };
            (!text.trim().is_empty()).then(|| Block { message_id: id.clone(), text: text.to_string() })
        })
        .collect()
}

/// Claude Code помечает промежуточные сообщения словом narration в начале подписи блока thinking.
fn is_narration(signature: &str) -> bool {
    let head: String = signature.chars().take(160).collect();
    let head = &head[..head.len() / 4 * 4];
    base64::engine::general_purpose::STANDARD
        .decode(head)
        .map(|b| b.windows(9).any(|w| w == b"narration"))
        .unwrap_or(false)
}

fn is_real_user(e: &Value) -> bool {
    if e["type"] != "user" || e["isMeta"] == true {
        return false;
    }
    match &e["message"]["content"] {
        Value::String(_) => true,
        Value::Array(a) => !a.iter().any(|c| c["type"] == "tool_result"),
        _ => false,
    }
}

pub fn last_turn(content: &str) -> Vec<String> {
    let (mut cur, mut prev): (Vec<String>, Vec<String>) = (vec![], vec![]);
    for line in content.lines() {
        let Ok(e) = serde_json::from_str::<Value>(line) else { continue };
        if is_real_user(&e) {
            if !cur.is_empty() {
                prev = std::mem::take(&mut cur);
            }
        } else {
            cur.extend(parse_line(line).into_iter().map(|b| b.text));
        }
    }
    if cur.is_empty() { prev } else { cur }
}
```

- [ ] **Step 6: Реализация `codex.rs`**

```rust
//! Rollout Codex: все сообщения агента за ход.

use super::Block;
use serde_json::Value;

pub fn parse_line(line: &str) -> Vec<Block> {
    let Ok(e) = serde_json::from_str::<Value>(line) else { return vec![] };
    let p = &e["payload"];
    if e["type"] != "response_item" || p["type"] != "message" || p["role"] != "assistant" {
        return vec![];
    }
    let text: Vec<&str> = p["content"].as_array().into_iter().flatten().filter_map(|c| c["text"].as_str()).collect();
    let text = text.join("\n");
    if text.trim().is_empty() {
        return vec![];
    }
    vec![Block { message_id: p["id"].as_str().unwrap_or_default().to_string(), text }]
}

pub fn last_turn(content: &str) -> Vec<String> {
    let (mut cur, mut prev): (Vec<String>, Vec<String>) = (vec![], vec![]);
    for line in content.lines() {
        let Ok(e) = serde_json::from_str::<Value>(line) else { continue };
        if e["type"] == "response_item" && e["payload"]["type"] == "message" && e["payload"]["role"] == "user" {
            if !cur.is_empty() {
                prev = std::mem::take(&mut cur);
            }
        } else {
            cur.extend(parse_line(line).into_iter().map(|b| b.text));
        }
    }
    if cur.is_empty() { prev } else { cur }
}

pub fn is_subagent(first_line: &str) -> bool {
    let Ok(e) = serde_json::from_str::<Value>(first_line) else { return false };
    match e["payload"]["thread_source"].as_str() {
        None | Some("user") | Some("cli") | Some("exec") => false,
        Some(_) => true,
    }
}
```

- [ ] **Step 7: Проходят**

Run: `cargo test source`
Expected: ok. Если `main_session_not_subagent` падает — посмотреть `head -1 tests/fixtures/codex.jsonl | jq .payload.thread_source` и добавить реальное значение главной сессии в список `false`.

- [ ] **Step 8: Commit**

```bash
git add -A && git commit -m "feat: transcript tailing, Claude and Codex parsers"
```

---

### Task 7: Защита от повторов

**Files:**
- Create: `src/dedup.rs`
- Modify: `src/main.rs` (`mod dedup;`)

**Interfaces:**
- Produces: `pub struct Dedup`, `Dedup::new()`,
  - `fn mark_display(&mut self, session: &str, message_id: &str)` — по сообщению пришёл текст из `MessageDisplay`;
  - `fn from_transcript_ok(&self, session: &str, message_id: &str) -> bool` — `false`, если по этому сообщению уже был `MessageDisplay`;
  - `fn first_time(&mut self, session: &str, text: &str) -> bool` — `true` при первой встрече нормализованного текста в сессии;
  - `fn forget(&mut self, session: &str)` — очистить сессию (новый ход пользователя).

- [ ] **Step 1: Тесты** (низ `dedup.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_suppresses_transcript_same_message() {
        let mut d = Dedup::new();
        d.mark_display("s1", "msg_1");
        assert!(!d.from_transcript_ok("s1", "msg_1"));
        assert!(d.from_transcript_ok("s1", "msg_2"));
        assert!(d.from_transcript_ok("s2", "msg_1"));
    }

    #[test]
    fn normalized_repeat_detected() {
        let mut d = Dedup::new();
        assert!(d.first_time("s1", "Готово, всё работает."));
        assert!(!d.first_time("s1", "готово всё работает"));
        assert!(d.first_time("s2", "Готово, всё работает."));
    }

    #[test]
    fn forget_resets_session() {
        let mut d = Dedup::new();
        d.first_time("s1", "Раз");
        d.forget("s1");
        assert!(d.first_time("s1", "Раз"));
    }
}
```

- [ ] **Step 2: Падают**

Run: `cargo test dedup`
Expected: FAIL — нет `Dedup`.

- [ ] **Step 3: Реализация** (над тестами)

```rust
//! Одна фраза может прийти из хука и из транскрипта — читаем один раз.

use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};

#[derive(Default)]
struct Session {
    display_ids: HashSet<String>,
    hashes: HashSet<u64>,
}

pub struct Dedup {
    sessions: HashMap<String, Session>,
}

impl Dedup {
    pub fn new() -> Dedup {
        Dedup { sessions: HashMap::new() }
    }

    pub fn mark_display(&mut self, session: &str, message_id: &str) {
        self.sessions.entry(session.into()).or_default().display_ids.insert(message_id.into());
    }

    pub fn from_transcript_ok(&self, session: &str, message_id: &str) -> bool {
        self.sessions.get(session).is_none_or(|s| !s.display_ids.contains(message_id))
    }

    pub fn first_time(&mut self, session: &str, text: &str) -> bool {
        let norm: String = text.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect();
        let mut h = DefaultHasher::new();
        norm.hash(&mut h);
        self.sessions.entry(session.into()).or_default().hashes.insert(h.finish())
    }

    pub fn forget(&mut self, session: &str) {
        self.sessions.remove(session);
    }
}
```

- [ ] **Step 4: Проходят** — `cargo test dedup` → ok.

- [ ] **Step 5: Commit** — `git add -A && git commit -m "feat: dedup by message id and normalized text"`

---

### Task 8: Очередь

**Files:**
- Create: `src/queue.rs`
- Modify: `src/main.rs` (`mod queue;`)

**Interfaces:**
- Produces:
  - `#[derive(Clone, Debug, PartialEq)] pub enum Kind { Status, Urgent, Manual }` — `Status` устаревает, `Urgent` вперёд, `Manual` (ручное чтение) не устаревает.
  - `#[derive(Clone, Debug)] pub struct Item { pub session: String, pub text: String, pub kind: Kind, pub born: Instant }`
  - `pub struct Queue`, `Queue::new(max_age: Duration)`,
  - `fn push(&mut self, item: Item)` (Urgent — в начало, за другими Urgent),
  - `fn push_front(&mut self, item: Item)` (возврат прерванной паузой фразы),
  - `fn pop(&mut self, now: Instant) -> Option<Item>` (выбрасывает устаревшие `Status`; на паузе — `None`),
  - `fn pause(&mut self, now: Instant)`, `fn resume(&mut self, now: Instant)`, `fn paused(&self) -> bool`,
  - `fn clear(&mut self)`, `fn clear_session(&mut self, session: &str)`, `fn is_empty(&self) -> bool`,
  - `fn set_max_age(&mut self, d: Duration)`.

- [ ] **Step 1: Тесты** (низ `queue.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn it(text: &str, kind: Kind, born: Instant) -> Item {
        Item { session: "s".into(), text: text.into(), kind, born }
    }

    #[test]
    fn stale_status_dropped_manual_kept() {
        let t0 = Instant::now();
        let mut q = Queue::new(Duration::from_secs(30));
        q.push(it("старый", Kind::Status, t0));
        q.push(it("ручной", Kind::Manual, t0));
        let later = t0 + Duration::from_secs(31);
        assert_eq!(q.pop(later).unwrap().text, "ручной");
        assert!(q.pop(later).is_none());
    }

    #[test]
    fn urgent_goes_first_in_order() {
        let t0 = Instant::now();
        let mut q = Queue::new(Duration::from_secs(30));
        q.push(it("a", Kind::Status, t0));
        q.push(it("u1", Kind::Urgent, t0));
        q.push(it("u2", Kind::Urgent, t0));
        let got: Vec<String> = std::iter::from_fn(|| q.pop(t0)).map(|i| i.text).collect();
        assert_eq!(got, vec!["u1", "u2", "a"]);
    }

    #[test]
    fn pause_freezes_age() {
        let t0 = Instant::now();
        let mut q = Queue::new(Duration::from_secs(30));
        q.push(it("статус", Kind::Status, t0));
        q.pause(t0 + Duration::from_secs(5));
        assert!(q.pop(t0 + Duration::from_secs(10)).is_none());
        q.resume(t0 + Duration::from_secs(65)); // пауза 60 с
        assert_eq!(q.pop(t0 + Duration::from_secs(66)).unwrap().text, "статус");
    }

    #[test]
    fn clear_session_only() {
        let t0 = Instant::now();
        let mut q = Queue::new(Duration::from_secs(30));
        q.push(Item { session: "a".into(), text: "1".into(), kind: Kind::Status, born: t0 });
        q.push(Item { session: "b".into(), text: "2".into(), kind: Kind::Status, born: t0 });
        q.clear_session("a");
        assert_eq!(q.pop(t0).unwrap().text, "2");
    }
}
```

- [ ] **Step 2: Падают** — `cargo test queue` → FAIL.

- [ ] **Step 3: Реализация** (над тестами)

```rust
//! Очередь фраз: устаревшие статусы выбрасываются, срочные вперёд, на паузе возраст не растёт.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    Status,
    Urgent,
    Manual,
}

#[derive(Clone, Debug)]
pub struct Item {
    pub session: String,
    pub text: String,
    pub kind: Kind,
    pub born: Instant,
}

pub struct Queue {
    items: VecDeque<Item>,
    max_age: Duration,
    paused_at: Option<Instant>,
    paused_total: Duration,
}

impl Queue {
    pub fn new(max_age: Duration) -> Queue {
        Queue { items: VecDeque::new(), max_age, paused_at: None, paused_total: Duration::ZERO }
    }

    pub fn set_max_age(&mut self, d: Duration) {
        self.max_age = d;
    }

    /// born хранится со сдвигом на суммарную паузу: возраст = now - born - пауза.
    pub fn push(&mut self, mut item: Item) {
        item.born += self.paused_total;
        if item.kind == Kind::Urgent {
            let pos = self.items.iter().take_while(|i| i.kind == Kind::Urgent).count();
            self.items.insert(pos, item);
        } else {
            self.items.push_back(item);
        }
    }

    pub fn push_front(&mut self, item: Item) {
        self.items.push_front(item);
    }

    pub fn pop(&mut self, now: Instant) -> Option<Item> {
        if self.paused_at.is_some() {
            return None;
        }
        while let Some(item) = self.items.pop_front() {
            let age = (now - self.paused_total).saturating_duration_since(item.born);
            if item.kind == Kind::Status && age > self.max_age {
                continue;
            }
            return Some(item);
        }
        None
    }

    pub fn pause(&mut self, now: Instant) {
        self.paused_at.get_or_insert(now);
    }

    pub fn resume(&mut self, now: Instant) {
        if let Some(p) = self.paused_at.take() {
            self.paused_total += now - p;
        }
    }

    pub fn paused(&self) -> bool {
        self.paused_at.is_some()
    }

    pub fn clear(&mut self) {
        self.items.clear();
    }

    pub fn clear_session(&mut self, session: &str) {
        self.items.retain(|i| i.session != session);
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}
```

Примечание к `pop`: `now - self.paused_total` и `item.born + paused_total` при `push` вместе дают «возраст без пауз, прошедших после постановки». В `pause_freezes_age`: born=t0 (paused_total=0 при push), после resume paused_total=60 с, now=t0+66 → (t0+66−60)−t0 = 6 с < 30 — фраза жива.

- [ ] **Step 4: Проходят** — `cargo test queue` → ok.

- [ ] **Step 5: Commit** — `git add -A && git commit -m "feat: speech queue with age, urgency, pause"`

---

### Task 9: Сборка предложений из `MessageDisplay`

**Files:**
- Create: `src/assemble.rs`
- Modify: `src/main.rs` (`mod assemble;`)

**Interfaces:**
- Consumes: факты из Task 1 (имена полей). Ниже предполагается `{"session_id","message_id","delta","final"}`; если Task 1 показал иное — поменять только `MdEvent::from_json`.
- Produces:
  - `pub struct MdEvent { pub session: String, pub message_id: String, pub delta: String, pub done: bool }`, `MdEvent::from_json(v: &serde_json::Value) -> Option<MdEvent>`
  - `pub struct Assembler`, `Assembler::new()`, `fn push(&mut self, ev: &MdEvent) -> Vec<String>` — законченные куски текста (до конца предложения или абзаца) по мере поступления; при `done` — остаток.
  - `fn flush_session(&mut self, session: &str) -> Vec<String>` — остаток всех сообщений сессии (хук `Stop`).

- [ ] **Step 1: Тесты** (низ `assemble.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn ev(delta: &str, done: bool) -> MdEvent {
        MdEvent { session: "s".into(), message_id: "m".into(), delta: delta.into(), done }
    }

    #[test]
    fn emits_complete_sentences_only() {
        let mut a = Assembler::new();
        assert!(a.push(&ev("Начинаю пер", false)).is_empty());
        assert_eq!(a.push(&ev("вый шаг. Потом вто", false)), vec!["Начинаю первый шаг."]);
        assert_eq!(a.push(&ev("рой", true)), vec!["Потом второй"]);
    }

    #[test]
    fn code_fence_waits_until_closed() {
        let mut a = Assembler::new();
        assert!(a.push(&ev("Код:\n```\nls. -la\n", false)).iter().all(|s| !s.contains("ls")));
        let out = a.push(&ev("```\nГотово.", true));
        assert!(out.join(" ").contains("```"));
    }

    #[test]
    fn flush_session_returns_rest() {
        let mut a = Assembler::new();
        a.push(&ev("Хвост без точки", false));
        assert_eq!(a.flush_session("s"), vec!["Хвост без точки"]);
        assert!(a.flush_session("s").is_empty());
    }

    #[test]
    fn real_fixture_parses() {
        let fx = include_str!("../tests/fixtures/message_display.jsonl");
        let n = fx.lines().filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok()).filter_map(|v| MdEvent::from_json(&v)).count();
        assert!(n > 0, "MdEvent::from_json не понимает настоящие события — сверить поля с Task 1");
    }
}
```

- [ ] **Step 2: Падают** — `cargo test assemble` → FAIL.

- [ ] **Step 3: Реализация** (над тестами)

```rust
//! Хук MessageDisplay присылает текст кусками — отдаём законченные предложения сразу.

use std::collections::HashMap;

pub struct MdEvent {
    pub session: String,
    pub message_id: String,
    pub delta: String,
    pub done: bool,
}

impl MdEvent {
    pub fn from_json(v: &serde_json::Value) -> Option<MdEvent> {
        Some(MdEvent {
            session: v["session_id"].as_str()?.to_string(),
            message_id: v["message_id"].as_str()?.to_string(),
            delta: v["delta"].as_str().unwrap_or_default().to_string(),
            done: v["final"].as_bool().unwrap_or(false),
        })
    }
}

pub struct Assembler {
    bufs: HashMap<(String, String), String>,
}

impl Assembler {
    pub fn new() -> Assembler {
        Assembler { bufs: HashMap::new() }
    }

    pub fn push(&mut self, ev: &MdEvent) -> Vec<String> {
        let key = (ev.session.clone(), ev.message_id.clone());
        let buf = self.bufs.entry(key.clone()).or_default();
        buf.push_str(&ev.delta);
        let mut out = Vec::new();
        while let Some(cut) = sentence_end(buf) {
            let s: String = buf.drain(..cut).collect();
            if !s.trim().is_empty() {
                out.push(s.trim().to_string());
            }
        }
        if ev.done {
            let rest = self.bufs.remove(&key).unwrap_or_default();
            if !rest.trim().is_empty() {
                out.push(rest.trim().to_string());
            }
        }
        out
    }

    pub fn flush_session(&mut self, session: &str) -> Vec<String> {
        let keys: Vec<_> = self.bufs.keys().filter(|(s, _)| s == session).cloned().collect();
        keys.into_iter()
            .filter_map(|k| self.bufs.remove(&k))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    }
}

/// Байтовая позиция конца первого законченного предложения/абзаца; внутри незакрытого ``` — None.
fn sentence_end(buf: &str) -> Option<usize> {
    let mut in_code = false;
    let mut line_start = 0;
    let b = buf.as_bytes();
    for (i, c) in buf.char_indices() {
        if c == '\n' {
            if buf[line_start..i].trim_start().starts_with("```") {
                in_code = !in_code;
                if !in_code {
                    return Some(i + 1); // блок кода целиком — очистка его выбросит
                }
            } else if !in_code && i > 0 {
                return Some(i + 1);
            }
            line_start = i + 1;
            continue;
        }
        if in_code {
            continue;
        }
        if matches!(c, '.' | '!' | '?' | '…') {
            let next = i + c.len_utf8();
            if next < b.len() && (b[next] as char).is_whitespace() {
                return Some(next);
            }
        }
    }
    None
}
```

- [ ] **Step 4: Проходят** — `cargo test assemble` → ok.

- [ ] **Step 5: Commit** — `git add -A && git commit -m "feat: sentence assembly from MessageDisplay deltas"`

---

### Task 10: Синтез и звук: `Tts`, `Player`, поток воспроизведения

**Files:**
- Create: `src/tts.rs`, `src/audio.rs`, `src/speaker.rs`, `src/notice.rs`
- Modify: `src/main.rs` (`mod tts; mod audio; mod speaker; mod notice;`)

**Interfaces:**
- Consumes: `Queue`, `Item`, `Kind` (Task 8); протокол `silero_tts.py` (Task 2).
- Produces:
  - `notice::notify(title: &str, body: &str)`
  - `pub struct Tts`, `Tts::new(python: PathBuf, script: PathBuf, model: PathBuf, sock: PathBuf) -> Tts`, `fn synth(&mut self, text: &str, speaker: &str, rate: &str) -> Option<Vec<u8>>` — при первом вызове/после сбоя запускает процесс и ждёт сокет (до 30 с); 3 неудачи подряд → уведомление «Синтез упал», `None`.
  - `pub struct Player`, `Player::new() -> Player`, `fn write(&mut self, pcm: &[u8]) -> bool`, `fn reset(&mut self)` (убить `pw-cat`; следующий `write` запустит новый).
  - `pub struct Shared { pub queue: Mutex<Queue>, pub cv: Condvar, pub generation: AtomicU64, pub voice: Mutex<(String, String)>, pub busy: AtomicBool }`
  - `pub fn run(shared: Arc<Shared>, tts: Tts)` — бесконечный цикл воспроизведения (запускается в отдельном потоке).
  - Правило остановки: любой, кто хочет оборвать звук (стоп, пауза), увеличивает `generation` и делает `cv.notify_all()`.

- [ ] **Step 1: `notice.rs`**

```rust
pub fn notify(title: &str, body: &str) {
    let _ = std::process::Command::new("notify-send").args(["-t", "2500", "-a", "Озвучка", title, body]).spawn();
}
```

- [ ] **Step 2: Тест `Tts` (интеграционный, с настоящей моделью)** — низ `tts.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore] // нужна модель: cargo test tts -- --ignored
    fn synthesizes_and_restarts() {
        let data = std::path::PathBuf::from(std::env::var("HOME").unwrap()).join(".local/share/agent-speak");
        let sock = std::env::temp_dir().join(format!("tts-test-{}.sock", std::process::id()));
        let mut t = Tts::new(
            data.join("venv/bin/python"),
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("python/silero_tts.py"),
            data.join("v5_ru.pt"),
            sock,
        );
        assert!(t.synth("Проверка связи.", "xenia", "medium").unwrap().len() > 48000);
        t.kill();
        assert!(t.synth("Снова работает.", "xenia", "medium").unwrap().len() > 48000);
    }
}
```

- [ ] **Step 3: Падает** — `cargo test tts -- --ignored` → FAIL (нет `Tts`).

- [ ] **Step 4: Реализация `tts.rs`** (над тестом)

```rust
//! Дочерний процесс silero_tts.py: модель в памяти, перезапуск при сбое.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

pub struct Tts {
    python: PathBuf,
    script: PathBuf,
    model: PathBuf,
    sock: PathBuf,
    child: Option<Child>,
    failures: u32,
}

impl Tts {
    pub fn new(python: PathBuf, script: PathBuf, model: PathBuf, sock: PathBuf) -> Tts {
        Tts { python, script, model, sock, child: None, failures: 0 }
    }

    pub fn kill(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        let _ = std::fs::remove_file(&self.sock);
    }

    fn ensure(&mut self) -> bool {
        if let Some(c) = &mut self.child {
            if c.try_wait().ok().flatten().is_none() {
                return true;
            }
        }
        self.kill();
        let Ok(child) = Command::new(&self.python).arg(&self.script).arg(&self.model).arg(&self.sock).spawn() else {
            return false;
        };
        self.child = Some(child);
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_secs(30) {
            if self.sock.exists() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        false
    }

    fn request(&self, text: &str, speaker: &str, rate: &str) -> std::io::Result<Vec<u8>> {
        let mut s = UnixStream::connect(&self.sock)?;
        s.set_read_timeout(Some(Duration::from_secs(60)))?;
        let req = serde_json::json!({"text": text, "speaker": speaker, "rate": rate});
        s.write_all(format!("{req}\n").as_bytes())?;
        let mut len = [0u8; 4];
        s.read_exact(&mut len)?;
        let mut pcm = vec![0u8; u32::from_le_bytes(len) as usize];
        s.read_exact(&mut pcm)?;
        Ok(pcm)
    }

    pub fn synth(&mut self, text: &str, speaker: &str, rate: &str) -> Option<Vec<u8>> {
        for _ in 0..3 {
            if self.ensure() {
                if let Ok(pcm) = self.request(text, speaker, rate) {
                    self.failures = 0;
                    return Some(pcm);
                }
            }
            self.kill();
            self.failures += 1;
        }
        if self.failures >= 3 {
            crate::notice::notify("Синтез упал", "silero_tts.py не отвечает");
            self.failures = 0;
        }
        None
    }
}

impl Drop for Tts {
    fn drop(&mut self) {
        self.kill();
    }
}
```

- [ ] **Step 5: Проходит** — `cargo test tts -- --ignored` → ok (~5 с).

- [ ] **Step 6: Реализация `audio.rs`**

```rust
//! Постоянный pw-cat: PCM пишется в stdin; reset — мгновенно оборвать звук.

use std::io::Write;
use std::process::{Child, Command, Stdio};

pub struct Player {
    child: Option<Child>,
}

impl Player {
    pub fn new() -> Player {
        Player { child: None }
    }

    pub fn write(&mut self, pcm: &[u8]) -> bool {
        if self.child.is_none() {
            self.child = Command::new("pw-cat")
                .args(["--playback", "--raw", "--format", "s16", "--rate", "48000", "--channels", "1", "-"])
                .stdin(Stdio::piped())
                .spawn()
                .ok();
        }
        let ok = self.child.as_mut().and_then(|c| c.stdin.as_mut()).is_some_and(|s| s.write_all(pcm).is_ok());
        if !ok {
            self.reset();
        }
        ok
    }

    pub fn reset(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}
```

- [ ] **Step 7: Реализация `speaker.rs`**

```rust
//! Поток воспроизведения: берёт фразу, синтезирует, пишет в pw-cat кусками по 0,1 с,
//! между кусками проверяет generation (стоп/пауза).

use crate::audio::Player;
use crate::queue::Queue;
use crate::tts::Tts;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

pub struct Shared {
    pub queue: Mutex<Queue>,
    pub cv: Condvar,
    pub generation: AtomicU64,
    pub voice: Mutex<(String, String)>, // (speaker, rate)
    pub busy: AtomicBool,
}

impl Shared {
    pub fn new(queue: Queue, speaker: String, rate: String) -> Shared {
        Shared {
            queue: Mutex::new(queue),
            cv: Condvar::new(),
            generation: AtomicU64::new(0),
            voice: Mutex::new((speaker, rate)),
            busy: AtomicBool::new(false),
        }
    }

    /// Оборвать текущую фразу (стоп, пауза).
    pub fn interrupt(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.cv.notify_all();
    }
}

const CHUNK: usize = 9600; // 0,1 с s16 48 кГц моно

// ponytail: «синтез наперёд» — за счёт буфера канала pw-cat (~0,7 с): пока доигрывает хвост,
// синтезируется следующая фраза. Явный конвейер на 1–2 фразы — если паузы между фразами заметны.
pub fn run(shared: Arc<Shared>, mut tts: Tts) {
    let mut player = Player::new();
    loop {
        let item = {
            let mut q = shared.queue.lock().unwrap();
            loop {
                if let Some(it) = q.pop(Instant::now()) {
                    break it;
                }
                shared.busy.store(false, Ordering::SeqCst);
                q = shared.cv.wait_timeout(q, Duration::from_millis(500)).unwrap().0;
            }
        };
        shared.busy.store(true, Ordering::SeqCst);
        let gen = shared.generation.load(Ordering::SeqCst);
        let (speaker, rate) = shared.voice.lock().unwrap().clone();
        let Some(pcm) = tts.synth(&item.text, &speaker, &rate) else { continue };
        for chunk in pcm.chunks(CHUNK) {
            if shared.generation.load(Ordering::SeqCst) != gen {
                player.reset();
                let mut q = shared.queue.lock().unwrap();
                if q.paused() {
                    q.push_front(item.clone()); // после паузы — с начала предложения
                }
                break;
            }
            if !player.write(chunk) {
                break;
            }
        }
    }
}
```

- [ ] **Step 8: Сборка**

Run: `cargo build && cargo test`
Expected: сборка без ошибок, все неигнорируемые тесты ok. Предупреждения `dead_code` допустимы до Task 12.

- [ ] **Step 9: Commit** — `git add -A && git commit -m "feat: TTS process, pw-cat player, speaker thread"`

---

### Task 11: Окно в фокусе и фоновое пополнение словаря

**Files:**
- Create: `src/focus.rs`, `src/learn.rs`
- Modify: `src/main.rs` (`mod focus; mod learn;`)

**Interfaces:**
- Produces:
  - `#[derive(Clone, Debug, PartialEq)] pub enum Agent { Claude, Codex }`
  - `#[derive(Clone, Debug, PartialEq)] pub struct SessionRef { pub agent: Agent, pub id: String, pub transcript: PathBuf }`
  - `focus::focused() -> Option<SessionRef>` — окно в фокусе (`hyprctl activewindow -j`).
  - `focus::session_of(path: &Path) -> Option<(Agent, String)>` — по пути транскрипта: Claude — имя файла без `.jsonl`; Codex — 36 последних символов имени без `.jsonl` (UUID).
  - `focus::parse_learned(out: &str, asked: &[String]) -> Vec<(String, String)>` — строки `слово<TAB>произношение`, только из `asked`, произношение — кириллица без латиницы.
  - `learn::spawn(terms: Arc<Mutex<Terms>>) -> mpsc::Sender<(Agent, String)>` — поток: копит слова 2 с (до 20), спрашивает модель агента, дописывает словарь.

- [ ] **Step 1: Тесты** — низ `focus.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_from_paths() {
        let c = Path::new("/home/u/.claude/projects/-x/00000000-0000-0000-0000-000000000001.jsonl");
        assert_eq!(session_of(c), Some((Agent::Claude, "00000000-0000-0000-0000-000000000001".into())));
        let x = Path::new("/home/u/.codex/sessions/2026/10/02/rollout-2026-10-02T19-44-44-00000000-0000-0000-0000-000000000002.jsonl");
        assert_eq!(session_of(x), Some((Agent::Codex, "00000000-0000-0000-0000-000000000002".into())));
        assert_eq!(session_of(Path::new("/tmp/a.txt")), None);
    }

    #[test]
    fn learned_lines_validated() {
        let asked = vec!["tokio".to_string(), "serde".to_string()];
        let out = "tokio\tток+ио\nserde\tserde\nfoo\tфу\nмусор";
        assert_eq!(parse_learned(out, &asked), vec![("tokio".to_string(), "ток+ио".to_string())]);
    }
}
```

- [ ] **Step 2: Падают** — `cargo test focus` → FAIL.

- [ ] **Step 3: Реализация `focus.rs`** (над тестами)

```rust
//! Окно в фокусе → сессия агента; путь транскрипта → сессия.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Debug, PartialEq)]
pub enum Agent {
    Claude,
    Codex,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SessionRef {
    pub agent: Agent,
    pub id: String,
    pub transcript: PathBuf,
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default())
}

pub fn session_of(path: &Path) -> Option<(Agent, String)> {
    let stem = path.file_name()?.to_str()?.strip_suffix(".jsonl")?;
    let s = path.to_str()?;
    if s.contains("/.claude/projects/") {
        Some((Agent::Claude, stem.to_string()))
    } else if s.contains("/.codex/sessions/") && stem.len() >= 36 {
        Some((Agent::Codex, stem[stem.len() - 36..].to_string()))
    } else {
        None
    }
}

fn ppid(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    stat.rsplit_once(')')?.1.split_whitespace().nth(1)?.parse().ok()
}

fn under(mut pid: u32, ancestor: u32) -> bool {
    while pid > 1 {
        if pid == ancestor {
            return true;
        }
        match ppid(pid) {
            Some(p) => pid = p,
            None => return false,
        }
    }
    false
}

pub fn focused() -> Option<SessionRef> {
    let out = Command::new("hyprctl").args(["activewindow", "-j"]).output().ok()?;
    let win = serde_json::from_slice::<Value>(&out.stdout).ok()?["pid"].as_u64()? as u32;
    claude_under(win).or_else(|| codex_under(win))
}

fn claude_under(win: u32) -> Option<SessionRef> {
    for f in std::fs::read_dir(home().join(".claude/sessions")).ok()?.flatten() {
        let Ok(s) = std::fs::read_to_string(f.path()) else { continue };
        let Ok(v) = serde_json::from_str::<Value>(&s) else { continue };
        let (Some(pid), Some(sid)) = (v["pid"].as_u64(), v["sessionId"].as_str()) else { continue };
        if !under(pid as u32, win) {
            continue;
        }
        let name = format!("{sid}.jsonl");
        for d in std::fs::read_dir(home().join(".claude/projects")).ok()?.flatten() {
            let p = d.path().join(&name);
            if p.exists() {
                return Some(SessionRef { agent: Agent::Claude, id: sid.to_string(), transcript: p });
            }
        }
    }
    None
}

fn codex_under(win: u32) -> Option<SessionRef> {
    for p in std::fs::read_dir("/proc").ok()?.flatten() {
        let Some(pid) = p.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else { continue };
        if std::fs::read_to_string(format!("/proc/{pid}/comm")).map(|c| c.trim() == "codex") != Ok(true) || !under(pid, win) {
            continue;
        }
        for fd in std::fs::read_dir(format!("/proc/{pid}/fd")).ok()?.flatten() {
            let Ok(target) = std::fs::read_link(fd.path()) else { continue };
            if let Some((Agent::Codex, id)) = session_of(&target) {
                return Some(SessionRef { agent: Agent::Codex, id, transcript: target });
            }
        }
    }
    None
}

pub fn parse_learned(out: &str, asked: &[String]) -> Vec<(String, String)> {
    out.lines()
        .filter_map(|l| l.split_once('\t'))
        .map(|(w, p)| (w.trim().to_lowercase(), p.trim().to_string()))
        .filter(|(w, p)| {
            asked.contains(w) && !p.is_empty() && p.chars().any(|c| ('а'..='я').contains(&c.to_lowercase().next().unwrap()))
                && !p.chars().any(|c| c.is_ascii_alphabetic())
        })
        .collect()
}
```

- [ ] **Step 4: Реализация `learn.rs`**

```rust
//! Фон: незнакомые слова → самая лёгкая модель агента → словарь. На путь к звуку не влияет.

use crate::focus::{parse_learned, Agent};
use crate::text::terms::Terms;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const PROMPT: &str = "Для каждого английского слова или аббревиатуры ниже напиши, как его произносят русскоязычные разработчики, кириллицей. \
Аббревиатуры по буквам через дефис (npm — эн-пи-эм). Ударение можно отметить знаком + перед ударной гласной. \
Формат строго: слово<TAB>произношение, по строке на слово, без пояснений.\n\n";

pub fn spawn(terms: Arc<Mutex<Terms>>) -> Sender<(Agent, String)> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || run(rx, terms));
    tx
}

fn run(rx: Receiver<(Agent, String)>, terms: Arc<Mutex<Terms>>) {
    let mut asked_once: std::collections::HashSet<String> = Default::default();
    loop {
        let Ok(first) = rx.recv() else { return };
        let mut batch = vec![first];
        while batch.len() < 20 {
            match rx.recv_timeout(Duration::from_secs(2)) {
                Ok(x) => batch.push(x),
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
        let agent = batch[0].0.clone();
        let words: Vec<String> = {
            let t = terms.lock().unwrap();
            let mut w: Vec<String> = batch.into_iter().map(|(_, w)| w).filter(|w| !t.contains(w)).collect();
            w.sort();
            w.dedup();
            w
        };
        // ponytail: слово спрашиваем раз за запуск сервиса; неудача — повтор после перезапуска
        let words: Vec<String> = words.into_iter().filter(|w| asked_once.insert(w.clone())).collect();
        if words.is_empty() {
            continue;
        }
        let Some(out) = ask(&agent, &format!("{PROMPT}{}", words.join("\n"))) else { continue };
        let mut t = terms.lock().unwrap();
        for (w, p) in parse_learned(&out, &words) {
            t.add(&w, &p);
        }
    }
}

fn ask(agent: &Agent, prompt: &str) -> Option<String> {
    let mut cmd = match agent {
        Agent::Claude => {
            let mut c = Command::new("timeout");
            // промпт сразу после -p: --tools принимает несколько значений и проглотил бы его
            c.args(["60", "claude", "-p", prompt, "--model", "haiku", "--no-session-persistence", "--setting-sources", "",
                    "--strict-mcp-config", "--tools", ""])
                .env("MAX_THINKING_TOKENS", "0")
                .env("CLAUDE_CODE_DISABLE_AUTO_MEMORY", "1");
            c
        }
        Agent::Codex => {
            let mut c = Command::new("timeout");
            c.args(["60", "codex", "exec", "--ephemeral", "--skip-git-repo-check", "-m", &codex_small()?,
                    "-c", "model_reasoning_effort=low", prompt]);
            c
        }
    };
    let out = cmd.env("AGENT_SPEAK", "1").stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Самая лёгкая модель Codex: из «fast»-моделей в списке — первая по priority.
fn codex_small() -> Option<String> {
    let home = std::env::var("HOME").ok()?;
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(format!("{home}/.codex/models_cache.json")).ok()?).ok()?;
    v["models"].as_array()?
        .iter()
        .filter(|m| m["visibility"] == "list" && m["description"].as_str().is_some_and(|d| d.to_lowercase().contains("fast")))
        .min_by_key(|m| m["priority"].as_i64().unwrap_or(i64::MAX))
        .and_then(|m| m["slug"].as_str().map(String::from))
}
```

- [ ] **Step 5: Проходят + ручная проверка модели**

Run: `cargo test focus`
Expected: ok.

Ручная проверка промпта (одноразово):
```bash
cd /tmp && printf '%s' "$(sed -n '/^const PROMPT/,/;$/p' ~/Projects/agent-speak/src/learn.rs | sed 's/^const PROMPT: &str = "//; s/\\$//; s/";$//; s/\\n/\n/g')
tokio
serde
ssh" | MAX_THINKING_TOKENS=0 claude -p --model haiku --no-session-persistence --setting-sources "" --strict-mcp-config --tools ""
```
Expected: три строки `слово<TAB>кириллица`. Если модель добавляет пояснения — `parse_learned` их отсеет; если формат систематически другой (например, « — » вместо TAB) — добавить разбор ` — ` в `parse_learned` с тестом.

- [ ] **Step 6: Commit** — `git add -A && git commit -m "feat: focused session lookup, background term learning"`

---

### Task 12: Сервис, IPC, команды, конфиг

**Files:**
- Create: `src/ipc.rs`, `src/config.rs`, `src/daemon.rs`
- Modify: `src/main.rs` (целиком)

**Interfaces:**
- Consumes: всё из Task 3–11.
- Produces:
  - `#[derive(Serialize, Deserialize, Debug)] #[serde(tag = "cmd", rename_all = "snake_case")] pub enum Msg { Hook { kind: String, payload: serde_json::Value }, Read, Stop, Pause, Mode }`
  - `ipc::socket_path() -> PathBuf`, `ipc::send(msg: &Msg) -> bool` (`false` — сервис недоступен)
  - `config::Config { mode: String, speaker: String, rate: String, max_age_secs: u64, read_intermediate: bool }` + `Config::load() -> Config`, `fn save(&self)`; путь `~/.config/agent-speak/config.toml`.
  - `daemon::run()`
  - CLI: `agent-speak daemon | read | stop | pause | mode | hook <kind> [json]` (`json` из аргумента — для Codex `notify`, иначе из stdin).

- [ ] **Step 1: Тесты IPC и конфига** — низ `ipc.rs`:

```rust
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
    }

    #[test]
    fn send_without_daemon_is_false() {
        unsafe { std::env::set_var("XDG_RUNTIME_DIR", "/nonexistent-agent-speak") };
        assert!(!send(&Msg::Stop));
    }
}
```

Низ `config.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_partial_file() {
        let c: Config = toml::from_str("speaker = \"baya\"").unwrap();
        assert_eq!(c.speaker, "baya");
        assert_eq!(c.mode, "manual");
        assert_eq!(c.rate, "medium");
        assert_eq!(c.max_age_secs, 30);
        assert!(c.read_intermediate);
    }
}
```

- [ ] **Step 2: Падают** — `cargo test ipc config` → FAIL.

- [ ] **Step 3: `ipc.rs`** (над тестами)

```rust
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
}

pub fn socket_path() -> PathBuf {
    PathBuf::from(std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into())).join("agent-speak.sock")
}

pub fn send(msg: &Msg) -> bool {
    let Ok(mut s) = UnixStream::connect(socket_path()) else { return false };
    s.write_all(format!("{}\n", serde_json::to_string(msg).unwrap()).as_bytes()).is_ok()
}
```

- [ ] **Step 4: `config.rs`** (над тестами)

```rust
//! ~/.config/agent-speak/config.toml; отсутствующие поля — по умолчанию.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(default)]
pub struct Config {
    pub mode: String,
    pub speaker: String,
    pub rate: String,
    pub max_age_secs: u64,
    /// false — в авторежиме только финальный ответ хода, без промежуточных статусов
    pub read_intermediate: bool,
}

impl Default for Config {
    fn default() -> Config {
        Config { mode: "manual".into(), speaker: "xenia".into(), rate: "medium".into(), max_age_secs: 30, read_intermediate: true }
    }
}

pub fn path() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config/agent-speak/config.toml")
}

impl Config {
    pub fn load() -> Config {
        std::fs::read_to_string(path()).ok().and_then(|s| toml::from_str(&s).ok()).unwrap_or_default()
    }

    pub fn save(&self) {
        let p = path();
        let _ = std::fs::create_dir_all(p.parent().unwrap());
        let _ = std::fs::write(p, toml::to_string(self).unwrap());
    }
}
```

- [ ] **Step 5: Проходят** — `cargo test ipc config` → ok.

- [ ] **Step 6: `daemon.rs`**

```rust
//! Цикл событий сервиса: команды и хуки из сокета, изменения транскриптов (inotify).

use crate::assemble::{Assembler, MdEvent};
use crate::config::Config;
use crate::dedup::Dedup;
use crate::focus::{self, Agent, SessionRef};
use crate::ipc::{self, Msg};
use crate::notice::notify;
use crate::queue::{Item, Kind, Queue};
use crate::source::{claude, codex, tail::Tailer};
use crate::speaker::{self, Shared};
use crate::text::{prepare, terms::Terms};
use crate::tts::Tts;
use notify::{RecursiveMode, Watcher};
use std::io::BufRead;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

enum Event {
    Msg(Msg),
    File(PathBuf),
    Config,
}

struct State {
    cfg: Config,
    shared: Arc<Shared>,
    terms: Arc<Mutex<Terms>>,
    learn: Sender<(Agent, String)>,
    dedup: Dedup,
    asm: Assembler,
    tail: Tailer,
    focus_cache: (Instant, Option<SessionRef>),
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default())
}

pub fn run() {
    let data = home().join(".local/share/agent-speak");
    let cfg = Config::load();
    let shared = Arc::new(Shared::new(
        Queue::new(Duration::from_secs(cfg.max_age_secs)),
        cfg.speaker.clone(),
        cfg.rate.clone(),
    ));
    let tts = Tts::new(
        data.join("venv/bin/python"),
        data.join("silero_tts.py"),
        data.join("v5_ru.pt"),
        PathBuf::from(std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into())).join("agent-speak-tts.sock"),
    );
    {
        let shared = shared.clone();
        std::thread::spawn(move || speaker::run(shared, tts));
    }
    let terms = Arc::new(Mutex::new(Terms::load(&data.join("terms.tsv"))));
    let learn = crate::learn::spawn(terms.clone());

    let (tx, rx) = mpsc::channel::<Event>();
    let sock = ipc::socket_path();
    let _ = std::fs::remove_file(&sock);
    let listener = UnixListener::bind(&sock).expect("bind agent-speak.sock");
    {
        let tx = tx.clone();
        std::thread::spawn(move || {
            for conn in listener.incoming().flatten() {
                let mut line = String::new();
                if std::io::BufReader::new(conn).read_line(&mut line).is_ok() {
                    match serde_json::from_str::<Msg>(&line) {
                        Ok(m) => {
                            let _ = tx.send(Event::Msg(m));
                        }
                        Err(e) => eprintln!("agent-speak: плохое сообщение: {e}"),
                    }
                }
            }
        });
    }
    let ftx = tx.clone();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(ev) = res {
            if matches!(ev.kind, notify::EventKind::Modify(_) | notify::EventKind::Create(_)) {
                for p in ev.paths {
                    if p.extension().is_some_and(|e| e == "jsonl") {
                        let _ = ftx.send(Event::File(p));
                    } else if p == crate::config::path() {
                        let _ = ftx.send(Event::Config);
                    }
                }
            }
        }
    })
    .expect("inotify");
    for dir in [home().join(".claude/projects"), home().join(".codex/sessions")] {
        if let Err(e) = watcher.watch(&dir, RecursiveMode::Recursive) {
            eprintln!("agent-speak: не слежу за {}: {e}", dir.display());
        }
    }
    let cfg_dir = crate::config::path().parent().unwrap().to_path_buf();
    let _ = std::fs::create_dir_all(&cfg_dir);
    if let Err(e) = watcher.watch(&cfg_dir, RecursiveMode::NonRecursive) {
        eprintln!("agent-speak: не слежу за настройками: {e}");
    }

    let mut st = State {
        cfg,
        shared,
        terms,
        learn,
        dedup: Dedup::new(),
        asm: Assembler::new(),
        tail: Tailer::new(),
        focus_cache: (Instant::now() - Duration::from_secs(10), None),
    };
    for ev in rx {
        match ev {
            Event::Msg(m) => st.on_msg(m),
            Event::File(p) => st.on_file(&p),
            Event::Config => st.reload(),
        }
    }
}

impl State {
    fn focused(&mut self) -> Option<SessionRef> {
        if self.focus_cache.0.elapsed() > Duration::from_millis(500) {
            self.focus_cache = (Instant::now(), focus::focused());
        }
        self.focus_cache.1.clone()
    }

    fn auto(&self) -> bool {
        self.cfg.mode == "auto"
    }

    /// Промежуточные статусы читаются по ходу работы (авто + read_intermediate).
    fn live(&self) -> bool {
        self.auto() && self.cfg.read_intermediate
    }

    /// config.toml изменён (интерфейсом или руками) — применить без перезапуска.
    fn reload(&mut self) {
        self.cfg = Config::load();
        *self.shared.voice.lock().unwrap() = (self.cfg.speaker.clone(), self.cfg.rate.clone());
        self.shared.queue.lock().unwrap().set_max_age(Duration::from_secs(self.cfg.max_age_secs));
    }

    /// Подготовить и поставить в очередь; незнакомые слова — в фон.
    fn enqueue(&mut self, agent: Agent, session: &str, raw: &str, kind: Kind) -> usize {
        let (sentences, unknown) = prepare(raw, &self.terms.lock().unwrap());
        for w in unknown {
            let _ = self.learn.send((agent.clone(), w));
        }
        let n = sentences.len();
        let mut q = self.shared.queue.lock().unwrap();
        for text in sentences {
            q.push(Item { session: session.into(), text, kind: kind.clone(), born: Instant::now() });
        }
        drop(q);
        self.shared.cv.notify_all();
        n
    }

    fn stop(&mut self) {
        let mut q = self.shared.queue.lock().unwrap();
        q.clear();
        q.resume(Instant::now());
        drop(q);
        self.shared.interrupt();
    }

    fn on_msg(&mut self, m: Msg) {
        match m {
            Msg::Stop => self.stop(),
            Msg::Pause => {
                let mut q = self.shared.queue.lock().unwrap();
                if q.paused() {
                    q.resume(Instant::now());
                    drop(q);
                    self.shared.cv.notify_all();
                } else {
                    q.pause(Instant::now());
                    drop(q);
                    self.shared.interrupt();
                }
            }
            Msg::Mode => {
                self.cfg.mode = if self.auto() { "manual".into() } else { "auto".into() };
                self.cfg.save();
                if self.auto() {
                    notify("Режим: авто", "Агент в фокусе читается по ходу работы");
                } else {
                    notify("Режим: вручную", "Чтение по хоткею");
                }
            }
            Msg::Read => self.read(),
            Msg::Hook { kind, payload } => self.on_hook(&kind, &payload),
        }
    }

    fn read(&mut self) {
        let busy = self.shared.busy.load(Ordering::SeqCst) || !self.shared.queue.lock().unwrap().is_empty();
        if busy {
            self.stop();
            notify("Чтение остановлено", "");
            return;
        }
        let Some(s) = self.focused() else {
            notify("В фокусе нет агента", "");
            return;
        };
        let content = std::fs::read_to_string(&s.transcript).unwrap_or_default();
        let texts = match s.agent {
            Agent::Claude => claude::last_turn(&content),
            Agent::Codex => codex::last_turn(&content),
        };
        let joined = texts.join("\n\n");
        if self.enqueue(s.agent.clone(), &s.id, &joined, Kind::Manual) == 0 {
            notify("Нечего читать", "");
        } else {
            notify("Читаю…", &joined.chars().take(120).collect::<String>());
        }
    }

    fn on_hook(&mut self, kind: &str, p: &serde_json::Value) {
        let session = p["session_id"].as_str().unwrap_or_default().to_string();
        match kind {
            "user-prompt-submit" => {
                self.shared.queue.lock().unwrap().clear_session(&session);
                self.dedup.forget(&session);
                if self.focused().is_some_and(|f| f.id == session) {
                    self.shared.interrupt();
                }
            }
            "notification" => {
                let msg = p["message"].as_str().unwrap_or("Агент ждёт ответа");
                let prefix = if self.focused().is_some_and(|f| f.id == session) {
                    String::new()
                } else {
                    let cwd = p["cwd"].as_str().unwrap_or_default();
                    format!("{}: ", Path::new(cwd).file_name().and_then(|n| n.to_str()).unwrap_or("Агент"))
                };
                self.enqueue(Agent::Claude, &session, &format!("{prefix}{msg}"), Kind::Urgent);
            }
            "message-display" => {
                let Some(ev) = MdEvent::from_json(p) else {
                    eprintln!("agent-speak: непонятный MessageDisplay: {p}");
                    return;
                };
                self.dedup.mark_display(&ev.session, &ev.message_id);
                if !self.live() || !self.focused().is_some_and(|f| f.id == ev.session) {
                    self.asm.push(&ev); // держим буфер, чтобы не потерять при смене фокуса
                    return;
                }
                for s in self.asm.push(&ev) {
                    if self.dedup.first_time(&ev.session, &s) {
                        self.enqueue(Agent::Claude, &ev.session, &s, Kind::Status);
                    }
                }
            }
            "stop" => {
                let rest = self.asm.flush_session(&session);
                let Some(f) = self.focused().filter(|f| self.auto() && f.id == session) else { return };
                if self.cfg.read_intermediate {
                    for s in rest {
                        if self.dedup.first_time(&session, &s) {
                            self.enqueue(Agent::Claude, &session, &s, Kind::Status);
                        }
                    }
                } else if let Some(last) = claude::last_turn(&std::fs::read_to_string(&f.transcript).unwrap_or_default()).pop() {
                    self.enqueue(Agent::Claude, &session, &last, Kind::Manual); // только финальный ответ
                }
            }
            "codex-notify" => {
                // в живом режиме весь текст уже пришёл из rollout; без промежуточных — читаем финальный
                let thread = p["thread-id"].as_str().unwrap_or_default().to_string();
                if self.auto() && !self.cfg.read_intermediate && self.focused().is_some_and(|f| f.id == thread) {
                    if let Some(last) = p["last-assistant-message"].as_str() {
                        self.enqueue(Agent::Codex, &thread, last, Kind::Manual);
                    }
                }
            }
            other => eprintln!("agent-speak: неизвестный хук {other}"),
        }
    }

    fn on_file(&mut self, path: &Path) {
        let lines = self.tail.read_new(path);
        if lines.is_empty() || !self.live() {
            return;
        }
        let Some((agent, id)) = focus::session_of(path) else { return };
        if !self.focused().is_some_and(|f| f.id == id) {
            return;
        }
        for line in lines {
            let blocks = match agent {
                Agent::Claude => claude::parse_line(&line),
                Agent::Codex => codex::parse_line(&line),
            };
            for b in blocks {
                if self.dedup.from_transcript_ok(&id, &b.message_id) && self.dedup.first_time(&id, &b.text) {
                    self.enqueue(agent.clone(), &id, &b.text, Kind::Status);
                }
            }
        }
    }
}
```

Примечание: сабагенты Codex сюда не попадают сами — `focused()` указывает на rollout главного процесса `codex` в окне; файлы сабагентов имеют другой UUID и отсекаются проверкой `f.id == id`. Отдельный `is_subagent` используется в Task 13 только при отладке и может быть удалён, если не понадобится.

- [ ] **Step 7: `main.rs` целиком**

```rust
mod assemble;
mod audio;
mod config;
mod daemon;
mod dedup;
mod focus;
mod ipc;
mod learn;
mod notice;
mod queue;
mod source;
mod speaker;
mod text;
mod tts;

use ipc::Msg;
use std::io::Read;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let msg = match args.first().map(String::as_str) {
        Some("daemon") => return daemon::run(),
        Some("read") => Msg::Read,
        Some("stop") => Msg::Stop,
        Some("pause") => Msg::Pause,
        Some("mode") => Msg::Mode,
        Some("hook") => {
            // вложенные вызовы моделей словаря не должны озвучиваться
            if std::env::var_os("AGENT_SPEAK").is_some() {
                return;
            }
            let kind = args.get(1).cloned().unwrap_or_default();
            let raw = match args.get(2) {
                Some(j) => j.clone(),
                None => {
                    let mut s = String::new();
                    let _ = std::io::stdin().read_to_string(&mut s);
                    s
                }
            };
            Msg::Hook { kind, payload: serde_json::from_str(&raw).unwrap_or_default() }
        }
        _ => {
            eprintln!("agent-speak daemon | read | stop | pause | mode | hook <kind> [json]");
            std::process::exit(2);
        }
    };
    let read = matches!(msg, Msg::Read);
    if !ipc::send(&msg) && read {
        notice::notify("Сервис озвучки не запущен", "systemctl --user start agent-speakd");
    }
}
```

- [ ] **Step 8: Сборка, все тесты**

Run: `cargo build --release && cargo test`
Expected: сборка без ошибок и предупреждений (кроме `is_subagent` unused — пометить `#[allow(dead_code)]` с комментарием «для отладки сабагентов Codex» или удалить вместе с тестом), тесты ok.

- [ ] **Step 9: Дымовая проверка вручную**

```bash
cp python/silero_tts.py ~/.local/share/agent-speak/silero_tts.py
./target/release/agent-speak daemon & sleep 1
./target/release/agent-speak hook notification '{"session_id":"x","cwd":"/home/u/Demo","message":"Нужно разрешение на запуск команды"}'
sleep 8   # первый запуск грузит модель
./target/release/agent-speak stop; kill %1
```
Expected: прозвучало «Demo: нужно разрешение на запуск команды» (срочная фраза из чужой сессии с префиксом проекта).

- [ ] **Step 10: Commit** — `git add -A && git commit -m "feat: daemon event loop, IPC, commands, config"`

---

### Task 13: Установка, systemd, подключение хуков и хоткеев, замена старого скрипта

**Files:**
- Create: `install.sh`, `systemd/agent-speakd.service`, `README.md`
- Modify: `~/.claude/settings.json`, `~/.codex/config.toml` (не в репозиториях), `~/omarchy-dotfiles/home/.config/hypr/hyprland.lua`, `~/omarchy-dotfiles/README.md`, `~/omarchy-dotfiles/docs/changelog.md`
- Delete: `~/omarchy-dotfiles/home/.local/bin/agent-speak`, `~/omarchy-dotfiles/home/.local/bin/agent-speak-tts`

**Interfaces:**
- Consumes: бинарник и подкоманды Task 12.

- [ ] **Step 1: `systemd/agent-speakd.service`**

```ini
[Unit]
Description=agent-speak: озвучка ИИ-агентов
After=pipewire.service

[Service]
ExecStart=%h/.local/bin/agent-speak daemon
Restart=on-failure
RestartSec=2
Environment=PATH=%h/.local/share/mise/shims:%h/.local/bin:/usr/local/bin:/usr/bin

[Install]
WantedBy=graphical-session.target
```

- [ ] **Step 2: `install.sh`**

```bash
#!/bin/bash
# Сборка и установка agent-speak: бинарник, сервер синтеза, стартовый словарь, служба systemd.
set -euo pipefail
cd "$(dirname "$0")"
DATA=~/.local/share/agent-speak

cargo build --release
mkdir -p ~/.local/bin "$DATA" ~/.config/systemd/user
ln -sfn "$PWD/target/release/agent-speak" ~/.local/bin/agent-speak
ln -sfn "$PWD/python/silero_tts.py" "$DATA/silero_tts.py"
[[ -f $DATA/terms.tsv ]] || cp data/terms.tsv "$DATA/terms.tsv"
ln -sfn "$PWD/systemd/agent-speakd.service" ~/.config/systemd/user/agent-speakd.service

# Silero: venv и модель, если ещё нет
if [[ ! -x $DATA/venv/bin/python ]]; then
  python3 -m venv "$DATA/venv"
  "$DATA/venv/bin/pip" install -q torch numpy scipy --index-url https://download.pytorch.org/whl/cpu
fi
[[ -f $DATA/v5_ru.pt ]] || curl -sL -o "$DATA/v5_ru.pt" https://models.silero.ai/models/tts/ru/v5_ru.pt

systemctl --user daemon-reload
systemctl --user enable --now agent-speakd.service
systemctl --user restart agent-speakd.service
```

- [ ] **Step 3: Снять старый скрипт и установить**

```bash
cd ~/omarchy-dotfiles && git rm -q home/.local/bin/agent-speak home/.local/bin/agent-speak-tts
rm -f ~/.local/bin/agent-speak-tts
cd ~/Projects/agent-speak && chmod +x install.sh && ./install.sh
systemctl --user status agent-speakd --no-pager | head -5
readlink ~/.local/bin/agent-speak
```
Expected: служба `active (running)`; ссылка указывает в `~/Projects/agent-speak/target/release/agent-speak`.

Проверить, что служба видит Hyprland: `systemctl --user show-environment | grep HYPRLAND_INSTANCE_SIGNATURE`. Если пусто — добавить в unit `PassEnvironment=HYPRLAND_INSTANCE_SIGNATURE WAYLAND_DISPLAY` не поможет (служба берёт окружение менеджера); тогда выполнить `systemctl --user import-environment HYPRLAND_INSTANCE_SIGNATURE WAYLAND_DISPLAY` и проверить, делает ли это uwsm при входе (`uwsm` обычно экспортирует) — после перезагрузки повторить проверку.

- [ ] **Step 4: Хуки Claude Code**

```bash
f=~/.claude/settings.json; cp $f $f.bak.agent-speak-rs
jq --arg b "$HOME/.local/bin/agent-speak" '
  .hooks.Stop = [{"hooks":[{"type":"command","command":"\($b) hook stop"}]}]
  | .hooks.UserPromptSubmit = [{"hooks":[{"type":"command","command":"\($b) hook user-prompt-submit"}]}]
  | .hooks.Notification = [{"hooks":[{"type":"command","command":"\($b) hook notification"}]}]
  | .hooks.MessageDisplay = [{"hooks":[{"type":"command","command":"\($b) hook message-display"}]}]
' $f > /tmp/s.json && mv /tmp/s.json $f && chmod 600 $f
jq .hooks $f
```
Перед заменой посмотреть `jq .hooks $f`: если там есть чужие (не agent-speak) хуки в этих событиях — сохранить их, добавив наш элемент через `+=` вместо `=`.

- [ ] **Step 5: Codex notify**

```bash
sed -i 's|^notify = .*|notify = ["'"$HOME"'/.local/bin/agent-speak", "hook", "codex-notify"]|' ~/.codex/config.toml
head -1 ~/.codex/config.toml
```

- [ ] **Step 6: Хоткеи**

Проверить, что Super+Alt+P свободна:
```bash
hyprctl binds -j | jq -r '.[]|select((.key=="P" or .key=="p") and .modmask==72)|.description'
```
Пусто — свободна; иначе взять Super+Alt+Shift+P и поправить в README и спецификации.

В `~/omarchy-dotfiles/home/.config/hypr/hyprland.lua` после строки `o.bind("SUPER + ALT + SHIFT + R", ...)` добавить:
```lua
o.bind("SUPER + ALT + P", "Pause agent reading", "agent-speak pause")
```
и обновить комментарий над блоком:
```lua
-- AI agent reading aloud (agent-speak, ~/Projects/agent-speak): read focused agent's last turn (again = stop),
-- toggle live auto-reading, pause/resume.
```
Run: `hyprctl reload && hyprctl configerrors` → пусто.

- [ ] **Step 7: Проверка вживую (авто)**

1. `agent-speak mode` → уведомление «Режим: авто».
2. В окне Claude Code (в фокусе) отправить: «Напиши "Начинаю первый шаг", выполни sleep 3, напиши "Первый готов, делаю второй", выполни sleep 3, напиши "Всё готово"».
3. Ожидается: каждая фраза звучит примерно через секунду после появления, по порядку, без повторов.
4. Во время чтения — Super+Alt+P: звук обрывается; через 5 с ещё раз — фраза повторяется с начала.
5. Отправить новое сообщение во время чтения — чтение обрывается.
6. Переключиться на другое окно — новые фразы первого окна не читаются.
7. То же в окне Codex (п. 2–3).
8. `journalctl --user -u agent-speakd -n 50` — нет повторяющихся ошибок; если есть «непонятный MessageDisplay» — сверить поля с Task 1.

Результаты каждого пункта записать в коммит/журнал: что проверено, что нет.

- [ ] **Step 8: Проверка вживую (вручную)**

`printf 'read_intermediate = false\nmode = "auto"\n' > ~/.config/agent-speak/config.toml` — без перезапуска: в окне агента промежуточные статусы молчат, в конце хода звучит только финальный ответ. Вернуть `read_intermediate = true`.

`agent-speak mode` → «Режим: вручную». В окне агента с готовым ответом — Super+Alt+R: «Читаю…», звучит весь последний ход с промежуточными текстами; повторное нажатие — «Чтение остановлено». В окне без агента — «В фокусе нет агента». Через минуту `cat ~/.local/share/agent-speak/terms.tsv | tail` — появились слова, выученные моделью.

- [ ] **Step 9: README репозитория**

`README.md`:
```markdown
# agent-speak

Озвучка ИИ-агентов (Claude Code, Codex) по ходу работы: промежуточные статусы звучат сразу, по-русски, голосом Silero.

- Сервис `agent-speakd` (`systemctl --user status agent-speakd`), журнал — `journalctl --user -u agent-speakd`.
- Команды: `agent-speak read | stop | pause | mode`; хоткеи — в `omarchy-dotfiles` (`hyprland.lua`).
- Хуки: `~/.claude/settings.json` (Stop, UserPromptSubmit, Notification, MessageDisplay), `notify` в `~/.codex/config.toml`.
- Настройки: `~/.config/agent-speak/config.toml` (`mode`, `speaker`, `rate`, `max_age_secs`, `read_intermediate`); сервис перечитывает файл сам.
- Словарь произношений: `~/.local/share/agent-speak/terms.tsv` (`слово<TAB>произношение`, `+` — ударение); незнакомые слова дописывает фоновая модель.
- Установка: `./install.sh`. Тесты: `cargo test`, с моделью — `cargo test -- --ignored` и `python/test_silero_tts.py`.
- Проект: `docs/superpowers/specs/2026-10-02-agent-speak-design.md`.
```

- [ ] **Step 10: Dotfiles — README и журнал, коммиты**

В `~/omarchy-dotfiles/README.md` заменить строку таблицы про `.local/bin/agent-speak`, `agent-speak-tts` на:
```
| (внешний) `~/Projects/agent-speak` | Озвучка ИИ-агентов по ходу работы — свой сервис на Rust + Silero (`agent-speakd`, ставится его `install.sh`). Здесь только хоткеи в `hyprland.lua`: `Super+Alt+R` — прочитать ход окна в фокусе (повтор — стоп), `Super+Alt+Shift+R` — авто/вручную, `Super+Alt+P` — пауза, `Home` (диктовка) останавливает чтение |
```
В `docs/changelog.md` под `## 2026-10-02` первой записью — что заменено, что проверено вживую (по результатам Step 7–8), что нет.

```bash
cd ~/Projects/agent-speak && git add -A && git commit -m "feat: install script, systemd unit, README"
cd ~/omarchy-dotfiles && git add -A && git commit -m "agent-speak: replace bash script with Rust service, pause hotkey

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>" && git push
```
Публикацию `~/Projects/agent-speak` на GitHub (`gh repo create predmaxim/agent-speak`) — только после явного согласия пользователя.

---

## Факты из Task 1

(заполняется при выполнении Task 1: реальные поля события `MessageDisplay`)

Реальные поля события `MessageDisplay` (совпадают с планом: `message_id` / `delta` / `final`, замена в Task 9 не нужна):
- идентификатор сообщения: `message_id`
- кусок текста: `delta`
- конец сообщения: `final` (bool)
- сессия: `session_id`
- прочее: `index`, `turn_id`, `prompt_id`, `cwd`, `hook_event_name`, `transcript_path`
- headless (`claude -p`) хук вызывает; каждое сообщение пришло одним событием (index 0, final true).

```json
{"session_id":"00000000-0000-0000-0000-000000000003","cwd":"/tmp","prompt_id":"00000000-0000-0000-0000-000000000004","hook_event_name":"MessageDisplay","turn_id":"00000000-0000-0000-0000-000000000005","message_id":"00000000-0000-0000-0000-000000000006","index":0,"final":true,"delta":"Начинаю"}
```
