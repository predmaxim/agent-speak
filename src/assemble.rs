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
