//! ~/.config/agent-speak/config.toml; отсутствующие поля — по умолчанию.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(default)]
pub struct Config {
    pub speaker: String,
    pub rate: String,
    pub max_age_secs: u64,
    /// false — в диалоге только финальный ответ хода, без промежуточных статусов
    pub read_intermediate: bool,
    /// тихий режим по расписанию (окно quiet_from..quiet_to, ЧЧ:ММ)
    pub quiet_schedule: bool,
    pub quiet_from: String,
    pub quiet_to: String,
    /// ручное переопределение расписания: "" (нет) / "on" / "off"
    pub quiet_override: String,
    /// угол экрана для орба: см. CORNERS
    pub orb_corner: String,
}

impl Default for Config {
    fn default() -> Config {
        Config { speaker: "xenia".into(), rate: "medium".into(), max_age_secs: 30, read_intermediate: true,
            quiet_schedule: true, quiet_from: "23:00".into(), quiet_to: "08:00".into(), quiet_override: String::new(), orb_corner: "bottom-right".into() }
    }
}

pub fn path() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config/agent-speak/config.toml")
}

pub const SPEAKERS: [&str; 5] = ["xenia", "baya", "kseniya", "aidar", "eugene"];
pub const RATES: [&str; 5] = ["x-slow", "slow", "medium", "fast", "x-fast"];
pub const CORNERS: [&str; 4] = ["top-left", "top-right", "bottom-left", "bottom-right"];

/// Ровно `ЧЧ:ММ` (00..23:00..59) -> минуты от полуночи.
pub fn hhmm(s: &str) -> Option<u32> {
    let b = s.as_bytes();
    if b.len() != 5 || b[2] != b':' || ![0, 1, 3, 4].iter().all(|&i| b[i].is_ascii_digit()) {
        return None;
    }
    let (h, m) = (s[..2].parse::<u32>().ok()?, s[3..].parse::<u32>().ok()?);
    (h < 24 && m < 60).then_some(h * 60 + m)
}

/// now в [from, to), окно может идти через полночь; from == to — пустое.
pub fn in_window(from: u32, to: u32, now: u32) -> bool {
    if from <= to { from <= now && now < to } else { now >= from || now < to }
}

impl Config {
    pub fn scheduled_quiet(&self, now: u32) -> bool {
        self.quiet_schedule
            && matches!((hhmm(&self.quiet_from), hhmm(&self.quiet_to)), (Some(f), Some(t)) if in_window(f, t, now))
    }

    /// Тихо ли сейчас: ручное переопределение сильнее расписания.
    pub fn quiet(&self, now: u32) -> bool {
        match self.quiet_override.as_str() {
            "on" => true,
            "off" => false,
            _ => self.scheduled_quiet(now),
        }
    }

    /// Настройка из интерфейса (команда `set`); неверный ключ или значение — false, ничего не меняется.
    pub fn set(&mut self, key: &str, value: &serde_json::Value) -> bool {
        let s = value.as_str().unwrap_or_default();
        match key {
            "speaker" if SPEAKERS.contains(&s) => self.speaker = s.into(),
            "rate" if RATES.contains(&s) => self.rate = s.into(),
            "read_intermediate" if value.is_boolean() => self.read_intermediate = value.as_bool().unwrap(),
            "quiet_schedule" if value.is_boolean() => self.quiet_schedule = value.as_bool().unwrap(),
            "quiet_from" if hhmm(s).is_some() => self.quiet_from = s.into(),
            "quiet_to" if hhmm(s).is_some() => self.quiet_to = s.into(),
            "quiet_override" if ["", "on", "off"].contains(&s) => self.quiet_override = s.into(),
            "orb_corner" if CORNERS.contains(&s) => self.orb_corner = s.into(),
            _ => return false,
        }
        true
    }

    pub fn load() -> Config {
        std::fs::read_to_string(path()).ok().and_then(|s| toml::from_str(&s).ok()).unwrap_or_default()
    }

    pub fn save(&self) {
        let p = path();
        let _ = std::fs::create_dir_all(p.parent().unwrap());
        // атомарно: наблюдатель не должен увидеть обрезанный файл
        let tmp = p.with_extension("toml.tmp");
        if std::fs::write(&tmp, toml::to_string(self).unwrap()).is_ok() {
            let _ = std::fs::rename(&tmp, p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_partial_file() {
        // старый файл: mode и voice_last_agent больше не поля — игнорируются
        let c: Config = toml::from_str("speaker = \"baya\"\nmode = \"auto\"\nvoice_last_agent = \"codex\"").unwrap();
        assert_eq!(c.speaker, "baya");
        assert_eq!(c.rate, "medium");
        assert_eq!(c.max_age_secs, 30);
        assert!(c.read_intermediate);
        assert_eq!(
            (c.quiet_schedule, c.quiet_from.as_str(), c.quiet_to.as_str(), c.quiet_override.as_str(), c.orb_corner.as_str()),
            (true, "23:00", "08:00", "", "bottom-right")
        );
    }

    #[test]
    fn quiet_window_wraps_midnight() {
        let m = |s| hhmm(s).unwrap();
        assert!(in_window(m("23:00"), m("08:00"), m("23:30")));
        assert!(in_window(m("23:00"), m("08:00"), m("07:59")));
        assert!(!in_window(m("23:00"), m("08:00"), m("08:00")));
        assert!(!in_window(m("23:00"), m("08:00"), m("12:00")));
        assert!(in_window(m("09:00"), m("18:00"), m("12:00")));
        assert!(!in_window(600, 600, 600));
        assert_eq!((hhmm("24:00"), hhmm("7:5"), hhmm("07:05")), (None, None, Some(425)));
    }

    #[test]
    fn override_beats_schedule() {
        let mut c = Config::default();
        let (night, day) = (hhmm("01:00").unwrap(), hhmm("12:00").unwrap());
        assert!(c.quiet(night) && !c.quiet(day));
        c.quiet_override = "on".into();
        assert!(c.quiet(day));
        c.quiet_override = "off".into();
        assert!(!c.quiet(night));
        c.quiet_override = String::new();
        c.quiet_schedule = false;
        assert!(!c.quiet(night) && !c.scheduled_quiet(night));
    }

    #[test]
    fn set_validates_keys_and_values() {
        use serde_json::json;
        let mut c = Config::default();
        assert!(!c.set("mode", &json!("auto"))); // режима больше нет
        assert!(c.set("speaker", &json!("eugene")));
        assert!(c.set("rate", &json!("x-fast")));
        assert!(c.set("read_intermediate", &json!(false)));
        assert!(!c.set("read_intermediate", &json!("true"))); // строка вместо булева
        assert!(!c.set("speaker", &json!(1)));
        assert!(!c.set("rate", &json!("warp")));
        assert!(c.set("quiet_schedule", &json!(false)));
        assert!(c.set("quiet_from", &json!("22:30")));
        assert!(c.set("quiet_override", &json!("on")));
        assert!(c.set("orb_corner", &json!("top-left")));
        assert!(!c.set("quiet_from", &json!("25:00")));
        assert!(!c.set("quiet_override", &json!("maybe")));
        assert!(!c.set("orb_corner", &json!("middle")));
        assert!(!c.set("quiet_schedule", &json!("true")));
        assert!(!c.set("max_age_secs", &json!(5))); // не настраивается из интерфейса
        assert_eq!(
            (c.speaker.as_str(), c.rate.as_str(), c.read_intermediate, c.max_age_secs),
            ("eugene", "x-fast", false, 30)
        );
    }
}
