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

/// Сколько помощников агента запущено и ещё не отчиталось, по хвосту транскрипта; фоновые команды не считаются: сервер не отчитывается никогда.
pub fn pending_background(content: &str) -> usize {
    let mut pending = std::collections::HashSet::new();
    for line in content.lines() {
        if line.contains("<task-notification>") {
            // в одной строке бывает несколько отчётов, в отчёте после --resume — несколько id со статусом stopped
            for part in line.split("<task-notification>").skip(1) {
                if ["completed", "failed", "killed", "stopped"].iter().any(|s| part.contains(&format!("<status>{s}</status>"))) {
                    for id in part.split("<task-id>").skip(1) {
                        pending.remove(&id.chars().take_while(char::is_ascii_alphanumeric).collect::<String>());
                    }
                }
            }
        } else if line.contains("async_launched") || line.contains("resumedAgentId") {
            // запуск — только из настоящего результата инструмента: цитата в тексте агента или выводе Read не считается
            let Ok(e) = serde_json::from_str::<Value>(line) else { continue };
            let r = &e["toolUseResult"];
            let id = if r["status"] == "async_launched" { &r["agentId"] } else { &r["resumedAgentId"] };
            pending.extend(id.as_str().filter(|id| !id.is_empty()).map(str::to_string));
        }
    }
    pending.len()
}

/// Фоновые помощники, которые ещё работают, — по их `.meta.json` в папке `subagents` сессии (нет `finished`).
/// Хвост транскрипта их не видит, если запуск был давно, а помощник-сервер работает часами.
pub fn running_helpers(subagents: &std::path::Path) -> usize {
    std::fs::read_dir(subagents)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|f| f.file_name().to_string_lossy().ends_with(".meta.json"))
        .filter_map(|f| serde_json::from_str::<Value>(&std::fs::read_to_string(f.path()).ok()?).ok())
        .filter(|m| m["requestShape"] == "background" && m["finished"] != true)
        .count()
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

/// Ошибка API в конце хода — по ней сторож решает, что делать.
#[derive(Clone, Debug, PartialEq)]
pub struct ApiError {
    pub kind: String, // поле error: rate_limit | server_error | invalid_request | authentication_failed | …
    pub at: u64,
    pub resets_at: Option<u64>,
}

/// Записи главной ветки: реплики пользователя и агента без сабагентов (isSidechain).
fn main_records(content: &str) -> impl DoubleEndedIterator<Item = Value> + '_ {
    content.lines().filter(|l| l.contains("\"type\":\"user\"") || l.contains("\"type\":\"assistant\"") || l.contains("queued_command")).filter_map(|l| {
        let e: Value = serde_json::from_str(l).ok()?;
        (matches!(e["type"].as_str(), Some("user" | "assistant" | "attachment")) && e["isSidechain"] != true).then_some(e)
    })
}

/// Ход кончился ошибкой API: последняя реплика главной ветки — сообщение об ошибке (лимит, связь, вход…).
pub fn last_api_error(content: &str) -> Option<ApiError> {
    let e = main_records(content).rev().find(|e| e["type"] != "attachment")?;
    if e["isApiErrorMessage"] != true {
        return None;
    }
    let reset = &e["apiErrorParams"]["rate_limit_info"]["resetsAt"];
    let resets_at = reset.as_u64().or_else(|| e["quotaLimits"]["resetsAt"].as_u64());
    let at = e["timestamp"].as_str().and_then(crate::agents::epoch).unwrap_or(0);
    Some(ApiError { kind: e["error"].as_str().unwrap_or("unknown").to_string(), at, resets_at })
}

/// Последний вызов инструмента главной ветки, на который ещё нет результата; граница хода сбрасывает недождавшиеся.
pub fn in_flight(content: &str) -> Option<Value> {
    let mut open: Vec<Value> = Vec::new();
    for e in main_records(content) {
        if is_turn_start(&e) {
            open.clear();
            continue;
        }
        for c in e["message"]["content"].as_array().into_iter().flatten() {
            match c["type"].as_str() {
                Some("tool_use") if e["type"] == "assistant" => open.push(c.clone()),
                Some("tool_result") => open.retain(|u| u["id"] != c["tool_use_id"]),
                _ => {}
            }
        }
    }
    open.pop()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool_result(text: &str) -> String {
        serde_json::json!({"type": "user", "message": {"content": [{"type": "tool_result", "content": [{"type": "text", "text": text}]}]}}).to_string()
    }
    /// Настоящий запуск помощника: результат инструмента с toolUseResult.status = async_launched, как в транскрипте.
    fn launched(id: &str) -> String {
        let mut e: serde_json::Value = serde_json::from_str(&tool_result(&format!("Async agent launched successfully.\nagentId: {id} (internal ID)"))).unwrap();
        e["toolUseResult"] = serde_json::json!({"isAsync": true, "status": "async_launched", "agentId": id});
        e.to_string()
    }
    fn report(id: &str, status: &str) -> String {
        serde_json::json!({"type": "queue-operation", "content": format!("<task-notification>\n<task-id>{id}</task-id>\n<status>{status}</status>\n</task-notification>")}).to_string()
    }

    #[test]
    fn pending_counts_launched_minus_reported() {
        let log = [launched("aaa"), launched("bbb"), launched("ccc"), report("aaa", "completed"), report("aaa", "completed")];
        assert_eq!(pending_background(&log.join("\n")), 2);
        assert_eq!(pending_background(&tool_result("Command running in background with ID: b3jh6h2yt. Output is being written")), 0);
    }

    #[test]
    fn several_reports_in_one_line() {
        let two = serde_json::json!({"type": "queue-operation", "content": "<task-notification>\n<task-id>aaa</task-id>\n<status>completed</status>\n</task-notification>\n<task-notification>\n<task-id>bbb</task-id>\n<status>failed</status>\n</task-notification>"}).to_string();
        assert_eq!(pending_background(&[launched("aaa"), launched("bbb"), two].join("\n")), 0);
    }

    #[test]
    fn stopped_report_with_several_ids_is_terminal() {
        // --resume: один отчёт на всех помощников прошлой сессии
        let stopped = serde_json::json!({"type": "queue-operation", "operation": "enqueue", "content": "<task-notification>\n<task-id>a1d0aca83830798a1</task-id>\n<task-id>a9576ea011ca11eae</task-id>\n<task-id>aeec3f40d3b667aa7</task-id>\n<status>stopped</status>\n<summary>3 background agents didn't finish before the previous session ended</summary>\n</task-notification>"}).to_string();
        let log = [launched("a1d0aca83830798a1"), launched("a9576ea011ca11eae"), launched("aeec3f40d3b667aa7"), stopped];
        assert_eq!(pending_background(&log.join("\n")), 0);
    }

    #[test]
    fn quoted_launch_is_not_counted() {
        let quote = "Async agent launched successfully.\nagentId: abc123 (internal ID)";
        let said = serde_json::json!({"type": "assistant", "message": {"content": [{"type": "text", "text": quote}]}}).to_string();
        let read = tool_result(&format!("{quote}\nresumedAgentId: def456"));
        assert_eq!(pending_background(&[said, read].join("\n")), 0);
    }

    #[test]
    fn resumed_helper_pending_again() {
        let mut resumed: serde_json::Value = serde_json::from_str(&tool_result(r#"{"success":true,"resumedAgentId":"aaa","pin":{"id":"aaa"}}"#)).unwrap();
        resumed["toolUseResult"] = serde_json::json!({"success": true, "resumedAgentId": "aaa", "pin": {"id": "aaa"}});
        let resumed = resumed.to_string();
        assert_eq!(pending_background(&[launched("aaa"), report("aaa", "completed")].join("\n")), 0);
        assert_eq!(pending_background(&[launched("aaa"), report("aaa", "completed"), resumed].join("\n")), 1);
    }

    #[test]
    fn no_background_is_zero() {
        let text = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"x"}]}}"#;
        assert_eq!(pending_background(&[text.to_string(), tool_result("ok"), "обрывок".into()].join("\n")), 0);
    }


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

    fn err_line(kind: &str, side: bool) -> String {
        serde_json::json!({"type":"assistant","isSidechain":side,"isApiErrorMessage":true,"error":kind,"timestamp":"2026-10-24T23:16:42.069Z",
            "apiErrorParams":{"rate_limit_info":{"resetsAt":1791961800u64}},"message":{"content":[{"type":"text","text":"x"}]}}).to_string()
    }
    fn call(id: &str, name: &str) -> String {
        serde_json::json!({"type":"assistant","message":{"content":[{"type":"tool_use","id":id,"name":name,"input":{"command":"git push"}}]}}).to_string()
    }
    fn result_of(id: &str) -> String {
        serde_json::json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":id,"content":"ok"}]}}).to_string()
    }

    #[test]
    fn api_error_only_when_last_main_record() {
        let e = last_api_error(&err_line("rate_limit", false)).unwrap();
        assert_eq!((e.kind.as_str(), e.resets_at, e.at), ("rate_limit", Some(1791961800), 1_792_883_802));
        assert_eq!(last_api_error(&[err_line("server_error", false), call("a", "Bash")].join("\n")), None, "после ошибки работа пошла");
        assert_eq!(last_api_error(&[call("a", "Bash"), err_line("rate_limit", true)].join("\n")), None, "ошибка сабагента — не конец хода");
        let tail = [err_line("server_error", false), r#"{"type":"system","subtype":"x"}"#.to_string()].join("\n");
        assert_eq!(last_api_error(&tail).unwrap().kind, "server_error", "служебные записи после ошибки не в счёт");
        let quota = serde_json::json!({"type":"assistant","isApiErrorMessage":true,"error":"rate_limit","timestamp":"2026-10-24T23:16:42Z","quotaLimits":{"resetsAt":5u64}}).to_string();
        assert_eq!(last_api_error(&quota).unwrap().resets_at, Some(5));
        assert_eq!(last_api_error(""), None);
    }

    #[test]
    fn in_flight_is_unanswered_call() {
        assert_eq!(in_flight(&[call("a", "Bash"), result_of("a")].join("\n")), None);
        assert_eq!(in_flight(&[call("a", "Read"), result_of("a"), call("b", "Bash")].join("\n")).unwrap()["name"], "Bash");
        let side = serde_json::json!({"type":"assistant","isSidechain":true,"message":{"content":[{"type":"tool_use","id":"s","name":"Grep","input":{}}]}}).to_string();
        assert_eq!(in_flight(&[call("a", "Agent"), side].join("\n")).unwrap()["name"], "Agent", "вызовы сабагента — не главной ветки");
        assert_eq!(in_flight(""), None);
    }

    #[test]
    fn running_helpers_from_meta_files() {
        let dir = std::env::temp_dir().join(format!("oma-subagents-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let meta = |name: &str, v: serde_json::Value| std::fs::write(dir.join(format!("{name}.meta.json")), v.to_string()).unwrap();
        meta("agent-a", serde_json::json!({"description": "сервер", "requestShape": "background"}));
        meta("agent-b", serde_json::json!({"description": "готов", "requestShape": "background", "finished": true}));
        meta("agent-c", serde_json::json!({"description": "на переднем плане", "requestShape": "foreground"}));
        std::fs::write(dir.join("agent-a.jsonl"), "{}").unwrap();
        assert_eq!(running_helpers(&dir), 1);
        assert_eq!(running_helpers(&dir.join("нет")), 0);
        let _ = std::fs::remove_dir_all(dir);
    }
}
