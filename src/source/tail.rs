//! Новые полные строки дописываемых файлов; первая встреча — с конца (историю не читаем).

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// Окно хвоста транскрипта: сначала столько, потом вдвое больше — но не больше TAIL_MAX.
const TAIL: u64 = 256 * 1024;
const TAIL_MAX: u64 = 16 << 20;

/// Хвост транскрипта целыми строками: последний вызов инструмента всегда в конце, весь файл (мегабайты) читать незачем.
/// Одна строка (результат инструмента, api-request-blob) бывает больше окна — тогда окно растёт вдвое, пока `enough`
/// не найдёт в нём нужную запись, но не дальше TAIL_MAX.
pub fn tail_of(path: &str, enough: impl Fn(&str) -> bool) -> String {
    let Ok(mut f) = std::fs::File::open(path) else { return String::new() };
    let len = f.metadata().map_or(0, |m| m.len());
    let mut window = TAIL;
    loop {
        let start = len.saturating_sub(window);
        let mut buf = Vec::new();
        if f.seek(SeekFrom::Start(start)).is_err() || f.read_to_end(&mut buf).is_err() {
            return String::new();
        }
        // первая строка окна посреди файла — обрезок
        let from = if start == 0 { 0 } else { buf.iter().position(|&b| b == b'\n').map_or(buf.len(), |i| i + 1) };
        let text = String::from_utf8_lossy(&buf[from..]).into_owned();
        if start == 0 || window >= TAIL_MAX || enough(&text) {
            return text;
        }
        window *= 2;
    }
}

/// В тексте есть целая строка JSON — для файлов без разбора записей (rollout Codex).
pub fn has_line(text: &str) -> bool {
    text.lines().any(|l| serde_json::from_str::<serde_json::Value>(l).is_ok())
}

pub struct Tailer {
    offsets: HashMap<PathBuf, (u64, Vec<u8>)>,
}

impl Tailer {
    pub fn new() -> Tailer {
        Tailer { offsets: HashMap::new() }
    }

    pub fn read_new(&mut self, path: &Path) -> Vec<String> {
        let Ok(mut f) = std::fs::File::open(path) else { return vec![] };
        let len = f.metadata().map(|m| m.len()).unwrap_or(0);
        let Some((off, partial)) = self.offsets.get_mut(path) else {
            self.offsets.insert(path.to_path_buf(), (len, Vec::new()));
            return vec![];
        };
        if len < *off {
            *off = 0; // файл пересоздан
            partial.clear();
        }
        let mut buf = Vec::new();
        if f.seek(SeekFrom::Start(*off)).is_err() || f.read_to_end(&mut buf).is_err() {
            return vec![];
        }
        *off += buf.len() as u64;
        // неполная строка (и обрезанный UTF-8) остаётся в partial до следующего вызова
        partial.extend_from_slice(&buf);
        let mut parts: Vec<Vec<u8>> = partial.split(|&b| b == b'\n').map(<[u8]>::to_vec).collect();
        *partial = parts.pop().unwrap_or_default();
        let mut lines: Vec<String> = parts.iter().map(|l| String::from_utf8_lossy(l).into_owned()).collect();
        lines.retain(|l| !l.trim().is_empty());
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn tail_reads_back_past_a_line_bigger_than_the_window() {
        // результат инструмента или api-request-blob больше 256 КиБ: окно из одного обрезка — сторож слеп
        let p = std::env::temp_dir().join(format!("tail-big-{}", std::process::id()));
        let call = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Bash"}]}}"#;
        let huge = format!("{{\"type\":\"api-request-blob\",\"x\":\"{}\"}}", "я".repeat(300 * 1024));
        let old = "{\"type\":\"user\"}\n".repeat(100_000); // 1,5 МБ истории
        std::fs::write(&p, format!("{old}{call}\n{huge}\n")).unwrap();
        let path = p.to_string_lossy();
        let t = tail_of(&path, |t| t.lines().any(|l| l.contains("\"assistant\"")));
        assert!(t.contains(call), "окно дочитано назад до целой записи");
        assert_eq!(t.lines().next(), Some("{\"type\":\"user\"}"), "обрезок строки в начале окна отброшен");
        assert!(t.len() <= 1 << 20, "дальше нужного не читает: {}", t.len());
        assert_eq!(tail_of(&path, |_| true), "", "окно не растёт без нужды; обрезок строки — не запись");
        assert!(has_line(&t) && !has_line("обрезок\n"));
        std::fs::remove_file(&p).unwrap();
    }

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

    #[test]
    fn tail_survives_invalid_utf8() {
        let p = std::env::temp_dir().join(format!("tail-c-{}", std::process::id()));
        std::fs::write(&p, "").unwrap();
        let mut t = Tailer::new();
        t.read_new(&p);
        std::fs::OpenOptions::new().append(true).open(&p).unwrap().write_all(b"ab\xffcd\nok\n").unwrap();
        let l = t.read_new(&p);
        assert_eq!(l.len(), 2);
        assert_eq!(l[1], "ok");
        std::fs::remove_file(&p).unwrap();
    }

    #[test]
    fn tail_split_multibyte() {
        let p = std::env::temp_dir().join(format!("tail-d-{}", std::process::id()));
        std::fs::write(&p, "").unwrap();
        let mut t = Tailer::new();
        t.read_new(&p);
        let b = "привет\n".as_bytes();
        let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
        f.write_all(&b[..3]).unwrap(); // разрез посреди «р»... байт 3 — середина «и»
        assert!(t.read_new(&p).is_empty());
        f.write_all(&b[3..]).unwrap();
        assert_eq!(t.read_new(&p), vec!["привет"]);
        std::fs::remove_file(&p).unwrap();
    }
}
