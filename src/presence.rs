//! Присутствие пользователя: чистая машина состояний по снимкам сервиса `predmaxim.presence`.
//! Ввода-вывода нет; время — эпоха в секундах, как у доски.

use serde::{Deserialize, Deserializer};

pub const UNSURE_AFTER: u64 = 90;
pub const AWAY_AFTER: u64 = 600;
pub const BACK_AFTER: u64 = 180; // отсутствие не короче — приветствие и повтор
pub const BUSY_FOR: u64 = 900;
pub const AWAY_GRACE: u64 = 60;
pub const STALE_AFTER: u64 = 90; // сервис пишет снимок раз в 30 с; старше — фактов нет
pub const FILE: &str = "predmaxim-presence.json"; // в $XDG_RUNTIME_DIR

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Here,
    Busy,
    Unsure,
    Away,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Buds {
    #[default]
    Unknown,
    In,
    Out,
}

fn buds_from<'de, D: Deserializer<'de>>(d: D) -> Result<Buds, D::Error> {
    Ok(match Option::<String>::deserialize(d)?.as_deref() {
        Some("in") => Buds::In,
        Some("out") => Buds::Out,
        _ => Buds::Unknown,
    })
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Facts {
    pub at: u64,
    pub idle_since: u64,
    pub locked: bool,
    pub mic: bool,
    pub camera: bool,
    pub video: bool,
    #[serde(deserialize_with = "buds_from")]
    pub buds: Buds,
}

pub fn parse_facts(s: &str) -> Option<Facts> {
    serde_json::from_str(s).ok()
}

#[derive(Default)]
pub struct Presence {
    facts: Option<Facts>,
    last_input: u64,
    busy_until: u64,
    away: Option<u64>,       // «я отойду» / «Ты тут?» без ответа — до ввода позже этого срока
    buds_dropped: bool,      // наушники вынуты — «не уверена» до ввода
}

impl Presence {
    /// Режим по последнему снимку; `stale` — учитывать ли устаревание снимка.
    fn mode_at(&self, now: u64, stale: bool) -> Mode {
        let Some(f) = &self.facts else { return Mode::Here };
        if stale && now.saturating_sub(f.at) > STALE_AFTER {
            return Mode::Here;
        }
        if f.locked || self.away.is_some() {
            return Mode::Away;
        }
        if f.mic || f.camera || f.video || self.busy_until > now {
            return Mode::Busy;
        }
        if f.buds == Buds::In {
            return Mode::Here;
        }
        if self.buds_dropped {
            return Mode::Unsure;
        }
        match now.saturating_sub(self.last_input) {
            i if i >= AWAY_AFTER => Mode::Away,
            i if i >= UNSURE_AFTER => Mode::Unsure,
            _ => Mode::Here,
        }
    }

    fn input(&mut self, now: u64) {
        self.last_input = now;
        self.buds_dropped = false;
        // ввод в отсрочке «я отойду» away не снимает
        if self.away.is_some_and(|until| now > until) {
            self.away = None;
        }
    }

    pub fn apply(&mut self, f: Facts, now: u64) -> Option<u64> {
        let before = self.mode_at(now, false);
        let old = self.facts.take().unwrap_or_default();
        let was_in = old.buds == Buds::In;
        let typed = (old.idle_since != 0 && f.idle_since == 0) || (old.locked && !f.locked);
        let absence = now.saturating_sub(self.last_input);
        if f.idle_since == 0 {
            // непрерывный ввод тоже снимает away и «наушники вынуты»; typed — только для возврата
            self.input(now);
        } else {
            // голос двигает last_input вперёд, простой клавиатуры его не откатывает
            self.last_input = self.last_input.max(f.idle_since);
        }
        if (old.video || old.mic || old.camera) && !(f.video || f.mic || f.camera || f.locked) {
            self.last_input = now; // видео или звонок смолкли сами, экран не заблокирован — он тут: новости без приветствия
        }
        if was_in && f.buds == Buds::Out {
            self.buds_dropped = true;
        }
        self.facts = Some(f);
        (typed && matches!(before, Mode::Unsure | Mode::Away)).then_some(absence)
    }

    pub fn said(&mut self, m: Mode, now: u64) -> Option<u64> {
        match m {
            Mode::Here => {
                let absence = now.saturating_sub(self.last_input);
                let back = matches!(self.mode_at(now, false), Mode::Unsure | Mode::Away);
                self.last_input = now;
                self.buds_dropped = false;
                self.busy_until = 0;
                self.away = None;
                back.then_some(absence)
            }
            Mode::Busy => {
                self.last_input = now;
                self.busy_until = now + BUSY_FOR;
                None
            }
            _ => {
                self.last_input = now;
                self.away = Some(now + AWAY_GRACE);
                None
            }
        }
    }

    /// «Ты тут?» без ответа: отсрочки нет — первый же ввод и есть возвращение.
    pub fn unanswered(&mut self) {
        self.away = Some(0);
    }

    /// Ноутбук проснулся (slept — последний tick до сна): снимок, свежий до сна, держится ещё STALE_AFTER — пока сервис не написал новый, блокировка и простой в силе.
    pub fn woke(&mut self, slept: u64, now: u64) {
        if let Some(f) = self.facts.as_mut().filter(|f| slept.saturating_sub(f.at) <= STALE_AFTER) {
            f.at = now;
        }
    }

    pub fn mode(&self, now: u64) -> Mode {
        self.mode_at(now, true)
    }

    pub fn last_input(&self, now: u64) -> u64 {
        if self.last_input == 0 { now } else { self.last_input }
    }

    pub fn sure(&self) -> bool {
        self.facts.as_ref().is_some_and(|f| f.buds == Buds::In)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(at: u64, idle_since: u64) -> Facts {
        Facts { at, idle_since, ..Default::default() }
    }

    #[test]
    fn parse_facts_tolerates_junk() {
        assert_eq!(parse_facts("{"), None);
        assert_eq!(parse_facts("{}"), Some(Facts::default()));
        assert_eq!(parse_facts(r#"{"buds":"in"}"#).unwrap().buds, Buds::In);
        assert_eq!(parse_facts(r#"{"buds":"out"}"#).unwrap().buds, Buds::Out);
        assert_eq!(parse_facts(r#"{"buds":"x"}"#).unwrap().buds, Buds::Unknown);
    }

    #[test]
    fn idle_thresholds() {
        let mut p = Presence::default();
        p.apply(facts(1005, 1000), 1005);
        assert_eq!([p.mode(1089), p.mode(1090)], [Mode::Here, Mode::Unsure]);
        p.apply(facts(1595, 1000), 1595);
        assert_eq!([p.mode(1599), p.mode(1600)], [Mode::Unsure, Mode::Away]);
    }

    #[test]
    fn input_returns_absence_only_after_unsure_or_away() {
        let mut p = Presence::default();
        p.apply(facts(1030, 1000), 1030);
        assert_eq!(p.apply(facts(1060, 0), 1060), None);
        p.apply(facts(1230, 1060), 1230);
        assert_eq!(p.apply(facts(1260, 0), 1260), Some(200));
    }

    #[test]
    fn busy_beats_idle_but_not_lock() {
        let mut p = Presence::default();
        let mut f = facts(1700, 1000);
        f.video = true;
        p.apply(f.clone(), 1700);
        assert_eq!(p.mode(1700), Mode::Busy);
        f.locked = true;
        p.apply(f, 1700);
        assert_eq!(p.mode(1700), Mode::Away);
        for (mic, camera) in [(true, false), (false, true)] {
            let mut f = facts(1700, 1000);
            f.mic = mic;
            f.camera = camera;
            p.apply(f, 1700);
            assert_eq!(p.mode(1700), Mode::Busy);
        }
    }

    #[test]
    fn buds_in_beats_idle() {
        let mut p = Presence::default();
        let mut f = facts(1700, 1000);
        f.buds = Buds::In;
        p.apply(f, 1700);
        assert_eq!(p.mode(1700), Mode::Here);
        assert!(p.sure());
    }

    #[test]
    fn buds_out_unsure_until_input() {
        let mut p = Presence::default();
        let mut f = facts(1010, 1000);
        f.buds = Buds::In;
        p.apply(f.clone(), 1010);
        f.buds = Buds::Out;
        p.apply(f, 1010);
        assert_eq!(p.mode(1010), Mode::Unsure);
        assert!(!p.sure());
        let mut f = facts(1020, 0);
        f.buds = Buds::Out;
        p.apply(f, 1020);
        assert_eq!(p.mode(1020), Mode::Here);
    }

    #[test]
    fn said_away_has_grace() {
        let mut p = Presence::default();
        p.apply(facts(1000, 0), 1000);
        p.said(Mode::Away, 1000);
        assert_eq!(p.mode(1000), Mode::Away);
        p.apply(facts(1030, 1000), 1030);
        p.apply(facts(1030, 0), 1030);
        assert_eq!(p.mode(1030), Mode::Away);
        p.apply(facts(1061, 1031), 1061);
        p.apply(facts(1061, 0), 1061);
        assert_eq!(p.mode(1061), Mode::Here);
    }

    #[test]
    fn said_busy_for_fifteen_minutes() {
        let mut p = Presence::default();
        p.apply(facts(1000, 0), 1000);
        assert_eq!(p.said(Mode::Busy, 1000), None);
        p.apply(facts(1890, 0), 1890);
        assert_eq!([p.mode(1899), p.mode(1900)], [Mode::Busy, Mode::Here]);
    }

    #[test]
    fn said_here_clears_busy_and_away() {
        let mut p = Presence::default();
        p.apply(facts(1000, 0), 1000);
        p.said(Mode::Busy, 1000);
        p.said(Mode::Away, 1001);
        assert_eq!(p.mode(1002), Mode::Away);
        p.said(Mode::Here, 1002);
        assert_eq!(p.mode(1003), Mode::Here);
    }

    #[test]
    fn unanswered_is_away_until_input() {
        let mut p = Presence::default();
        p.apply(facts(1000, 1000), 1000);
        p.unanswered();
        p.apply(facts(1030, 1000), 1030);
        p.apply(facts(1100, 1000), 1100);
        assert_eq!(p.mode(1100), Mode::Away);
        p.apply(facts(1130, 0), 1130);
        assert_eq!(p.mode(1130), Mode::Here);
    }

    #[test]
    fn unanswered_has_no_grace() {
        let mut p = Presence::default();
        p.apply(facts(1100, 1000), 1100);
        p.unanswered();
        assert_eq!(p.mode(1100), Mode::Away);
        assert_eq!(p.apply(facts(1130, 0), 1130), Some(130));
        assert_eq!(p.mode(1130), Mode::Here);
    }

    #[test]
    fn continuous_input_clears_away_after_grace() {
        let mut p = Presence::default();
        p.apply(facts(1000, 0), 1000);
        p.said(Mode::Away, 1000);
        p.apply(facts(1030, 0), 1030);
        assert_eq!(p.mode(1030), Mode::Away);
        p.apply(facts(1090, 0), 1090);
        assert_eq!(p.mode(1090), Mode::Here);
    }

    #[test]
    fn no_facts_is_here() {
        let p = Presence::default();
        assert_eq!([p.mode(0), p.mode(99999)], [Mode::Here, Mode::Here]);
    }

    #[test]
    fn stale_facts_are_here() {
        let mut p = Presence::default();
        p.apply(facts(1005, 1000), 1005);
        assert_eq!(p.mode(1005 + STALE_AFTER + 1), Mode::Here);
    }

    #[test]
    fn woke_keeps_only_fresh_snapshot() {
        let mut f = facts(1000, 990);
        f.locked = true;
        let mut p = Presence::default();
        p.apply(f.clone(), 1000);
        p.woke(1010, 4600);
        assert_eq!(p.mode(4600 + STALE_AFTER), Mode::Away);
        let mut p = Presence::default();
        p.apply(f, 1000);
        p.woke(1000 + STALE_AFTER + 1, 4600); // сервис замолк ещё до сна — фактов нет
        assert_eq!(p.mode(4600), Mode::Here);
    }

    #[test]
    fn wake_from_sleep_is_return() {
        let mut p = Presence::default();
        p.apply(facts(1005, 1000), 1005);
        assert_eq!(p.apply(facts(4600, 0), 4600), Some(3600));
    }
}
