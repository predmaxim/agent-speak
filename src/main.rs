mod assemble;
mod agents;
mod audio;
mod config;
mod daemon;
mod digest;
mod focus;
mod ipc;
mod learn;
mod limits;
mod news;
mod mux;
mod notice;
mod presence;
mod queue;
mod source;
mod speaker;
mod text;
mod tts;
mod watch;

use ipc::Msg;
use std::io::Read;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let msg = match args.first().map(String::as_str) {
        Some("daemon") => return daemon::run(),
        Some(cmd @ ("on" | "off")) => return power(cmd == "on"),
        Some("read") => Msg::Read,
        Some("stop") => Msg::Stop,
        Some("pause") => Msg::Pause,
        Some("voice") => Msg::Oma { action: serde_json::json!({"do": "talk"}) },
        Some("statusline") => return statusline(&args[1..]),
        Some("hook") => {
            // вложенные вызовы моделей словаря не должны озвучиваться
            if std::env::var_os("AGENT_SPEAK").is_some() {
                return;
            }
            let kind = args.get(1).cloned().unwrap_or_default();
            if kind == "post-tool-use" {
                // в демон не идёт: срабатывает на каждый вызов инструмента, ответ нужен сразу
                let mut raw = String::new();
                let _ = std::io::stdin().read_to_string(&mut raw);
                if let Some(out) = post_tool_use(&raw, &limits::hints_dir()) {
                    println!("{out}");
                }
                return;
            }
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
            eprintln!("agent-speak on | off | daemon | read | stop | pause | voice | hook <kind> [json] | statusline [команда …]");
            std::process::exit(2);
        }
    };
    let read = matches!(msg, Msg::Read);
    if !ipc::send(&msg) && read {
        eprintln!("agent-speak: сервис озвучки не запущен (systemctl --user start agent-speakd)");
    }
}

/// Ома целиком: служба с её слухом и моделями; выключенная не стартует и после входа в систему.
fn power(on: bool) {
    let st = std::process::Command::new("systemctl")
        .args(["--user", if on { "enable" } else { "disable" }, "--now", "agent-speakd.service"])
        .status();
    std::process::exit(st.map_or(1, |s| s.code().unwrap_or(1)));
}

/// Ответ хука PostToolUse: подсказка про лимит агенту, один раз на агента; нечего сказать — None.
fn post_tool_use(raw: &str, dir: &std::path::Path) -> Option<String> {
    let p: serde_json::Value = serde_json::from_str(raw).ok()?;
    let agent = p["agent_id"].as_str().filter(|a| !a.is_empty()).unwrap_or("main");
    let text = limits::take_hint(dir, p["session_id"].as_str()?, agent)?;
    let event = p["hook_event_name"].as_str().unwrap_or("PostToolUse");
    Some(serde_json::json!({"hookSpecificOutput": {"hookEventName": event, "additionalContext": text}}).to_string())
}

/// Строка статуса Claude Code: проценты лимитов — демону, ввод и вывод — прежней команде строки (chain), её код выхода.
fn statusline(chain: &[String]) {
    let mut raw = String::new();
    let _ = std::io::stdin().read_to_string(&mut raw);
    if std::env::var_os("AGENT_SPEAK").is_none() {
        ipc::send(&Msg::Hook { kind: "statusline".into(), payload: serde_json::from_str(&raw).unwrap_or_default() });
    }
    let Some((cmd, args)) = chain.split_first() else { return };
    let child = std::process::Command::new(cmd).args(args).stdin(std::process::Stdio::piped()).spawn();
    let code = match child {
        Ok(mut c) => {
            if let Some(mut w) = c.stdin.take() {
                use std::io::Write;
                let _ = w.write_all(raw.as_bytes());
            }
            c.wait().ok().and_then(|s| s.code()).unwrap_or(1)
        }
        Err(e) => {
            eprintln!("agent-speak statusline: {cmd}: {e}");
            1
        }
    };
    std::process::exit(code);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn post_tool_use_gives_hint_once_in_hook_format() {
        let dir = std::env::temp_dir().join(format!("oma-ptu-{}", std::process::id()));
        assert_eq!(post_tool_use(r#"{"session_id":"s1"}"#, &dir), None);
        limits::put_hint(&dir, "s1", "лимит 96%");
        let out: serde_json::Value = serde_json::from_str(&post_tool_use(r#"{"session_id":"s1","hook_event_name":"PostToolUse"}"#, &dir).unwrap()).unwrap();
        assert_eq!(out["hookSpecificOutput"]["hookEventName"], "PostToolUse");
        assert_eq!(out["hookSpecificOutput"]["additionalContext"], "лимит 96%");
        assert_eq!(post_tool_use(r#"{"session_id":"s1"}"#, &dir), None);
        assert!(post_tool_use(r#"{"session_id":"s1","agent_id":"a1"}"#, &dir).is_some(), "сабагент — свой раз");
        assert_eq!(post_tool_use("мусор", &dir), None);
        let _ = std::fs::remove_dir_all(dir);
    }
}
