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
    // только base64-символы: срез по байтам безопасен и для не-ASCII подписи
    let head: String = signature.chars().take(160).filter(|c| c.is_ascii_alphanumeric() || "+/=".contains(*c)).collect();
    let head = &head[..head.len() / 4 * 4];
    base64::engine::general_purpose::STANDARD
        .decode(head)
        .map(|b| b.windows(9).any(|w| w == b"narration"))
        .unwrap_or(false)
}

/// Граница хода: сообщение человека или реплика, набранная пока агент работал (queued_command).
fn is_turn_start(e: &Value) -> bool {
    if e["type"] == "attachment" {
        return e["attachment"]["type"] == "queued_command";
    }
    if e["type"] != "user" || e["isMeta"] == true {
        return false;
    }
    match &e["message"]["content"] {
        Value::String(t) => !super::is_injected(t),
        Value::Array(a) if !a.iter().any(|c| c["type"] == "tool_result") => {
            let t: String = a.iter().filter_map(|c| c["text"].as_str()).collect();
            !super::is_injected(&t)
        }
        _ => false,
    }
}

pub fn last_turn(content: &str) -> Vec<String> {
    let (mut cur, mut prev): (Vec<String>, Vec<String>) = (vec![], vec![]);
    for line in content.lines() {
        let Ok(e) = serde_json::from_str::<Value>(line) else { continue };
        if is_turn_start(&e) {
            if !cur.is_empty() {
                prev = std::mem::take(&mut cur);
            }
        } else {
            cur.extend(parse_line(line).into_iter().map(|b| b.text));
        }
    }
    if cur.is_empty() { prev } else { cur }
}

#[cfg(test)]
mod tests {
    use super::*;
    const FX: &str = include_str!("../../tests/fixtures/claude.jsonl");

    #[test]
    fn non_ascii_signature_no_panic() {
        assert!(!is_narration("ЖЖЖЖЖЖЖЖ ёёё"));
        assert!(!is_narration("abc€defghij"));
    }

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

    fn user(content: &str) -> String {
        serde_json::json!({"type":"user","message":{"content":content}}).to_string()
    }
    fn said(text: &str) -> String {
        serde_json::json!({"type":"assistant","message":{"id":"m","content":[{"type":"text","text":text}]}}).to_string()
    }
    fn turn(lines: &[String]) -> Vec<String> {
        last_turn(&lines.join("\n"))
    }

    #[test]
    fn injected_user_entries_do_not_split_turn() {
        for inj in ["<task-notification>x</task-notification>", "<system-reminder>x", "<local-command-stdout>", "<command-name>/x", "Caveat: foo"] {
            let t = turn(&[user("вопрос"), said("раз"), user(inj), said("два")]);
            assert_eq!(t, ["раз", "два"], "{inj}");
        }
        let blocks = serde_json::json!({"type":"user","message":{"content":[{"type":"text","text":" <task-notification>x"}]}}).to_string();
        assert_eq!(turn(&[user("в"), said("раз"), blocks, said("два")]), ["раз", "два"]);
    }

    #[test]
    fn queued_command_splits_turn() {
        let q = r#"{"type":"attachment","attachment":{"type":"queued_command","prompt":"эй"}}"#.to_string();
        assert_eq!(turn(&[user("в"), said("раз"), q, said("два")]), ["два"]);
    }

    #[test]
    fn string_prompt_splits_and_meta_does_not() {
        assert_eq!(turn(&[user("в"), said("раз"), user("ещё"), said("два")]), ["два"]);
        let meta = r#"{"type":"user","isMeta":true,"message":{"content":"обычный текст"}}"#.to_string();
        assert_eq!(turn(&[user("в"), said("раз"), meta, said("два")]), ["раз", "два"]);
    }
}
