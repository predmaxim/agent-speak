//! Очередь фраз: устаревшие статусы выбрасываются, срочные вперёд, на паузе возраст не растёт.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    Status,
    Urgent,
    Manual,
}

#[derive(Clone, Debug)]
pub struct Item {
    pub session: String,
    pub text: String,
    pub kind: Kind,
    pub born: Instant,
}

pub struct Queue {
    items: VecDeque<Item>,
    max_age: Duration,
    paused_at: Option<Instant>,
    paused_total: Duration,
}

impl Queue {
    pub fn new(max_age: Duration) -> Queue {
        Queue { items: VecDeque::new(), max_age, paused_at: None, paused_total: Duration::ZERO }
    }

    pub fn set_max_age(&mut self, d: Duration) {
        self.max_age = d;
    }

    /// born хранится со сдвигом назад на уже набранную паузу, поэтому
    /// возраст = (now - paused_total) - born = now - born - пауза_после_постановки.
    pub fn push(&mut self, mut item: Item) {
        item.born = item.born.checked_sub(self.paused_total).unwrap_or(item.born);
        if item.kind == Kind::Urgent {
            let pos = self.items.iter().take_while(|i| i.kind == Kind::Urgent).count();
            self.items.insert(pos, item);
        } else {
            self.items.push_back(item);
        }
    }

    pub fn push_front(&mut self, item: Item) {
        self.items.push_front(item);
    }

    pub fn pop(&mut self, now: Instant) -> Option<Item> {
        if self.paused_at.is_some() {
            return None;
        }
        let eff_now = now.checked_sub(self.paused_total).unwrap_or(now);
        while let Some(item) = self.items.pop_front() {
            let age = eff_now.saturating_duration_since(item.born);
            if item.kind == Kind::Status && age > self.max_age {
                continue;
            }
            return Some(item);
        }
        None
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

    pub fn clear(&mut self) {
        self.items.clear();
    }

    pub fn clear_session(&mut self, session: &str) {
        self.items.retain(|i| i.session != session);
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn it(text: &str, kind: Kind, born: Instant) -> Item {
        Item { session: "s".into(), text: text.into(), kind, born }
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
        q.push(Item { session: "a".into(), text: "1".into(), kind: Kind::Status, born: t0 });
        q.push(Item { session: "b".into(), text: "2".into(), kind: Kind::Status, born: t0 });
        q.clear_session("a");
        assert_eq!(q.pop(t0).unwrap().text, "2");
    }
}
