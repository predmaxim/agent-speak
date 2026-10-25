//! Реестр живых агентов пользователя: кто работает, ждёт ответа или закончил; лента событий каждого.
use crate::text::clean::plain;
use crate::focus;
use serde::Serialize;
use serde_json::Value;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};

pub const FEED_MAX: usize = 20; // строк ленты на агента
pub const LINE_MAX: usize = 8000; // символов в строке ленты: чат окна показывает сообщение целиком
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
    /// Картинки промпта (скриншоты) — файлы в кэше, см. `images_dir`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<String>,
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
    pub heard: bool, // хук уже говорил об этом агенте — файл сессии меняет состояние, только если он новее
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
    pub state: State,   // по файлу сессии: busy → работает, waiting → ждёт ответа, прочее → простаивает
    pub status_at: u64, // statusUpdatedAt файла сессии, с; 0 — неизвестно
}

/// start_of(pid): время старта живого процесса (см. proc_start), None — процесса нет.
pub fn scan_claude(sessions: &Path, projects: &Path, start_of: impl Fn(u32) -> Option<String>) -> Vec<Live> {
    let Ok(dir) = std::fs::read_dir(sessions) else { return vec![] };
    let dirs: Vec<PathBuf> = std::fs::read_dir(projects).into_iter().flatten().flatten().map(|d| d.path()).collect();
    dir.flatten()
        .filter(|f| f.path().extension().is_some_and(|e| e == "json"))
        .filter_map(|f| {
            let v: Value = serde_json::from_str(&std::fs::read_to_string(f.path()).ok()?).ok()?;
            let pid = u32::try_from(v["pid"].as_u64()?).ok()?;
            let id = v["sessionId"].as_str()?.to_string();
            if !matches!(v["kind"].as_str(), Some("interactive" | "bg")) || v["entrypoint"] != "cli" {
                return None;
            }
            let started = start_of(pid)?;
            // pid переиспользован: под тем же номером живёт другой, позже запущенный процесс
            let ps = &v["procStart"];
            if !ps.is_null() && ps.as_str().map_or_else(|| ps.to_string(), str::to_string) != started {
                return None;
            }
            let cwd = v["cwd"].as_str().unwrap_or_default().to_string();
            let name = v["name"].as_str().unwrap_or_default();
            let file = format!("{id}.jsonl");
            let transcript = dirs.iter().map(|d| d.join(&file)).find(|p| p.exists()).unwrap_or_default();
            // имя сессии, иначе её заголовок из транскрипта (как в окне), иначе папка
            let title = if v["nameSource"] != "derived" && !name.is_empty() {
                name.to_string()
            } else {
                session_title(&transcript).unwrap_or_else(|| folder(&cwd))
            };
            let state = match v["status"].as_str() {
                Some("busy") => State::Working,
                Some("waiting") => State::Waiting,
                _ => State::Idle,
            };
            let status_at = v["statusUpdatedAt"].as_u64().unwrap_or(0) / 1000;
            Some(Live { id, agent: "claude", pid, title, cwd, transcript, state, status_at })
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
        out.push(Live { id, agent: "codex", pid, title: focus::project_of(pid), cwd, transcript, state: State::Idle, status_at: 0 });
    }
    out
}

pub fn scan_all() -> Vec<Live> {
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".claude");
    let mut v = scan_claude(&home.join("sessions"), &home.join("projects"), proc_start);
    v.extend(scan_codex());
    v
}

/// Строки транскрипта с конца: читаем кусками по 64 КиБ, не дальше `limit` байт от конца; `each` вернул true — хватит.
fn tail_lines(transcript: &Path, limit: u64, mut each: impl FnMut(&[u8]) -> bool) {
    use std::io::{Read, Seek, SeekFrom};
    const CHUNK: u64 = 64 << 10;
    let Ok(mut f) = std::fs::File::open(transcript) else { return };
    let Ok(len) = f.metadata().map(|m| m.len()) else { return };
    let (mut pos, mut carry) = (len, Vec::new());
    while pos > 0 && len - pos < limit {
        let start = pos.saturating_sub(CHUNK);
        let mut buf = vec![0; (pos - start) as usize];
        if f.seek(SeekFrom::Start(start)).is_err() || f.read_exact(&mut buf).is_err() {
            return;
        }
        buf.extend_from_slice(&carry);
        pos = start;
        // начало куска может быть обрезанной строкой: оставляем её для склейки со следующим куском
        let from = if pos == 0 { 0 } else { buf.iter().position(|&b| b == b'\n').map_or(buf.len(), |i| i + 1) };
        if buf[from..].split(|&b| b == b'\n').rev().any(&mut each) {
            return;
        }
        buf.truncate(from);
        carry = buf;
    }
}

/// Последний заголовок сессии (`custom-title` или `ai-title`) из хвоста транскрипта, не дальше 1 МиБ от конца.
// ponytail: кэш по размеру файла, если скан раз в 2 с станет заметен.
fn session_title(transcript: &Path) -> Option<String> {
    let mut title = None;
    tail_lines(transcript, 1 << 20, |l| {
        title = line_title(l);
        title.is_some()
    });
    title
}

/// История агента для ленты — последние FEED_MAX реплик транскрипта Claude: промпты пользователя, тексты
/// агента и пересказ его хода. Хвост транскрипта, не дальше 16 МиБ (там и тяжёлые строки запросов к API).
pub fn history(transcript: &Path, images: &Path) -> VecDeque<Line> {
    let mut lines = VecDeque::new();
    tail_lines(transcript, 16 << 20, |l| {
        if let Some(line) = history_line(l, images) {
            lines.push_front(line);
        }
        lines.len() >= FEED_MAX
    });
    lines
}

/// Реплика транскрипта: промпт пользователя — только набранный человеком (`origin.kind` = human; у старых
/// записей без `origin` — не isMeta, без тега в начале), без тела навыков, сообщений других сессий
/// (`<agent-message>`), `<task-notification>`, подписей картинок; текст агента — его блоки `text`.
fn history_line(l: &[u8], images: &Path) -> Option<Line> {
    let has = |pat: &[u8]| l.windows(pat.len()).any(|w| w == pat);
    if !(has(b"\"type\":\"user\"") || has(b"\"type\":\"assistant\"") || has(b"\"queued_command\"")) {
        return None;
    }
    let v: Value = serde_json::from_slice(l).ok()?;
    if v["isSidechain"].as_bool() == Some(true) {
        return None;
    }
    // сообщение, отправленное посреди хода, — вложение queued_command со своим origin
    let (kind, content, origin, meta) = match v["type"].as_str()? {
        "user" => ("prompt", &v["message"]["content"], &v["origin"], &v["isMeta"]),
        "attachment" if v["attachment"]["type"] == "queued_command" => {
            let a = &v["attachment"];
            ("prompt", &a["prompt"], &a["origin"], &a["isMeta"])
        }
        "assistant" => ("status", &v["message"]["content"], &Value::Null, &Value::Null),
        _ => return None,
    };
    let text = match content {
        Value::String(t) => t.clone(),
        // у агента ещё и пересказ хода (thinking при thinkingDisplay = updates): терминал показывает его как
        // реплику, а MessageDisplay его не шлёт; скрытое мышление в транскрипте пустое
        Value::Array(blocks) => blocks.iter().filter_map(|b| match b["type"].as_str() {
            Some("text") => b["text"].as_str(),
            Some("thinking") if v["thinkingDisplay"] == "updates" => b["thinking"].as_str(),
            _ => None,
        }).collect::<Vec<_>>().join("\n"),
        _ => return None,
    };
    if kind == "prompt" {
        let human = match origin["kind"].as_str() {
            Some(k) => k == "human",
            None => meta.as_bool() != Some(true) && !technical(&text),
        };
        if !human || content.as_array().is_some_and(|b| b.iter().any(|b| b["type"] == "tool_result")) {
            return None;
        }
    }
    let pics = if kind == "prompt" { save_images(content, images) } else { Vec::new() };
    let text = shown(&text);
    (!text.is_empty() || !pics.is_empty())
        .then(|| Line { at: v["timestamp"].as_str().and_then(epoch).unwrap_or(0), kind, text: text.chars().take(LINE_MAX).collect(), images: pics })
}

/// Кэш картинок промптов: `$XDG_CACHE_HOME/agent-speak/images` (по умолчанию `~/.cache/…`).
// ponytail: кэш не чистится — скриншоты по сотне КиБ; чистить по возрасту, если разрастётся.
pub fn images_dir() -> PathBuf {
    let cache = std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".cache"));
    cache.join("agent-speak/images")
}

/// Картинки base64 из блоков промпта — в файлы `<хеш данных>.<тип>` (уже записанный не пишется заново); пути.
fn save_images(content: &Value, dir: &Path) -> Vec<String> {
    use base64::Engine;
    use std::hash::{Hash, Hasher};
    let Some(blocks) = content.as_array() else { return Vec::new() };
    blocks.iter().filter(|b| b["type"] == "image" && b["source"]["type"] == "base64").filter_map(|b| {
        let data = b["source"]["data"].as_str()?;
        let ext = b["source"]["media_type"].as_str()?.strip_prefix("image/")?;
        let mut h = std::collections::hash_map::DefaultHasher::new();
        data.hash(&mut h);
        let path = dir.join(format!("{:016x}.{ext}", h.finish()));
        if !path.exists() {
            let bytes = base64::engine::general_purpose::STANDARD.decode(data).ok()?;
            std::fs::create_dir_all(dir).ok()?;
            std::fs::write(&path, bytes).ok()?;
        }
        Some(path.to_string_lossy().into_owned())
    }).collect()
}

/// Текст для чата окна: Markdown как есть (окно его рисует), без картинок-ссылок `![alt](url)` — вместо
/// них alt: Text.MarkdownText качал бы их сам, а на загрузке картинок он падает.
fn shown(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text.trim();
    while let Some(i) = rest.find("![") {
        out.push_str(&rest[..i]);
        let after = &rest[i + 2..];
        match after.find("](").and_then(|j| after[j..].find(')').map(|k| (j, j + k))) {
            Some((j, end)) => {
                out.push_str(&after[..j]);
                rest = &after[end + 1..];
            }
            None => {
                out.push_str("![");
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Служебная вставка Claude Code в виде промпта: начинается с XML-тега (`<agent-message …>`,
/// `<task-notification>`, `<command-name>` …) или это сообщение другой сессии.
fn technical(text: &str) -> bool {
    let t = text.trim_start();
    (t.starts_with('<') && t[1..].starts_with(|c: char| c.is_ascii_lowercase())) || t.starts_with("Another Claude session sent a message")
}

/// «2026-10-18T12:57:03.123Z» → секунды Unix (UTC).
pub(crate) fn epoch(ts: &str) -> Option<u64> {
    let n = |r: std::ops::Range<usize>| ts.get(r)?.parse::<i64>().ok();
    let (y, m, d) = (n(0..4)?, n(5..7)?, n(8..10)?);
    let (hh, mm, ss) = (n(11..13)?, n(14..16)?, n(17..19)?);
    // дни от 1970-01-01 по григорианскому календарю (days_from_civil, Howard Hinnant)
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    u64::try_from(days * 86400 + hh * 3600 + mm * 60 + ss).ok()
}

fn line_title(line: &[u8]) -> Option<String> {
    if !(line.windows(7).any(|w| w == b"-title\"")) {
        return None;
    }
    let v: Value = serde_json::from_slice(line).ok()?;
    let key = match v["type"].as_str()? {
        "custom-title" => "customTitle",
        "ai-title" => "aiTitle",
        _ => return None,
    };
    v[key].as_str().filter(|t| !t.trim().is_empty()).map(str::to_string)
}

/// Время старта процесса — поле 22 /proc/<pid>/stat (такты с загрузки); None — процесса нет.
/// Имя процесса в скобках бывает с пробелами — поля считаются после последней «)».
pub fn proc_start(pid: u32) -> Option<String> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    stat.rsplit_once(')')?.1.split_whitespace().nth(19).map(str::to_string)
}

fn folder(cwd: &str) -> String {
    Path::new(cwd).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Первое предложение (до . ! ? … перед пробелом или концом: «1.2», «main.rs» — не конец), не длиннее STATUS_MAX символов.
fn first_sentence(text: &str) -> String {
    let t = text.trim();
    let end = t
        .char_indices()
        .find(|&(i, c)| ".!?…".contains(c) && t[i + c.len_utf8()..].chars().next().is_none_or(char::is_whitespace))
        .map_or(t.len(), |(i, c)| i + c.len_utf8());
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
            let transcript = l.transcript.to_string_lossy().into_owned();
            if let Some(e) = self.entries.iter_mut().find(|e| e.id == l.id) {
                (e.title, e.project, e.cwd, e.transcript) = (l.title, folder(&l.cwd), l.cwd, transcript);
                // после хуков файл главнее, только если обновлён позже последнего события: разрешение одобрено
                // в терминале и Esc хуков не шлют; «простаивает» не стирает «готово»
                let state = if l.state == State::Idle && e.state == State::Done { State::Done } else { l.state };
                if (!e.heard || l.status_at > e.since) && e.state != state {
                    (e.state, e.since) = (state, now);
                }
            } else {
                changed = true;
                self.entries.push(Entry {
                    id: l.id, agent: l.agent, title: l.title, project: folder(&l.cwd), cwd: l.cwd, transcript,
                    state: l.state, status: String::new(), since: now, focused: false, heard: false,
                    // агент впервые на глазах (и после перезапуска демона): лента начинается с его истории
                    feed: if l.agent == "claude" { history(&l.transcript, &images_dir()) } else { VecDeque::new() },
                });
            }
        }
        changed
    }

    /// Единственный агент, если он один.
    pub fn only(&self) -> Option<&Entry> {
        if let [e] = self.entries.as_slice() { Some(e) } else { None }
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
        if text.is_empty() {
            return true;
        }
        if e.feed.len() == FEED_MAX {
            e.feed.pop_front();
        }
        e.feed.push_back(Line { at: now, kind, text: text.chars().take(LINE_MAX).collect(), images: Vec::new() });
        true
    }

    /// Ход агента начат. Служебные вставки Claude Code под видом промпта (отчёт
    /// сабагента `<agent-message …>`, `<task-notification>` …) — не реплика
    /// пользователя: ход идёт, в ленту не пишем.
    /// Конец хода: лента — снова по транскрипту (живые события хуков теряют пересказ хода и пропускают
    /// служебные вставки, история их правит). Пустая история (файла нет) ленту не трогает. Последний ответ
    /// (`last`, из хука Stop) Claude Code к этому моменту часто ещё не дописал в файл — такой остаётся в конце.
    pub fn reload(&mut self, id: &str, last: &str) {
        let Some(e) = self.entries.iter_mut().find(|e| e.id == id && e.agent == "claude") else { return };
        let mut h = history(Path::new(&e.transcript), &images_dir());
        if h.is_empty() {
            return;
        }
        let last = shown(last);
        if !last.is_empty() && h.back().is_none_or(|l| l.text != last) {
            if let Some(f) = e.feed.iter().rev().find(|l| l.kind == "final" && l.text == last) {
                if h.len() == FEED_MAX {
                    h.pop_front();
                }
                h.push_back(f.clone());
            }
        }
        e.feed = h;
    }

    pub fn prompt(&mut self, id: &str, text: &str, now: u64) -> bool {
        let text = if technical(text) { String::new() } else { shown(text) };
        self.event(id, "prompt", &text, State::Working, None, now)
    }

    pub fn status(&mut self, id: &str, text: &str, now: u64) -> bool {
        let text = &plain(text);
        self.event(id, "status", text, State::Working, Some(text.chars().take(LINE_MAX).collect()), now)
    }

    pub fn question(&mut self, id: &str, text: &str, now: u64) -> bool {
        let text = &plain(text);
        self.event(id, "question", text, State::Waiting, Some(text.chars().take(LINE_MAX).collect()), now)
    }

    pub fn finished(&mut self, id: &str, text: &str, now: u64) -> bool {
        self.event(id, "final", &shown(text), State::Done, Some(first_sentence(&plain(text))), now)
    }

    /// Пересказ Омы — строка «что сейчас» в окне; в ленту не пишется: там дословный текст агента.
    pub fn summary(&mut self, id: &str, text: &str) -> bool {
        let Some(e) = self.entries.iter_mut().find(|e| e.id == id) else { return false };
        e.status = text.to_string();
        true
    }

    /// Последний промпт пользователя из ленты; нет — пусто.
    pub fn last_prompt(&self, id: &str) -> String {
        self.get(id).and_then(|e| e.feed.iter().rev().find(|l| l.kind == "prompt")).map(|l| l.text.clone()).unwrap_or_default()
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
        let state = if busy { State::Working } else { State::Idle };
        Live { id: id.into(), agent: "claude", pid: 1, title: id.into(), cwd: format!("/p/{id}"), transcript: PathBuf::new(), state, status_at: 0 }
    }

    #[test]
    fn first_sentence_ends_only_before_space_or_end() {
        assert_eq!(first_sentence("Версия 1.2 готова. Дальше"), "Версия 1.2 готова.");
        assert_eq!(first_sentence("Готово... Дальше"), "Готово...");
        assert_eq!(first_sentence("Правлю main.rs"), "Правлю main.rs");
    }

    #[test]
    fn session_file_status_followed_when_newer_than_hooks() {
        let f = |id: &str, state, at| Live { state, status_at: at, ..live(id, false) };
        let state = |r: &Registry, id| r.get(id).unwrap().state;
        let mut r = Registry::new();
        // демон перезапущен: хуков ещё не было — состояние из файла
        r.sync(vec![f("a", State::Waiting, 90), f("b", State::Working, 90), f("c", State::Idle, 90)], 100);
        assert_eq!([state(&r, "a"), state(&r, "b"), state(&r, "c")], [State::Waiting, State::Working, State::Idle]);
        r.sync(vec![f("a", State::Idle, 95)], 101);
        assert_eq!(state(&r, "a"), State::Idle);
        r.question("a", "Клоду нужно разрешение на команду", 110);
        r.sync(vec![f("a", State::Working, 105)], 111); // файл старше хука — хук главнее
        assert_eq!(state(&r, "a"), State::Waiting);
        r.sync(vec![f("a", State::Working, 120)], 121); // разрешение одобрено в терминале: хука нет
        assert_eq!((state(&r, "a"), r.get("a").unwrap().since), (State::Working, 121));
        r.sync(vec![f("a", State::Waiting, 125)], 126); // снова спросил
        assert_eq!(state(&r, "a"), State::Waiting);
        r.sync(vec![f("a", State::Idle, 130)], 131); // Esc: ни Stop, ни ответа
        assert_eq!(state(&r, "a"), State::Idle);
        r.prompt("a", "дальше", 140);
        r.sync(vec![f("a", State::Idle, 150)], 151); // Esc посреди работы
        assert_eq!(state(&r, "a"), State::Idle);
        r.finished("a", "Готово.", 160);
        r.sync(vec![f("a", State::Idle, 161)], 162); // «готово» остаётся «готово»
        assert_eq!(state(&r, "a"), State::Done);
        r.sync(vec![f("a", State::Working, 170)], 171);
        assert_eq!(state(&r, "a"), State::Working);
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
        // pid переиспользован: у процесса с этим номером другое время старта
        std::fs::write(ss.join("5.json"), r#"{"pid":5,"sessionId":"e","cwd":"/w","kind":"interactive","entrypoint":"cli","procStart":"999"}"#).unwrap();
        std::fs::write(ss.join("6.json"), r#"{"pid":6,"sessionId":"f","cwd":"/w","kind":"interactive","entrypoint":"cli","procStart":"100"}"#).unwrap();
        std::fs::write(pp.join("-home-u-Work/a.jsonl"), "{\"type\":\"ai-title\",\"aiTitle\":\"Скилл GitGuardian\"}\n").unwrap();
        let got = scan_claude(&ss, &pp, |pid| (pid != 4).then(|| "100".to_string()));
        let mut ids: Vec<(String, String)> = got.iter().map(|l| (l.id.clone(), l.title.clone())).collect();
        ids.sort();
        assert_eq!(ids, [("a".into(), "Скилл GitGuardian".into()), ("b".into(), "Почтовый клиент".into()), ("f".into(), "w".into())]);
        assert!(proc_start(std::process::id()).is_some_and(|s| s.parse::<u64>().is_ok()));
        assert!(got.iter().find(|l| l.id == "a").unwrap().transcript.ends_with("-home-u-Work/a.jsonl"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn history_keeps_only_human_prompts_and_shown_agent_text() {
        let root = std::env::temp_dir().join(format!("oma-history-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let p = root.join("t.jsonl");
        let user = |c: &str, extra: &str| format!(r#"{{"type":"user","message":{{"role":"user","content":{c}}},"timestamp":"2026-10-18T13:14:36.268Z"{extra}}}"#);
        let lines = [
            user(r#""сделай орб""#, r#","origin":{"kind":"human"},"promptSource":"typed""#),
            user(r#""Base directory for this skill: …""#, r#","isMeta":true"#),
            user(r#""Another Claude session sent a message:
<agent-message from="a1">отчёт""#, r#","isMeta":true,"origin":{"kind":"peer"}"#),
            user(r#""<task-notification>готово</task-notification>""#, ""),
            user(r#""старый промпт""#, ""),
            user(r#"[{"type":"tool_result","content":"ok"}]"#, ""),
            user(r#"[{"type":"text","text":"[Image #3] это не мои"},{"type":"image"}]"#, r#","origin":{"kind":"human"}"#),
            r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":""}]},"thinkingDisplay":"updates"}"#.into(),
            r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"Демон в порядке."}]},"thinkingDisplay":"updates"}"#.into(),
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash"}]}}"#.into(),
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Готово."}]}}"#.into(),
            r#"{"type":"assistant","isSidechain":true,"message":{"content":[{"type":"text","text":"сабагент"}]}}"#.into(),
            r#"{"type":"attachment","attachment":{"type":"queued_command","prompt":"посреди хода","commandMode":"prompt","origin":{"kind":"human"}}}"#.into(),
            r#"{"type":"attachment","attachment":{"type":"queued_command","prompt":[{"type":"text","text":"с картинкой"},{"type":"image","source":{"type":"base64","media_type":"image/png","data":"iVBORw0K"}}],"origin":{"kind":"human"}}}"#.into(),
            r#"{"type":"attachment","attachment":{"type":"queued_command","prompt":"<task-notification>x</task-notification>","origin":{"kind":"task-notification"}}}"#.into(),
            r#"{"type":"attachment","attachment":{"type":"queued_command","prompt":"<agent-message from=\"a\">отчёт","origin":{"kind":"peer"},"isMeta":true}}"#.into(),
            r#"{"type":"api-request-blob","x":"user"}"#.into(),
        ];
        std::fs::write(&p, lines.join("\n") + "\n").unwrap();
        let pics = root.join("img");
        let all = history(&p, &pics);
        let h: Vec<(&str, String)> = all.iter().map(|l| (l.kind, l.text.clone())).collect();
        assert_eq!(h, [("prompt", "сделай орб".to_string()), ("prompt", "старый промпт".into()), ("prompt", "[Image #3] это не мои".into()),
            ("status", "Демон в порядке.".into()), ("status", "Готово.".into()), ("prompt", "посреди хода".into()), ("prompt", "с картинкой".into())]);
        assert_eq!(history_line(lines[0].as_bytes(), &pics).unwrap().at, 1_792_329_276);
        assert!(history(&root.join("нет.jsonl"), &pics).is_empty());
        // картинка промпта — файлом в кэше, тот же файл при повторном чтении
        let img = &all.back().unwrap().images;
        assert_eq!(img.len(), 1);
        assert_eq!(std::fs::read(&img[0]).unwrap(), b"\x89PNG\r\n");
        assert_eq!(&history(&p, &pics).back().unwrap().images, img);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn reload_keeps_the_last_answer_not_yet_in_the_transcript_and_markdown() {
        let root = std::env::temp_dir().join(format!("oma-reload-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let p = root.join("t.jsonl");
        std::fs::write(&p, r#"{"type":"user","message":{"content":"сделай"},"origin":{"kind":"human"}}"#.to_string() + "\n").unwrap();
        let mut r = Registry::new();
        let mut l = live("a", true);
        l.transcript = p.clone();
        r.sync(vec![l], 0);
        let answer = "Готово:\n\n- **один**\n- два ![схема](http://x/y.png)";
        r.finished("a", answer, 5);
        r.reload("a", answer); // ответа в файле ещё нет
        let a = r.get("a").unwrap();
        let got: Vec<(&str, &str)> = a.feed.iter().map(|l| (l.kind, l.text.as_str())).collect();
        assert_eq!(got, [("prompt", "сделай"), ("final", "Готово:\n\n- **один**\n- два схема")]);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn title_from_transcript() {
        let root = std::env::temp_dir().join(format!("oma-title-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let p = root.join("t.jsonl");
        let junk = "{\"type\":\"assistant\",\"x\":\"юникод\"}\n".repeat(10_000);
        let mut text = format!("{{\"type\":\"ai-title\",\"aiTitle\":\"Старое\"}}\n{junk}");
        text += &format!("{{\"type\":\"ai-title\",\"aiTitle\":\"Скилл GitGuardian\"}}\n{junk}");
        std::fs::write(&p, &text).unwrap();
        assert_eq!(session_title(&p).as_deref(), Some("Скилл GitGuardian"));
        text += "{\"type\":\"custom-title\",\"customTitle\":\"мой\"}\n";
        std::fs::write(&p, &text).unwrap();
        assert_eq!(session_title(&p).as_deref(), Some("мой"));
        std::fs::write(&p, &junk).unwrap();
        assert_eq!(session_title(&p), None);
        assert_eq!(session_title(&root.join("нет.jsonl")), None);
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
        let n = r.get("a").unwrap().feed.len();
        assert!(r.prompt("a", "<agent-message from=\"x\">\n[Subagent hand-back] отчёт", 901));
        assert!(r.prompt("a", "  <task-notification>готово</task-notification>", 902));
        let a = r.get("a").unwrap();
        assert_eq!((a.feed.len(), a.state, a.since), (n, State::Working, 902));
        assert!(r.prompt("a", "<3 спасибо", 903)); // не тег — реплика
        assert_eq!(r.get("a").unwrap().feed.len(), n + 1);
        assert!(!r.status("чужой", "текст", 1));
    }

    #[test]
    fn feed_capped_and_trimmed() {
        let mut r = Registry::new();
        r.sync(vec![live("a", true)], 0);
        for i in 0..25 { r.status("a", &format!("статус {i}"), i); }
        let a = r.get("a").unwrap();
        assert_eq!((a.feed.len(), a.feed.front().unwrap().text.as_str()), (FEED_MAX, "статус 5"));
        r.status("a", &"я".repeat(LINE_MAX + 100), 30);
        assert_eq!(r.get("a").unwrap().feed.back().unwrap().text.chars().count(), LINE_MAX);
    }

    #[test]
    fn summary_sets_status_without_feed_line() {
        let mut r = Registry::new();
        r.sync(vec![live("a", true)], 0);
        r.status("a", "Правлю орб.", 1);
        let n = r.get("a").unwrap().feed.len();
        assert!(r.summary("a", "Орб почти готов."));
        let a = r.get("a").unwrap();
        assert_eq!((a.status.as_str(), a.feed.len(), a.state), ("Орб почти готов.", n, State::Working));
        assert!(!r.summary("чужой", "текст"));
    }

    #[test]
    fn last_prompt_is_latest_prompt_line() {
        let mut r = Registry::new();
        r.sync(vec![live("a", true)], 0);
        assert_eq!(r.last_prompt("a"), "");
        r.prompt("a", "сделай орб", 1);
        r.prompt("a", "и пузыри", 2);
        r.status("a", "Делаю.", 3);
        assert_eq!(r.last_prompt("a"), "и пузыри");
        assert_eq!(r.last_prompt("чужой"), "");
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
