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

/// Последний вызов инструмента — то, чего агент ждёт, когда просит разрешения.
pub fn last_tool(content: &str) -> Option<String> {
    content.lines().rev().find_map(|line| {
        let e = serde_json::from_str::<Value>(line).ok().filter(|e| e["type"] == "assistant")?;
        let calls = e["message"]["content"].as_array()?.iter().filter(|c| c["type"] == "tool_use");
        calls.last()?["name"].as_str().map(str::to_string)
    })
}

/// Самый поздний вызов инструмента целиком.
fn last_call(content: &str) -> Option<Value> {
    content.lines().rev().find_map(|line| {
        let e = serde_json::from_str::<Value>(line).ok().filter(|e| e["type"] == "assistant")?;
        e["message"]["content"].as_array()?.iter().filter(|c| c["type"] == "tool_use").last().cloned()
    })
}

/// Вопрос пользователю, если самый поздний вызов инструмента — AskUserQuestion.
pub fn last_question(content: &str) -> Option<String> {
    let call = last_call(content)?;
    (call["name"] == "AskUserQuestion").then(|| call["input"]["questions"][0]["question"].as_str().map(str::to_string))?
}

/// Символов ввода инструмента в тексте вопроса: хвост пересказа (digest::MAX_INPUT) не съест текст агента.
const TOOL_MAX: usize = 3000;

/// Текст для пересказа вопроса: последний текст агента в ходе и ввод его последнего вызова —
/// вопрос с вариантами, план, команда (у прочих инструментов — ввод как есть).
pub fn question_context(content: &str) -> String {
    let join = |parts: &[&str], sep: &str| parts.iter().filter(|p| !p.is_empty()).copied().collect::<Vec<_>>().join(sep);
    let tool = last_call(content).map(|c| {
        let (name, i) = (c["name"].as_str().unwrap_or_default(), &c["input"]);
        let s = |k: &str| i[k].as_str().unwrap_or_default().to_string();
        let body = match name {
            "AskUserQuestion" => i["questions"].as_array().into_iter().flatten().map(|q| {
                let opts: Vec<String> = q["options"].as_array().into_iter().flatten()
                    .map(|o| join(&[o["label"].as_str().unwrap_or_default(), o["description"].as_str().unwrap_or_default()], " — ")).collect();
                join(&[q["question"].as_str().unwrap_or_default(), &format!("Варианты: {}", opts.join("; "))], " ")
            }).collect::<Vec<_>>().join("\n"),
            "ExitPlanMode" => s("plan"),
            "Bash" => join(&[&s("description"), &s("command")], ": "),
            _ => i.to_string(),
        };
        format!("Ждёт ответа на {name}: {body}").chars().take(TOOL_MAX).collect::<String>()
    });
    join(&[&last_turn(content).pop().unwrap_or_default(), &tool.unwrap_or_default()], "\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_tool_is_latest_call() {
        let t = |n: &str| format!(r#"{{"type":"assistant","message":{{"content":[{{"type":"tool_use","name":"{n}"}}]}}}}"#);
        let text = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"x"}]}}"#;
        let log = [t("Bash"), t("AskUserQuestion"), text.into(), r#"{"type":"user"}"#.into()].join("\n");
        assert_eq!(last_tool(&log).as_deref(), Some("AskUserQuestion"));
        assert_eq!(last_tool("обрывок строки"), None);
    }
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

    #[test]
    fn last_question_only_if_latest_call_is_ask() {
        let ask = |q: &str| format!(r#"{{"type":"assistant","message":{{"content":[{{"type":"tool_use","name":"AskUserQuestion","input":{{"questions":[{{"question":"{q}"}}]}}}}]}}}}"#);
        let bash = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{}}]}}"#;
        assert_eq!(last_question(&[ask("Старый?"), ask("Какой вариант окна?"), r#"{"type":"user"}"#.into()].join("\n")).as_deref(), Some("Какой вариант окна?"));
        assert_eq!(last_question(&[ask("Старый?"), bash.into()].join("\n")), None);
    }

    #[test]
    fn question_context_has_agent_text_and_tool_input() {
        let call = |name: &str, input: serde_json::Value| serde_json::json!({"type":"assistant","message":{"content":[{"type":"tool_use","name":name,"input":input}]}}).to_string();
        let ask = call("AskUserQuestion", serde_json::json!({"questions":[{"question":"Что делаем с мёртвым кодом?","options":[
            {"label":"Удалить","description":"Рекомендую: он нигде не вызывается"},{"label":"Оставить"}]}]}));
        let c = question_context(&[user("почисти"), said("Нашёл мёртвый код."), ask].join("\n"));
        assert_eq!(c, "Нашёл мёртвый код.\n\nЖдёт ответа на AskUserQuestion: Что делаем с мёртвым кодом? Варианты: Удалить — Рекомендую: он нигде не вызывается; Оставить");
        let plan = call("ExitPlanMode", serde_json::json!({"plan":"1. Вынести орб. 2. Тесты."}));
        assert!(question_context(&plan).ends_with("ExitPlanMode: 1. Вынести орб. 2. Тесты."));
        let bash = call("Bash", serde_json::json!({"command":"rm -rf build","description":"Чищу сборку"}));
        assert!(question_context(&bash).ends_with("Bash: Чищу сборку: rm -rf build"));
        let big = call("Write", serde_json::json!({"content":"я".repeat(TOOL_MAX * 2)}));
        assert_eq!(question_context(&big).chars().count(), TOOL_MAX);
        assert_eq!(question_context(""), "");
    }
}
