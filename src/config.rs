//! ~/.config/agent-speak/config.toml; отсутствующие поля — по умолчанию.

use serde::Serialize;
use std::path::PathBuf;

#[derive(Serialize, Debug, Clone)]
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
    /// спутники агентов: точки у орба, по одной на агента
    pub orb_satellites: bool,
    /// настроение орба: кольцо «готово» до взгляда, «поник» при сбоях агента, волна новости без слов
    pub orb_moods: bool,
    /// редкий вдох орба в покое — только от сети и без «меньше движения»
    pub orb_idle_motion: bool,
    /// сторож проверяет агентов раз в столько секунд
    pub watch_every: u64,
    /// агент «работает», а событий нет столько минут — завис
    pub hung_after: u64,
    /// процент лимита, с которого Ома считает прогноз и подсказывает агентам
    pub limit_threshold: u64,
    /// пока экран показывают — Ома молчит, а пузыри только «проект: вопрос», без текста
    pub screencast_privacy: bool,
    /// кончилась тишина по причине (показ экрана) — одна фраза кодом о накопленном: «Пока было тихо: …»
    pub hush_summary: bool,
    /// пауза музыки (MPRIS), пока Ома говорит и слушает: плагин ставит на паузу игравшее и продолжает его после
    pub music_pause: bool,
    /// предупреждать, что окно лимита почти кончилось («Предупреждать о лимите»): фраза, подсказка орба, строка мозгу
    pub limit_warn: bool,
    /// процент окна лимита, с которого Ома предупреждает пользователя; только config.toml
    pub limit_warn_at: u64,
    /// память между днями: служба пишет пересказы дня, таймер сжимает день, мозг получает заметки и свой файл «Помню»
    pub memory: bool,
    /// итоги ходов и «починила сама» звучат пачкой не чаще раза в столько минут, 0..=60; 0 — сразу, без пачки
    pub news_every: u64,
    /// звуковой знак перед новостью: вопрос, итог, нужна помощь
    pub earcons: bool,
    /// интонация по смыслу: пауза после проекта, вопрос выше, беда ниже и медленнее, итог быстрее (SSML адаптера Silero)
    pub intonation: bool,
    /// агенты пишут Оме (`agent-speak note`): «нужно» агента звучит сразу, видно в окне и на орбе; строка хука SessionStart
    pub agent_inbox: bool,
}

impl Default for Config {
    fn default() -> Config {
        Config { speaker: "kseniya".into(), rate: "medium".into(),
            orb_satellites: true, orb_moods: true, orb_idle_motion: true,
            quiet_schedule: true, quiet_from: "23:00".into(), quiet_to: "08:00".into(), quiet_override: String::new(), agents_voice: true, progress_digest: false, orb_pos: "bottom-right,0,0".into(),
            orb_over_fullscreen: true, watch_every: 60, hung_after: 12, limit_threshold: 95, screencast_privacy: true, hush_summary: true, music_pause: true,
            limit_warn: true, limit_warn_at: 90, memory: true, news_every: 10, earcons: true, intonation: true, agent_inbox: true }
    }
}

pub fn path() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config/agent-speak/config.toml")
}

pub const SPEAKERS: [&str; 5] = ["kseniya", "xenia", "baya", "aidar", "eugene"];
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
            "orb_satellites" if value.is_boolean() => self.orb_satellites = value.as_bool().unwrap(),
            "orb_moods" if value.is_boolean() => self.orb_moods = value.as_bool().unwrap(),
            "orb_idle_motion" if value.is_boolean() => self.orb_idle_motion = value.as_bool().unwrap(),
            "watch_every" if in_range(value, 15, 3600) => self.watch_every = value.as_u64().unwrap(),
            "hung_after" if in_range(value, 2, 240) => self.hung_after = value.as_u64().unwrap(),
            "limit_threshold" if in_range(value, 50, 100) => self.limit_threshold = value.as_u64().unwrap(),
            "screencast_privacy" if value.is_boolean() => self.screencast_privacy = value.as_bool().unwrap(),
            "hush_summary" if value.is_boolean() => self.hush_summary = value.as_bool().unwrap(),
            "music_pause" if value.is_boolean() => self.music_pause = value.as_bool().unwrap(),
            "limit_warn" if value.is_boolean() => self.limit_warn = value.as_bool().unwrap(),
            "limit_warn_at" if in_range(value, 50, 100) => self.limit_warn_at = value.as_u64().unwrap(),
            "memory" if value.is_boolean() => self.memory = value.as_bool().unwrap(),
            "news_every" if in_range(value, 0, 60) => self.news_every = value.as_u64().unwrap(),
            "earcons" if value.is_boolean() => self.earcons = value.as_bool().unwrap(),
            "intonation" if value.is_boolean() => self.intonation = value.as_bool().unwrap(),
            "agent_inbox" if value.is_boolean() => self.agent_inbox = value.as_bool().unwrap(),
            _ => return false,
        }
        true
    }

    /// Файл настроек; нет файла — умолчания. Не читается (битый TOML) — Err: прежние настройки остаются.
    pub fn load() -> Result<Config, String> {
        match std::fs::read_to_string(path()) {
            Ok(s) => parse(&s),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(e.to_string()),
        }
    }

    pub fn save(&self) {
        let p = path();
        // файл, правленный руками, не читается — не затирать: правку пользователя исправляет он сам
        if std::fs::read_to_string(&p).is_ok_and(|s| s.parse::<toml::Table>().is_err()) {
            eprintln!("agent-speak: настройки не сохранены: {} не читается — исправь его", p.display());
            return;
        }
        let _ = std::fs::create_dir_all(p.parent().unwrap());
        // атомарно: наблюдатель не должен увидеть обрезанный файл
        let tmp = p.with_extension("toml.tmp");
        if std::fs::write(&tmp, toml::to_string(self).unwrap()).is_ok() {
            let _ = std::fs::rename(&tmp, p);
        }
    }
}

/// Текст config.toml: каждое значение проверяет тот же `set`, что и настройку из окна; неверное (hung_after = 0
/// заставил бы сторожа жать Esc у всех работающих агентов) — умолчание ключа и строка в журнал. Старые ключи
/// (их нет среди полей) — молча мимо. Битый TOML — Err.
pub fn parse(text: &str) -> Result<Config, String> {
    let table: toml::Table = text.parse().map_err(|e: toml::de::Error| e.to_string())?;
    let mut c = Config::default();
    let known = serde_json::to_value(&c).map_err(|e| e.to_string())?;
    for (key, value) in table.into_iter().filter(|(k, _)| known.get(k).is_some()) {
        if !c.set(&key, &serde_json::to_value(&value).unwrap_or_default()) {
            eprintln!("agent-speak: настройки: неверно {key} = {value}, беру {}", known[&key]);
        }
    }
    Ok(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_partial_file() {
        // старый файл: mode и voice_last_agent больше не поля — игнорируются
        let c = parse("speaker = \"baya\"\nmode = \"auto\"\nvoice_last_agent = \"codex\"").unwrap();
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
        let c = parse("read_intermediate = false\nmax_age_secs = 5\nremind_after = 15\nspeaker = \"baya\"\n").unwrap();
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
        assert!(c.orb_satellites && c.set("orb_satellites", &json!(false)) && !c.orb_satellites);
        assert!(!c.set("orb_satellites", &json!("false")));
        assert!(parse("speaker = \"baya\"").unwrap().orb_satellites, "старый файл без ключа — спутники есть");
        // память между днями — по умолчанию вкл (дизайн «Джарвис»); старый файл без ключа — тоже вкл
        assert!(c.memory && c.set("memory", &json!(false)) && !c.memory);
        assert!(!c.set("memory", &json!("false")));
        assert!(parse("speaker = \"baya\"").unwrap().memory);
    }

    #[test]
    fn quiet_by_reason_switches_default_on() {
        use serde_json::json;
        let old = parse("speaker = \"baya\"").unwrap();
        assert!(old.screencast_privacy && old.hush_summary, "старый файл без ключей — включены");
        let mut c = Config::default();
        assert!(c.set("screencast_privacy", &json!(false)) && !c.screencast_privacy);
        assert!(c.set("hush_summary", &json!(false)) && !c.hush_summary);
        assert!(!c.set("hush_summary", &json!("false")) && !c.set("dnd_quiet", &json!(true)), "«Не беспокоить» ждёт сигнала Omarchy — настройки нет");
    }

    #[test]
    fn orb_moods_and_idle_motion_on_by_default_and_set() {
        use serde_json::json;
        let old = parse("speaker = \"baya\"").unwrap();
        assert!(old.orb_moods && old.orb_idle_motion, "старый файл без ключей — включены");
        let mut c = Config::default();
        assert!(c.set("orb_moods", &json!(false)) && !c.orb_moods);
        assert!(c.set("orb_idle_motion", &json!(false)) && !c.orb_idle_motion);
        assert!(!c.set("orb_moods", &json!("false")) && !c.set("orb_idle_motion", &json!(1)));
    }

    #[test]
    fn music_pause_on_by_default_and_set() {
        use serde_json::json;
        let mut c = Config::default();
        assert!(c.music_pause && parse("speaker = \"baya\"").unwrap().music_pause, "старый файл без ключа — пауза музыки включена");
        assert!(c.set("music_pause", &json!(false)) && !c.music_pause);
        assert!(!c.set("music_pause", &json!("false")));
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
    fn file_values_checked_like_set() {
        // hung_after = 0, вписанный руками, заставил бы сторожа жать Esc у всех работающих агентов
        let c = parse("hung_after = 0\nwatch_every = 1\nspeaker = \"nobody\"\nrate = \"fast\"\nquiet_from = \"25:00\"").unwrap();
        assert_eq!((c.hung_after, c.watch_every, c.speaker.as_str(), c.rate.as_str(), c.quiet_from.as_str()), (12, 60, "kseniya", "fast", "23:00"));
        assert!(parse("hung_after = ").is_err(), "битый файл — ошибка, а не умолчания");
        let good = Config { speaker: "baya".into(), hung_after: 30, ..Config::default() };
        let back = parse(&toml::to_string(&good).unwrap()).unwrap();
        assert_eq!((back.speaker, back.hung_after), (good.speaker, good.hung_after), "сохранённое читается обратно");
    }

    #[test]
    fn watch_keys_default_and_validate() {
        use serde_json::json;
        let mut c = Config::default();
        assert_eq!((c.watch_every, c.hung_after, c.limit_threshold), (60, 12, 95));
        let old = parse("speaker = \"baya\"").unwrap();
        assert_eq!((old.watch_every, old.hung_after), (60, 12), "старый файл без ключей сторожа");
        assert!(!c.set("hung_after", &json!(1)) && !c.set("limit_threshold", &json!(101)) && !c.set("watch_every", &json!(10)));
        assert!(!c.set("remind_after", &json!(15)), "повторов «ждёт ответа» больше нет");
        assert!(!c.set("hung_after", &json!(12.5)));
        assert!(c.set("hung_after", &json!(20)) && c.set("watch_every", &json!(15)) && c.set("limit_threshold", &json!(50)));
        assert_eq!((c.watch_every, c.hung_after, c.limit_threshold), (15, 20, 50));
    }

    #[test]
    fn limit_warn_keys_default_and_validate() {
        use serde_json::json;
        let mut c = Config::default();
        assert_eq!((c.limit_warn, c.limit_warn_at), (true, 90));
        assert!(c.set("limit_warn", &json!(false)) && !c.limit_warn);
        assert!(!c.set("limit_warn", &json!("false")));
        assert!(!c.set("limit_warn_at", &json!(49)) && !c.set("limit_warn_at", &json!(101)) && c.limit_warn_at == 90);
        assert!(c.set("limit_warn_at", &json!(100)) && c.limit_warn_at == 100);
        assert_eq!(parse("limit_warn_at = 30").unwrap().limit_warn_at, 90, "из файла — так же, как set");
    }

    #[test]
    fn lanes_keys_default_and_validate() {
        use serde_json::json;
        let mut c = Config::default();
        assert_eq!((c.news_every, c.earcons, c.intonation), (10, true, true));
        assert!(c.set("news_every", &json!(0)) && c.news_every == 0, "0 — без пачки, как раньше");
        assert!(c.set("news_every", &json!(60)) && !c.set("news_every", &json!(61)) && !c.set("news_every", &json!("5")) && c.news_every == 60);
        assert!(c.set("earcons", &json!(false)) && !c.earcons && !c.set("earcons", &json!(1)));
        assert!(c.set("intonation", &json!(false)) && !c.intonation && !c.set("intonation", &json!("on")));
        assert_eq!(parse("news_every = -1").unwrap().news_every, 10, "из файла — так же, как set");
    }

    #[test]
    fn agent_inbox_on_by_default() {
        let mut c = Config::default();
        assert!(c.agent_inbox);
        assert!(c.set("agent_inbox", &serde_json::json!(false)) && !c.agent_inbox && !c.set("agent_inbox", &serde_json::json!("on")));
    }
}
