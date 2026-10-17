//! Реестр живых агентов пользователя: кто работает, ждёт ответа или закончил; лента событий каждого.
use crate::focus;
use serde::Serialize;
use serde_json::Value;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};

pub const FEED_MAX: usize = 20; // строк ленты на агента
pub const LINE_MAX: usize = 300; // символов в строке ленты
pub const IDLE_AFTER: u64 = 600; // с после «готово» → «простаивает»
const STATUS_MAX: usize = 120;

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Working,
    Waiting,
    Done,
    Idle,
}

#[derive(Clone, Debug, Serialize)]
pub struct Line {
    pub at: u64,
    pub kind: &'static str, // prompt | status | question | final
    pub text: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct Entry {
    pub id: String,
    pub agent: &'static str,
    pub title: String,
    pub project: String,
    pub cwd: String,
    pub transcript: String,
    pub state: State,
    pub status: String,
    pub since: u64,
    pub focused: bool,
    pub feed: VecDeque<Line>,
    #[serde(skip)]
    pub heard: bool, // хук уже говорил об этом агенте — файл сессии состояние не трогает
}

/// Живая сессия, найденная на диске / в /proc.
#[derive(Clone, Debug)]
pub struct Live {
    pub id: String,
    pub agent: &'static str,
    pub pid: u32,
    pub title: String,
    pub cwd: String,
    pub transcript: PathBuf,
    pub busy: bool,
}

pub fn scan_claude(sessions: &Path, projects: &Path, alive: impl Fn(u32) -> bool) -> Vec<Live> {
    let Ok(dir) = std::fs::read_dir(sessions) else { return vec![] };
    let dirs: Vec<PathBuf> = std::fs::read_dir(projects).into_iter().flatten().flatten().map(|d| d.path()).collect();
    dir.flatten()
        .filter(|f| f.path().extension().is_some_and(|e| e == "json"))
        .filter_map(|f| {
            let v: Value = serde_json::from_str(&std::fs::read_to_string(f.path()).ok()?).ok()?;
            let pid = u32::try_from(v["pid"].as_u64()?).ok()?;
            let id = v["sessionId"].as_str()?.to_string();
            if !matches!(v["kind"].as_str(), Some("interactive" | "bg")) || v["entrypoint"] != "cli" || !alive(pid) {
                return None;
            }
            let cwd = v["cwd"].as_str().unwrap_or_default().to_string();
            let name = v["name"].as_str().unwrap_or_default();
            let title = if v["nameSource"] != "derived" && !name.is_empty() { name.to_string() } else { folder(&cwd) };
            let file = format!("{id}.jsonl");
            let transcript = dirs.iter().map(|d| d.join(&file)).find(|p| p.exists()).unwrap_or_default();
            Some(Live { id, agent: "claude", pid, title, cwd, transcript, busy: v["status"] == "busy" })
        })
        .collect()
}

pub fn scan_codex() -> Vec<Live> {
    let mut out = vec![];
    for p in std::fs::read_dir("/proc").into_iter().flatten().flatten() {
        let Some(pid) = p.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else { continue };
        if !std::fs::read_to_string(format!("/proc/{pid}/comm")).is_ok_and(|c| c.trim() == "codex")
            || !std::fs::read_to_string(format!("/proc/{pid}/cmdline")).is_ok_and(|c| focus::is_user_codex(&c))
        {
            continue;
        }
        let Some((id, transcript)) = focus::codex_rollout(pid) else { continue };
        let cwd = std::fs::read_link(format!("/proc/{pid}/cwd")).map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        out.push(Live { id, agent: "codex", pid, title: focus::project_of(pid), cwd, transcript, busy: false });
    }
    out
}

pub fn scan_all() -> Vec<Live> {
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".claude");
    let mut v = scan_claude(&home.join("sessions"), &home.join("projects"), |pid| Path::new(&format!("/proc/{pid}")).exists());
    v.extend(scan_codex());
    v
}

fn folder(cwd: &str) -> String {
    Path::new(cwd).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Первое предложение (до . ! ? …), не длиннее STATUS_MAX символов.
fn first_sentence(text: &str) -> String {
    let t = text.trim();
    let end = t.char_indices().find(|(_, c)| ".!?…".contains(*c)).map_or(t.len(), |(i, c)| i + c.len_utf8());
    t[..end].chars().take(STATUS_MAX).collect()
}

#[derive(Default)]
pub struct Registry {
    entries: Vec<Entry>,
}

impl Registry {
    pub fn new() -> Registry {
        Registry::default()
    }

    /// Привести состав к списку живых. true — состав изменился (пришёл или ушёл агент).
    pub fn sync(&mut self, live: Vec<Live>, now: u64) -> bool {
        let before = self.entries.len();
        self.entries.retain(|e| live.iter().any(|l| l.id == e.id));
        let mut changed = self.entries.len() != before;
        for l in live {
            let state = if l.busy { State::Working } else { State::Idle };
            let transcript = l.transcript.to_string_lossy().into_owned();
            if let Some(e) = self.entries.iter_mut().find(|e| e.id == l.id) {
                (e.title, e.project, e.cwd, e.transcript) = (l.title, folder(&l.cwd), l.cwd, transcript);
                if !e.heard && e.state != state {
                    (e.state, e.since) = (state, now);
                }
            } else {
                changed = true;
                self.entries.push(Entry {
                    id: l.id, agent: l.agent, title: l.title, project: folder(&l.cwd), cwd: l.cwd, transcript,
                    state, status: String::new(), since: now, focused: false, feed: VecDeque::new(), heard: false,
                });
            }
        }
        changed
    }

    pub fn get(&self, id: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.id == id)
    }

    fn event(&mut self, id: &str, kind: &'static str, text: &str, state: State, status: Option<String>, now: u64) -> bool {
        let Some(e) = self.entries.iter_mut().find(|e| e.id == id) else { return false };
        e.heard = true;
        (e.state, e.since) = (state, now);
        if let Some(s) = status {
            e.status = s;
        }
        if e.feed.len() == FEED_MAX {
            e.feed.pop_front();
        }
        e.feed.push_back(Line { at: now, kind, text: text.chars().take(LINE_MAX).collect() });
        true
    }

    pub fn prompt(&mut self, id: &str, text: &str, now: u64) -> bool {
        self.event(id, "prompt", text, State::Working, None, now)
    }

    pub fn status(&mut self, id: &str, text: &str, now: u64) -> bool {
        self.event(id, "status", text, State::Working, Some(text.chars().take(LINE_MAX).collect()), now)
    }

    pub fn question(&mut self, id: &str, text: &str, now: u64) -> bool {
        self.event(id, "question", text, State::Waiting, Some(text.chars().take(LINE_MAX).collect()), now)
    }

    pub fn finished(&mut self, id: &str, text: &str, now: u64) -> bool {
        self.event(id, "final", text, State::Done, Some(first_sentence(text)), now)
    }

    /// done старше IDLE_AFTER → idle. true — что-то изменилось.
    pub fn tick(&mut self, now: u64) -> bool {
        let mut changed = false;
        for e in self.entries.iter_mut().filter(|e| e.state == State::Done && now.saturating_sub(e.since) >= IDLE_AFTER) {
            e.state = State::Idle;
            changed = true;
        }
        changed
    }

    pub fn set_focused(&mut self, id: Option<&str>) -> bool {
        let mut changed = false;
        for e in &mut self.entries {
            let f = Some(e.id.as_str()) == id;
            changed |= e.focused != f;
            e.focused = f;
        }
        changed
    }

    pub fn waiting(&self) -> usize {
        self.entries.iter().filter(|e| e.state == State::Waiting).count()
    }

    /// Строка для панели: ждущие, работающие, готовые, простаивающие; внутри — свежие первыми.
    pub fn line(&self) -> String {
        let mut v: Vec<&Entry> = self.entries.iter().collect();
        let group = |s: State| match s {
            State::Waiting => 0,
            State::Working => 1,
            State::Done => 2,
            State::Idle => 3,
        };
        v.sort_by_key(|e| (group(e.state), std::cmp::Reverse(e.since)));
        format!("{}\n", serde_json::json!({ "agents": v }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn live(id: &str, busy: bool) -> Live {
        Live { id: id.into(), agent: "claude", pid: 1, title: id.into(), cwd: format!("/p/{id}"), transcript: PathBuf::new(), busy }
    }

    #[test]
    fn scan_keeps_only_live_user_sessions() {
        let root = std::env::temp_dir().join(format!("oma-scan-{}", std::process::id()));
        let (ss, pp) = (root.join("sessions"), root.join("projects"));
        std::fs::create_dir_all(&ss).unwrap();
        std::fs::create_dir_all(pp.join("-home-u-Work")).unwrap();
        let s = |pid: u32, id: &str, kind: &str, entry: &str, name: &str, src: &str| format!(
            r#"{{"pid":{pid},"sessionId":"{id}","cwd":"/home/u/Work","kind":"{kind}","entrypoint":"{entry}","name":"{name}","nameSource":"{src}","status":"busy"}}"#);
        std::fs::write(ss.join("1.json"), s(1, "a", "interactive", "cli", "work-3d", "derived")).unwrap();
        std::fs::write(ss.join("2.json"), s(2, "b", "bg", "cli", "Почтовый клиент", "auto")).unwrap();
        std::fs::write(ss.join("3.json"), s(3, "c", "interactive", "sdk-cli", "tmp-1", "derived")).unwrap();
        std::fs::write(ss.join("4.json"), s(4, "d", "interactive", "cli", "x", "derived")).unwrap();
        std::fs::write(ss.join("1.abc.key"), "мусор").unwrap();
        std::fs::write(pp.join("-home-u-Work/a.jsonl"), "").unwrap();
        let got = scan_claude(&ss, &pp, |pid| pid != 4);
        let mut ids: Vec<(String, String)> = got.iter().map(|l| (l.id.clone(), l.title.clone())).collect();
        ids.sort();
        assert_eq!(ids, [("a".into(), "Work".into()), ("b".into(), "Почтовый клиент".into())]);
        assert!(got.iter().find(|l| l.id == "a").unwrap().transcript.ends_with("-home-u-Work/a.jsonl"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn states_follow_hooks() {
        let mut r = Registry::new();
        assert!(r.sync(vec![live("a", true), live("b", false)], 100));
        assert_eq!((r.get("a").unwrap().state, r.get("b").unwrap().state), (State::Working, State::Idle));
        assert!(r.question("a", "Клод задал вопрос: какой вариант?", 110));
        assert_eq!((r.get("a").unwrap().state, r.waiting()), (State::Waiting, 1));
        assert!(r.status("a", "Читаю план.", 120));
        assert_eq!(r.get("a").unwrap().state, State::Working);
        assert!(r.finished("a", "Готово. Тесты прошли, PR создан.", 130));
        r.sync(vec![live("a", true), live("b", false)], 131); // файл сессии не перебивает хуки
        let a = r.get("a").unwrap();
        assert_eq!((a.state, a.status.as_str(), a.since), (State::Done, "Готово.", 130));
        assert!(!r.tick(130 + IDLE_AFTER - 1));
        assert!(r.tick(130 + IDLE_AFTER));
        assert_eq!(r.get("a").unwrap().state, State::Idle);
        assert!(r.prompt("a", "запусти тесты", 900));
        assert_eq!(r.get("a").unwrap().feed.back().unwrap().kind, "prompt");
        assert!(!r.status("чужой", "текст", 1));
    }

    #[test]
    fn feed_capped_and_trimmed() {
        let mut r = Registry::new();
        r.sync(vec![live("a", true)], 0);
        for i in 0..25 { r.status("a", &format!("статус {i}"), i); }
        let a = r.get("a").unwrap();
        assert_eq!((a.feed.len(), a.feed.front().unwrap().text.as_str()), (FEED_MAX, "статус 5"));
        r.status("a", &"я".repeat(500), 30);
        assert_eq!(r.get("a").unwrap().feed.back().unwrap().text.chars().count(), LINE_MAX);
    }

    #[test]
    fn line_sorted_and_dead_removed() {
        let mut r = Registry::new();
        r.sync(vec![live("a", true), live("b", true), live("c", false)], 10);
        r.question("b", "вопрос", 20);
        r.status("a", "работаю", 30);
        assert!(r.set_focused(Some("a")) && !r.set_focused(Some("a")));
        let v: serde_json::Value = serde_json::from_str(&r.line()).unwrap();
        let ids: Vec<&str> = v["agents"].as_array().unwrap().iter().map(|e| e["id"].as_str().unwrap()).collect();
        assert_eq!(ids, ["b", "a", "c"]);
        assert_eq!((v["agents"][0]["state"].as_str(), v["agents"][1]["focused"].as_bool()), (Some("waiting"), Some(true)));
        assert!(v["agents"][0].get("heard").is_none());
        assert!(r.sync(vec![live("a", true)], 40) && r.get("b").is_none());
        assert!(!r.sync(vec![live("a", true)], 50));
    }
}
