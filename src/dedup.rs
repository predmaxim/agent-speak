//! Точные повторы фразы в сессии читаем один раз.

use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};

#[derive(Default)]
struct Session {
    hashes: HashSet<u64>,
}

pub struct Dedup {
    sessions: HashMap<String, Session>,
}

impl Dedup {
    pub fn new() -> Dedup {
        Dedup { sessions: HashMap::new() }
    }

    pub fn first_time(&mut self, session: &str, text: &str) -> bool {
        let norm: String = text.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect();
        let mut h = DefaultHasher::new();
        norm.hash(&mut h);
        self.sessions.entry(session.into()).or_default().hashes.insert(h.finish())
    }

    pub fn forget(&mut self, session: &str) {
        self.sessions.remove(session);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalized_repeat_detected() {
        let mut d = Dedup::new();
        assert!(d.first_time("s1", "Готово, всё работает."));
        assert!(!d.first_time("s1", "готово всё работает"));
        assert!(d.first_time("s2", "Готово, всё работает."));
    }

    #[test]
    fn forget_resets_session() {
        let mut d = Dedup::new();
        d.first_time("s1", "Раз");
        d.forget("s1");
        assert!(d.first_time("s1", "Раз"));
    }
}
