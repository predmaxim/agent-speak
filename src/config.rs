//! ~/.config/agent-speak/config.toml; отсутствующие поля — по умолчанию.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(default)]
pub struct Config {
    pub speaker: String,
    pub rate: String,
    /// тихий режим по расписанию (окно quiet_from..quiet_to, ЧЧ:ММ)
    pub quiet_schedule: bool,
    pub quiet_from: String,
    pub quiet_to: String,
    /// ручное переопределение расписания: "" (нет) / "on" / "off"
    pub quiet_override: String,
    /// следить за агентами («Следить за агентами»): пересказы моделью, пузыри и голос о них;
    /// выкл — об агентах ни слова и ни одного вызова модели, Ома — только голосовой помощник
    pub agents_voice: bool,
    /// пересказывать промежуточное моделью (раз в минуту на агента); выключено — в окне дословная последняя фраза
    pub progress_digest: bool,
    /// позиция орба: "<угол>,<dx>,<dy>", см. valid_pos
    pub orb_pos: String,
    /// орб и пузыри — и над полноэкранными окнами (слой Overlay), иначе под ними (Top)
    pub orb_over_fullscreen: bool,
    /// сторож проверяет агентов раз в столько секунд
    pub watch_every: u64,
    /// агент «работает», а событий нет столько минут — завис
    pub hung_after: u64,
    /// вопрос агента ждёт ответа — напоминать раз в столько минут
    pub remind_after: u64,
    /// процент лимита, с которого Ома считает прогноз и подсказывает агентам
    pub limit_threshold: u64,
}

impl Default for Config {
    fn default() -> Config {
        Config { speaker: "xenia".into(), rate: "medium".into(),
            quiet_schedule: true, quiet_from: "23:00".into(), quiet_to: "08:00".into(), quiet_override: String::new(), agents_voice: true, progress_digest: false, orb_pos: "bottom-right,0,0".into(),
            orb_over_fullscreen: true, watch_every: 60, hung_after: 12, remind_after: 15, limit_threshold: 95 }
    }
}

pub fn path() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config/agent-speak/config.toml")
}

pub const SPEAKERS: [&str; 5] = ["xenia", "baya", "kseniya", "aidar", "eugene"];
pub const RATES: [&str; 5] = ["x-slow", "slow", "medium", "fast", "x-fast"];
pub const CORNERS: [&str; 4] = ["top-left", "top-right", "bottom-left", "bottom-right"];

/// `<угол>,<dx>,<dy>`: угол из CORNERS, смещения — целые 0..=10000.
pub fn valid_pos(s: &str) -> bool {
    let p: Vec<&str> = s.split(',').collect();
    p.len() == 3 && CORNERS.contains(&p[0]) && p[1..].iter().all(|n| n.parse::<u32>().is_ok_and(|n| n <= 10000))
}

/// Целое из lo..=hi.
fn in_range(v: &serde_json::Value, lo: u64, hi: u64) -> bool {
    v.as_u64().is_some_and(|n| (lo..=hi).contains(&n))
}

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
            "quiet_schedule" if value.is_boolean() => self.quiet_schedule = value.as_bool().unwrap(),
            "quiet_from" if hhmm(s).is_some() => self.quiet_from = s.into(),
            "quiet_to" if hhmm(s).is_some() => self.quiet_to = s.into(),
            "quiet_override" if ["", "on", "off"].contains(&s) => self.quiet_override = s.into(),
            "agents_voice" if value.is_boolean() => self.agents_voice = value.as_bool().unwrap(),
            "progress_digest" if value.is_boolean() => self.progress_digest = value.as_bool().unwrap(),
            "orb_pos" if valid_pos(s) => self.orb_pos = s.into(),
            "orb_over_fullscreen" if value.is_boolean() => self.orb_over_fullscreen = value.as_bool().unwrap(),
            "watch_every" if in_range(value, 15, 3600) => self.watch_every = value.as_u64().unwrap(),
            "hung_after" if in_range(value, 2, 240) => self.hung_after = value.as_u64().unwrap(),
            "remind_after" if in_range(value, 1, 1440) => self.remind_after = value.as_u64().unwrap(),
            "limit_threshold" if in_range(value, 50, 100) => self.limit_threshold = value.as_u64().unwrap(),
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
        assert_eq!(
            (c.quiet_schedule, c.quiet_from.as_str(), c.quiet_to.as_str(), c.quiet_override.as_str()),
            (true, "23:00", "08:00", "")
        );
        assert_eq!((c.agents_voice, c.orb_pos.as_str()), (true, "bottom-right,0,0"));
    }

    #[test]
    fn old_config_keys_ignored() {
        let c: Config = toml::from_str("read_intermediate = false\nmax_age_secs = 5\nspeaker = \"baya\"\n").unwrap();
        assert_eq!(c.speaker, "baya");
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
    fn orb_pos_and_agents_voice_set() {
        use serde_json::json;
        let mut c = Config::default();
        assert!(c.set("orb_pos", &json!("top-left,120,340")) && c.orb_pos == "top-left,120,340");
        for bad in ["middle,1,1", "top-left,1", "top-left,-1,0", "top-left,1,x", "top-left,10001,0", ""] {
            assert!(!c.set("orb_pos", &json!(bad)), "{bad}");
        }
        assert!(c.set("agents_voice", &json!(true)) && c.agents_voice);
        assert!(!c.set("agents_voice", &json!("true")));
        assert!(!c.progress_digest && c.set("progress_digest", &json!(true)) && c.progress_digest);
        assert!(!c.set("progress_digest", &json!("true")));
        assert!(c.orb_over_fullscreen && c.set("orb_over_fullscreen", &json!(false)) && !c.orb_over_fullscreen);
        assert!(!c.set("orb_corner", &json!("top-left")));
    }

    #[test]
    fn set_validates_keys_and_values() {
        use serde_json::json;
        let mut c = Config::default();
        assert!(!c.set("mode", &json!("auto"))); // режима больше нет
        assert!(c.set("speaker", &json!("eugene")));
        assert!(c.set("rate", &json!("x-fast")));
        assert!(!c.set("read_intermediate", &json!(false))); // настройки больше нет
        assert!(!c.set("speaker", &json!(1)));
        assert!(!c.set("rate", &json!("warp")));
        assert!(c.set("quiet_schedule", &json!(false)));
        assert!(c.set("quiet_from", &json!("22:30")));
        assert!(c.set("quiet_override", &json!("on")));
        assert!(!c.set("quiet_from", &json!("25:00")));
        assert!(!c.set("quiet_override", &json!("maybe")));
        assert!(!c.set("quiet_schedule", &json!("true")));
        assert!(!c.set("max_age_secs", &json!(5))); // настройки больше нет
        assert_eq!((c.speaker.as_str(), c.rate.as_str()), ("eugene", "x-fast"));
    }

    #[test]
    fn watch_keys_default_and_validate() {
        use serde_json::json;
        let mut c = Config::default();
        assert_eq!((c.watch_every, c.hung_after, c.remind_after, c.limit_threshold), (60, 12, 15, 95));
        let old: Config = toml::from_str("speaker = \"baya\"").unwrap();
        assert_eq!((old.watch_every, old.hung_after), (60, 12), "старый файл без ключей сторожа");
        assert!(!c.set("hung_after", &json!(1)) && !c.set("limit_threshold", &json!(101)) && !c.set("watch_every", &json!(10)));
        assert!(!c.set("remind_after", &json!("15")) && !c.set("hung_after", &json!(12.5)));
        assert!(c.set("hung_after", &json!(20)) && c.set("watch_every", &json!(15)) && c.set("remind_after", &json!(1440)) && c.set("limit_threshold", &json!(50)));
        assert_eq!((c.watch_every, c.hung_after, c.remind_after, c.limit_threshold), (15, 20, 1440, 50));
    }
}
