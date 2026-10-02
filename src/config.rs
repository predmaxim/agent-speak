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

impl Config {
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
}
