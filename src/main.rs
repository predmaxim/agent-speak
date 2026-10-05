mod assemble;
mod audio;
mod config;
mod daemon;
mod dedup;
mod focus;
mod ipc;
mod learn;
mod notice;
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
        Some("read") => Msg::Read,
        Some("stop") => Msg::Stop,
        Some("pause") => Msg::Pause,
        Some("mode") => Msg::Mode,
        Some("voice") => Msg::VoiceToggle,
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
            eprintln!("agent-speak daemon | read | stop | pause | mode | voice | hook <kind> [json]");
            std::process::exit(2);
        }
    };
    let read = matches!(msg, Msg::Read);
    if !ipc::send(&msg) && read {
        eprintln!("agent-speak: сервис озвучки не запущен (systemctl --user start agent-speakd)");
    }
}
