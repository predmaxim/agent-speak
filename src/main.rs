mod assemble;
mod agents;
mod audio;
mod config;
mod daemon;
mod digest;
mod focus;
mod ipc;
mod learn;
mod news;
mod notice;
mod presence;
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
        Some(cmd @ ("on" | "off")) => return power(cmd == "on"),
        Some("read") => Msg::Read,
        Some("stop") => Msg::Stop,
        Some("pause") => Msg::Pause,
        Some("voice") => Msg::Oma { action: serde_json::json!({"do": "talk"}) },
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
            eprintln!("agent-speak on | off | daemon | read | stop | pause | voice | hook <kind> [json]");
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
