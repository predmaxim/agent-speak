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
/// Сессия сводки возвращения: не агент, поэтому retain_sessions её не снимает.
pub const SUMMARY: &str = "summary";
/// Сказанное позже last_input + столько секунд считается неуслышанным.
#[allow(dead_code)] // daemon подключит в следующих задачах
pub const UNHEARD_AFTER: u64 = 60;

#[derive(Default)]
pub struct Board {
    items: Vec<News>,
    asked: Option<(News, u64)>, // (вопрос, когда озвучен) — на него ждём ответ
    spoken: Vec<(News, u64)>,   // сказанное не наверняка: вернётся на доску, если пользователь не слышал
}

impl Board {
    /// Новость той же сессии и класса заменяет прежнюю: старое уже неактуально.
    /// Итог заменяет и промежуточное своей сессии: иначе оно прозвучало бы после итога.
    pub fn push(&mut self, n: News) {
        let superseded = |c: Class| c == n.class || (n.class == Class::Final && c == Class::Progress);
        self.items.retain(|i| !(i.session == n.session && superseded(i.class)));
        self.items.push(n);
    }

    /// Пользователь ответил (хоть в терминале раньше Омы): ждать и спрашивать больше нечего.
    pub fn answered(&mut self, session: &str) {
        if self.asked.as_ref().is_some_and(|(q, _)| q.session == session) {
            self.asked = None;
        }
        self.items.retain(|i| !(i.session == session && i.class == Class::Question));
        self.spoken.retain(|(i, _)| !(i.session == session && i.class == Class::Question));
    }

    pub fn retain_sessions(&mut self, alive: impl Fn(&str) -> bool) {
        let keep = |s: &str| s == SUMMARY || alive(s);
        self.items.retain(|i| keep(&i.session));
        self.spoken.retain(|(i, _)| keep(&i.session));
        // закрытый агент не ответит: итоги не ждут его ANSWER_WAIT
        if self.asked.as_ref().is_some_and(|(q, _)| !alive(&q.session)) {
            self.asked = None;
        }
    }

    /// Сессии, чей вопрос ждёт на доске или уже озвучен и ждёт ответа.
    pub fn question_sessions(&self) -> Vec<String> {
        let queued = self.items.iter().filter(|i| i.class == Class::Question).map(|i| i.session.clone());
        queued.chain(self.asked.iter().map(|(q, _)| q.session.clone())).collect()
    }

    pub fn retain_classes(&mut self, keep: &[Class]) {
        self.items.retain(|i| keep.contains(&i.class));
    }

    /// Фраза следующей пачки; взятое с доски снимается. skip — агент в фокусе: его итог и
    /// промежуточное снимаются без речи, вопрос остаётся. sure — пользователь точно слышит: журнал не ведём.
    pub fn take(&mut self, now: u64, skip: Option<&str>, sure: bool) -> Option<String> {
        if let Some(f) = skip {
            self.items.retain(|i| !(i.session == f && matches!(i.class, Class::Final | Class::Progress)));
        }
        let mut waiting = self.asked.as_ref().is_some_and(|(_, at)| now < at + ANSWER_WAIT);
        if !waiting {
            self.asked = None;
        }
        let mut taken = self.drain(Class::Restore);
        if !waiting {
            let (mut qs, focused): (Vec<News>, Vec<News>) = self.drain(Class::Question).into_iter().partition(|i| Some(i.session.as_str()) != skip);
            self.items.extend(focused);
            if !qs.is_empty() {
                let mut q = qs.remove(0);
                self.asked = Some((q.clone(), now));
                if !qs.is_empty() {
                    q.say = format!("{} {}", q.say, more_questions(qs.len()));
                }
                self.items.extend(qs);
                taken.push(q);
                waiting = true;
            }
        }
        if !waiting {
            taken.extend(self.drain(Class::Final));
            if taken.is_empty() {
                taken.extend(self.drain(Class::Progress));
            }
        }
        if taken.is_empty() {
            return None;
        }
        let say = taken.iter().map(|i| i.say.as_str()).collect::<Vec<_>>().join(" ");
        if !sure {
            // в журнале вопрос без «после этого ещё N»: при повторе счёт будет свежий
            self.spoken.extend(taken.into_iter().map(|i| (i, now)));
        }
        Some(say)
    }

    /// Пользователь вернулся: при долгом отсутствии неуслышанное из журнала снова на доске.
    #[allow(dead_code)] // daemon подключит в следующих задачах
    pub fn returned(&mut self, last_input: u64, long: bool) {
        for (n, at) in std::mem::take(&mut self.spoken) {
            if !long || at < last_input + UNHEARD_AFTER || self.items.iter().any(|i| i.session == n.session && i.class == n.class) {
                continue;
            }
            if self.asked.as_ref().is_some_and(|(q, _)| q.session == n.session) && n.class == Class::Question {
                self.asked = None;
            }
            self.items.push(n);
        }
    }

    /// Озвученный вопрос не услышан: снова на доску, итоги его не ждут.
    #[allow(dead_code)] // daemon подключит в следующих задачах
    pub fn unask(&mut self) {
        if let Some((q, _)) = self.asked.take() {
            self.items.push(q);
        }
    }

    /// «Пока тебя не было: …» — по счётчикам доски; пусто — None.
    #[allow(dead_code)] // daemon подключит в следующих задачах
    pub fn overview(&self) -> Option<String> {
        let mut done: Vec<&str> = self.items.iter().filter(|i| i.class == Class::Final).map(|i| i.session.as_str()).collect();
        done.sort_unstable();
        done.dedup();
        let (d, w) = (done.len(), self.question_sessions().len());
        let mut parts = Vec::new();
        if d > 0 {
            parts.push(format!("{d} {} {}", plural(d, ["проект", "проекта", "проектов"]), plural(d, ["закончил", "закончили", "закончили"])));
        }
        if w > 0 {
            parts.push(format!("{w} {} ответа", plural(w, ["ждёт", "ждут", "ждут"])));
        }
        if parts.is_empty() { None } else { Some(format!("Пока тебя не было: {}.", parts.join(", "))) }
    }

    #[allow(dead_code)] // daemon подключит в следующих задачах
    pub fn finals(&self) -> usize {
        self.items.iter().filter(|i| i.class == Class::Final).count()
    }

    #[allow(dead_code)] // daemon подключит в следующих задачах
    pub fn take_finals(&mut self) -> Vec<News> {
        self.drain(Class::Final)
    }

    #[allow(dead_code)] // daemon подключит в следующих задачах
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Сессия озвученного вопроса, ответа на который ждём.
    pub fn asked(&self) -> Option<&str> {
        self.asked.as_ref().map(|(q, _)| q.session.as_str())
    }

    /// Снять с доски все новости класса, старые первыми.
    fn drain(&mut self, class: Class) -> Vec<News> {
        let (mut got, rest): (Vec<News>, Vec<News>) = std::mem::take(&mut self.items).into_iter().partition(|i| i.class == class);
        self.items = rest;
        got.sort_by_key(|i| i.born);
        got
    }
}

/// Русское склонение: один / два-четыре / пять.
fn plural(n: usize, forms: [&'static str; 3]) -> &'static str {
    match (n % 10, n % 100) {
        (1, r) if r != 11 => forms[0],
        (2..=4, r) if !(12..=14).contains(&r) => forms[1],
        _ => forms[2],
    }
}

/// «После этого ещё N вопрос(а/ов)» с русским склонением.
pub fn more_questions(n: usize) -> String {
    let word = plural(n, ["вопрос", "вопроса", "вопросов"]);
    if n == 1 { format!("После этого ещё один {word}.") } else { format!("После этого ещё {n} {word}.") }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(session: &str, class: Class, say: &str, born: u64) -> News { News { session: session.into(), class, say: say.into(), born } }

    fn sp(b: &mut Board, now: u64) -> Option<String> { b.take(now, None, false) }

    #[test]
    fn take_logs_spoken_unless_sure() {
        let mut b = Board::default();
        b.push(n("s", Class::Final, "s готов.", 1));
        b.take(10, None, true);
        b.returned(0, true);
        assert_eq!(b.take(100, None, true), None);
        b.push(n("s", Class::Final, "s готов.", 1));
        sp(&mut b, 1000);
        b.returned(0, true);
        assert_eq!(b.take(2000, None, true).unwrap(), "s готов.");
    }

    #[test]
    fn returned_long_replays_only_unheard() {
        let mut b = Board::default();
        b.push(n("q", Class::Question, "Q?", 1));
        sp(&mut b, 1060);
        b.push(n("f", Class::Final, "F.", 2));
        b.asked = None;
        sp(&mut b, 1050);
        b.returned(1000, true);
        assert_eq!(b.take(5000, None, true).unwrap(), "Q?");
        assert_eq!(b.take(5001, None, true), None);
    }

    #[test]
    fn returned_short_forgets_spoken() {
        let mut b = Board::default();
        b.push(n("f", Class::Final, "F.", 1));
        sp(&mut b, 1100);
        b.returned(1000, false);
        assert_eq!(b.take(2000, None, true), None);
        b.returned(1000, true);
        assert_eq!(b.take(2000, None, true), None);
    }

    #[test]
    fn replay_never_overrides_newer_news() {
        let mut b = Board::default();
        b.push(n("s", Class::Final, "старый", 1));
        sp(&mut b, 1100);
        b.push(n("s", Class::Final, "новый", 2));
        b.returned(1000, true);
        assert_eq!(b.take(2000, None, true).unwrap(), "новый");
        assert_eq!(b.take(2001, None, true), None);
    }

    #[test]
    fn replayed_question_clears_asked() {
        let mut b = Board::default();
        b.push(n("q", Class::Question, "Q?", 1));
        sp(&mut b, 1100);
        assert_eq!(b.asked(), Some("q"));
        b.returned(1000, true);
        assert_eq!(b.asked(), None);
    }

    #[test]
    fn answered_drops_spoken_question() {
        let mut b = Board::default();
        b.push(n("q", Class::Question, "Q?", 1));
        sp(&mut b, 1100);
        b.answered("q");
        b.returned(1000, true);
        assert_eq!(b.take(2000, None, true), None);
    }

    #[test]
    fn closed_session_not_replayed() {
        let mut b = Board::default();
        b.push(n("f", Class::Final, "F.", 1));
        sp(&mut b, 1100);
        b.retain_sessions(|_| false);
        b.returned(1000, true);
        assert_eq!(b.take(2000, None, true), None);
    }

    #[test]
    fn summary_survives_retain_sessions() {
        let mut b = Board::default();
        b.push(n(SUMMARY, Class::Final, "Сводка.", 1));
        b.retain_sessions(|_| false);
        assert_eq!(b.take(10, None, true).unwrap(), "Сводка.");
    }

    #[test]
    fn focused_final_dropped_question_kept() {
        let mut b = Board::default();
        b.push(n("f", Class::Final, "F готов.", 1));
        b.push(n("f", Class::Question, "F?", 2));
        assert_eq!(b.take(10, Some("f"), true), None);
        assert_eq!(b.take(10, None, true).unwrap(), "F?");
        assert_eq!(b.take(11, None, true), None);
    }

    #[test]
    fn unask_puts_question_back_and_frees_finals() {
        let mut b = Board::default();
        b.push(n("q", Class::Question, "Q?", 1));
        b.push(n("d", Class::Final, "D готов.", 2));
        assert_eq!(b.take(10, None, true).unwrap(), "Q?");
        b.unask();
        assert_eq!(b.asked(), None);
        assert_eq!(b.take(11, None, true).unwrap(), "Q?");
    }

    #[test]
    fn overview_counts_and_plurals() {
        let mut b = Board::default();
        assert_eq!(b.overview(), None);
        b.push(n("a", Class::Final, "x", 1));
        assert_eq!(b.overview().unwrap(), "Пока тебя не было: 1 проект закончил.");
        b.push(n("b", Class::Final, "x", 1));
        b.push(n("q", Class::Question, "Q?", 1));
        assert_eq!(b.overview().unwrap(), "Пока тебя не было: 2 проекта закончили, 1 ждёт ответа.");
        for s in ["c", "d", "e"] { b.push(n(s, Class::Final, "x", 1)); }
        b.push(n("r", Class::Question, "R?", 1));
        assert_eq!(b.overview().unwrap(), "Пока тебя не было: 5 проектов закончили, 2 ждут ответа.");
        assert_eq!(b.finals(), 5);
    }

    #[test]
    fn five_agents_example() {
        let mut b = Board::default();
        b.push(n("e", Class::Progress, "E: нашёл утечку.", 1));
        b.push(n("d", Class::Final, "D: фича готова.", 2));
        b.push(n("c", Class::Question, "C: что делаем с мёртвым кодом?", 3));
        b.push(n("a", Class::Restore, "A: агент повис — перезапустила.", 4));
        b.push(n("b", Class::Restore, "B: сабагент повис — перезапустила.", 5));
        assert_eq!(b.take(10, None, true).unwrap(), "A: агент повис — перезапустила. B: сабагент повис — перезапустила. C: что делаем с мёртвым кодом?");
        assert_eq!(b.take(20, None, true), None);                 // ждём ответа на вопрос
        b.answered("c");
        assert_eq!(b.take(21, None, true).unwrap(), "D: фича готова.");
        assert_eq!(b.take(22, None, true).unwrap(), "E: нашёл утечку."); // промежуточное — когда больше нечего
        assert_eq!(b.take(23, None, true), None);
    }

    #[test]
    fn questions_one_at_a_time_oldest_first_with_count() {
        let mut b = Board::default();
        b.push(n("y", Class::Question, "Y?", 2));
        b.push(n("x", Class::Question, "X?", 1));
        b.push(n("z", Class::Question, "Z?", 3));
        assert_eq!(b.take(10, None, true).unwrap(), "X? После этого ещё 2 вопроса.");
        b.answered("x");
        assert_eq!(b.take(11, None, true).unwrap(), "Y? После этого ещё один вопрос.");
    }

    #[test]
    fn answer_wait_expires() {
        let mut b = Board::default();
        b.push(n("c", Class::Question, "C?", 1));
        b.push(n("d", Class::Final, "D готов.", 1));
        assert_eq!(b.take(10, None, true).unwrap(), "C?");
        assert_eq!(b.take(10 + ANSWER_WAIT - 1, None, true), None);
        assert_eq!(b.take(10 + ANSWER_WAIT, None, true).unwrap(), "D готов.");
    }

    #[test]
    fn answered_drops_pending_question() {
        let mut b = Board::default();
        b.push(n("c", Class::Question, "C?", 1));
        b.answered("c");                               // ответили в терминале раньше Омы
        assert_eq!(b.take(10, None, true), None);
    }

    #[test]
    fn push_replaces_same_session_and_class() {
        let mut b = Board::default();
        b.push(n("a", Class::Progress, "старое", 1));
        b.push(n("a", Class::Progress, "новое", 2));
        assert_eq!(b.take(10, None, true).unwrap(), "новое");
    }

    #[test]
    fn retain_drops_gone_sessions_and_muted_classes() {
        let mut b = Board::default();
        b.push(n("gone", Class::Final, "G.", 1));
        b.push(n("a", Class::Final, "A.", 1));
        b.push(n("q", Class::Question, "Q?", 1));
        b.retain_sessions(|s| s != "gone");
        b.retain_classes(&[Class::Restore, Class::Question]); // agents_voice выключен
        assert_eq!(b.take(10, None, true).unwrap(), "Q?");
        b.answered("q");
        assert_eq!(b.take(11, None, true), None);
    }

    #[test]
    fn final_drops_its_own_progress() {
        let mut b = Board::default();
        b.push(n("a", Class::Progress, "A: шаг закрыт.", 1));
        b.push(n("b", Class::Progress, "B: нашла утечку.", 1));
        b.push(n("a", Class::Final, "A: готово.", 2));
        assert_eq!(b.take(10, None, true).unwrap(), "A: готово.");
        assert_eq!(b.take(11, None, true).unwrap(), "B: нашла утечку."); // чужое промежуточное остаётся
        assert_eq!(b.take(12, None, true), None);
    }

    #[test]
    fn gone_session_frees_answer_wait() {
        let mut b = Board::default();
        b.push(n("q", Class::Question, "Q?", 1));
        b.push(n("d", Class::Final, "D готов.", 1));
        assert_eq!(b.take(10, None, true).unwrap(), "Q?");
        b.retain_sessions(|s| s != "q");               // спросивший агент закрыт — ответа не будет
        assert_eq!(b.take(11, None, true).unwrap(), "D готов.");
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
