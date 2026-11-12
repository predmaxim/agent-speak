//! Карточки разрешений (хук PermissionRequest, `agent-speak hook permission-request`): процесс хука держит запрос и
//! открытое соединение со службой, окно и пузырь Омы показывают запрос с кнопками «Разрешить» и «Отклонить». Диалог в
//! терминале виден сразу и отвечается сам по себе (проба волны 0, Claude Code 2.1.322), поэтому карточка уходит, как
//! только запрос решён где угодно: кнопкой (решение — хуку), ответом в терминале («Yes» — по PostToolUse или
//! PostToolUseFailure того же вызова, «No» и Esc — Claude Code убивает хук, служба видит EOF) и концом хода (Stop).

use serde_json::{json, Value};
use std::collections::HashMap;
use std::os::unix::net::UnixStream;

/// Инструменты, которым решение без `updatedInput` не годится (hooks.md, PermissionRequest: «need this field»): их
/// вопросы отвечает `tell`, карточки нет.
const NOT_CARDED: [&str; 2] = ["AskUserQuestion", "ExitPlanMode"];

/// Отказ кнопкой — агенту: он продолжает ход (interrupt false) и знает, кто отказал.
pub const DENIED: &str = "Отклонено пользователем в окне Омы";

pub struct Card {
    pub id: String,
    pub session: String,
    prompt: String, // prompt_id: у PermissionRequest нет tool_use_id — вызов узнаётся по сессии, промпту и вводу
    tool: String,
    input: Value,
    sub: bool,      // вызов сабагента (agent_id)
    since: u64,
    hook: UnixStream, // соединение хука: строка решения — сюда
}

/// Строка решения хуку: allow | deny | "" (без решения — терминал спрашивает как обычно). false — хука уже нет.
fn answer(hook: &UnixStream, decision: &str) -> bool {
    crate::daemon::answer(hook, &format!("{}\n", json!({"decision": decision})))
}

impl Card {
    /// «Bash: cargo test» — пузырю и ленте агента.
    pub fn line(&self) -> String {
        let (tool, text, _) = view(&self.tool, &self.input);
        format!("{tool}: {text}")
    }

    fn is(&self, p: &Value) -> bool {
        p["session_id"] == self.session.as_str() && p["prompt_id"] == self.prompt.as_str() && p["tool_name"] == self.tool.as_str() && p["tool_input"] == self.input
    }
}

/// Что показать о вызове (дизайн «Джарвис», «Текст карточки»): инструмент («Bash», «github: create_issue»), главная
/// строка и тусклая строка (описание команды). У Bash, MCP и прочих — ввод целиком; у правок и записи — только файл
/// (их содержимое — в диалоге терминала), у WebFetch — адрес, у WebSearch — запрос.
pub fn view(tool: &str, input: &Value) -> (String, String, String) {
    let s = |k: &str| input[k].as_str().unwrap_or_default().to_string();
    let whole = || input.to_string();
    match tool {
        "Bash" => (tool.into(), s("command"), s("description")),
        "Edit" | "MultiEdit" | "Write" | "Read" => (tool.into(), s("file_path"), String::new()),
        "NotebookEdit" => (tool.into(), s("notebook_path"), String::new()),
        "WebFetch" => (tool.into(), s("url"), String::new()),
        "WebSearch" => (tool.into(), s("query"), String::new()),
        t => match t.strip_prefix("mcp__").and_then(|r| r.split_once("__")) {
            Some((server, name)) => (format!("{server}: {name}"), whole(), String::new()),
            None => (t.into(), whole(), String::new()),
        },
    }
}

#[derive(Default)]
pub struct Cards {
    list: Vec<Card>,
    /// сессия → (prompt_id, была ли карточка) последнего запроса: Notification permission_prompt того же запроса
    /// (через ~6 с, пока он открыт) не озвучивается второй раз
    last: HashMap<String, (String, bool)>,
}

impl Cards {
    /// Запрос PermissionRequest: карточка, если инструмент её допускает (иначе — None, хуку сразу «без решения»).
    /// shown — показывать ли карточки вообще (сессия в реестре, «Кнопки разрешений» включены).
    pub fn add(&mut self, id: &str, p: &Value, hook: UnixStream, shown: bool, now: u64) -> Option<&Card> {
        let (session, prompt, tool) = (p["session_id"].as_str().unwrap_or_default(), p["prompt_id"].as_str().unwrap_or_default(), p["tool_name"].as_str().unwrap_or_default());
        let carded = shown && !session.is_empty() && !tool.is_empty() && !NOT_CARDED.contains(&tool);
        self.last.insert(session.into(), (prompt.into(), carded));
        if !carded {
            answer(&hook, "");
            return None;
        }
        let sub = p["agent_id"].as_str().is_some_and(|a| !a.is_empty());
        self.list.push(Card { id: id.into(), session: session.into(), prompt: prompt.into(), tool: tool.into(), input: p["tool_input"].clone(), sub, since: now, hook });
        self.list.last()
    }

    /// Notification permission_prompt этой сессии и промпта — про запрос, уже показанный карточкой (и, может быть, уже
    /// решённый кнопкой): «просит разрешение» прозвучало при карточке.
    pub fn carded(&self, session: &str, prompt: &str) -> bool {
        self.last.get(session).is_some_and(|(p, c)| *c && p == prompt)
    }

    /// Клик: решение хуку (allow | deny; "" — снять без решения), карточка снята. None — карточки нет (уже решена или
    /// снята); false — хука уже нет: решение не дошло.
    pub fn decide(&mut self, id: &str, decision: &str) -> Option<(Card, bool)> {
        let i = self.list.iter().position(|c| c.id == id)?;
        let card = self.list.remove(i);
        let ok = answer(&card.hook, decision);
        Some((card, ok))
    }

    /// Хук вышел или убит (EOF его соединения): карточка снята без записи. true — была.
    pub fn gone(&mut self, id: &str) -> bool {
        let n = self.list.len();
        self.list.retain(|c| c.id != id);
        self.list.len() != n
    }

    /// Запрос решён в терминале: вызов отработал (PostToolUse, PostToolUseFailure — тот же session_id, prompt_id и ввод).
    /// Хуку — «без решения», он выходит. true — карточка была.
    pub fn ran(&mut self, p: &Value) -> bool {
        self.withdraw(|c| c.is(p))
    }

    /// Ход сессии кончился (Stop): её карточки сняты, хуки выходят; subs false — помощники ещё работают в фоне, их
    /// запросы (agent_id) живы.
    pub fn end(&mut self, session: &str, subs: bool) -> bool {
        if subs {
            self.last.remove(session); // уведомлений этого хода больше не будет
        }
        self.withdraw(|c| c.session == session && (subs || !c.sub))
    }

    /// Ещё открытые карточки сессии.
    pub fn open(&self, session: &str) -> usize {
        self.list.iter().filter(|c| c.session == session).count()
    }

    /// Снять карточки pick: хукам — «без решения», терминал спрашивает как обычно. true — были.
    pub fn withdraw(&mut self, pick: impl Fn(&Card) -> bool) -> bool {
        let (gone, keep): (Vec<Card>, Vec<Card>) = std::mem::take(&mut self.list).into_iter().partition(|c| pick(c));
        self.list = keep;
        for c in &gone {
            answer(&c.hook, "");
        }
        !gone.is_empty()
    }

    /// (сессия, вызов сабагента) открытой карточки.
    pub fn session_of(&self, id: &str) -> Option<(String, bool)> {
        self.list.iter().find(|c| c.id == id).map(|c| (c.session.clone(), c.sub))
    }

    /// Строка подписчикам: `{"permissions":[{id, agent, who, tool, text, note, sub, since, short}]}`, старые первыми; who —
    /// имя агента для окна (проект); short — текст пузыря у орба «Bash: cargo test»: пузыри не показывают ключей, и если
    /// маска (`mask_secrets`) его меняет, short пуст — пузырь без команды и кнопок, карточка целиком — в окне.
    pub fn line(&self, who: impl Fn(&str) -> String) -> String {
        let list: Vec<Value> = self.list.iter().map(|c| {
            let (tool, text, note) = view(&c.tool, &c.input);
            let line = c.line();
            let short = if crate::text::clean::mask_secrets(&line) == line { line } else { String::new() };
            json!({"id": c.id, "agent": c.session, "who": who(&c.session), "tool": tool, "text": text, "note": note, "sub": c.sub, "since": c.since, "short": short})
        }).collect();
        format!("{}\n", json!({"permissions": list}))
    }
}

/// Ответ хука Claude Code на решение службы: allow | deny — JSON PermissionRequest, иначе (без решения) — None, пустой
/// вывод: терминал спрашивает как обычно. `updatedInput` не шлётся — одобряется ровно показанный ввод.
pub fn hook_output(decision: &str) -> Option<String> {
    let d = match decision {
        "allow" => json!({"behavior": "allow"}),
        "deny" => json!({"behavior": "deny", "message": DENIED, "interrupt": false}),
        _ => return None,
    };
    Some(json!({"hookSpecificOutput": {"hookEventName": "PermissionRequest", "decision": d}}).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};

    fn req(session: &str, prompt: &str, tool: &str, input: Value) -> Value {
        json!({"session_id": session, "prompt_id": prompt, "hook_event_name": "PermissionRequest", "tool_name": tool, "tool_input": input, "permission_suggestions": []})
    }

    fn bash(cmd: &str) -> Value {
        req("s", "p1", "Bash", json!({"command": cmd, "description": "Run the tests"}))
    }

    /// Соединение хука: (сторона службы, читатель хука).
    fn hook() -> (UnixStream, BufReader<UnixStream>) {
        let (a, b) = UnixStream::pair().unwrap();
        (a, BufReader::new(b))
    }

    fn said(r: &mut BufReader<UnixStream>) -> String {
        let mut l = String::new();
        r.read_line(&mut l).unwrap();
        serde_json::from_str::<Value>(&l).unwrap()["decision"].as_str().unwrap().to_string()
    }

    #[test]
    fn view_shows_exactly_what_is_approved() {
        let v = |t: &str, i: Value| view(t, &i);
        assert_eq!(v("Bash", json!({"command": "cargo test --release", "description": "Run the tests"})), ("Bash".into(), "cargo test --release".into(), "Run the tests".into()));
        assert_eq!(v("Edit", json!({"file_path": "/p/a.rs", "old_string": "x"})).1, "/p/a.rs");
        assert_eq!(v("Write", json!({"file_path": "/p/b.rs", "content": "x"})).1, "/p/b.rs");
        assert_eq!(v("NotebookEdit", json!({"notebook_path": "/p/n.ipynb"})).1, "/p/n.ipynb");
        assert_eq!(v("WebFetch", json!({"url": "https://x.dev", "prompt": "p"})).1, "https://x.dev");
        assert_eq!(v("WebSearch", json!({"query": "silero"})).1, "silero");
        assert_eq!(v("mcp__github__create_issue", json!({"title": "t"})), ("github: create_issue".into(), "{\"title\":\"t\"}".into(), String::new()));
        assert_eq!(v("Skill", json!({"skill": "x"})), ("Skill".into(), "{\"skill\":\"x\"}".into(), String::new()), "прочее — имя и JSON ввода");
    }

    #[test]
    fn card_until_decided_then_hook_gets_the_decision_once() {
        let mut c = Cards::default();
        let (h, mut r) = hook();
        assert_eq!(c.add("perm-1", &bash("cargo test"), h, true, 100).unwrap().line(), "Bash: cargo test");
        let line: Value = serde_json::from_str(&c.line(|s| format!("<{s}>"))).unwrap();
        assert_eq!(line["permissions"][0], json!({"id": "perm-1", "agent": "s", "who": "<s>", "tool": "Bash", "text": "cargo test", "note": "Run the tests", "sub": false, "since": 100, "short": "Bash: cargo test"}));
        assert!(c.carded("s", "p1") && !c.carded("s", "p2"));
        let (card, ok) = c.decide("perm-1", "allow").unwrap();
        assert!(ok && card.session == "s");
        assert_eq!(said(&mut r), "allow");
        assert!(c.decide("perm-1", "deny").is_none(), "клик по уже решённому — мимо");
        assert!(c.carded("s", "p1"), "уведомление через 6 с — про решённую карточку: тоже не озвучивается");
        assert_eq!(c.line(|s| s.into()), "{\"permissions\":[]}\n");
    }

    #[test]
    fn deny_reaches_a_hook_that_is_gone_as_not_delivered() {
        let mut c = Cards::default();
        let (h, r) = hook();
        c.add("perm-1", &bash("rm -rf build"), h, true, 1);
        drop(r); // хук убит: No или Esc в терминале
        let (_, ok) = c.decide("perm-1", "deny").unwrap();
        assert!(!ok, "решение не дошло — кнопка не делает вид, что подействовала");
    }

    #[test]
    fn not_carded_answers_at_once_and_is_voiced_as_usual() {
        let mut c = Cards::default();
        for (p, shown) in [(req("s", "p1", "AskUserQuestion", json!({})), true), (req("s", "p1", "ExitPlanMode", json!({})), true), (bash("ls"), false)] {
            let (h, mut r) = hook();
            assert!(c.add("perm-x", &p, h, shown, 1).is_none());
            assert_eq!(said(&mut r), "", "без решения: терминал спрашивает как обычно");
            assert!(!c.carded("s", "p1"), "уведомление такого запроса озвучивается как раньше");
        }
    }

    #[test]
    fn terminal_answer_withdraws_the_matching_card_only() {
        let mut c = Cards::default();
        let (h1, mut r1) = hook();
        let (h2, _r2) = hook();
        c.add("perm-1", &bash("cargo test"), h1, true, 1);
        c.add("perm-2", &bash("cargo build"), h2, true, 2);
        let mut ran = bash("cargo test");
        ran["hook_event_name"] = "PostToolUse".into();
        ran["tool_use_id"] = "toolu_1".into();
        assert!(!c.ran(&json!({"session_id": "s", "prompt_id": "p0", "tool_name": "Bash", "tool_input": ran["tool_input"]})), "другой промпт — другой вызов");
        assert!(c.ran(&ran));
        assert_eq!(said(&mut r1), "", "хук выходит без решения");
        assert!(c.decide("perm-1", "allow").is_none() && c.session_of("perm-2").is_some() && c.open("s") == 1);
        assert!(c.end("s", true) && c.session_of("perm-2").is_none(), "Stop снимает все карточки сессии");
        assert!(!c.carded("s", "p1"), "после конца хода уведомлений этого запроса нет — запись не копится");
        assert!(!c.gone("perm-2"));
    }

    #[test]
    fn gone_hook_takes_its_card() {
        let mut c = Cards::default();
        let (h, _r) = hook();
        let mut p = bash("ls");
        p["agent_id"] = "a1".into();
        c.add("perm-1", &p, h, true, 1);
        assert_eq!(serde_json::from_str::<Value>(&c.line(|s| s.into())).unwrap()["permissions"][0]["sub"], true, "сабагент — с пометкой");
        assert!(!c.end("s", false), "помощник в фоне: его запрос жив и после Stop главного хода");
        assert!(c.gone("perm-1") && !c.gone("perm-1"));
    }

    #[test]
    fn hook_output_is_claude_codes_format() {
        let a: Value = serde_json::from_str(&hook_output("allow").unwrap()).unwrap();
        assert_eq!(a, json!({"hookSpecificOutput": {"hookEventName": "PermissionRequest", "decision": {"behavior": "allow"}}}));
        let d: Value = serde_json::from_str(&hook_output("deny").unwrap()).unwrap();
        assert_eq!(d["hookSpecificOutput"]["decision"], json!({"behavior": "deny", "message": DENIED, "interrupt": false}));
        assert_eq!((hook_output(""), hook_output("junk")), (None, None));
    }
}
