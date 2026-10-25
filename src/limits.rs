//! Лимиты аккаунтов агентов: проценты окон, прогноз «кончится раньше сброса», подсказки агентам через хук.

use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};

/// Строка, которой агент, вставший из-за лимита, кончает ответ: по ней Ома будит его после сброса.
pub const LIMIT_STOP: &str = "Остановился по лимиту";

#[derive(Clone, Debug, PartialEq)]
pub struct Window {
    pub key: String, // claude:five_hour | claude:seven_day | codex:primary | codex:secondary
    pub label: &'static str,
    pub pct: f64,
    pub resets_at: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Forecast {
    pub key: String,
    pub label: &'static str,
    pub pct: f64,
    pub out_in: u64,
    pub reset_in: u64,
    pub resets_at: u64,
}

#[derive(Default)]
pub struct Limits {
    series: HashMap<String, (Window, VecDeque<(u64, f64)>)>,
}

/// Темп расхода — по замерам за столько секунд.
const RATE_SPAN: u64 = 900;
/// Меньше этого размаха замеров темп не считается.
const MIN_SPAN: u64 = 120;
/// Замеры старше этого не хранятся.
const KEEP: u64 = 1800;
/// Чаще без смены процента замер не пишется: строка статуса рисуется на каждое событие.
const SAMPLE_EVERY: u64 = 30;

fn window(key: &str, label: &'static str, w: &Value, pct: &str) -> Option<Window> {
    let pct = w[pct].as_f64()?;
    let resets_at = w["resets_at"].as_f64()? as u64;
    Some(Window { key: key.into(), label, pct, resets_at })
}

/// Окна лимита Claude из ввода строки статуса (`rate_limits`); окна может не быть.
pub fn from_statusline(v: &Value) -> Vec<Window> {
    let r = &v["rate_limits"];
    [("five_hour", "5ч"), ("seven_day", "неделя")]
        .into_iter()
        .filter_map(|(k, label)| window(&format!("claude:{k}"), label, &r[k], "used_percentage"))
        .collect()
}

/// Окна лимита Codex из строки rollout (событие token_count).
pub fn from_codex(line: &str) -> Vec<Window> {
    let Ok(v) = serde_json::from_str::<Value>(line) else { return vec![] };
    if v["payload"]["type"] != "token_count" {
        return vec![];
    }
    let r = &v["payload"]["rate_limits"];
    ["primary", "secondary"].into_iter().filter_map(|k| window(&format!("codex:{k}"), "Codex", &r[k], "used_percent")).collect()
}

impl Limits {
    /// Замер окна; новый срок сброса — новое окно, замеры заново.
    pub fn sample(&mut self, w: Window, now: u64) {
        let entry = self.series.entry(w.key.clone()).or_insert_with(|| (w.clone(), VecDeque::new()));
        if entry.0.resets_at != w.resets_at {
            entry.1.clear();
        }
        let s = &mut entry.1;
        s.retain(|&(t, _)| now.saturating_sub(t) <= KEEP);
        if !s.back().is_some_and(|&(t, p)| p == w.pct && now.saturating_sub(t) < SAMPLE_EVERY) {
            s.push_back((now, w.pct));
        }
        entry.0 = w;
    }

    /// Плохие прогнозы: процент не ниже порога, и при нынешнем темпе лимит кончится раньше сброса.
    pub fn forecast(&self, now: u64, threshold: f64) -> Vec<Forecast> {
        let mut out: Vec<Forecast> = self
            .series
            .values()
            .filter_map(|(w, s)| {
                if w.pct < threshold || w.resets_at <= now {
                    return None;
                }
                let mut recent = s.iter().filter(|&&(t, _)| now.saturating_sub(t) <= RATE_SPAN);
                let (t0, p0) = *recent.next()?;
                let (t1, p1) = *recent.last()?;
                let span = t1.saturating_sub(t0);
                if span < MIN_SPAN || p1 <= p0 {
                    return None;
                }
                let out_in = ((100.0 - w.pct).max(0.0) / ((p1 - p0) / span as f64)).round() as u64;
                let reset_in = w.resets_at - now;
                (out_in < reset_in).then(|| Forecast { key: w.key.clone(), label: w.label, pct: w.pct, out_in, reset_in, resets_at: w.resets_at })
            })
            .collect();
        out.sort_by(|a, b| a.key.cmp(&b.key));
        out
    }

    /// Срок сброса самого полного окна агента (claude | codex).
    pub fn reset_of(&self, agent: &str) -> Option<u64> {
        let prefix = format!("{agent}:");
        self.series.values().filter(|(w, _)| w.key.starts_with(&prefix)).max_by(|a, b| a.0.pct.total_cmp(&b.0.pct)).map(|(w, _)| w.resets_at)
    }
}

/// Факты агенту: решает он сам — только он знает, сколько ему осталось.
pub fn hint(f: &Forecast) -> String {
    let min = |s: u64| s.div_ceil(60);
    format!(
        "Ома, сторож лимитов: лимит {} — {:.0}%, при текущем темпе кончится через ~{} мин, сброс через {} мин. \
Успеваешь доделать задачу до конца лимита — доделывай. Не успеваешь — доведи текущий шаг до конца, запиши в документацию проекта, \
что сделано и что осталось, и остановись; последней строкой ответа напиши «{LIMIT_STOP}».",
        f.label, f.pct, min(f.out_in), min(f.reset_in)
    )
}

pub fn hints_dir() -> PathBuf {
    PathBuf::from(std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into())).join("agent-speak-hints")
}

/// id сессии и агента — часть имени файла: только буквы, цифры, «-» и «_».
fn safe(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Подсказка сессии — каждый тик заново: минуты в ней относительные, сабагент читает её позже главного.
pub fn put_hint(dir: &Path, session: &str, text: &str) {
    if !safe(session) || std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let tmp = dir.join(format!(".{session}.tmp"));
    if std::fs::write(&tmp, text).is_ok() {
        let _ = std::fs::rename(&tmp, dir.join(session)); // хук не прочтёт половину
    }
}

/// Убрать подсказки и метки сессий, которых нет в keep.
pub fn clear_hints(dir: &Path, keep: impl Fn(&str) -> bool) {
    for f in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = f.file_name().to_string_lossy().into_owned();
        let session = name.split('.').next().unwrap_or_default();
        if !session.is_empty() && !keep(session) {
            let _ = std::fs::remove_file(f.path());
        }
    }
}

/// Хук: подсказка сессии один раз на агента (главный — `main`, сабагент — его id); метка — `<session>.<agent>.seen`.
pub fn take_hint(dir: &Path, session: &str, agent: &str) -> Option<String> {
    if !safe(session) || !safe(agent) {
        return None;
    }
    let text = std::fs::read_to_string(dir.join(session)).ok()?;
    let seen = dir.join(format!("{session}.{agent}.seen"));
    std::fs::OpenOptions::new().write(true).create_new(true).open(seen).ok()?;
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(key: &str, pct: f64, resets_at: u64) -> Window {
        Window { key: key.into(), label: "5ч", pct, resets_at }
    }

    #[test]
    fn forecast_bad_only_when_runs_out_before_reset() {
        let mut l = Limits::default();
        l.sample(w("claude:five_hour", 93.0, 10_000), 1000);
        l.sample(w("claude:five_hour", 96.0, 10_000), 1300); // 1 % в 100 с → кончится через 400 с, сброс через 8700
        let f = l.forecast(1300, 95.0);
        assert_eq!((f.len(), f[0].out_in, f[0].reset_in), (1, 400, 8700));
        assert!(l.forecast(1300, 97.0).is_empty(), "ниже порога");
        let mut l = Limits::default();
        l.sample(w("claude:five_hour", 96.0, 1500), 1000);
        l.sample(w("claude:five_hour", 99.0, 1500), 1200); // кончится через ~67 с, сброс через 300 с
        assert!(!l.forecast(1200, 95.0).is_empty());
        let mut l = Limits::default();
        l.sample(w("claude:five_hour", 96.0, 1300), 1000);
        l.sample(w("claude:five_hour", 97.0, 1300), 1200); // кончится через 600 с, а сброс через 100 с — успевает
        assert!(l.forecast(1200, 95.0).is_empty());
    }

    #[test]
    fn no_forecast_without_span_or_growth() {
        let mut l = Limits::default();
        l.sample(w("claude:five_hour", 96.0, 9000), 1000);
        l.sample(w("claude:five_hour", 97.0, 9000), 1060); // размах 60 с < MIN_SPAN
        assert!(l.forecast(1060, 95.0).is_empty());
        l.sample(w("claude:five_hour", 97.0, 9000), 1300);
        l.sample(w("claude:five_hour", 20.0, 27000), 1400); // новое окно — замеры заново
        assert!(l.forecast(1400, 95.0).is_empty());
        assert_eq!(l.reset_of("claude"), Some(27000));
        assert_eq!(l.reset_of("codex"), None);
        let mut l = Limits::default();
        l.sample(w("claude:five_hour", 97.0, 9000), 1000);
        l.sample(w("claude:five_hour", 97.0, 9000), 1300); // темп 0 — прогноза нет
        assert!(l.forecast(1300, 95.0).is_empty());
    }

    #[test]
    fn old_samples_leave_the_rate() {
        let mut l = Limits::default();
        l.sample(w("claude:five_hour", 50.0, 99_000), 0);
        l.sample(w("claude:five_hour", 95.0, 99_000), 2000);
        l.sample(w("claude:five_hour", 96.0, 99_000), 2200); // за 15 мин — 1 % в 200 с; первый замер старше RATE_SPAN
        let f = l.forecast(2200, 95.0);
        assert_eq!(f[0].out_in, 800);
    }

    #[test]
    fn parses_statusline_and_codex() {
        let v = serde_json::json!({"rate_limits": {"five_hour": {"used_percentage": 96.5, "resets_at": 1792900000u64}}});
        let got = from_statusline(&v);
        assert_eq!((got.len(), got[0].key.as_str(), got[0].pct, got[0].resets_at, got[0].label), (1, "claude:five_hour", 96.5, 1792900000, "5ч"));
        let v = serde_json::json!({"rate_limits": {"five_hour": {"used_percentage": 1, "resets_at": 5}, "seven_day": {"used_percentage": 40.0, "resets_at": 9}}});
        assert_eq!(from_statusline(&v).iter().map(|w| (w.key.as_str(), w.label)).collect::<Vec<_>>(), [("claude:five_hour", "5ч"), ("claude:seven_day", "неделя")]);
        assert!(from_statusline(&serde_json::json!({})).is_empty());
        let line = r#"{"type":"event_msg","payload":{"type":"token_count","rate_limits":{"primary":{"used_percent":12.0,"window_minutes":43200,"resets_at":1793548673},"secondary":null}}}"#;
        assert_eq!(from_codex(line).iter().map(|w| (w.key.as_str(), w.pct, w.label)).collect::<Vec<_>>(), [("codex:primary", 12.0, "Codex")]);
        assert!(from_codex(r#"{"type":"event_msg","payload":{"type":"agent_message"}}"#).is_empty());
        assert!(from_codex("мусор").is_empty());
    }

    #[test]
    fn hint_carries_facts_and_stop_line() {
        let h = hint(&Forecast { key: "claude:five_hour".into(), label: "5ч", pct: 96.4, out_in: 690, reset_in: 2400, resets_at: 0 });
        assert!(h.contains("96%") && h.contains("~12 мин") && h.contains("через 40 мин") && h.contains(LIMIT_STOP), "{h}");
    }

    #[test]
    fn hint_once_per_agent_until_cleared() {
        let dir = std::env::temp_dir().join(format!("oma-hints-{}", std::process::id()));
        put_hint(&dir, "s", "сброс через 40 мин");
        put_hint(&dir, "other", "лимит");
        assert_eq!(take_hint(&dir, "s", "main").as_deref(), Some("сброс через 40 мин"));
        assert_eq!(take_hint(&dir, "s", "main"), None);
        put_hint(&dir, "s", "сброс через 10 мин"); // тот же эпизод, минуты прошли
        assert_eq!(take_hint(&dir, "s", "main"), None, "главный уже прочёл");
        assert_eq!(take_hint(&dir, "s", "a81492c2").as_deref(), Some("сброс через 10 мин"), "сабагент — свой раз и свежие минуты");
        assert_eq!(take_hint(&dir, "нет", "main"), None);
        clear_hints(&dir, |s| s == "other");
        assert_eq!(take_hint(&dir, "s", "main"), None);
        assert_eq!(take_hint(&dir, "other", "main").as_deref(), Some("лимит"));
        put_hint(&dir, "s", "новый эпизод");
        assert_eq!(take_hint(&dir, "s", "main").as_deref(), Some("новый эпизод"));
        assert_eq!(take_hint(&dir, "../x", "main"), None, "id сессии — не путь");
        let _ = std::fs::remove_dir_all(dir);
    }
}
