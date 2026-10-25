//! Сторож: раз в минуту — здоровье агентов и что делать, чтобы работа не стояла.
//! Чистая логика: факты на вход, действия на выход; терминалы, файлы и модель — у демона.

use crate::agents::State;
use crate::mux::{Job, Key, Pane, Step};
use crate::source::claude::ApiError;
use serde_json::Value;
use std::collections::HashMap;

/// Что Ома сделала в терминале агента — по этому `done` выбирает фразу пользователю.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum What {
    Interrupt,
    Kill,
    Resume,
    Wake,
    Continue,
    Nudge,
}

/// Агент глазами сторожа — снимок на этот тик.
#[derive(Clone, Debug)]
pub struct Facts {
    pub id: String,
    pub project: String,
    pub state: State,
    pub since: u64,
    pub status: String,
    pub active_at: u64,
    pub helpers: usize,
    pub call: Option<Value>,
    pub error: Option<ApiError>,
    pub pane: Option<Pane>,
    pub shell: Option<u32>,
    pub resume: String,
    pub pid: u32,
    /// ход кончился по транскрипту (конец хода, Esc, ошибка API): файл сессии бывает «busy» часами
    pub turn_over: bool,
    /// вызов в полёте у сабагента, которого ждёт агент (вызов Agent/Task)
    pub sub_call: Option<Value>,
}

/// Время и пороги тика, в секундах; idle — сколько пользователь не трогал ввод.
pub struct Ctx {
    pub now: u64,
    pub idle: u64,
    pub hung_after: u64,
    pub remind_after: u64,
}

#[derive(Debug)]
pub enum Act {
    Say { session: String, text: String, urgent: bool },
    Remind { session: String, text: String },
    Judge { session: String, text: String },
    Run(Job),
}

/// Ома перезапускает агента, сообщение — вторым аргументом командной строки.
pub const RESUME_PROMPT: &str = "Ты закрылся посреди хода, я перезапустила. Продолжай с того места, где остановился.";
const NET: &str = "Связь оборвалась посреди ответа. Продолжай с того места, где остановился.";
const WAKE_DOC: &str = "Лимит сброшен, продолжай по своей записи в доке.";
const WAKE: &str = "Лимит сброшен, продолжай с того места, где остановился.";

/// Пользователь трогал ввод недавнее этого — агента, скорее всего, закрыл он сам.
const USER_ACTIVE: u64 = 60;
/// После сброса лимита будим через столько секунд: счётчики сервера обновляются не мгновенно.
const WAKE_AFTER: u64 = 60;
/// Связь: после ошибки ждём столько, просим продолжить не чаще NET_TRIES раз за NET_WINDOW.
const NET_WAIT: u64 = 60;
const NET_TRIES: usize = 3;
const NET_WINDOW: u64 = 1800;
/// Больше стольких попыток поднять зависшего за час — зовём пользователя; попытка вышла из часа — пробуем снова.
const ATTEMPTS: usize = 3;
const ATTEMPTS_WINDOW: u64 = 3600;
/// Знаков ввода вызова в вопросе модели о зависании.
const JUDGE_INPUT: usize = 1500;
/// Ошибка API старше этого — история (демон перезапустили), не новость: окно лимита Claude — 5 ч.
const ERROR_FRESH: u64 = 6 * 3600;
/// Своё Ctrl+C считается причиной выхода агента столько секунд.
const KILL_WINDOW: u64 = 120;
/// Ждём возвращения перезапущенного агента столько секунд.
const RESUME_WAIT: u64 = 300;
/// Нерешённая беда пропавшего агента держит сигнал орба столько секунд.
const ALARM_KEEP: u64 = 1800;
/// Вызовы, которые ждут пользователя: уведомление могло потеряться, закрывать их Esc нельзя.
const ASKS_USER: &[&str] = &["AskUserQuestion", "ExitPlanMode"];

/// Ступень лестницы при зависании.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Ladder {
    #[default]
    None,
    Judging,
    Hung,
    Long(u64),
    Interrupted(u64),
    Killed(u64),
}

#[derive(Default)]
struct Track {
    project: String,
    active: u64, // active_at прошлого тика: сменился — агент ожил
    ladder: Ladder,
    said: bool,   // о нынешней тишине уже сказано (опасное, вне терминала, сдалась)
    nudged: u64,  // когда за эту тишину написали агенту про помощника; 0 — не писали
    gave_up: bool, // попытки за час кончились, пользователю сказано
    sub: bool,    // прерван ход с сабагентом
    attempts: Vec<u64>,
    killed_at: u64, // Ома нажала Ctrl+C: агент, закрывшийся вскоре, поднять и при пользователе за компьютером
    ended: bool,    // SessionEnd: агент вышел сам (закрыл пользователь), а не упал
    revived: u64,   // перезапущен Омой — до этого срока ждём его возвращения
    ours: bool,     // перезапуск — после своего Ctrl+C, не после падения
    alarm: bool,    // срочное, не решённое
    alarm_at: u64,
    health: String,
    err_at: u64, // ошибка API с этим временем уже разобрана
    net: Vec<u64>,
    net_said: bool,
    limited: Option<(u64, bool)>, // (сброс, встал по строке «Остановился по лимиту»)
    reminded: u64,
}

impl Track {
    /// Агент ожил: тишина и всё, что о ней сказано и сделано, позади.
    fn calm(&mut self) {
        (self.ladder, self.said, self.nudged, self.gave_up, self.alarm) = (Ladder::None, false, 0, false, false);
        self.health = self.limited.map(|(u, _)| limit_health(u)).unwrap_or_default();
    }
}

#[derive(Default)]
pub struct Watch {
    tracks: HashMap<String, Track>,
    last: HashMap<String, Facts>,
    limit_said: Option<u64>, // о кончившемся лимите с этим сбросом уже сказано
    hung_after: u64,
}

fn say(session: &str, text: String, urgent: bool) -> Vec<Act> {
    vec![Act::Say { session: session.into(), text, urgent }]
}

fn run(f: &Facts, pane: &Pane, what: What, steps: Vec<Step>) -> Vec<Act> {
    vec![Act::Run(Job { session: f.id.clone(), pane: pane.clone(), steps, what })]
}

/// «ЧЧ:ММ» по местному времени.
fn clock(epoch: u64) -> String {
    // SAFETY: localtime_r потокобезопасна, tm — локальный буфер
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let t = epoch as libc::time_t;
    unsafe { libc::localtime_r(&t, &mut tm) };
    format!("{:02}:{:02}", tm.tm_hour, tm.tm_min)
}

fn limit_health(until: u64) -> String {
    format!("лимит до {}", clock(until))
}

impl Watch {
    /// shell_idle — оболочка жива и стоит на приглашении; alive — процесс жив.
    pub fn check(&mut self, ctx: &Ctx, facts: Vec<Facts>, shell_idle: impl Fn(u32) -> bool, alive: impl Fn(u32) -> bool) -> Vec<Act> {
        self.hung_after = ctx.hung_after;
        let mut acts = Vec::new();
        let gone: Vec<Facts> = self.last.values().filter(|l| !facts.iter().any(|f| f.id == l.id)).cloned().collect();
        for l in gone {
            self.last.remove(&l.id);
            acts.extend(self.gone(ctx, &l, &shell_idle, &alive));
        }
        // пропавшие: ждём перезапущенного, держим нерешённую беду недолго
        let now = ctx.now;
        self.tracks.retain(|id, t| facts.iter().any(|f| &f.id == id) || now < t.revived || (t.alarm && now < t.alarm_at + ALARM_KEEP) || t.limited.is_some());
        for f in facts {
            let t = self.tracks.entry(f.id.clone()).or_default();
            t.project = f.project.clone();
            if f.active_at != t.active {
                t.active = f.active_at;
                t.calm();
            }
            acts.extend(agent(t, &mut self.limit_said, ctx, &f));
            self.last.insert(f.id.clone(), f);
        }
        acts
    }

    /// Агент исчез: посреди хода — поднять его в той же оболочке или сказать, что не вышло.
    fn gone(&mut self, ctx: &Ctx, l: &Facts, shell_idle: &impl Fn(u32) -> bool, alive: &impl Fn(u32) -> bool) -> Vec<Act> {
        let Some(mut t) = self.tracks.remove(&l.id) else { return vec![] };
        let ours = t.killed_at > 0 && ctx.now < t.killed_at + KILL_WINDOW;
        // процесс жив — сменилась сессия (/clear, /resume); вышел сам (SessionEnd) — закрыл пользователь
        if l.state != State::Working || l.turn_over || t.limited.is_some() || alive(l.pid) || (t.ended && !ours) {
            return vec![];
        }
        let p = &l.project;
        if ctx.idle < USER_ACTIVE && !ours {
            return say(&l.id, format!("{p}: агент закрылся посреди хода."), false);
        }
        match (&l.pane, l.shell) {
            (Some(pane), Some(sh)) if shell_idle(sh) => {
                (t.killed_at, t.ended, t.ours, t.revived) = (0, false, ours, ctx.now + RESUME_WAIT);
                t.attempts.push(ctx.now);
                self.tracks.insert(l.id.clone(), t); // для done и возвращения агента
                run(l, pane, What::Resume, vec![Step::Text(l.resume.clone())])
            }
            _ => say(&l.id, format!("{p}: агент закрылся посреди хода, а поднять его я не могу — он не в herdr или Orca."), true),
        }
    }

    /// Ответ модели на «завис ли»: не ждали его — опоздал, не в счёт.
    pub fn judged(&mut self, session: &str, hung: bool, now: u64) {
        if let Some(t) = self.tracks.get_mut(session).filter(|t| t.ladder == Ladder::Judging) {
            t.ladder = if hung { Ladder::Hung } else { Ladder::Long(now + self.hung_after) };
        }
    }

    /// Исход действия в терминале — фраза пользователю.
    pub fn done(&mut self, session: &str, what: What, err: Option<String>, now: u64) -> Option<Act> {
        let t = self.tracks.get_mut(session)?;
        let p = t.project.clone();
        if err.is_some() {
            (t.alarm, t.alarm_at, t.revived) = (true, now, 0);
            return Some(Act::Say { session: session.into(), text: format!("{p}: написать агенту не вышло — посмотри сам."), urgent: true });
        }
        let text = match what {
            What::Kill => {
                t.killed_at = now;
                return None;
            }
            What::Resume if t.ours => "агент завис — перезапустила.",
            What::Interrupt if t.sub => "сабагент завис — прервала, агент перезапустит.",
            What::Interrupt => "агент завис — прервала и попросила продолжить.",
            What::Resume => "агент закрылся посреди хода — перезапустила.",
            What::Wake => "лимит сброшен — разбудила агента.",
            What::Continue => "оборвалась связь — попросила агента продолжить.",
            What::Nudge => "помощник агента молчит — написала агенту.",
        };
        Some(Act::Say { session: session.into(), text: format!("{p}: {text}"), urgent: false })
    }

    /// SessionEnd: агент вышел сам — его исчезновение не падение.
    pub fn ended(&mut self, session: &str) {
        self.tracks.entry(session.to_string()).or_default().ended = true;
    }

    /// Пользователь дал агенту новую задачу: будить «по записи в доке» больше нечего.
    pub fn prompted(&mut self, session: &str) {
        if let Some(t) = self.tracks.get_mut(session) {
            if t.limited.take().is_some() {
                t.health.clear();
            }
        }
    }

    /// Агент встал по лимиту сам (строка «Остановился по лимиту»): разбудить после сброса.
    pub fn limited(&mut self, session: &str, until: u64) {
        let t = self.tracks.entry(session.to_string()).or_default();
        t.limited = Some((until, true));
        t.health = limit_health(until);
    }

    pub fn health(&self, session: &str) -> String {
        self.tracks.get(session).map(|t| t.health.clone()).unwrap_or_default()
    }

    /// Агентов со срочной нерешённой проблемой — сигнал орба.
    pub fn trouble(&self) -> usize {
        self.tracks.values().filter(|t| t.alarm).count()
    }
}

/// Правила для живого агента по порядку: ошибка API, пора будить, тишина, забытый вопрос.
fn agent(t: &mut Track, limit_said: &mut Option<u64>, ctx: &Ctx, f: &Facts) -> Vec<Act> {
    let (p, now) = (&f.project, ctx.now);
    // старая ошибка — история: демон перезапустили, агент давно стоит или уже ушёл дальше
    if let Some(e) = f.error.as_ref().filter(|e| e.at != t.err_at && now.saturating_sub(e.at) > ERROR_FRESH) {
        t.err_at = e.at;
    }
    if let Some(e) = f.error.as_ref().filter(|e| e.at != t.err_at) {
        match e.kind.as_str() {
            "rate_limit" => {
                t.err_at = e.at;
                if let Some(r) = e.resets_at {
                    t.limited = Some((r, false));
                    t.health = limit_health(r);
                }
                // лимит общий на аккаунт: одна фраза на все агенты; сброс уже прошёл — не новость, сразу будить
                let key = e.resets_at.unwrap_or(0);
                if *limit_said != Some(key) && !e.resets_at.is_some_and(|r| r <= now) {
                    *limit_said = Some(key);
                    let text = e.resets_at.map_or("Кончился лимит Claude.".into(), |r| format!("Кончился лимит Claude, сброс в {}.", clock(r)));
                    return say("limit", text, false);
                }
            }
            "server_error" => {
                if f.state != State::Working && now >= e.at + NET_WAIT {
                    t.err_at = e.at;
                    t.health = "нет связи".into();
                    t.net.retain(|&x| now.saturating_sub(x) < NET_WINDOW);
                    t.net_said &= !t.net.is_empty();
                    if t.net.len() < NET_TRIES {
                        t.net.push(now);
                        return match &f.pane {
                            Some(pane) => run(f, pane, What::Continue, vec![Step::Text(NET.into())]),
                            None => outside(t, f, now),
                        };
                    }
                    if !t.net_said {
                        (t.net_said, t.alarm, t.alarm_at) = (true, true, now);
                        return say(&f.id, format!("{p}: связь с Claude не держится — посмотри сам."), true);
                    }
                    return vec![];
                }
            }
            other => {
                t.err_at = e.at;
                (t.alarm, t.alarm_at, t.health) = (true, now, "ошибка API".into());
                let what = match other {
                    "invalid_request" => "запрос отклонён",
                    "authentication_failed" | "oauth_org_not_allowed" => "нужен вход в Claude",
                    _ => "неизвестная ошибка",
                };
                return say(&f.id, format!("{p}: ошибка API — {what}."), true);
            }
        }
    }
    if let Some((_, doc)) = t.limited.filter(|&(u, _)| now >= u + WAKE_AFTER && f.state != State::Working) {
        t.limited = None;
        t.health.clear();
        return match &f.pane {
            Some(pane) => run(f, pane, What::Wake, vec![Step::Text(if doc { WAKE_DOC } else { WAKE }.into())]),
            None => say(&f.id, format!("{p}: лимит сброшен — разбуди агента сам, он не в herdr или Orca."), false),
        };
    }
    let quiet = now.saturating_sub(f.active_at);
    // active_at 0 — ни транскрипта, ни хуков: судить о тишине не по чему; ход кончился без помощников — агент ждёт, а не висит
    let asks = f.call.as_ref().is_some_and(|c| ASKS_USER.contains(&c["name"].as_str().unwrap_or_default()));
    let running = !f.turn_over || f.helpers > 0;
    if f.state == State::Working && f.active_at > 0 && quiet >= ctx.hung_after && running && t.limited.is_none() && !asks {
        return hang(t, ctx, f, quiet);
    }
    if f.state == State::Waiting {
        if now.saturating_sub(f.since) >= ctx.remind_after && now.saturating_sub(t.reminded) >= ctx.remind_after {
            t.reminded = now;
            t.health = "ждёт ответа давно".into();
            return vec![Act::Remind { session: f.id.clone(), text: format!("{p} всё ещё ждёт ответа: {}", f.status) }];
        }
    } else {
        t.reminded = 0;
        if t.health == "ждёт ответа давно" {
            t.health.clear();
        }
    }
    vec![]
}

/// Агент не в herdr и не в Orca: сделать ничего нельзя — только сказать.
fn outside(t: &mut Track, f: &Facts, now: u64) -> Vec<Act> {
    (t.said, t.alarm, t.alarm_at) = (true, true, now);
    say(&f.id, format!("{}: агент завис, но он не в herdr или Orca — перезапусти сам.", f.project), true)
}

/// Попыток за час не осталось — сказать пользователю один раз; None — пробовать можно.
fn spent(t: &mut Track, f: &Facts, now: u64, what: &str) -> Option<Vec<Act>> {
    t.attempts.retain(|&x| now.saturating_sub(x) < ATTEMPTS_WINDOW);
    if t.attempts.len() < ATTEMPTS {
        return None;
    }
    if t.gave_up {
        return Some(vec![]);
    }
    (t.gave_up, t.alarm, t.alarm_at) = (true, true, now);
    let n = (t.attempts[0] + ATTEMPTS_WINDOW - now).div_ceil(60);
    Some(say(&f.id, format!("{}: {what}. Через {n} мин попробую снова.", f.project), true))
}

/// Работает, а событий нет `quiet` секунд: помощники, опасное, лестница Esc → Ctrl+C → перезапуск.
fn hang(t: &mut Track, ctx: &Ctx, f: &Facts, quiet: u64) -> Vec<Act> {
    let (p, now, n) = (&f.project, ctx.now, quiet / 60);
    if f.helpers > 0 {
        // помощник бывает сервером и молчит часами: писать агенту — только по вердикту модели «висит»
        match t.ladder {
            // просьба агента не разбудила — зовём пользователя, один раз
            _ if t.nudged > 0 => {
                if now.saturating_sub(t.nudged) < ctx.hung_after || t.said {
                    return vec![];
                }
                (t.said, t.alarm, t.alarm_at) = (true, true, now);
                return say(&f.id, format!("{p}: агент не ответил на просьбу проверить помощника — посмотри сам."), true);
            }
            Ladder::Hung => {
                if let Some(acts) = spent(t, f, now, "помощник агента снова молчит — посмотри сам") {
                    return acts;
                }
                t.nudged = now;
                t.attempts.push(now);
            }
            Ladder::Judging => return vec![],
            Ladder::Long(until) if now < until => return vec![],
            _ => {
                t.ladder = Ladder::Judging;
                let text = format!("Тишина {n} мин. Агент закончил ход и ждёт своих фоновых помощников ({}), они тоже молчат.", f.helpers);
                return vec![Act::Judge { session: f.id.clone(), text }];
            }
        }
        let text = format!("Твой фоновый помощник молчит {n} мин — похоже, завис. Проверь его: завис — останови и перезапусти.");
        return match &f.pane {
            Some(pane) => run(f, pane, What::Nudge, vec![Step::Text(text)]),
            None => outside(t, f, now),
        };
    }
    t.health = "завис".into();
    match t.ladder {
        Ladder::Interrupted(at) => {
            if now.saturating_sub(at) < ctx.hung_after / 2 {
                return vec![];
            }
            t.ladder = Ladder::Killed(now);
            let Some(pane) = &f.pane else { return outside(t, f, now) };
            return run(f, pane, What::Kill, vec![Step::Key(Key::CtrlC), Step::Pause(400), Step::Key(Key::CtrlC)]);
        }
        Ladder::Killed(at) => {
            if now.saturating_sub(at) < ctx.hung_after || t.said {
                return vec![];
            }
            (t.said, t.alarm, t.alarm_at) = (true, true, now);
            return say(&f.id, format!("{p}: агент снова завис — поднять не получилось, посмотри сам."), true);
        }
        _ => {}
    }
    if t.said {
        return vec![];
    }
    let name = f.call.as_ref().and_then(|c| c["name"].as_str()).unwrap_or_default();
    // висит сабагент — опасно то, что делает он
    if let Some(what) = f.sub_call.as_ref().filter(|_| matches!(name, "Agent" | "Task")).and_then(danger) {
        (t.said, t.alarm, t.alarm_at) = (true, true, now);
        let tool = f.sub_call.as_ref().and_then(|c| c["name"].as_str()).unwrap_or_default();
        let text = if what == "непонятный вызов" {
            format!("{p}: сабагент завис на вызове {tool} — не знаю, безопасно ли прервать. Посмотри сам.")
        } else {
            format!("{p}: сабагент завис посреди {what} — не трогаю, посмотри сам.")
        };
        return say(&f.id, text, true);
    }
    if let Some(what) = f.call.as_ref().and_then(danger) {
        (t.said, t.alarm, t.alarm_at) = (true, true, now);
        let text = if what == "непонятный вызов" {
            format!("{p}: агент завис на вызове {name} — не знаю, безопасно ли прервать. Посмотри сам.")
        } else {
            format!("{p}: агент завис посреди {what} — не трогаю, посмотри сам.")
        };
        return say(&f.id, text, true);
    }
    if let Some(acts) = spent(t, f, now, "агент снова завис — поднять не получилось, посмотри сам") {
        return acts;
    }
    let sub = matches!(name, "Agent" | "Task");
    // без модели и при долгой тишине — завис; сабагент, молчащий вместе с родителем, — тоже
    if !(sub || quiet >= 3 * ctx.hung_after || t.ladder == Ladder::Hung) {
        match t.ladder {
            Ladder::Judging => return vec![],
            Ladder::Long(until) if now < until => return vec![],
            _ => {
                t.ladder = Ladder::Judging;
                let call = f.call.as_ref().map_or("вызова инструмента нет: агент ждёт ответа модели".to_string(), |c| {
                    format!("{name} {}", c["input"].to_string().chars().take(JUDGE_INPUT).collect::<String>())
                });
                return vec![Act::Judge { session: f.id.clone(), text: format!("Тишина {n} мин. Вызов: {call}.") }];
            }
        }
    }
    // вне мультиплексора — только сказать, и лишь когда зависание подтверждено
    let Some(pane) = &f.pane else { return outside(t, f, now) };
    t.attempts.push(now);
    (t.sub, t.ladder) = (sub, Ladder::Interrupted(now));
    let text = if sub {
        format!("Сабагент висел {n} мин, я прервала ход. Перезапусти его, если он ещё нужен.")
    } else {
        format!("Ход завис на {n} мин, я прервала его. Продолжай с того места, где остановился.")
    };
    run(f, pane, What::Interrupt, vec![Step::Key(Key::Esc), Step::Pause(1500), Step::Text(text)])
}

/// Инструменты, прерывать которые безопасно: чтение, поиск, правка файлов под git, помощники, ожидание.
const SAFE_TOOLS: &[&str] = &[
    "Read", "Grep", "Glob", "LS", "WebFetch", "WebSearch", "Agent", "Task", "Skill", "TodoWrite", "ToolSearch", "Monitor",
    "Edit", "Write", "MultiEdit", "NotebookEdit", "BashOutput", "KillShell", "TaskOutput", "TaskStop", "TaskCreate",
    "TaskUpdate", "TaskList", "TaskGet", "AskUserQuestion", "ExitPlanMode", "EnterPlanMode", "SendMessage", "Workflow", "LSP",
];
/// Начала имён инструментов MCP только для чтения.
const READ_PREFIXES: &[&str] = &["get", "list", "read", "search", "find", "query", "fetch", "describe", "show"];

/// Что опасного в вызове — его прерывать нельзя без пользователя; непонятное — тоже.
pub fn danger(call: &Value) -> Option<&'static str> {
    let name = call["name"].as_str().unwrap_or_default();
    if name == "Bash" {
        return bash_danger(call["input"]["command"].as_str().unwrap_or_default());
    }
    if let Some(tool) = name.strip_prefix("mcp__") {
        let last = tool.rsplit("__").next().unwrap_or(tool).to_lowercase();
        return (!READ_PREFIXES.iter().any(|p| last.starts_with(p))).then_some("непонятный вызов");
    }
    (!SAFE_TOOLS.contains(&name)).then_some("непонятный вызов")
}

fn bash_danger(command: &str) -> Option<&'static str> {
    let l = command.to_lowercase();
    let words: Vec<&str> = l.split(|c: char| c.is_whitespace() || ";&|()`".contains(c)).filter(|w| !w.is_empty()).collect();
    let has = |s: &str| l.contains(s);
    if has("git push") {
        Some("push")
    } else if words.iter().any(|w| matches!(*w, "rm" | "rmdir" | "shred" | "unlink")) || has("git clean") || has("kubectl delete") {
        Some("удаление")
    } else if has("git reset") {
        Some("сброс git")
    } else if has("migrat") {
        Some("миграция")
    } else if ["deploy", "kubectl apply", "terraform apply", "helm upgrade", "helm install"].iter().any(|k| has(k)) {
        Some("деплой")
    } else if ["insert into", "delete from", "drop table", "drop database", "truncate", "alter table"].iter().any(|k| has(k)) || (has("update ") && has(" set ")) {
        Some("запись в базу")
    } else if has("--force") || (words.contains(&"-f") && has("git ")) {
        Some("force")
    } else {
        None
    }
}

/// Флаги, которые переживают перезапуск: без значения и со значением. Прочее (промпт, --resume, конфиги) — нет.
const CLAUDE_FLAGS: &[&str] = &["--dangerously-skip-permissions", "--allow-dangerously-skip-permissions", "--chrome", "--verbose"];
const CLAUDE_VALUED: &[&str] = &["--permission-mode", "--model", "--effort", "--add-dir"];
const CODEX_FLAGS: &[&str] = &["--yolo", "--dangerously-bypass-approvals-and-sandbox", "--search"];
const CODEX_VALUED: &[&str] = &["-m", "--model", "-s", "--sandbox", "-a", "--ask-for-approval"];

fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Команда оболочки, возобновляющая сессию агента с флагами пользователя и сообщением prompt.
pub fn resume_cmd(agent: &str, id: &str, argv: &[String], prompt: &str) -> String {
    let codex = agent == "codex";
    let (flags, valued) = if codex { (CODEX_FLAGS, CODEX_VALUED) } else { (CLAUDE_FLAGS, CLAUDE_VALUED) };
    let prog = argv.first().filter(|p| p.starts_with('/') && std::path::Path::new(p).exists()).map_or(if codex { "codex" } else { "claude" }, String::as_str);
    let mut kept: Vec<&str> = Vec::new();
    let mut args = argv.iter().skip(1);
    while let Some(a) = args.next() {
        let key = a.split_once('=').map_or(a.as_str(), |(k, _)| k);
        if flags.contains(&a.as_str()) || (a.contains('=') && valued.contains(&key)) {
            kept.push(a);
        } else if valued.contains(&a.as_str()) {
            if let Some(v) = args.next() {
                kept.extend([a.as_str(), v.as_str()]);
            }
        }
    }
    let mut out = vec![prog];
    if codex {
        out.extend(["resume", id]);
        out.extend(kept);
    } else {
        out.extend(kept);
        out.extend(["--resume", id]);
    }
    out.push(prompt);
    out.into_iter().map(quote).collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn f(id: &str) -> Facts {
        Facts {
            id: id.into(), project: "p".into(), state: State::Working, since: 0, status: String::new(), active_at: 1,
            helpers: 0, call: None, error: None, pane: Some(Pane { mux: "herdr", id: "w1:p1".into(), via: "/s".into() }), shell: Some(1),
            resume: "claude --resume id".into(), pid: 7, turn_over: false, sub_call: None,
        }
    }
    fn ctx(now: u64, idle: u64) -> Ctx {
        Ctx { now, idle, hung_after: 720, remind_after: 900 }
    }
    fn run_of(a: &Act) -> Option<&Job> {
        if let Act::Run(j) = a { Some(j) } else { None }
    }
    fn text_of(j: &Job) -> &str {
        j.steps.iter().find_map(|s| if let Step::Text(t) = s { Some(t.as_str()) } else { None }).unwrap_or_default()
    }

    #[test]
    fn crash_while_user_away_resumes_in_its_shell() {
        let mut w = Watch::default();
        assert!(w.check(&ctx(100, 0), vec![f("a")], |_| true, |_| false).is_empty());
        let acts = w.check(&ctx(160, 120), vec![], |_| true, |_| false);
        let j = run_of(&acts[0]).unwrap();
        assert_eq!((acts.len(), j.what, &j.steps[..]), (1, What::Resume, &[Step::Text("claude --resume id".into())][..]));
    }

    #[test]
    fn crash_while_user_active_only_reports() {
        let mut w = Watch::default();
        w.check(&ctx(100, 0), vec![f("a")], |_| true, |_| false);
        let acts = w.check(&ctx(110, 5), vec![], |_| true, |_| false);
        assert!(matches!(&acts[..], [Act::Say { urgent: false, text, .. }] if text == "p: агент закрылся посреди хода."), "{acts:?}");
    }

    #[test]
    fn crash_without_shell_says_it_cannot_raise() {
        let mut w = Watch::default();
        w.check(&ctx(100, 0), vec![f("a"), Facts { state: State::Idle, ..f("b") }], |_| false, |_| false);
        let acts = w.check(&ctx(200, 999), vec![], |_| false, |_| false);
        assert!(matches!(&acts[..], [Act::Say { urgent: true, text, .. }] if text.contains("не в herdr или Orca")), "закрытый в покое агент — не новость: {acts:?}");
    }

    #[test]
    fn own_kill_resumes_even_when_user_is_here() {
        let mut w = Watch::default();
        assert!(matches!(&w.check(&ctx(800, 0), vec![f("a")], |_| true, |_| false)[..], [Act::Judge { .. }]));
        w.judged("a", true, 805);
        assert_eq!(run_of(&w.check(&ctx(860, 0), vec![f("a")], |_| true, |_| false)[0]).unwrap().what, What::Interrupt);
        assert!(w.done("a", What::Interrupt, None, 861).is_some());
        assert_eq!(run_of(&w.check(&ctx(1221, 0), vec![f("a")], |_| true, |_| false)[0]).unwrap().what, What::Kill);
        assert!(w.done("a", What::Kill, None, 1222).is_none());
        assert_eq!(run_of(&w.check(&ctx(1280, 0), vec![], |_| true, |_| false)[0]).unwrap().what, What::Resume);
    }

    #[test]
    fn dangerous_call_is_never_interrupted() {
        let mut w = Watch::default();
        let a = Facts { call: Some(json!({"name": "Bash", "input": {"command": "git push origin main"}})), ..f("a") };
        let acts = w.check(&ctx(800, 999), vec![a.clone()], |_| true, |_| false);
        assert!(matches!(&acts[..], [Act::Say { urgent: true, text, .. }] if text == "p: агент завис посреди push — не трогаю, посмотри сам."), "{acts:?}");
        assert!(w.check(&ctx(900, 999), vec![a], |_| true, |_| false).is_empty(), "один раз");
        assert_eq!((w.trouble(), w.health("a").as_str()), (1, "завис"));
    }

    #[test]
    fn uncertain_call_asks_the_user() {
        let mut w = Watch::default();
        let a = Facts { call: Some(json!({"name": "mcp__db__delete_row", "input": {}})), ..f("a") };
        let acts = w.check(&ctx(800, 999), vec![a], |_| true, |_| false);
        assert!(matches!(&acts[..], [Act::Say { urgent: true, text, .. }] if text.contains("delete_row") && text.contains("безопасно ли прервать")), "{acts:?}");
    }

    #[test]
    fn hung_asks_model_then_interrupts_then_kills() {
        let mut w = Watch::default();
        let a = Facts { call: Some(json!({"name": "mcp__x__get_page", "input": {"url": "u"}})), ..f("a") };
        let acts = w.check(&ctx(800, 999), vec![a.clone()], |_| true, |_| false);
        assert!(matches!(&acts[..], [Act::Judge { session, text }] if session == "a" && text.contains("13 мин") && text.contains("mcp__x__get_page")), "{acts:?}");
        assert!(w.check(&ctx(830, 999), vec![a.clone()], |_| true, |_| false).is_empty(), "ждём ответа модели");
        w.judged("a", true, 810);
        let acts = w.check(&ctx(860, 999), vec![a.clone()], |_| true, |_| false);
        let j = run_of(&acts[0]).unwrap();
        assert_eq!((j.what, &j.steps[..2]), (What::Interrupt, &[Step::Key(Key::Esc), Step::Pause(1500)][..]));
        assert!(text_of(j).starts_with("Ход завис на 14 мин, я прервала его."), "{}", text_of(j));
        assert!(matches!(w.done("a", What::Interrupt, None, 861), Some(Act::Say { urgent: false, text, .. }) if text == "p: агент завис — прервала и попросила продолжить."));
        assert!(w.check(&ctx(1000, 999), vec![a.clone()], |_| true, |_| false).is_empty());
        assert_eq!(run_of(&w.check(&ctx(861 + 360, 999), vec![a], |_| true, |_| false)[0]).unwrap().what, What::Kill);
    }

    #[test]
    fn long_by_nature_waits_then_hard_cap() {
        let mut w = Watch::default();
        let a = Facts { call: Some(json!({"name": "Bash", "input": {"command": "cargo build --release"}})), ..f("a") };
        assert!(matches!(&w.check(&ctx(800, 999), vec![a.clone()], |_| true, |_| false)[..], [Act::Judge { .. }]));
        w.judged("a", false, 805);
        assert!(w.check(&ctx(1000, 999), vec![a.clone()], |_| true, |_| false).is_empty());
        assert!(matches!(&w.check(&ctx(1600, 999), vec![a.clone()], |_| true, |_| false)[..], [Act::Judge { .. }]), "срок «долгой» вышел — спросить снова");
        assert_eq!(run_of(&w.check(&ctx(3 * 720 + 1, 999), vec![a], |_| true, |_| false)[0]).unwrap().what, What::Interrupt);
    }

    #[test]
    fn activity_after_action_resets_ladder() {
        let mut w = Watch::default();
        let a = Facts { call: Some(json!({"name": "Agent", "input": {}})), ..f("a") };
        assert_eq!(run_of(&w.check(&ctx(800, 999), vec![a.clone()], |_| true, |_| false)[0]).unwrap().what, What::Interrupt);
        w.done("a", What::Interrupt, None, 801);
        assert!(w.check(&ctx(1000, 999), vec![Facts { active_at: 900, ..a }], |_| true, |_| false).is_empty());
        assert_eq!((w.trouble(), w.health("a").as_str()), (0, ""));
    }

    #[test]
    fn three_attempts_an_hour_then_waits_for_the_window() {
        let mut w = Watch::default();
        let mut t = 1;
        for i in 0..3 {
            let a = Facts { call: Some(json!({"name": "Agent", "input": {}})), active_at: t, ..f("a") };
            assert_eq!(run_of(&w.check(&ctx(t + 800, 999), vec![a], |_| true, |_| false)[0]).unwrap().what, What::Interrupt, "попытка {i}");
            t += 801;
        }
        let a = Facts { call: Some(json!({"name": "Agent", "input": {}})), active_at: t, ..f("a") };
        let acts = w.check(&ctx(t + 800, 999), vec![a.clone()], |_| true, |_| false);
        assert!(matches!(&acts[..], [Act::Say { urgent: true, text, .. }] if text == "p: агент снова завис — поднять не получилось, посмотри сам. Через 20 мин попробую снова."), "{acts:?}");
        assert_eq!(w.trouble(), 1);
        assert!(w.check(&ctx(4400, 999), vec![a.clone()], |_| true, |_| false).is_empty(), "сказано один раз, попытки ещё в окне");
        assert_eq!(run_of(&w.check(&ctx(4401, 999), vec![a], |_| true, |_| false)[0]).unwrap().what, What::Interrupt, "первая попытка вышла из часа — не сдаёмся");
    }

    #[test]
    fn quiet_helpers_nudge_parent_not_interrupt() {
        let mut w = Watch::default();
        let a = Facts { helpers: 2, ..f("a") };
        let acts = w.check(&ctx(800, 0), vec![a.clone()], |_| true, |_| false);
        assert!(matches!(&acts[..], [Act::Judge { text, .. }] if text.contains("фоновых помощников")), "сначала — модель: помощник-сервер молчит часами: {acts:?}");
        w.judged("a", true, 805);
        let acts = w.check(&ctx(860, 0), vec![a.clone()], |_| true, |_| false);
        let j = run_of(&acts[0]).unwrap();
        assert_eq!((j.what, &j.steps[..]), (What::Nudge, &[Step::Text("Твой фоновый помощник молчит 14 мин — похоже, завис. Проверь его: завис — останови и перезапусти.".into())][..]));
        assert!(w.check(&ctx(900, 0), vec![a], |_| true, |_| false).is_empty());
        assert!(matches!(w.done("a", What::Nudge, None, 901), Some(Act::Say { text, .. }) if text == "p: помощник агента молчит — написала агенту."));
    }

    #[test]
    fn long_helpers_are_left_alone() {
        let mut w = Watch::default();
        let a = Facts { helpers: 1, ..f("a") };
        w.check(&ctx(800, 0), vec![a.clone()], |_| true, |_| false);
        w.judged("a", false, 805); // помощник держит сервер — так и задумано
        assert!(w.check(&ctx(1500, 0), vec![a.clone()], |_| true, |_| false).is_empty());
        assert!(w.check(&ctx(4 * 720, 0), vec![a], |_| true, |_| false).iter().all(|a| !matches!(a, Act::Run(_))), "без вердикта «висит» помощникам не пишем");
    }

    #[test]
    fn quiet_subagent_interrupted_without_model() {
        let mut w = Watch::default();
        let a = Facts { call: Some(json!({"name": "Agent", "input": {"description": "ревью"}})), ..f("a") };
        let acts = w.check(&ctx(800, 999), vec![a], |_| true, |_| false);
        let j = run_of(&acts[0]).unwrap();
        assert_eq!(text_of(j), "Сабагент висел 13 мин, я прервала ход. Перезапусти его, если он ещё нужен.");
        assert!(matches!(w.done("a", What::Interrupt, None, 801), Some(Act::Say { text, .. }) if text == "p: сабагент завис — прервала, агент перезапустит."));
    }

    #[test]
    fn rate_limit_said_once_per_reset_and_wakes_after() {
        let mut w = Watch::default();
        let err = Some(ApiError { kind: "rate_limit".into(), at: 100, resets_at: Some(5000) });
        let five: Vec<Facts> = (0..5).map(|i| Facts { state: State::Idle, active_at: 100, error: err.clone(), ..f(&format!("a{i}")) }).collect();
        let says = w.check(&ctx(110, 0), five.clone(), |_| true, |_| false);
        assert_eq!(says.len(), 1, "{says:?}");
        assert!(matches!(&says[0], Act::Say { session, urgent: false, text } if session == "limit" && text.starts_with("Кончился лимит Claude, сброс в ")));
        assert!(w.health("a0").starts_with("лимит до "));
        assert!(w.check(&ctx(5030, 0), five.clone(), |_| true, |_| false).is_empty(), "сброс + 60 с ещё не настал");
        let wakes = w.check(&ctx(5061, 0), five.clone(), |_| true, |_| false);
        assert_eq!(wakes.iter().filter_map(run_of).filter(|j| j.what == What::Wake && text_of(j) == "Лимит сброшен, продолжай с того места, где остановился.").count(), 5);
        assert!(w.check(&ctx(5200, 0), five, |_| true, |_| false).is_empty(), "будим один раз");
        assert!(matches!(w.done("a0", What::Wake, None, 5201), Some(Act::Say { text, .. }) if text == "p: лимит сброшен — разбудила агента."));
    }

    #[test]
    fn limited_by_stop_line_wakes_with_doc_phrase() {
        let mut w = Watch::default();
        let a = Facts { state: State::Done, active_at: 100, ..f("a") };
        w.check(&ctx(110, 0), vec![a.clone()], |_| true, |_| false);
        w.limited("a", 5000);
        let acts = w.check(&ctx(5061, 0), vec![a.clone()], |_| true, |_| false);
        assert_eq!(text_of(run_of(&acts[0]).unwrap()), "Лимит сброшен, продолжай по своей записи в доке.");
        let mut w = Watch::default();
        w.limited("b", 5000);
        let acts = w.check(&ctx(5061, 0), vec![Facts { pane: None, ..a }.clone_as("b")], |_| true, |_| false);
        assert!(matches!(&acts[..], [Act::Say { urgent: false, text, .. }] if text == "p: лимит сброшен — разбуди агента сам, он не в herdr или Orca."), "{acts:?}");
    }

    #[test]
    fn connection_lost_retries_three_times_then_reports() {
        let mut w = Watch::default();
        let lost = |at| Facts { state: State::Idle, active_at: at, error: Some(ApiError { kind: "server_error".into(), at, resets_at: None }), ..f("a") };
        assert!(w.check(&ctx(130, 0), vec![lost(100)], |_| true, |_| false).is_empty(), "ошибке меньше минуты");
        for at in [100, 300, 500] {
            let acts = w.check(&ctx(at + 100, 0), vec![lost(at)], |_| true, |_| false);
            let j = run_of(&acts[0]).unwrap();
            assert_eq!((j.what, text_of(j)), (What::Continue, "Связь оборвалась посреди ответа. Продолжай с того места, где остановился."));
            assert!(w.check(&ctx(at + 150, 0), vec![lost(at)], |_| true, |_| false).is_empty(), "та же ошибка — одна просьба");
        }
        let acts = w.check(&ctx(800, 0), vec![lost(700)], |_| true, |_| false);
        assert!(matches!(&acts[..], [Act::Say { urgent: true, text, .. }] if text == "p: связь с Claude не держится — посмотри сам."), "{acts:?}");
        assert!(w.check(&ctx(1000, 0), vec![lost(900)], |_| true, |_| false).is_empty(), "сказано один раз");
    }

    #[test]
    fn other_api_error_reported_once() {
        let mut w = Watch::default();
        let a = Facts { state: State::Idle, active_at: 100, error: Some(ApiError { kind: "authentication_failed".into(), at: 100, resets_at: None }), ..f("a") };
        let acts = w.check(&ctx(110, 0), vec![a.clone()], |_| true, |_| false);
        assert!(matches!(&acts[..], [Act::Say { urgent: true, text, .. }] if text == "p: ошибка API — нужен вход в Claude."), "{acts:?}");
        assert!(w.check(&ctx(200, 0), vec![a], |_| true, |_| false).is_empty());
        assert_eq!((w.trouble(), w.health("a").as_str()), (1, "ошибка API"));
    }

    #[test]
    fn forgotten_question_reminded_every_interval() {
        let mut w = Watch::default();
        let q = Facts { state: State::Waiting, since: 0, status: "что с мёртвым кодом?".into(), ..f("a") };
        assert!(w.check(&ctx(800, 0), vec![q.clone()], |_| true, |_| false).is_empty());
        let acts = w.check(&ctx(900, 0), vec![q.clone()], |_| true, |_| false);
        assert!(matches!(&acts[..], [Act::Remind { text, .. }] if text == "p всё ещё ждёт ответа: что с мёртвым кодом?"), "{acts:?}");
        assert!(w.check(&ctx(1000, 0), vec![q.clone()], |_| true, |_| false).is_empty());
        assert_eq!(w.check(&ctx(1800, 0), vec![q], |_| true, |_| false).len(), 1);
    }

    #[test]
    fn outside_mux_says_restart_yourself() {
        let mut w = Watch::default();
        let a = Facts { pane: None, call: Some(json!({"name": "Agent", "input": {}})), ..f("a") };
        let acts = w.check(&ctx(800, 999), vec![a], |_| true, |_| false);
        assert!(matches!(&acts[..], [Act::Say { urgent: true, text, .. }] if text == "p: агент завис, но он не в herdr или Orca — перезапусти сам."), "{acts:?}");
    }

    #[test]
    fn outside_mux_asks_model_before_saying_hung() {
        let mut w = Watch::default();
        let a = Facts { pane: None, call: Some(json!({"name": "Bash", "input": {"command": "cargo build --release"}})), ..f("a") };
        assert!(matches!(&w.check(&ctx(800, 999), vec![a.clone()], |_| true, |_| false)[..], [Act::Judge { .. }]), "долгая сборка вне мультиплексора — сперва модель");
        w.judged("a", false, 805);
        assert!(w.check(&ctx(900, 999), vec![a.clone()], |_| true, |_| false).is_empty(), "работа долгая — молчим");
        assert!(matches!(&w.check(&ctx(1600, 999), vec![a.clone()], |_| true, |_| false)[..], [Act::Judge { .. }]));
        w.judged("a", true, 1605);
        let acts = w.check(&ctx(1660, 999), vec![a], |_| true, |_| false);
        assert!(matches!(&acts[..], [Act::Say { urgent: true, text, .. }] if text == "p: агент завис, но он не в herdr или Orca — перезапусти сам."), "{acts:?}");
    }

    #[test]
    fn failed_action_is_urgent() {
        let mut w = Watch::default();
        w.check(&ctx(800, 999), vec![Facts { call: Some(json!({"name": "Agent", "input": {}})), ..f("a") }], |_| true, |_| false);
        assert!(matches!(w.done("a", What::Interrupt, Some("таймаут".into()), 801), Some(Act::Say { urgent: true, text, .. }) if text == "p: написать агенту не вышло — посмотри сам."));
        assert_eq!(w.trouble(), 1);
    }

    #[test]
    fn danger_table() {
        let b = |c: &str| danger(&json!({"name": "Bash", "input": {"command": c}}));
        assert_eq!((b("git push -f"), b("rm -rf target"), b("npx prisma migrate deploy"), b("cargo test")), (Some("push"), Some("удаление"), Some("миграция"), None));
        assert_eq!((b("git reset --hard HEAD~1"), b("kubectl apply -f x.yaml"), b("psql -c 'DELETE FROM users'"), b("cp a b && git commit --force")), (Some("сброс git"), Some("деплой"), Some("запись в базу"), Some("force")));
        assert_eq!(b("grep -rn transform src"), None, "rm внутри слова — не удаление");
        assert_eq!(danger(&json!({"name": "mcp__db__delete_row", "input": {}})), Some("непонятный вызов"));
        assert_eq!(danger(&json!({"name": "mcp__gh__get_issue", "input": {}})), None);
        assert_eq!((danger(&json!({"name": "Read", "input": {}})), danger(&json!({"name": "Frobnicate", "input": {}}))), (None, Some("непонятный вызов")));
    }

    #[test]
    fn resume_keeps_only_known_flags() {
        let argv: Vec<String> = ["/bin/sh", "--dangerously-skip-permissions", "--model", "opus", "сделай фичу", "--resume", "old", "--mcp-config", "x.json", "--effort=high"].map(String::from).to_vec();
        assert_eq!(resume_cmd("claude", "id1", &argv, "Продолжай."), "'/bin/sh' '--dangerously-skip-permissions' '--model' 'opus' '--effort=high' '--resume' 'id1' 'Продолжай.'");
        assert_eq!(resume_cmd("claude", "id1", &["/nonexistent/claude".into()], "x"), "'claude' '--resume' 'id1' 'x'");
        assert_eq!(resume_cmd("codex", "t1", &["codex".into(), "--yolo".into()], "Дальше. Это 'важно'"), "'codex' 'resume' 't1' '--yolo' 'Дальше. Это '\\''важно'\\'''");
    }

    #[test]
    fn unknown_activity_is_not_hung() {
        let mut w = Watch::default();
        let a = Facts { active_at: 0, call: Some(json!({"name": "Agent", "input": {}})), ..f("a") };
        let mut w2 = Watch::default();
        assert!(!w2.check(&ctx(800, 999), vec![Facts { active_at: 1, ..a.clone() }], |_| true, |_| false).is_empty());
        assert!(w.check(&ctx(800, 999), vec![a], |_| true, |_| false).is_empty(), "ни транскрипта, ни хуков — судить не по чему");
    }

    #[test]
    fn idle_after_turn_end_is_not_hung() {
        let mut w = Watch::default();
        let a = Facts { turn_over: true, ..f("a") }; // файл сессии «busy», а ход давно кончился
        assert!(w.check(&ctx(3 * 720 + 100, 999), vec![a.clone()], |_| true, |_| false).is_empty());
        assert_eq!(w.health("a"), "");
        let h = Facts { helpers: 1, ..a };
        assert!(matches!(&w.check(&ctx(3 * 720 + 200, 999), vec![h], |_| true, |_| false)[..], [Act::Judge { .. }]), "ход кончен, но помощники в фоне — их тишину разбираем");
    }

    #[test]
    fn crash_after_turn_end_is_not_raised() {
        let mut w = Watch::default();
        w.check(&ctx(100, 0), vec![Facts { turn_over: true, ..f("a") }], |_| true, |_| false);
        assert!(w.check(&ctx(200, 999), vec![], |_| true, |_| false).is_empty());
    }

    #[test]
    fn limited_agent_is_not_hung() {
        let mut w = Watch::default();
        w.limited("a", 99_999);
        assert!(w.check(&ctx(3000, 999), vec![f("a")], |_| true, |_| false).is_empty());
    }

    #[test]
    fn subagent_on_dangerous_call_is_not_interrupted() {
        let mut w = Watch::default();
        let a = Facts { call: Some(json!({"name": "Agent", "input": {}})), sub_call: Some(json!({"name": "Bash", "input": {"command": "npx prisma migrate deploy"}})), ..f("a") };
        let acts = w.check(&ctx(800, 999), vec![a], |_| true, |_| false);
        assert!(matches!(&acts[..], [Act::Say { urgent: true, text, .. }] if text == "p: сабагент завис посреди миграция — не трогаю, посмотри сам." || text.contains("миграц")), "{acts:?}");
        let mut w = Watch::default();
        let b = Facts { call: Some(json!({"name": "Agent", "input": {}})), sub_call: Some(json!({"name": "mcp__db__write", "input": {}})), ..f("b") };
        assert!(matches!(&w.check(&ctx(800, 999), vec![b], |_| true, |_| false)[..], [Act::Say { urgent: true, text, .. }] if text.contains("безопасно ли прервать")));
    }

    #[test]
    fn question_tool_in_flight_is_waiting_not_hung() {
        let mut w = Watch::default();
        for name in ["AskUserQuestion", "ExitPlanMode"] {
            let a = Facts { call: Some(json!({"name": name, "input": {}})), ..f("a") };
            assert!(w.check(&ctx(3000, 999), vec![a], |_| true, |_| false).is_empty(), "{name}: уведомление потерялось, а вопрос к пользователю закрывать нельзя");
        }
    }

    #[test]
    fn nudges_count_as_attempts() {
        let mut w = Watch::default();
        let mut t = 1;
        for _ in 0..3 {
            let a = Facts { helpers: 1, active_at: t, ..f("a") };
            w.check(&ctx(t + 800, 0), vec![a.clone()], |_| true, |_| false);
            w.judged("a", true, t + 801);
            assert_eq!(run_of(&w.check(&ctx(t + 802, 0), vec![a], |_| true, |_| false)[0]).unwrap().what, What::Nudge);
            t += 900;
        }
        let a = Facts { helpers: 1, active_at: t, ..f("a") };
        w.check(&ctx(t + 800, 0), vec![a.clone()], |_| true, |_| false);
        w.judged("a", true, t + 801);
        let acts = w.check(&ctx(t + 802, 0), vec![a.clone()], |_| true, |_| false);
        assert!(matches!(&acts[..], [Act::Say { urgent: true, text, .. }] if text == "p: помощник агента снова молчит — посмотри сам. Через 15 мин попробую снова."), "{acts:?}");
        assert!(w.check(&ctx(t + 900, 0), vec![a.clone()], |_| true, |_| false).is_empty());
        assert_eq!(run_of(&w.check(&ctx(803 + 3600, 0), vec![a], |_| true, |_| false)[0]).unwrap().what, What::Nudge, "окно освободилось — пишем снова");
    }

    #[test]
    fn unanswered_nudge_calls_the_user() {
        let mut w = Watch::default();
        let a = Facts { helpers: 1, ..f("a") };
        w.check(&ctx(800, 0), vec![a.clone()], |_| true, |_| false);
        w.judged("a", true, 801);
        assert_eq!(run_of(&w.check(&ctx(802, 0), vec![a.clone()], |_| true, |_| false)[0]).unwrap().what, What::Nudge);
        assert!(w.check(&ctx(802 + 719, 0), vec![a.clone()], |_| true, |_| false).is_empty(), "ждём ответа агента");
        let acts = w.check(&ctx(802 + 720, 0), vec![a.clone()], |_| true, |_| false);
        assert!(matches!(&acts[..], [Act::Say { urgent: true, text, .. }] if text == "p: агент не ответил на просьбу проверить помощника — посмотри сам."), "{acts:?}");
        assert!(w.check(&ctx(2000, 0), vec![a.clone()], |_| true, |_| false).is_empty(), "один раз");
        assert_eq!(w.trouble(), 1);
        assert!(w.check(&ctx(2100, 0), vec![Facts { active_at: 2050, ..a }], |_| true, |_| false).is_empty());
        assert_eq!(w.trouble(), 0, "агент ожил — тревога снята");
    }

    #[test]
    fn old_api_errors_are_history() {
        let mut w = Watch::default();
        let now = 100_000;
        let old = |kind: &str| Facts { state: State::Idle, active_at: 1, error: Some(ApiError { kind: kind.into(), at: now - 7 * 3600, resets_at: Some(now - 3600) }), ..f("a") };
        for k in ["server_error", "authentication_failed", "rate_limit"] {
            assert!(w.check(&ctx(now, 0), vec![old(k)], |_| true, |_| false).is_empty(), "{k}: ошибке больше 6 ч — демон просто перезапустили");
        }
    }

    #[test]
    fn past_reset_is_not_announced_but_wakes() {
        let mut w = Watch::default();
        let a = Facts { state: State::Idle, active_at: 100, error: Some(ApiError { kind: "rate_limit".into(), at: 100, resets_at: Some(1000) }), ..f("a") };
        let acts = w.check(&ctx(2000, 0), vec![a], |_| true, |_| false);
        assert!(acts.iter().all(|a| !matches!(a, Act::Say { session, .. } if session == "limit")), "сброс прошёл — «кончился лимит» не новость: {acts:?}");
        assert!(acts.iter().any(|a| run_of(a).is_some_and(|j| j.what == What::Wake)));
    }

    #[test]
    fn session_end_means_user_closed_it() {
        let mut w = Watch::default();
        w.check(&ctx(100, 0), vec![f("a")], |_| true, |_| false);
        w.ended("a");
        assert!(w.check(&ctx(200, 999), vec![], |_| true, |_| false).is_empty());
    }

    #[test]
    fn own_kill_resumes_despite_session_end_but_only_soon() {
        let mut w = Watch::default();
        let a = Facts { call: Some(json!({"name": "Agent", "input": {}})), ..f("a") };
        w.check(&ctx(800, 0), vec![a.clone()], |_| true, |_| false);
        w.check(&ctx(1200, 0), vec![a.clone()], |_| true, |_| false);
        w.done("a", What::Kill, None, 1201);
        w.ended("a"); // Ctrl+C дважды — Claude Code шлёт SessionEnd
        let acts = w.check(&ctx(1210, 0), vec![], |_| true, |_| false);
        assert_eq!(run_of(&acts[0]).unwrap().what, What::Resume);
        assert!(matches!(w.done("a", What::Resume, None, 1215), Some(Act::Say { text, .. }) if text == "p: агент завис — перезапустила."));
        let mut w = Watch::default();
        w.check(&ctx(800, 0), vec![a.clone()], |_| true, |_| false);
        w.check(&ctx(1200, 0), vec![a.clone()], |_| true, |_| false);
        w.done("a", What::Kill, None, 1201); // Ctrl+C не закрыл агента
        w.check(&ctx(1300, 0), vec![Facts { active_at: 1290, ..a }], |_| true, |_| false);
        assert!(matches!(&w.check(&ctx(1600, 0), vec![], |_| true, |_| false)[..], [Act::Say { urgent: false, .. }]), "закрыл пользователь, сидя за компьютером, — только сказать");
    }

    #[test]
    fn live_process_is_not_a_crash() {
        let mut w = Watch::default();
        w.check(&ctx(100, 0), vec![f("a")], |_| true, |_| false);
        assert!(w.check(&ctx(200, 999), vec![], |_| true, |pid| pid == 7).is_empty(), "/clear или /resume: id сменился, процесс жив");
    }

    #[test]
    fn prompt_clears_limited() {
        let mut w = Watch::default();
        w.limited("a", 1000);
        w.prompted("a");
        assert!(w.check(&ctx(2000, 0), vec![Facts { state: State::Idle, ..f("a") }], |_| true, |_| false).is_empty());
    }

    #[test]
    fn failed_resume_alarm_fades_with_the_agent() {
        let mut w = Watch::default();
        w.check(&ctx(100, 0), vec![f("a")], |_| true, |_| false);
        w.check(&ctx(200, 999), vec![], |_| true, |_| false);
        assert!(matches!(w.done("a", What::Resume, Some("нет терминала".into()), 201), Some(Act::Say { urgent: true, .. })));
        assert_eq!(w.trouble(), 1);
        w.check(&ctx(260, 999), vec![], |_| true, |_| false);
        assert_eq!(w.trouble(), 1, "оранжевый держится, пока пользователь не посмотрел");
        w.check(&ctx(201 + 1801, 999), vec![], |_| true, |_| false);
        assert_eq!(w.trouble(), 0, "агента нет — сигнал не вечный");
    }

    impl Facts {
        fn clone_as(&self, id: &str) -> Facts {
            Facts { id: id.into(), ..self.clone() }
        }
    }
}
