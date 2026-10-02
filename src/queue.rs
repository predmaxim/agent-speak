//! Очередь фраз: устаревшие статусы выбрасываются, срочные вперёд, на паузе возраст не растёт.
//! Играет только активная сессия (и срочные/образец); фразы остальных ждут своей очереди.

use std::collections::{HashSet, VecDeque};
use std::time::{Duration, Instant};

/// Финальный ответ, не дождавшийся своей сессии за это время, уже никому не нужен.
pub const FINAL_MAX_AGE: Duration = Duration::from_secs(30 * 60);

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    Status,
    Urgent,
    Manual,
    Preview, // образец голоса: один в очереди, впереди всех, играет и на паузе
}

#[derive(Clone, Debug)]
pub struct Item {
    pub session: String,
    pub text: String,
    pub kind: Kind,
    pub born: Instant,
    pub msg: String, // ключ сообщения: начатое сообщение дочитывается до конца, не устаревая
}

pub struct Queue {
    items: VecDeque<Item>,
    max_age: Duration,
    paused_at: Option<Instant>,
    paused_total: Duration,
    active: String,                 // активная сессия; пусто — ещё не выбрана, играют все
    popped: Option<(String, Kind)>,     // последняя отданная фраза — «что сейчас звучит»
    started: HashSet<(String, String)>, // (сессия, сообщение) начаты и не дочитаны — не устаревают
}

impl Queue {
    pub fn new(max_age: Duration) -> Queue {
        Queue { items: VecDeque::new(), max_age, paused_at: None, paused_total: Duration::ZERO, active: String::new(), popped: None, started: HashSet::new() }
    }

    pub fn set_max_age(&mut self, d: Duration) {
        self.max_age = d;
    }

    /// born хранится со сдвигом назад на уже набранную паузу, поэтому
    /// возраст = (now - paused_total) - born = now - born - пауза_после_постановки.
    pub fn push(&mut self, mut item: Item) {
        item.born = item.born.checked_sub(self.paused_total).unwrap_or(item.born);
        if item.kind == Kind::Preview {
            self.items.retain(|i| i.kind != Kind::Preview);
            self.items.push_front(item);
        } else if item.kind == Kind::Urgent {
            let pos = self.items.iter().take_while(|i| i.kind == Kind::Urgent).count();
            self.items.insert(pos, item);
        } else {
            self.items.push_back(item);
        }
    }

    /// Прерванная фраза встаёт за образцом голоса: образец играет первым (на паузе — иначе он застрянет).
    pub fn push_front(&mut self, item: Item) {
        let pos = self.items.iter().take_while(|i| i.kind == Kind::Preview).count();
        self.items.insert(pos, item);
    }

    pub fn pop(&mut self, now: Instant) -> Option<Item> {
        if self.paused_at.is_some() {
            return if self.items.front().is_some_and(|i| i.kind == Kind::Preview) { self.items.pop_front() } else { None };
        }
        let eff_now = now.checked_sub(self.paused_total).unwrap_or(now);
        let max_age = self.max_age;
        let (started, active) = (&self.started, &self.active);
        self.items.retain(|i| {
            let age = eff_now.saturating_duration_since(i.born);
            let stale = match i.kind {
                Kind::Status => age > max_age && !started.contains(&(i.session.clone(), i.msg.clone())),
                Kind::Manual => age > FINAL_MAX_AGE && i.session != *active,
                _ => false,
            };
            if stale {
                eprintln!("agent-speak: устарело, пропущено ({}): {}", i.session, i.text.chars().take(40).collect::<String>());
            }
            !stale
        });
        let active = &self.active;
        let pos = self.items.iter().position(|i| {
            active.is_empty() || i.session == *active || matches!(i.kind, Kind::Urgent | Kind::Preview)
        })?;
        let item = self.items.remove(pos)?;
        self.popped = Some((item.session.clone(), item.kind.clone()));
        let key = (item.session.clone(), item.msg.clone());
        if self.items.iter().any(|i| i.session == key.0 && i.msg == key.1) {
            self.started.insert(key);
        } else {
            self.started.remove(&key);
        }
        Some(item)
    }

    /// Смена активной сессии: не начатые чужие статусы выбрасываются, остальное ждёт возврата.
    pub fn set_active(&mut self, session: &str) {
        self.active = session.to_string();
        let started = &self.started;
        self.items.retain(|i| i.kind != Kind::Status || i.session == session || started.contains(&(i.session.clone(), i.msg.clone())));
    }

    /// Есть ли что играть сейчас (без учёта паузы).
    pub fn ready(&self) -> bool {
        self.items.iter().any(|i| self.active.is_empty() || i.session == self.active || matches!(i.kind, Kind::Urgent | Kind::Preview))
    }

    /// В очереди ждёт финальный ответ сессии.
    pub fn has_final(&self, session: &str) -> bool {
        self.items.iter().any(|i| i.session == session && i.kind == Kind::Manual)
    }

    /// Последней отдан финальный ответ сессии (звучит ли он — знает speaker через busy).
    pub fn popped_final(&self, session: &str) -> bool {
        self.popped.as_ref().is_some_and(|(s, k)| s == session && *k == Kind::Manual)
    }

    pub fn pause(&mut self, now: Instant) {
        self.paused_at.get_or_insert(now);
    }

    pub fn resume(&mut self, now: Instant) {
        if let Some(p) = self.paused_at.take() {
            self.paused_total += now.saturating_duration_since(p);
        }
    }

    pub fn paused(&self) -> bool {
        self.paused_at.is_some()
    }

    /// Стоп: молчит активная сессия; финалы остальных ждут своей очереди.
    pub fn clear_active(&mut self) {
        let active = self.active.clone();
        self.items.retain(|i| !active.is_empty() && i.session != active && i.kind == Kind::Manual);
        self.started.retain(|(s, _)| !active.is_empty() && *s != active);
    }

    pub fn clear_session(&mut self, session: &str) {
        self.items.retain(|i| i.session != session);
        if self.popped.as_ref().is_some_and(|(s, _)| s == session) {
            self.popped = None;
        }
        self.started.retain(|(s, _)| s != session);
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn it(text: &str, kind: Kind, born: Instant) -> Item {
        Item { session: "s".into(), text: text.into(), kind, born, msg: "m".into() }
    }

    #[test]
    fn stale_status_dropped_manual_kept() {
        let t0 = Instant::now();
        let mut q = Queue::new(Duration::from_secs(30));
        q.push(it("старый", Kind::Status, t0));
        q.push(it("ручной", Kind::Manual, t0));
        let later = t0 + Duration::from_secs(31);
        assert_eq!(q.pop(later).unwrap().text, "ручной");
        assert!(q.pop(later).is_none());
    }

    #[test]
    fn urgent_goes_first_in_order() {
        let t0 = Instant::now();
        let mut q = Queue::new(Duration::from_secs(30));
        q.push(it("a", Kind::Status, t0));
        q.push(it("u1", Kind::Urgent, t0));
        q.push(it("u2", Kind::Urgent, t0));
        let got: Vec<String> = std::iter::from_fn(|| q.pop(t0)).map(|i| i.text).collect();
        assert_eq!(got, vec!["u1", "u2", "a"]);
    }

    #[test]
    fn pause_freezes_age() {
        let t0 = Instant::now();
        let mut q = Queue::new(Duration::from_secs(30));
        q.push(it("статус", Kind::Status, t0));
        q.pause(t0 + Duration::from_secs(5));
        assert!(q.pop(t0 + Duration::from_secs(10)).is_none());
        q.resume(t0 + Duration::from_secs(65)); // пауза 60 с
        assert_eq!(q.pop(t0 + Duration::from_secs(66)).unwrap().text, "статус");
    }

    #[test]
    fn clear_session_only() {
        let t0 = Instant::now();
        let mut q = Queue::new(Duration::from_secs(30));
        q.push(Item { session: "a".into(), text: "1".into(), kind: Kind::Status, born: t0, msg: "m".into() });
        q.push(Item { session: "b".into(), text: "2".into(), kind: Kind::Status, born: t0, msg: "m".into() });
        q.clear_session("a");
        assert_eq!(q.pop(t0).unwrap().text, "2");
    }

    #[test]
    fn preview_supersedes_previous_and_goes_first() {
        let t0 = Instant::now();
        let mut q = Queue::new(Duration::from_secs(30));
        q.push(it("m", Kind::Manual, t0));
        q.push(it("p1", Kind::Preview, t0));
        q.push(it("u", Kind::Urgent, t0));
        q.push(it("p2", Kind::Preview, t0));
        let got: Vec<String> = std::iter::from_fn(|| q.pop(t0 + Duration::from_secs(999))).map(|i| i.text).collect();
        assert_eq!(got, vec!["p2", "u", "m"]);
    }

    #[test]
    fn paused_pop_returns_only_front_preview() {
        let t0 = Instant::now();
        let mut q = Queue::new(Duration::from_secs(30));
        q.push(it("m", Kind::Manual, t0));
        q.pause(t0);
        assert!(q.pop(t0).is_none());
        q.push(it("p", Kind::Preview, t0));
        assert_eq!(q.pop(t0).unwrap().text, "p");
        assert!(q.paused());
        assert!(q.pop(t0).is_none());
    }

    #[test]
    fn interrupted_item_stays_behind_preview_and_next_after_it() {
        let t0 = Instant::now();
        let mut q = Queue::new(Duration::from_secs(30));
        q.push(it("rest", Kind::Manual, t0));
        q.pause(t0);
        q.push(it("p", Kind::Preview, t0)); // образец пришёл раньше, чем speaker вернул фразу
        q.push_front(it("interrupted", Kind::Manual, t0));
        assert_eq!(q.pop(t0).unwrap().text, "p");
        assert!(q.pop(t0).is_none());
        q.resume(t0);
        assert_eq!(q.pop(t0).unwrap().text, "interrupted");
        assert_eq!(q.pop(t0).unwrap().text, "rest");
    }

    fn at(session: &str, text: &str, kind: Kind, born: Instant) -> Item {
        Item { session: session.into(), text: text.into(), kind, born, msg: "m".into() }
    }

    #[test]
    fn pop_takes_active_session_and_urgent_others_wait() {
        let t0 = Instant::now();
        let mut q = Queue::new(Duration::from_secs(30));
        q.set_active("a");
        q.push(at("b", "b1", Kind::Manual, t0));
        q.push(at("a", "a1", Kind::Manual, t0));
        q.push(at("b", "срочно", Kind::Urgent, t0));
        let got: Vec<String> = std::iter::from_fn(|| q.pop(t0)).map(|i| i.text).collect();
        assert_eq!(got, vec!["срочно", "a1"]);
        q.set_active("b");
        assert_eq!(q.pop(t0).unwrap().text, "b1");
    }

    #[test]
    fn switch_drops_statuses_of_other_sessions() {
        let t0 = Instant::now();
        let mut q = Queue::new(Duration::from_secs(30));
        q.set_active("a");
        q.push(at("a", "статус", Kind::Status, t0));
        q.push(at("a", "финал", Kind::Manual, t0));
        q.set_active("b");
        q.set_active("a");
        assert_eq!(q.pop(t0).unwrap().text, "финал");
        assert!(q.pop(t0).is_none());
    }

    #[test]
    fn final_of_session_known_queued_or_last_popped() {
        let t0 = Instant::now();
        let mut q = Queue::new(Duration::from_secs(30));
        q.push(at("a", "финал", Kind::Manual, t0));
        assert!(q.has_final("a") && !q.has_final("b"));
        q.pop(t0);
        assert!(!q.has_final("a"));
        assert!(q.popped_final("a"));
        q.clear_session("a");
        assert!(!q.popped_final("a"));
    }

    #[test]
    fn long_message_read_to_the_end() {
        // одно сообщение — 10 предложений-статусов разом; чтение дольше max_age
        let t0 = Instant::now();
        let mut q = Queue::new(Duration::from_secs(30));
        for n in 0..10 {
            q.push(it(&n.to_string(), Kind::Status, t0));
        }
        let got: Vec<String> = (1..=10).filter_map(|k| q.pop(t0 + Duration::from_secs(5 * k))).map(|i| i.text).collect();
        assert_eq!(got.len(), 10, "{got:?}");
    }

    fn msg(session: &str, text: &str, kind: Kind, born: Instant, m: &str) -> Item {
        Item { session: session.into(), text: text.into(), kind, born, msg: m.into() }
    }

    #[test]
    fn clear_active_keeps_finals_of_other_sessions() {
        let t0 = Instant::now();
        let mut q = Queue::new(Duration::from_secs(30));
        q.set_active("a");
        q.push(at("a", "а", Kind::Manual, t0));
        q.push(at("b", "финал бэ", Kind::Manual, t0));
        q.push(at("c", "срочно", Kind::Urgent, t0));
        q.clear_active();
        assert!(q.pop(t0).is_none());
        q.set_active("b");
        assert_eq!(q.pop(t0).unwrap().text, "финал бэ");
    }

    #[test]
    fn stale_final_of_non_active_dropped_after_cap() {
        let t0 = Instant::now();
        let mut q = Queue::new(Duration::from_secs(30));
        q.set_active("a");
        q.push(at("b", "старый финал", Kind::Manual, t0));
        q.push(at("a", "свой старый", Kind::Manual, t0));
        let later = t0 + FINAL_MAX_AGE + Duration::from_secs(1);
        assert_eq!(q.pop(later).unwrap().text, "свой старый");
        assert!(!q.ready());
        q.set_active("b");
        assert!(q.pop(later).is_none());
    }

    #[test]
    fn started_message_survives_urgent_and_switch_away() {
        let t0 = Instant::now();
        let mut q = Queue::new(Duration::from_secs(30));
        q.set_active("a");
        for n in 0..3 {
            q.push(msg("a", &format!("m{n}"), Kind::Status, t0, "m"));
        }
        q.push(msg("a", "не начато", Kind::Status, t0, "n"));
        assert_eq!(q.pop(t0).unwrap().text, "m0");
        q.push(msg("c", "срочно", Kind::Urgent, t0, "u"));
        assert_eq!(q.pop(t0).unwrap().text, "срочно");
        q.set_active("b"); // уход: начатое сообщение ждёт, не начатое выброшено
        q.set_active("a");
        let late = t0 + Duration::from_secs(60);
        let got: Vec<String> = std::iter::from_fn(|| q.pop(late)).map(|i| i.text).collect();
        assert_eq!(got, vec!["m1", "m2"]);
    }
}
