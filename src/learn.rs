//! Фон: незнакомые слова → самая лёгкая модель агента → словарь. На путь к звуку не влияет.

use crate::focus::{parse_learned, Agent};
use crate::text::terms::Terms;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const PROMPT: &str = "Для каждого английского слова или аббревиатуры ниже напиши, как его произносят русскоязычные разработчики, кириллицей. \
Аббревиатуры по буквам через дефис (npm — эн-пи-эм). Ударение можно отметить знаком + перед ударной гласной. \
Формат строго: слово<TAB>произношение, по строке на слово, без пояснений.\n\n";

pub fn spawn(terms: Arc<Mutex<Terms>>) -> Sender<(Agent, String)> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || run(rx, terms));
    tx
}

fn run(rx: Receiver<(Agent, String)>, terms: Arc<Mutex<Terms>>) {
    let mut asked_once: std::collections::HashSet<String> = Default::default();
    loop {
        let Ok(first) = rx.recv() else { return };
        let mut batch = vec![first];
        while batch.len() < 20 {
            match rx.recv_timeout(Duration::from_secs(2)) {
                Ok(x) => batch.push(x),
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
        let agent = batch[0].0.clone();
        let words: Vec<String> = {
            let t = terms.lock().unwrap();
            let mut w: Vec<String> = batch.into_iter().map(|(_, w)| w).filter(|w| !t.contains(w)).collect();
            w.sort();
            w.dedup();
            w
        };
        // ponytail: слово спрашиваем раз за запуск сервиса; неудача — повтор после перезапуска
        let words: Vec<String> = words.into_iter().filter(|w| asked_once.insert(w.clone())).collect();
        if words.is_empty() {
            continue;
        }
        let Some(out) = ask(&agent, &format!("{PROMPT}{}", words.join("\n"))) else { continue };
        let mut t = terms.lock().unwrap();
        for (w, p) in parse_learned(&out, &words) {
            t.add(&w, &p);
        }
    }
}

fn ask(agent: &Agent, prompt: &str) -> Option<String> {
    let mut cmd = match agent {
        Agent::Claude => {
            let mut c = Command::new("timeout");
            // промпт сразу после -p: --tools принимает несколько значений и проглотил бы его
            c.args(["60", "claude", "-p", prompt, "--model", "haiku", "--no-session-persistence", "--setting-sources", "",
                    "--strict-mcp-config", "--tools", ""])
                .env("MAX_THINKING_TOKENS", "0")
                .env("CLAUDE_CODE_DISABLE_AUTO_MEMORY", "1");
            c
        }
        Agent::Codex => {
            let mut c = Command::new("timeout");
            c.args(["60", "codex", "exec", "--ephemeral", "--skip-git-repo-check", "-m", &codex_small()?,
                    "-c", "model_reasoning_effort=low", prompt]);
            c
        }
    };
    let out = cmd.env("AGENT_SPEAK", "1").stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Самая лёгкая модель Codex: из «fast»-моделей в списке — первая по priority.
fn codex_small() -> Option<String> {
    let home = std::env::var("HOME").ok()?;
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(format!("{home}/.codex/models_cache.json")).ok()?).ok()?;
    v["models"].as_array()?
        .iter()
        .filter(|m| m["visibility"] == "list" && m["description"].as_str().is_some_and(|d| d.to_lowercase().contains("fast")))
        .min_by_key(|m| m["priority"].as_i64().unwrap_or(i64::MAX))
        .and_then(|m| m["slug"].as_str().map(String::from))
}
