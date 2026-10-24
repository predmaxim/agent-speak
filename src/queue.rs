//! Очередь фраз: образец голоса впереди всех, срочные — за ним, остальное по порядку.
//! Пока Ома думает или говорит, срочное ждёт; на паузе играет только образец.

use std::collections::VecDeque;

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    Urgent,  // вопрос агента: вперёд остальных, но за Омой
    Read,    // ручное чтение (read)
    Preview, // образец голоса: один в очереди, впереди всех, играет и на паузе
    Voice,   // голосовой разговор
}

#[derive(Clone, Debug)]
pub struct Item {
    pub session: String,
    pub text: String,
    pub kind: Kind,
}

pub struct Queue {
    items: VecDeque<Item>,
    paused: bool,
    hold: bool, // Ома думает или говорит: срочное ждёт
}

impl Queue {
    pub fn new() -> Queue {
        Queue { items: VecDeque::new(), paused: false, hold: false }
    }

    pub fn push(&mut self, item: Item) {
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

    pub fn set_hold(&mut self, on: bool) {
        self.hold = on;
    }

    /// Выбросить не слышное сейчас (тихий режим); см. daemon::audible.
    pub fn retain_audible(&mut self, quiet: bool) {
        self.items.retain(|i| crate::daemon::audible(&i.kind, quiet));
    }

    pub fn held(&self) -> bool {
        self.hold
    }

    /// Играет ли фраза сейчас: на удержании срочное ждёт Ому.
    fn playable(&self, i: &Item) -> bool {
        !(self.hold && i.kind == Kind::Urgent)
    }

    /// Прерванная фраза встаёт за образцом голоса: образец играет первым (на паузе — иначе он застрянет).
    pub fn push_front(&mut self, item: Item) {
        let pos = self.items.iter().take_while(|i| i.kind == Kind::Preview).count();
        self.items.insert(pos, item);
    }

    pub fn pop(&mut self) -> Option<Item> {
        if self.paused {
            return if self.items.front().is_some_and(|i| i.kind == Kind::Preview) { self.items.pop_front() } else { None };
        }
        let pos = self.items.iter().position(|i| self.playable(i))?;
        self.items.remove(pos)
    }

    /// Есть ли что играть сейчас (без учёта паузы).
    pub fn ready(&self) -> bool {
        self.items.iter().any(|i| self.playable(i))
    }

    pub fn pause(&mut self) {
        self.paused = true;
    }

    pub fn resume(&mut self) {
        self.paused = false;
    }

    pub fn paused(&self) -> bool {
        self.paused
    }

    /// Стоп: молчит всё.
    pub fn clear(&mut self) {
        self.items.clear();
    }

    /// Ому перебили: её ответ не нужен; срочное, чтение и образец ждут своей очереди.
    pub fn hush(&mut self) {
        self.items.retain(|i| i.kind != Kind::Voice);
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn it(text: &str, kind: Kind) -> Item {
        Item { session: "s".into(), text: text.into(), kind }
    }

    fn all(q: &mut Queue) -> Vec<String> {
        std::iter::from_fn(|| q.pop()).map(|i| i.text).collect()
    }

    #[test]
    fn hold_keeps_urgent_until_released() {
        let mut q = Queue::new();
        q.push(it("срочно", Kind::Urgent));
        q.push(it("ответ", Kind::Voice));
        q.set_hold(true);
        assert_eq!(q.pop().unwrap().text, "ответ");
        assert!(q.pop().is_none() && !q.ready());
        q.set_hold(false);
        assert_eq!(q.pop().unwrap().text, "срочно");
    }

    #[test]
    fn hush_drops_oma_keeps_urgent_and_read() {
        let mut q = Queue::new();
        q.set_hold(true); // Ома говорит: срочное ждёт
        q.push(it("чтение", Kind::Read));
        q.push(it("срочно", Kind::Urgent));
        q.push(it("ответ дальше", Kind::Voice));
        q.hush();
        q.set_hold(false);
        assert_eq!(all(&mut q), vec!["срочно", "чтение"]);
    }

    #[test]
    fn urgent_goes_first_in_order() {
        let mut q = Queue::new();
        q.push(it("a", Kind::Read));
        q.push(it("u1", Kind::Urgent));
        q.push(it("u2", Kind::Urgent));
        assert_eq!(all(&mut q), vec!["u1", "u2", "a"]);
    }

    #[test]
    fn preview_supersedes_previous_and_goes_first() {
        let mut q = Queue::new();
        q.push(it("r", Kind::Read));
        q.push(it("p1", Kind::Preview));
        q.push(it("u", Kind::Urgent));
        q.push(it("p2", Kind::Preview));
        assert_eq!(all(&mut q), vec!["p2", "u", "r"]);
    }

    #[test]
    fn paused_pop_returns_only_front_preview() {
        let mut q = Queue::new();
        q.push(it("r", Kind::Read));
        q.pause();
        assert!(q.pop().is_none());
        q.push(it("p", Kind::Preview));
        assert_eq!(q.pop().unwrap().text, "p");
        assert!(q.paused());
        assert!(q.pop().is_none());
    }

    #[test]
    fn interrupted_item_stays_behind_preview_and_next_after_it() {
        let mut q = Queue::new();
        q.push(it("rest", Kind::Read));
        q.pause();
        q.push(it("p", Kind::Preview)); // образец пришёл раньше, чем speaker вернул фразу
        q.push_front(it("interrupted", Kind::Read));
        assert_eq!(q.pop().unwrap().text, "p");
        assert!(q.pop().is_none());
        q.resume();
        assert_eq!(all(&mut q), vec!["interrupted", "rest"]);
    }
}
