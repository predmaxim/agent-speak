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

#[allow(dead_code)] // для отладки сабагентов Codex
pub fn is_subagent(first_line: &str) -> bool {
    let Ok(e) = serde_json::from_str::<Value>(first_line) else { return false };
    match e["payload"]["thread_source"].as_str() {
        None | Some("user") | Some("cli") | Some("exec") => false,
        Some(_) => true,
    }
}

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
