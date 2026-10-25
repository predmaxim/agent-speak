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

fn injected(e: &Value) -> bool {
    let t: String = e["payload"]["content"].as_array().into_iter().flatten().filter_map(|c| c["text"].as_str()).collect();
    super::is_injected(&t)
}

pub fn last_turn(content: &str) -> Vec<String> {
    let (mut cur, mut prev): (Vec<String>, Vec<String>) = (vec![], vec![]);
    for line in content.lines() {
        let Ok(e) = serde_json::from_str::<Value>(line) else { continue };
        if e["type"] == "response_item" && e["payload"]["type"] == "message" && e["payload"]["role"] == "user" && !injected(&e) {
            if !cur.is_empty() {
                prev = std::mem::take(&mut cur);
            }
        } else {
            cur.extend(parse_line(line).into_iter().map(|b| b.text));
        }
    }
    if cur.is_empty() { prev } else { cur }
}

/// Ход кончился: последнее из task_started / task_complete / turn_aborted — не начало (прерывание notify не шлёт).
pub fn turn_over(content: &str) -> bool {
    for l in content.lines().rev() {
        let Ok(e) = serde_json::from_str::<Value>(l) else { continue };
        match e["payload"]["type"].as_str().filter(|_| e["type"] == "event_msg") {
            Some("task_complete" | "turn_aborted") => return true,
            Some("task_started") => return false,
            _ => {}
        }
    }
    true
}

/// Вызов инструмента без вывода — как команда Bash с его вводом целиком: сторожу важно, нет ли в нём опасного.
pub fn in_flight(content: &str) -> Option<Value> {
    let mut open: Vec<(Value, String)> = Vec::new();
    for l in content.lines() {
        let Ok(e) = serde_json::from_str::<Value>(l) else { continue };
        let p = &e["payload"];
        match p["type"].as_str().filter(|_| e["type"] == "response_item") {
            Some("custom_tool_call" | "function_call") => {
                let input = p["input"].as_str().or(p["arguments"].as_str()).unwrap_or_default();
                open.push((p["call_id"].clone(), input.to_string()));
            }
            Some("custom_tool_call_output" | "function_call_output") => open.retain(|(id, _)| *id != p["call_id"]),
            _ => {}
        }
    }
    open.pop().map(|(_, cmd)| serde_json::json!({"name": "Bash", "input": {"command": cmd}}))
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

    #[test]
    fn injected_user_messages_do_not_split_turn() {
        let user = |t: &str| serde_json::json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":t}]}}).to_string();
        let said = |t: &str| serde_json::json!({"type":"response_item","payload":{"type":"message","role":"assistant","id":"i","content":[{"type":"output_text","text":t}]}}).to_string();
        let lines = [user("вопрос"), said("раз"), user("<environment_context>x</environment_context>"), user("<user_instructions>y"), said("два")];
        assert_eq!(last_turn(&lines.join("\n")), ["раз", "два"]);
        let lines = [user("в"), said("раз"), user("ещё"), said("два")];
        assert_eq!(last_turn(&lines.join("\n")), ["два"]);
    }

    fn ev(t: &str) -> String {
        serde_json::json!({"type": "event_msg", "payload": {"type": t}}).to_string()
    }
    fn tool(kind: &str, id: &str, input: &str) -> String {
        serde_json::json!({"type": "response_item", "payload": {"type": kind, "call_id": id, "name": "exec", "input": input}}).to_string()
    }

    #[test]
    fn codex_turn_over_by_complete_or_abort() {
        assert!(turn_over(&[ev("task_started"), ev("task_complete")].join("\n")));
        assert!(turn_over(&[ev("task_started"), ev("turn_aborted")].join("\n")), "прерван пользователем — notify не приходит");
        assert!(!turn_over(&[ev("task_complete"), ev("task_started"), ev("token_count")].join("\n")));
        assert!(turn_over(""));
    }

    #[test]
    fn codex_in_flight_as_command() {
        let open = tool("custom_tool_call", "c1", "tools.exec_command({\"cmd\":\"git push origin main\"})");
        let got = in_flight(&open).unwrap();
        assert_eq!(got["name"], "Bash");
        assert!(got["input"]["command"].as_str().unwrap().contains("git push"));
        let done = serde_json::json!({"type": "response_item", "payload": {"type": "custom_tool_call_output", "call_id": "c1"}}).to_string();
        assert_eq!(in_flight(&[open, done].join("\n")), None);
        let f = serde_json::json!({"type": "response_item", "payload": {"type": "function_call", "call_id": "f1", "name": "shell", "arguments": "{\"command\":[\"rm\",\"-rf\",\"x\"]}"}}).to_string();
        assert!(in_flight(&f).unwrap()["input"]["command"].as_str().unwrap().contains("rm"));
    }
}
