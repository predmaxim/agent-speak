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
mod orders;
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
        // Ома передаёт агентам ответы и задания: исход её мозгу нужен сразу — без демона
        Some("tell") if args.len() == 4 => return done(args[2].parse().map_err(|_| "option — число".to_string()).and_then(|o| orders::tell(&args[1], o, &args[3]))),
        Some("launch") if args.len() == 3 => return done(orders::launch(&args[1], &args[2])),
        Some("projects") => return println!("{}", orders::projects().join("\n")),
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
            eprintln!("agent-speak on | off | daemon | read | stop | pause | voice | hook <kind> [json] | statusline [команда …] | tell <session> <option> <text> | launch <path> <text> | projects");
            std::process::exit(2);
        }
    };
    let read = matches!(msg, Msg::Read);
    if !ipc::send(&msg) && read {
        eprintln!("agent-speak: сервис озвучки не запущен (systemctl --user start agent-speakd)");
    }
}

/// Исход команды Омы: ошибка — одна строка в stderr (её дословно получает мозг), код 1.
fn done(r: Result<(), String>) {
    if let Err(e) = r {
        eprintln!("{e}");
        std::process::exit(1);
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

/// Строка статуса Claude Code: проценты лимитов — демону, ввод и вывод — следующей команде (chain), её код выхода.
/// Без chain следующая — statusLine пользовательских настроек: обёртка стоит в managed-настройках Claude Code,
/// пользовательскую строку статуса (у автора — Orca, переписывает её при каждом запуске) не трогает.
fn statusline(chain: &[String]) {
    let mut raw = String::new();
    let _ = std::io::stdin().read_to_string(&mut raw);
    if std::env::var_os("AGENT_SPEAK").is_none() {
        ipc::send(&Msg::Hook { kind: "statusline".into(), payload: serde_json::from_str(&raw).unwrap_or_default() });
    }
    let dir = std::env::var_os("CLAUDE_CONFIG_DIR").map_or_else(|| std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".claude"), Into::into);
    let user = chain.is_empty().then(|| user_statusline(&dir.join("settings.json"))).flatten();
    let mut cmd = match (chain.split_first(), user) {
        (Some((c, args)), _) => {
            let mut x = std::process::Command::new(c);
            x.args(args);
            x
        }
        (None, Some(u)) => {
            let mut x = std::process::Command::new("sh");
            x.args(["-c", &u]);
            x
        }
        (None, None) => return,
    };
    let child = cmd.stdin(std::process::Stdio::piped()).spawn();
    let code = match child {
        Ok(mut c) => {
            if let Some(mut w) = c.stdin.take() {
                use std::io::Write;
                let _ = w.write_all(raw.as_bytes());
            }
            c.wait().ok().and_then(|s| s.code()).unwrap_or(1)
        }
        Err(e) => {
            eprintln!("agent-speak statusline: {:?}: {e}", cmd.get_program());
            1
        }
    };
    std::process::exit(code);
}

/// Команда statusLine из пользовательских настроек Claude Code; своя обёртка — нет: звала бы сама себя.
fn user_statusline(settings: &std::path::Path) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(settings).ok()?).ok()?;
    v["statusLine"]["command"].as_str().filter(|c| !c.contains("agent-speak statusline")).map(String::from)
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

    #[test]
    fn next_statusline_is_the_users_but_never_itself() {
        let p = std::env::temp_dir().join(format!("oma-settings-{}.json", std::process::id()));
        let with = |cmd: &str| {
            std::fs::write(&p, serde_json::json!({"statusLine": {"type": "command", "command": cmd}}).to_string()).unwrap();
            user_statusline(&p)
        };
        assert_eq!(with("/bin/sh ~/.orca/agent-hooks/claude-statusline.sh").as_deref(), Some("/bin/sh ~/.orca/agent-hooks/claude-statusline.sh"));
        assert_eq!(with("/home/u/.local/bin/agent-speak statusline /bin/sh x.sh"), None, "своя обёртка — цикл");
        std::fs::write(&p, "{}").unwrap();
        assert_eq!(user_statusline(&p), None);
        let _ = std::fs::remove_file(&p);
        assert_eq!(user_statusline(&p), None);
    }
}
