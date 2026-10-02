//! ~/.config/agent-speak/config.toml; отсутствующие поля — по умолчанию.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(default)]
pub struct Config {
    pub mode: String,
    pub speaker: String,
    pub rate: String,
    pub max_age_secs: u64,
    /// false — в авторежиме только финальный ответ хода, без промежуточных статусов
    pub read_intermediate: bool,
}

impl Default for Config {
    fn default() -> Config {
        Config { mode: "manual".into(), speaker: "xenia".into(), rate: "medium".into(), max_age_secs: 30, read_intermediate: true }
    }
}

pub fn path() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config/agent-speak/config.toml")
}

pub const SPEAKERS: [&str; 5] = ["xenia", "baya", "kseniya", "aidar", "eugene"];
pub const RATES: [&str; 5] = ["x-slow", "slow", "medium", "fast", "x-fast"];

impl Config {
    /// Настройка из интерфейса (команда `set`); неверный ключ или значение — false, ничего не меняется.
    pub fn set(&mut self, key: &str, value: &serde_json::Value) -> bool {
        let s = value.as_str().unwrap_or_default();
        match key {
            "mode" if ["auto", "manual"].contains(&s) => self.mode = s.into(),
            "speaker" if SPEAKERS.contains(&s) => self.speaker = s.into(),
            "rate" if RATES.contains(&s) => self.rate = s.into(),
            "read_intermediate" if value.is_boolean() => self.read_intermediate = value.as_bool().unwrap(),
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
        let c: Config = toml::from_str("speaker = \"baya\"").unwrap();
        assert_eq!(c.speaker, "baya");
        assert_eq!(c.mode, "manual");
        assert_eq!(c.rate, "medium");
        assert_eq!(c.max_age_secs, 30);
        assert!(c.read_intermediate);
    }

    #[test]
    fn set_validates_keys_and_values() {
        use serde_json::json;
        let mut c = Config::default();
        assert!(c.set("mode", &json!("auto")));
        assert!(c.set("speaker", &json!("eugene")));
        assert!(c.set("rate", &json!("x-fast")));
        assert!(c.set("read_intermediate", &json!(false)));
        assert!(!c.set("read_intermediate", &json!("true"))); // строка вместо булева
        assert!(!c.set("mode", &json!("loud")));
        assert!(!c.set("speaker", &json!(1)));
        assert!(!c.set("rate", &json!("warp")));
        assert!(!c.set("max_age_secs", &json!(5))); // не настраивается из интерфейса
        assert_eq!(
            (c.mode.as_str(), c.speaker.as_str(), c.rate.as_str(), c.read_intermediate, c.max_age_secs),
            ("auto", "eugene", "x-fast", false, 30)
        );
    }
}
