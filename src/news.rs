//! Доска новостей Омы: что говорить следующим — восстановление, вопросы по одному, итоги, промежуточное.

/// Порядок вариантов — приоритет.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Class {
    Restore,
    Question,
    Final,
    Progress,
}

#[derive(Clone, Debug)]
pub struct News {
    pub session: String,
    pub class: Class,
    pub say: String, // уже с «проект: »
    pub born: u64,
}

/// После озвученного вопроса итоги ждут ответа не дольше этого (с).
pub const ANSWER_WAIT: u64 = 120;

#[derive(Default)]
pub struct Board {
    items: Vec<News>,
    asked: Option<(String, u64)>, // (сессия, когда озвучен) — вопрос, на который ждём ответ
}

impl Board {
    /// Новость той же сессии и класса заменяет прежнюю: старое уже неактуально.
    pub fn push(&mut self, n: News) {
        self.items.retain(|i| !(i.session == n.session && i.class == n.class));
        self.items.push(n);
    }

    /// Пользователь ответил (хоть в терминале раньше Омы): ждать и спрашивать больше нечего.
    pub fn answered(&mut self, session: &str) {
        if self.asked.as_ref().is_some_and(|(s, _)| s == session) {
            self.asked = None;
        }
        self.items.retain(|i| !(i.session == session && i.class == Class::Question));
    }

    pub fn retain_sessions(&mut self, alive: impl Fn(&str) -> bool) {
        self.items.retain(|i| alive(&i.session));
        // закрытый агент не ответит: итоги не ждут его ANSWER_WAIT
        if self.asked.as_ref().is_some_and(|(s, _)| !alive(s)) {
            self.asked = None;
        }
    }

    pub fn retain_classes(&mut self, keep: &[Class]) {
        self.items.retain(|i| keep.contains(&i.class));
    }

    /// Фраза следующей пачки; взятое с доски снимается.
    pub fn take(&mut self, now: u64) -> Option<String> {
        let mut waiting = self.asked.as_ref().is_some_and(|(_, at)| now < at + ANSWER_WAIT);
        if !waiting {
            self.asked = None;
        }
        let mut parts: Vec<String> = self.drain(Class::Restore).into_iter().map(|i| i.say).collect();
        if !waiting {
            let mut qs = self.drain(Class::Question);
            if !qs.is_empty() {
                let q = qs.remove(0);
                let mut say = q.say;
                if !qs.is_empty() {
                    say = format!("{say} {}", more_questions(qs.len()));
                }
                self.items.extend(qs);
                parts.push(say);
                self.asked = Some((q.session, now));
                waiting = true;
            }
        }
        if !waiting {
            parts.extend(self.drain(Class::Final).into_iter().map(|i| i.say));
            if parts.is_empty() {
                parts.extend(self.drain(Class::Progress).into_iter().map(|i| i.say));
            }
        }
        if parts.is_empty() { None } else { Some(parts.join(" ")) }
    }

    /// Снять с доски все новости класса, старые первыми.
    fn drain(&mut self, class: Class) -> Vec<News> {
        let (mut got, rest): (Vec<News>, Vec<News>) = std::mem::take(&mut self.items).into_iter().partition(|i| i.class == class);
        self.items = rest;
        got.sort_by_key(|i| i.born);
        got
    }
}

/// «После этого ещё N вопрос(а/ов)» с русским склонением.
pub fn more_questions(n: usize) -> String {
    let word = match (n % 10, n % 100) {
        (1, r) if r != 11 => "вопрос",
        (2..=4, r) if !(12..=14).contains(&r) => "вопроса",
        _ => "вопросов",
    };
    if n == 1 { format!("После этого ещё один {word}.") } else { format!("После этого ещё {n} {word}.") }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(session: &str, class: Class, say: &str, born: u64) -> News { News { session: session.into(), class, say: say.into(), born } }

    #[test]
    fn five_agents_example() {
        let mut b = Board::default();
        b.push(n("e", Class::Progress, "E: нашёл утечку.", 1));
        b.push(n("d", Class::Final, "D: фича готова.", 2));
        b.push(n("c", Class::Question, "C: что делаем с мёртвым кодом?", 3));
        b.push(n("a", Class::Restore, "A: агент повис — перезапустила.", 4));
        b.push(n("b", Class::Restore, "B: сабагент повис — перезапустила.", 5));
        assert_eq!(b.take(10).unwrap(), "A: агент повис — перезапустила. B: сабагент повис — перезапустила. C: что делаем с мёртвым кодом?");
        assert_eq!(b.take(20), None);                 // ждём ответа на вопрос
        b.answered("c");
        assert_eq!(b.take(21).unwrap(), "D: фича готова.");
        assert_eq!(b.take(22).unwrap(), "E: нашёл утечку."); // промежуточное — когда больше нечего
        assert_eq!(b.take(23), None);
    }

    #[test]
    fn questions_one_at_a_time_oldest_first_with_count() {
        let mut b = Board::default();
        b.push(n("y", Class::Question, "Y?", 2));
        b.push(n("x", Class::Question, "X?", 1));
        b.push(n("z", Class::Question, "Z?", 3));
        assert_eq!(b.take(10).unwrap(), "X? После этого ещё 2 вопроса.");
        b.answered("x");
        assert_eq!(b.take(11).unwrap(), "Y? После этого ещё один вопрос.");
    }

    #[test]
    fn answer_wait_expires() {
        let mut b = Board::default();
        b.push(n("c", Class::Question, "C?", 1));
        b.push(n("d", Class::Final, "D готов.", 1));
        assert_eq!(b.take(10).unwrap(), "C?");
        assert_eq!(b.take(10 + ANSWER_WAIT - 1), None);
        assert_eq!(b.take(10 + ANSWER_WAIT).unwrap(), "D готов.");
    }

    #[test]
    fn answered_drops_pending_question() {
        let mut b = Board::default();
        b.push(n("c", Class::Question, "C?", 1));
        b.answered("c");                               // ответили в терминале раньше Омы
        assert_eq!(b.take(10), None);
    }

    #[test]
    fn push_replaces_same_session_and_class() {
        let mut b = Board::default();
        b.push(n("a", Class::Progress, "старое", 1));
        b.push(n("a", Class::Progress, "новое", 2));
        assert_eq!(b.take(10).unwrap(), "новое");
    }

    #[test]
    fn retain_drops_gone_sessions_and_muted_classes() {
        let mut b = Board::default();
        b.push(n("gone", Class::Final, "G.", 1));
        b.push(n("a", Class::Final, "A.", 1));
        b.push(n("q", Class::Question, "Q?", 1));
        b.retain_sessions(|s| s != "gone");
        b.retain_classes(&[Class::Restore, Class::Question]); // agents_voice выключен
        assert_eq!(b.take(10).unwrap(), "Q?");
        b.answered("q");
        assert_eq!(b.take(11), None);
    }

    #[test]
    fn gone_session_frees_answer_wait() {
        let mut b = Board::default();
        b.push(n("q", Class::Question, "Q?", 1));
        b.push(n("d", Class::Final, "D готов.", 1));
        assert_eq!(b.take(10).unwrap(), "Q?");
        b.retain_sessions(|s| s != "q");               // спросивший агент закрыт — ответа не будет
        assert_eq!(b.take(11).unwrap(), "D готов.");
    }

    #[test]
    fn more_questions_plural() {
        assert_eq!(more_questions(1), "После этого ещё один вопрос.");
        assert_eq!(more_questions(2), "После этого ещё 2 вопроса.");
        assert_eq!(more_questions(5), "После этого ещё 5 вопросов.");
        assert_eq!(more_questions(11), "После этого ещё 11 вопросов.");
        assert_eq!(more_questions(21), "После этого ещё 21 вопрос.");
        assert_eq!(more_questions(22), "После этого ещё 22 вопроса.");
    }
}
