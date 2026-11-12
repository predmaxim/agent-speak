mod assemble;
mod agents;
mod audio;
mod config;
mod daemon;
mod digest;
mod focus;
mod gitfacts;
mod ipc;
mod later;
mod learn;
mod limits;
mod memory;
mod news;
mod mpris;
mod mux;
mod notice;
mod orders;
mod perm;
mod phrases;
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
        // то же, что страница настроек: служба проверяет значение и сохраняет config.toml
        Some("set") if args.len() == 3 => Msg::Set { key: args[1].clone(), value: set_value(&args[2]) },
        Some("statusline") => return statusline(&args[1..]),
        // Ома передаёт агенту ответ: через очередь действий службы (с действиями сторожа не вперемешку), исход — сразу;
        // stdout — метка нажатого пункта меню с экрана (одобрение плана включает и auto mode — Ома её зачитывает)
        Some("tell") if matches!(args.len(), 4 | 5) => {
            let told = args[2].parse().map_err(|_| "option — число".to_string()).and_then(|option| {
                ipc::request(&Msg::Tell { session: args[1].clone(), option, text: args[3].clone(), expect: args.get(4).cloned().unwrap_or_default() })
            });
            return done(told.map(|chosen| if !chosen.is_empty() { println!("{chosen}") }));
        }
        // агента — на экран (вкладка Orca или панель herdr и окно); next или без аргумента — следующий ждущий по кругу.
        // В терминал ничего не пишется. Зовёт сама Ома (AGENT_SPEAK от службы) — исход скажет она, пузыря службы не нужно
        Some("show") if args.len() <= 2 => {
            let agent = args.get(1).filter(|a| *a != "next").cloned().unwrap_or_default();
            let quiet = std::env::var_os("AGENT_SPEAK").is_some();
            return done(ipc::request(&Msg::Show { agent, quiet }).map(|name| println!("{name}")));
        }
        // отложенное поручение: tell — передать агенту, когда закончит ход (голосом его ставит Ома только после «да,
        // передай» — проверка кода в orders.py), checks — сказать исход проверок PR; stdout — id или список словами
        Some("later") => match later_msg(&args[1..]) {
            Some(m) => return done(ipc::request(&m).map(|ok| println!("{ok}"))),
            None => {
                eprintln!("agent-speak later tell <session> <текст> [смысл] | checks <папка> <PR> | cancel <id> | list");
                std::process::exit(2);
            }
        },
        // пункты меню агента с экрана — только чтение, без службы: Ома называет пункт в вопросе подтверждения до tell
        Some("menu") if args.len() == 2 => {
            return done(orders::menu_of(&args[1]).map(|items| items.iter().enumerate().for_each(|(i, t)| println!("{}. {t}", i + 1))));
        }
        // запуск и проекты пишут не в терминал агента — без службы, исход её мозгу нужен сразу
        // где запускать: orca (по умолчанию) или herdr
        Some("launch") if matches!(args.len(), 3 | 4) => return done(orders::launch(&args[1], &args[2], args.get(3).map_or("orca", |s| s))),
        Some("projects") => return done(orders::projects(args.get(1).map_or("orca", |s| s)).map(|v| println!("{}", v.join("\n")))),
        // агент пишет Оме, что ему нужно от пользователя (README, «Агенты пишут Оме»); исход — ему, отказ — с подсказкой
        Some("note") if args.len() >= 2 => {
            let session = std::env::var("CLAUDE_CODE_SESSION_ID").unwrap_or_default();
            let r = ipc::request(&Msg::Note { session, pid: std::process::id(), text: args[1..].join(" ") });
            return done(r.map(|ok| println!("{ok}")).map_err(|e| format!("{e} — напиши это пользователю в ответе")));
        }
        Some("hook") => {
            // вложенные вызовы моделей словаря не должны озвучиваться
            if std::env::var_os("AGENT_SPEAK").is_some() {
                return;
            }
            let kind = args.get(1).cloned().unwrap_or_default();
            if kind == "session-start" {
                // в демон не идёт: stdout SessionStart — в контекст агента; Ома заметки не слушает — ни слова
                let _ = std::io::stdin().read_to_string(&mut String::new());
                let on = config::Config::load().is_ok_and(|c| c.agent_inbox && c.agents_voice);
                if on && std::os::unix::net::UnixStream::connect(ipc::socket_path()).is_ok() {
                    println!("{INBOX_HINT}");
                }
                return;
            }
            if kind == "post-tool-use" {
                // ответ нужен сразу и службу не ждёт: срабатывает на каждый вызов инструмента
                let mut raw = String::new();
                let _ = std::io::stdin().read_to_string(&mut raw);
                if let Some(out) = post_tool_use(&raw, &limits::hints_dir()) {
                    println!("{out}");
                }
                // службе — только то, по чему узнать вызов: карточка разрешения, одобренного в терминале, снимается, когда
                // вызов отработал (у PermissionRequest нет tool_use_id); вывод инструмента ей не нужен
                if let Ok(p) = serde_json::from_str::<serde_json::Value>(&raw) {
                    let keep = ["session_id", "prompt_id", "tool_name", "tool_input", "hook_event_name"];
                    let payload = keep.iter().map(|k| (k.to_string(), p[*k].clone())).collect();
                    ipc::send(&Msg::Hook { kind, payload: serde_json::Value::Object(payload) });
                }
                return;
            }
            if kind == "permission-request" {
                let mut raw = String::new();
                let _ = std::io::stdin().read_to_string(&mut raw);
                if let Some(out) = permission_request(&raw, &ipc::socket_path()) {
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
            eprintln!("agent-speak on | off | daemon | read | stop | pause | voice | set <ключ> <значение> | hook <kind> [json] | statusline [команда …] | tell <session> <option> <text> [метка варианта] | menu <session> | show [<session>|next] | launch <path> <text> [orca|herdr] | projects [orca|herdr] | note <text> | later tell|checks|cancel|list …");
            std::process::exit(2);
        }
    };
    let read = matches!(msg, Msg::Read | Msg::Set { .. });
    if !ipc::send(&msg) && read {
        eprintln!("agent-speak: сервис озвучки не запущен (systemctl --user start agent-speakd)");
    }
}

/// Строка агенту от хука SessionStart: как писать Оме. Каждая новая сессия платит за неё токенами — коротко.
const INBOX_HINT: &str = "Ома — голосовой помощник пользователя, слышит тебя, даже когда он не смотрит в терминал.
Если без пользователя дальше нельзя (доступ, токен, выбор A или B), выполни: agent-speak note \"одна строка\".
Секретов в ней не пиши; ответ придёт обычным сообщением.";

/// Значение `set` из командной строки: true, false, числа — как JSON, остальное — строка.
fn set_value(v: &str) -> serde_json::Value {
    serde_json::from_str(v).unwrap_or_else(|_| serde_json::json!(v))
}

/// `later …` из командной строки → сообщение службе; неверные аргументы — None.
fn later_msg(a: &[String]) -> Option<Msg> {
    let arg = |i: usize| a.get(i).cloned().unwrap_or_default();
    let (act, n) = (a.first()?.as_str(), a.len());
    let ok = matches!((act, n), ("tell", 3 | 4) | ("checks", 3) | ("cancel", 2) | ("list", 1));
    let pr = if act == "checks" { arg(2).trim_start_matches('#').parse().ok()? } else { 0 };
    let (session, text, gist) = if act == "tell" { (arg(1), arg(2), arg(3)) } else { Default::default() };
    ok.then(|| Msg::Later { act: act.into(), session, text, gist, path: if act == "checks" { absolute(&arg(1)) } else { String::new() }, pr, id: if act == "cancel" { arg(1) } else { String::new() } })
}

/// Папка от папки вызвавшего, а не службы: `later checks . 12` в проекте — этот проект.
fn absolute(p: &str) -> String {
    std::path::absolute(p).map_or_else(|_| p.to_string(), |a| a.to_string_lossy().into_owned())
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

/// Хук PermissionRequest: запрос — службе, соединение держится, пока он жив: до кнопки в окне или пузыре, ответа в
/// терминале (служба шлёт «без решения») или конца хода. На «No» и Esc Claude Code убивает хук — служба видит EOF и
/// снимает карточку. Службы нет или без решения — пустой вывод: терминал спрашивает как обычно.
fn permission_request(raw: &str, sock: &std::path::Path) -> Option<String> {
    use std::io::{BufRead, Write};
    let payload = serde_json::from_str(raw).ok()?;
    let mut s = std::os::unix::net::UnixStream::connect(sock).ok()?;
    s.write_all(format!("{}\n", serde_json::to_string(&Msg::Permission { payload }).ok()?).as_bytes()).ok()?;
    let mut line = String::new();
    std::io::BufReader::new(s).read_line(&mut line).ok()?;
    let v: serde_json::Value = serde_json::from_str(&line).ok()?;
    perm::hook_output(v["decision"].as_str()?)
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
    fn inbox_hint_is_short_and_names_the_command() {
        assert_eq!(INBOX_HINT.lines().count(), 3);
        assert!(INBOX_HINT.contains("agent-speak note \""));
        assert!(INBOX_HINT.chars().count() < 300, "около 70 токенов на сессию");
    }

    #[test]
    fn set_value_is_json_or_text() {
        assert_eq!(set_value("kseniya"), serde_json::json!("kseniya"));
        assert_eq!(set_value("23:00"), serde_json::json!("23:00"));
        assert_eq!((set_value("false"), set_value("30")), (serde_json::json!(false), serde_json::json!(30)));
    }

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
    fn later_args() {
        let a = |s: &[&str]| later_msg(&s.iter().map(|x| x.to_string()).collect::<Vec<_>>()).map(|m| serde_json::to_value(m).unwrap());
        let tell = a(&["tell", "s1", "Прогони тесты", "тесты"]).unwrap();
        assert_eq!((tell["do"].as_str(), tell["session"].as_str(), tell["text"].as_str(), tell["gist"].as_str()), (Some("tell"), Some("s1"), Some("Прогони тесты"), Some("тесты")));
        assert_eq!(a(&["tell", "s1", "x"]).unwrap()["gist"], "");
        let here = a(&["checks", ".", "3"]).unwrap();
        assert_eq!(here["path"].as_str().unwrap(), std::env::current_dir().unwrap().to_string_lossy(), "папка вызвавшего, а не службы");
        let c = a(&["checks", "/p/x", "#12"]).unwrap();
        assert_eq!((c["path"].as_str(), c["pr"].as_u64()), (Some("/p/x"), Some(12)));
        assert_eq!(a(&["cancel", "later-3"]).unwrap()["id"], "later-3");
        assert_eq!(a(&["list"]).unwrap()["do"], "list");
        for bad in [&["checks", "/p", "x"][..], &["tell", "s1"], &["cancel"], &["nope"], &[]] {
            assert!(a(bad).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn permission_hook_waits_for_the_decision_and_prints_it() {
        use std::io::{BufRead, Write};
        let sock = std::env::temp_dir().join(format!("oma-perm-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&sock);
        assert_eq!(permission_request("{}", &sock), None, "службы нет — терминал спрашивает как обычно");
        let l = std::os::unix::net::UnixListener::bind(&sock).unwrap();
        let serve = |decision: &'static str| {
            let (c, _) = l.accept().unwrap();
            let mut line = String::new();
            std::io::BufReader::new(c.try_clone().unwrap()).read_line(&mut line).unwrap();
            let v: serde_json::Value = serde_json::from_str(&line).unwrap();
            assert_eq!((v["cmd"].as_str(), v["payload"]["tool_name"].as_str()), (Some("permission"), Some("Bash")));
            if !decision.is_empty() {
                (&c).write_all(format!("{{\"decision\":\"{decision}\"}}\n").as_bytes()).unwrap();
            }
        };
        let raw = r#"{"session_id":"s","tool_name":"Bash","tool_input":{"command":"ls"}}"#;
        let t = std::thread::spawn({ let sock = sock.clone(); move || permission_request(raw, &sock) });
        serve("deny");
        let out: serde_json::Value = serde_json::from_str(&t.join().unwrap().unwrap()).unwrap();
        assert_eq!(out["hookSpecificOutput"]["decision"]["behavior"], "deny");
        let t = std::thread::spawn({ let sock = sock.clone(); move || permission_request(raw, &sock) });
        serve(""); // служба закрыла соединение без решения (перезапуск)
        assert_eq!(t.join().unwrap(), None);
        let _ = std::fs::remove_file(&sock);
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
