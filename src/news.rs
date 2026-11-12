//! Доска новостей Омы — полосы речи: СЕЙЧАС (срочное сторожа, «нужно» агента, вопросы по одному) звучит сразу;
//! ДАЛЬШЕ (итоги ходов, «починила сама») — пачкой не чаще раза в `every`; промежуточного на доске нет: оно только в окне.

use crate::audio::Cue;
use crate::gitfacts::Facts;
use crate::phrases::{self, Rotor};
use crate::text::plural;

/// Порядок вариантов — приоритет.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Class {
    /// сторож: нужен пользователь — звучит и когда он «занят»
    Urgent,
    /// агенту нужно сейчас (заметка агента, исход отложенного поручения): сразу, но не в «занят»; ответа не ждёт
    Now,
    Restore,
    Question,
    Final,
}

impl Class {
    /// Звуковой знак перед новостью: вопрос, итог, нужна помощь; «починила сама» — без знака.
    pub fn cue(self) -> Option<Cue> {
        match self {
            Class::Question | Class::Now => Some(Cue::Ask),
            Class::Final => Some(Cue::Done),
            Class::Urgent => Some(Cue::Help),
            Class::Restore => None,
        }
    }

    /// Интонация для адаптера синтеза (python/silero_tts.py `ssml`): вопрос выше, беда ниже и медленнее, итог быстрее.
    pub fn tone(self) -> &'static str {
        match self {
            Class::Question | Class::Now => "ask",
            Class::Urgent => "alarm",
            Class::Final => "done",
            Class::Restore => "",
        }
    }
}

#[derive(Clone, Debug)]
pub struct News {
    pub session: String,
    pub class: Class,
    pub project: String, // "" — фраза без проекта (сводка, лимит)
    pub say: String,     // без «проект: »
    pub important: bool, // пользователю нужно решить или сделать: в пачке итогов звучит своей фразой, а не именем проекта
    pub born: u64,
    pub facts: Facts,    // git после хода (итог): за фразой — «Закоммичено.»; в пачке из 2+ — только тревожное
}

impl News {
    /// «проект: фраза», как новость звучит без интонации.
    pub fn line(&self) -> String {
        Chunk::of(self).line()
    }
}

/// Кусок сказанного: класс новости (None — строки самой Омы), проект и текст — по ним речь ставит знак и интонацию.
#[derive(Clone, Debug, PartialEq)]
pub struct Chunk {
    pub class: Option<Class>,
    pub project: String, // "" — без «проект: » (сводка, склейка пачки, строки Омы)
    pub text: String,
}

impl Chunk {
    /// Строка самой Омы: без класса и проекта.
    pub fn oma(text: impl Into<String>) -> Chunk {
        Chunk { class: None, project: String::new(), text: text.into() }
    }

    fn of(n: &News) -> Chunk {
        Chunk { class: Some(n.class), project: n.project.clone(), text: n.facts.after(&n.say) }
    }

    /// «проект: текст», как кусок звучит без интонации.
    pub fn line(&self) -> String {
        if self.project.is_empty() { self.text.clone() } else { format!("{}: {}", self.project, self.text) }
    }
}

/// Куски одной строкой — для окна ответа и журнала.
pub fn line(chunks: &[Chunk]) -> String {
    chunks.iter().map(Chunk::line).collect::<Vec<_>>().join(" ")
}

/// После озвученного вопроса итоги ждут ответа не дольше этого (с).
pub const ANSWER_WAIT: u64 = 120;
/// Сессия сводки возвращения: не агент, поэтому retain_sessions её не снимает.
pub const SUMMARY: &str = "summary";
/// Сказанное позже last_input + столько секунд считается неуслышанным.
pub const UNHEARD_AFTER: u64 = 30;

#[derive(Default)]
pub struct Board {
    items: Vec<News>,
    asked: Option<(News, u64)>, // (вопрос, когда озвучен) — на него ждём ответ
    spoken: Vec<(News, u64)>,   // сказанное не наверняка: вернётся на доску, если пользователь не слышал
    rotor: Rotor,               // хвост «после этого ещё N» — по кругу
    /// Итоги и «починила сама» звучат не чаще раза в столько секунд (`news_every`); 0 — без бюджета.
    pub every: u64,
    batch_at: Option<u64>, // когда прозвучала прошлая пачка; None — бюджет снят (запуск, долгое отсутствие)
}

impl Board {
    /// Новость той же сессии и класса заменяет прежнюю: старое уже неактуально.
    /// Одинаковые фразы сторожа разных проектов склеиваются: «A, B: разбудила агента: лимит сброшен» — и разными
    /// вариантами одной фразы (phrases::canon): склейка берёт вариант первой.
    pub fn push(&mut self, n: News) {
        self.items.retain(|i| !(i.session == n.session && i.class == n.class));
        if matches!(n.class, Class::Urgent | Class::Restore) && !n.project.is_empty() {
            let key = phrases::canon(&n.say);
            if let Some(i) = self.items.iter_mut().find(|i| i.class == n.class && !i.project.is_empty() && phrases::canon(&i.say) == key) {
                i.project = format!("{}, {}", i.project, n.project);
                return;
            }
        }
        self.items.push(n);
    }

    /// Вопрос агента решён (ответили, ход кончился, он спросил новое): ждать и спрашивать больше нечего. «Нужно» агента
    /// остаётся: его снимает только ответ пользователя (prompted).
    pub fn answered(&mut self, session: &str) {
        self.drop_asks(session, &[Class::Question]);
    }

    /// Пользователь написал агенту (хоть в терминале раньше Омы): и вопрос, и «нужно» агента сняты.
    pub fn prompted(&mut self, session: &str) {
        self.drop_asks(session, &[Class::Question, Class::Now]);
    }

    pub fn drop_asks(&mut self, session: &str, classes: &[Class]) {
        if self.asked.as_ref().is_some_and(|(q, _)| q.session == session) {
            self.asked = None;
        }
        let asks = |i: &News| i.session == session && classes.contains(&i.class);
        self.items.retain(|i| !asks(i));
        self.spoken.retain(|(i, _)| !asks(i));
    }

    /// Новости закрытых агентов снимаются; сторожевые и «нужно» остаются: о закрывшемся агенте и надо сказать.
    pub fn retain_sessions(&mut self, alive: impl Fn(&str) -> bool) {
        let keep = |i: &News| i.session == SUMMARY || matches!(i.class, Class::Urgent | Class::Restore | Class::Now) || alive(&i.session);
        self.items.retain(|i| keep(i));
        self.spoken.retain(|(i, _)| keep(i));
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

    pub fn retain_classes(&mut self, keep: &[Class], sessions: &[String]) {
        self.items.retain(|i| keep.contains(&i.class) || sessions.contains(&i.session)); // sessions — их новости остаются все
    }

    /// Куски следующей пачки; взятое с доски снимается, пусто — сказать нечего. skip — агенты, которыми он занят (в фокусе,
    /// чаты Orca под окном Orca): их итог снимается без речи, вопрос остаётся. Сказанное — в журнал: не услышал — повторится после возвращения.
    pub fn take(&mut self, now: u64, skip: &[String]) -> Vec<Chunk> {
        self.drop_focused(skip);
        let mut waiting = self.waiting(now);
        if !waiting {
            self.asked = None;
        }
        let held = self.held(now);
        let mut taken = self.drain(|i| i.class == Class::Urgent);
        taken.extend(self.drain(|i| i.class == Class::Now));
        taken.extend(self.drain(|i| i.class == Class::Restore && !held(i)));
        let mut more = None; // хвост «после этого ещё N» — только во фразе, не в журнале
        if !waiting {
            let (mut qs, focused): (Vec<News>, Vec<News>) = self.drain(|i| i.class == Class::Question).into_iter().partition(|i| !skip.contains(&i.session));
            self.items.extend(focused);
            if !qs.is_empty() {
                let q = qs.remove(0);
                self.asked = Some((q.clone(), now));
                if !qs.is_empty() {
                    more = Some(self.rotor.next(phrases::MORE).replace("{n}", &questions(qs.len())));
                }
                self.items.extend(qs);
                taken.push(q);
                waiting = true;
            }
        }
        let finals = if waiting { vec![] } else { self.drain(|i| i.class == Class::Final && !held(i)) };
        if taken.iter().chain(&finals).any(|i| matches!(i.class, Class::Restore | Class::Final)) {
            self.batch_at = Some(now);
        }
        let mut chunks: Vec<Chunk> = taken.iter().map(Chunk::of).collect();
        if let Some(m) = more {
            chunks.push(Chunk::oma(m)); // вопрос в пачке последний: после него ничего не берётся
        }
        chunks.extend(batch(&finals));
        // при повторе счёт «ещё N» будет свежий
        self.spoken.extend(taken.into_iter().chain(finals).map(|i| (i, now)));
        chunks
    }

    /// Даст ли take(now, skip) фразу; доску не меняет: пока неясно, слушает ли он, итог агента в фокусе не снимается.
    pub fn sayable(&self, now: u64, skip: &[String]) -> bool {
        let (waiting, held) = (self.waiting(now), self.held(now));
        self.items.iter().any(|i| match i.class {
            Class::Urgent | Class::Now => true,
            Class::Restore => !held(i),
            Class::Question => !waiting && !skip.contains(&i.session),
            Class::Final => !waiting && !skip.contains(&i.session) && !held(i),
        })
    }

    /// Ждёт ли новость конца окна пачки: итог и «починила сама» — пока с прошлой пачки не прошло every.
    /// Сводка возвращения — ответ на само возвращение — не ждёт.
    fn held(&self, now: u64) -> impl Fn(&News) -> bool + use<> {
        let early = self.every > 0 && self.batch_at.is_some_and(|at| now < at + self.every);
        move |i: &News| early && i.session != SUMMARY && matches!(i.class, Class::Restore | Class::Final)
    }

    /// Итог агента, которым он занят, снимается без речи: он и так на него смотрит.
    pub fn drop_focused(&mut self, skip: &[String]) {
        self.items.retain(|i| !(skip.contains(&i.session) && i.class == Class::Final));
    }

    /// Озвученный вопрос ещё ждёт ответа: итоги и новые вопросы молчат.
    fn waiting(&self, now: u64) -> bool {
        self.asked.as_ref().is_some_and(|(_, at)| now < at + ANSWER_WAIT)
    }

    /// Пользователь вернулся: при долгом отсутствии неуслышанное из журнала снова на доске, а бюджет пачки снят —
    /// приветствие вводит в курс сразу.
    pub fn returned(&mut self, last_input: u64, long: bool) {
        if long {
            self.batch_at = None;
        }
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
    pub fn unask(&mut self) {
        if let Some((q, _)) = self.asked.take() {
            self.items.push(q);
        }
    }

    /// «<prefix>: …» («Пока тебя не было», «Пока было тихо») — по счётчикам доски; пусто — None. skip — агент в фокусе:
    /// его итог не в счёт, как в drop_focused.
    pub fn overview(&self, prefix: &str, skip: &[String]) -> Option<String> {
        let finals = self.items.iter().filter(|i| i.class == Class::Final && !skip.contains(&i.session));
        let mut done: Vec<&str> = finals.map(|i| i.session.as_str()).collect();
        done.sort_unstable();
        done.dedup();
        let w = self.question_sessions().iter().filter(|s| !skip.contains(s)).count(); // skip — и вопросы (заглушённые)
        let d = done.len();
        let mut parts = Vec::new();
        if d > 0 {
            parts.push(format!("{d} {} {}", plural(d, ["проект", "проекта", "проектов"]), plural(d, ["закончил", "закончили", "закончили"])));
        }
        if w > 0 {
            parts.push(format!("{w} {} ответа", plural(w, ["ждёт", "ждут", "ждут"])));
        }
        if parts.is_empty() { None } else { Some(format!("{prefix}: {}.", parts.join(", "))) }
    }

    /// Только срочное — пока пользователь занят; в журнал: не услышал — повторится по возвращении.
    pub fn take_urgent(&mut self, now: u64) -> Vec<Chunk> {
        let got = self.drain(|i| i.class == Class::Urgent);
        let chunks = got.iter().map(Chunk::of).collect();
        self.spoken.extend(got.into_iter().map(|i| (i, now)));
        chunks
    }

    /// На доске есть новость сессии этого класса — ждёт, Ома её ещё не сказала.
    pub fn has(&self, session: &str, class: Class) -> bool {
        self.items.iter().any(|i| i.session == session && i.class == class)
    }

    pub fn finals(&self) -> usize {
        self.items.iter().filter(|i| i.class == Class::Final).count()
    }

    /// Итоги на доске, кроме сессий except.
    pub fn take_finals(&mut self, except: &[String]) -> Vec<News> {
        self.drain(|i| i.class == Class::Final && !except.contains(&i.session))
    }

    /// Пользователь вернулся: итог сессии с готовым recap агента (of → фраза) звучит им — своей фразой, как важный,
    /// а не именем в пачке. Важный итог остаётся фразой Haiku: она начинается с того, что нужно от пользователя, а
    /// recap — с того, чем занимались (следующий шаг у него во втором предложении). Сессии с recap — в ответ: сводке
    /// Haiku их итоги не нужны.
    pub fn recap(&mut self, of: impl Fn(&str) -> Option<String>) -> Vec<String> {
        let mut got = Vec::new();
        for i in self.items.iter_mut().filter(|i| i.class == Class::Final && i.session != SUMMARY) {
            if let Some(say) = of(&i.session) {
                if !i.important {
                    (i.say, i.important) = (say, true);
                }
                got.push(i.session.clone());
            }
        }
        got
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Сессия озвученного вопроса, ответа на который ждём.
    pub fn asked(&self) -> Option<&str> {
        self.asked.as_ref().map(|(q, _)| q.session.as_str())
    }

    /// Снять с доски новости, которые берёт pick, старые первыми.
    fn drain(&mut self, pick: impl Fn(&News) -> bool) -> Vec<News> {
        let (mut got, rest): (Vec<News>, Vec<News>) = std::mem::take(&mut self.items).into_iter().partition(|i| pick(i));
        self.items = rest;
        got.sort_by_key(|i| i.born);
        got
    }
}

/// Пачка итогов фразой кода, без модели: один итог — как есть; несколько — важные (нужно решить или сделать) и сводка
/// своими фразами, остальные — именами проектов: «Закончили ещё: datebook, omarchy-mail.»
fn batch(finals: &[News]) -> Vec<Chunk> {
    if finals.len() < 2 {
        return finals.iter().map(Chunk::of).collect();
    }
    let (own, rest): (Vec<&News>, Vec<&News>) = finals.iter().partition(|i| i.important || i.project.is_empty());
    // факты git в пачке — только тревожные: «Закоммичено» не звучит
    let mut chunks: Vec<Chunk> = own.into_iter().map(|i| Chunk { text: if i.facts.dirty > 0 { i.facts.after(&i.say) } else { i.say.clone() }, ..Chunk::of(i) }).collect();
    let alarms: Vec<String> = rest.iter().filter_map(|i| i.facts.alarm(&i.project)).collect();
    let mut names: Vec<&str> = Vec::new();
    for i in rest {
        if !names.contains(&i.project.as_str()) {
            names.push(&i.project);
        }
    }
    if !names.is_empty() {
        let verb = if names.len() == 1 { "Закончил" } else { "Закончили" }; // перечень имён, а не числительное
        let more = if chunks.is_empty() { "" } else { " ещё" };
        chunks.push(Chunk { class: Some(Class::Final), project: String::new(), text: format!("{verb}{more}: {}.", names.join(", ")) });
    }
    for a in alarms {
        // одинаковая фраза — один раз: два агента в одном репозитории
        if !chunks.iter().any(|c| c.text == a) {
            chunks.push(Chunk { class: Some(Class::Final), project: String::new(), text: a });
        }
    }
    chunks
}

/// «один вопрос», «2 вопроса», «5 вопросов» — для хвоста phrases::MORE.
fn questions(n: usize) -> String {
    let word = plural(n, ["вопрос", "вопроса", "вопросов"]);
    if n == 1 { format!("один {word}") } else { format!("{n} {word}") }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Новость из строки «проект: фраза», как её собирают служба и сторож; без «: » — без проекта.
    fn n(session: &str, class: Class, line: &str, born: u64) -> News {
        let (project, say) = line.split_once(": ").unwrap_or(("", line));
        News { session: session.into(), class, project: project.into(), say: say.into(), important: false, born, facts: Facts::default() }
    }

    fn tk(b: &mut Board, now: u64, skip: Option<&str>) -> Option<String> {
        let c = b.take(now, &skip.map(String::from).into_iter().collect::<Vec<_>>());
        (!c.is_empty()).then(|| line(&c))
    }

    fn sp(b: &mut Board, now: u64) -> Option<String> { tk(b, now, None) }

    #[test]
    fn has_is_news_of_session_and_class_on_the_board() {
        let mut b = Board::default();
        b.push(n("s", Class::Final, "s: готов.", 1));
        assert!(b.has("s", Class::Final) && !b.has("s", Class::Question) && !b.has("t", Class::Final));
        sp(&mut b, 1000);
        assert!(!b.has("s", Class::Final), "сказана — с доски снята");
    }

    #[test]
    fn take_logs_spoken() {
        let mut b = Board::default();
        b.push(n("s", Class::Final, "s готов.", 1));
        sp(&mut b, 1000);
        b.returned(0, true);
        assert_eq!(tk(&mut b, 2000, None).unwrap(), "s готов.");
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
        assert_eq!(tk(&mut b, 5000, None).unwrap(), "Q?");
        assert_eq!(tk(&mut b, 5001, None), None);
    }

    #[test]
    fn returned_short_forgets_spoken() {
        let mut b = Board::default();
        b.push(n("f", Class::Final, "F.", 1));
        sp(&mut b, 1100);
        b.returned(1000, false);
        assert_eq!(tk(&mut b, 2000, None), None);
        b.returned(1000, true);
        assert_eq!(tk(&mut b, 2000, None), None);
    }

    #[test]
    fn replay_never_overrides_newer_news() {
        let mut b = Board::default();
        b.push(n("s", Class::Final, "старый", 1));
        sp(&mut b, 1100);
        b.push(n("s", Class::Final, "новый", 2));
        b.returned(1000, true);
        assert_eq!(tk(&mut b, 2000, None).unwrap(), "новый");
        assert_eq!(tk(&mut b, 2001, None), None);
    }

    #[test]
    fn spoken_question_logged_without_more_suffix() {
        let mut b = Board::default();
        b.push(n("a", Class::Question, "A?", 1));
        b.push(n("b", Class::Question, "B?", 2));
        assert_eq!(sp(&mut b, 1100).unwrap(), "A? После этого ещё один вопрос.");
        b.returned(1000, true);
        assert_eq!(tk(&mut b, 5000, None).unwrap(), "A? Дальше ещё один вопрос."); // A вернулась, B ещё на доске; хвост — следующий по кругу
        b.answered("b");
        b.unask();
        assert_eq!(tk(&mut b, 5001, None).unwrap(), "A?");
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
        assert_eq!(tk(&mut b, 2000, None), None);
    }

    #[test]
    fn closed_session_not_replayed() {
        let mut b = Board::default();
        b.push(n("f", Class::Final, "F.", 1));
        sp(&mut b, 1100);
        b.retain_sessions(|_| false);
        b.returned(1000, true);
        assert_eq!(tk(&mut b, 2000, None), None);
    }

    #[test]
    fn summary_survives_retain_sessions() {
        let mut b = Board::default();
        b.push(n(SUMMARY, Class::Final, "Сводка.", 1));
        b.retain_sessions(|_| false);
        assert_eq!(tk(&mut b, 10, None).unwrap(), "Сводка.");
    }

    #[test]
    fn focused_final_dropped_question_kept() {
        let mut b = Board::default();
        b.push(n("f", Class::Final, "F готов.", 1));
        b.push(n("f", Class::Question, "F?", 2));
        assert_eq!(tk(&mut b, 10, Some("f")), None);
        assert_eq!(tk(&mut b, 10, None).unwrap(), "F?");
        assert_eq!(tk(&mut b, 11, None), None);
    }

    #[test]
    fn sayable_follows_take() {
        let mut b = Board::default();
        b.push(n("f", Class::Final, "F готов.", 1));
        b.push(n("f", Class::Question, "F?", 1));
        assert!(!b.sayable(10, &["f".into()])); // итог в фокусе не звучит, вопрос в фокусе ждёт
        b.push(n("q", Class::Question, "Q?", 2));
        b.push(n("d", Class::Final, "D готов.", 3));
        assert!(b.sayable(10, &["f".into()]));
        assert_eq!(tk(&mut b, 10, Some("f")).unwrap(), "Q?");
        assert!(!b.sayable(11, &["f".into()]), "итог ждёт ответа на вопрос");
        b.push(n("r", Class::Restore, "R перезапущен.", 4));
        assert!(b.sayable(11, &["f".into()]));
        tk(&mut b, 11, Some("f"));
        assert!(b.sayable(10 + ANSWER_WAIT, &["f".into()]));
    }

    #[test]
    fn sayable_keeps_focused_final() {
        let mut b = Board::default();
        b.push(n("f", Class::Final, "F готов.", 1));
        assert!(!b.sayable(10, &["f".into()]));
        assert_eq!(tk(&mut b, 10, None).unwrap(), "F готов.", "проверка доску не меняет");
    }

    #[test]
    fn unask_puts_question_back_and_frees_finals() {
        let mut b = Board::default();
        b.push(n("q", Class::Question, "Q?", 1));
        b.push(n("d", Class::Final, "D готов.", 2));
        assert_eq!(tk(&mut b, 10, None).unwrap(), "Q?");
        b.unask();
        assert_eq!(b.asked(), None);
        assert_eq!(tk(&mut b, 11, None).unwrap(), "Q?");
    }

    #[test]
    fn overview_counts_and_plurals() {
        let mut b = Board::default();
        assert_eq!(b.overview("Пока тебя не было", &[]), None);
        b.push(n("a", Class::Final, "x", 1));
        assert_eq!(b.overview("Пока тебя не было", &[]).unwrap(), "Пока тебя не было: 1 проект закончил.");
        b.push(n("b", Class::Final, "x", 1));
        b.push(n("q", Class::Question, "Q?", 1));
        assert_eq!(b.overview("Пока было тихо", &[]).unwrap(), "Пока было тихо: 2 проекта закончили, 1 ждёт ответа.");
        for s in ["c", "d", "e"] { b.push(n(s, Class::Final, "x", 1)); }
        b.push(n("r", Class::Question, "R?", 1));
        assert_eq!(b.overview("Пока тебя не было", &[]).unwrap(), "Пока тебя не было: 5 проектов закончили, 2 ждут ответа.");
        assert_eq!(b.finals(), 5);
    }

    #[test]
    fn five_agents_example() {
        let mut b = Board::default();
        b.push(n("d", Class::Final, "D: фича готова.", 2));
        b.push(n("c", Class::Question, "C: что делаем с мёртвым кодом?", 3));
        b.push(n("a", Class::Restore, "A: агент повис — перезапустила.", 4));
        b.push(n("b", Class::Restore, "B: сабагент повис — перезапустила.", 5));
        b.push(n("f", Class::Urgent, "F: агент завис посреди push — не трогаю, посмотри сам.", 6));
        assert_eq!(tk(&mut b, 10, None).unwrap(), "F: агент завис посреди push — не трогаю, посмотри сам. A: агент повис — перезапустила. B: сабагент повис — перезапустила. C: что делаем с мёртвым кодом?");
        assert_eq!(tk(&mut b, 20, None), None);                 // ждём ответа на вопрос
        b.answered("c");
        assert_eq!(tk(&mut b, 21, None).unwrap(), "D: фича готова.");
        assert_eq!(tk(&mut b, 22, None), None);
    }

    #[test]
    fn questions_one_at_a_time_oldest_first_with_count() {
        let mut b = Board::default();
        b.push(n("y", Class::Question, "Y?", 2));
        b.push(n("x", Class::Question, "X?", 1));
        b.push(n("z", Class::Question, "Z?", 3));
        b.push(n("w", Class::Question, "W?", 4));
        assert_eq!(tk(&mut b, 10, None).unwrap(), "X? После этого ещё 3 вопроса.");
        b.answered("x");
        assert_eq!(tk(&mut b, 11, None).unwrap(), "Y? Дальше ещё 2 вопроса.", "хвост — по кругу, по порядку");
        b.answered("y");
        assert_eq!(tk(&mut b, 12, None).unwrap(), "Z? В очереди ещё один вопрос.");
        b.answered("z");
        assert_eq!(tk(&mut b, 13, None).unwrap(), "W?");
    }

    #[test]
    fn answer_wait_expires() {
        let mut b = Board::default();
        b.push(n("c", Class::Question, "C?", 1));
        b.push(n("d", Class::Final, "D готов.", 1));
        assert_eq!(tk(&mut b, 10, None).unwrap(), "C?");
        assert_eq!(tk(&mut b, 10 + ANSWER_WAIT - 1, None), None);
        assert_eq!(tk(&mut b, 10 + ANSWER_WAIT, None).unwrap(), "D готов.");
    }

    #[test]
    fn answered_drops_pending_question() {
        let mut b = Board::default();
        b.push(n("c", Class::Question, "C?", 1));
        b.answered("c");                               // ответили в терминале раньше Омы
        assert_eq!(tk(&mut b, 10, None), None);
    }

    #[test]
    fn push_replaces_same_session_and_class() {
        let mut b = Board::default();
        b.push(n("a", Class::Final, "старое", 1));
        b.push(n("a", Class::Final, "новое", 2));
        assert_eq!(tk(&mut b, 10, None).unwrap(), "новое");
    }

    #[test]
    fn retain_drops_gone_sessions_and_muted_classes() {
        let mut b = Board::default();
        b.push(n("gone", Class::Final, "G.", 1));
        b.push(n("a", Class::Final, "A.", 1));
        b.push(n("q", Class::Question, "Q?", 1));
        b.retain_sessions(|s| s != "gone");
        b.retain_classes(&[Class::Restore, Class::Question], &[]); // agents_voice выключен
        assert_eq!(tk(&mut b, 10, None).unwrap(), "Q?");
        b.answered("q");
        assert_eq!(tk(&mut b, 11, None), None);
    }

    #[test]
    fn gone_session_frees_answer_wait() {
        let mut b = Board::default();
        b.push(n("q", Class::Question, "Q?", 1));
        b.push(n("d", Class::Final, "D готов.", 1));
        assert_eq!(tk(&mut b, 10, None).unwrap(), "Q?");
        b.retain_sessions(|s| s != "q");               // спросивший агент закрыт — ответа не будет
        assert_eq!(tk(&mut b, 11, None).unwrap(), "D готов.");
    }

    #[test]
    fn questions_plural() {
        assert_eq!(questions(1), "один вопрос");
        assert_eq!(questions(2), "2 вопроса");
        assert_eq!(questions(5), "5 вопросов");
        assert_eq!(questions(11), "11 вопросов");
        assert_eq!(questions(21), "21 вопрос");
        assert_eq!(questions(22), "22 вопроса");
    }

    #[test]
    fn urgent_first_and_survives_gone_session() {
        let mut b = Board::default();
        b.push(n("q", Class::Question, "Q?", 1));
        b.push(n("r", Class::Restore, "R перезапущен.", 2));
        b.push(n("u", Class::Urgent, "U завис посреди push.", 3));
        b.retain_sessions(|s| s == "q");
        assert_eq!(tk(&mut b, 10, None).unwrap(), "U завис посреди push. R перезапущен. Q?");
    }

    #[test]
    fn take_urgent_leaves_the_rest() {
        let mut b = Board::default();
        b.push(n("f", Class::Final, "F.", 1));
        b.push(n("u", Class::Urgent, "U.", 2));
        b.push(n("w", Class::Now, "W.", 2));
        assert_eq!(line(&b.take_urgent(10)), "U.");
        assert!(b.take_urgent(11).is_empty(), "«нужно» агента в «занят» ждёт");
        assert_eq!(tk(&mut b, 12, None).unwrap(), "W. F.");
        b.push(n("u", Class::Urgent, "U.", 3));
        b.take_urgent(1100);
        b.returned(1000, true);
        assert_eq!(tk(&mut b, 2000, None).unwrap(), "U.", "срочное при «занят» — в журнал: не услышал — повторится");
    }

    #[test]
    fn urgent_sayable_while_waiting_for_answer() {
        let mut b = Board::default();
        b.push(n("q", Class::Question, "Q?", 1));
        b.take(10, &[]);
        b.push(n("u", Class::Urgent, "U.", 2));
        assert!(b.sayable(11, &[]));
        assert_eq!(tk(&mut b, 11, None).unwrap(), "U.");
    }

    #[test]
    fn same_watch_phrase_glues_projects() {
        let mut b = Board::default();
        b.push(n("a", Class::Restore, "A: лимит сброшен — разбудила агента.", 1));
        b.push(n("b", Class::Restore, "B: лимит сброшен — разбудила агента.", 2));
        b.push(n("c", Class::Restore, "C: оборвалась связь — попросила агента продолжить.", 3));
        b.push(n("d", Class::Final, "D: готово.", 4));
        assert_eq!(tk(&mut b, 10, None).unwrap(), "A, B: лимит сброшен — разбудила агента. C: оборвалась связь — попросила агента продолжить. D: готово.");
        // сторож крутит варианты: у каждого проекта свой, а новость одна — первым выпавшим вариантом
        b.push(n("a", Class::Restore, "A: перезапустила зависшего агента.", 11));
        b.push(n("b", Class::Restore, "B: агент висел — перезапустила.", 12));
        b.push(n("c", Class::Urgent, "C: связь с Claude не держится — загляни, пожалуйста.", 13));
        b.push(n("d", Class::Urgent, "D: связь с Claude не держится — нужна твоя помощь.", 14));
        b.push(n("e", Class::Urgent, "E: API отказал — нужен вход в Claude.", 15));
        b.push(n("f", Class::Urgent, "F: ошибка API — запрос отклонён.", 16));
        let c = b.take(20, &[]);
        assert_eq!(line(&c), "C, D: связь с Claude не держится — загляни, пожалуйста. E: API отказал — нужен вход в Claude. F: ошибка API — запрос отклонён. A, B: перезапустила зависшего агента.");
        assert_eq!(c[0].project, "C, D", "склейка — одним куском со всеми проектами");
    }

    // --- полосы и бюджет пачки (дизайн 2026-11-12, lanes) ---

    #[test]
    fn finals_wait_for_the_batch_window_first_one_at_once() {
        let mut b = Board { every: 600, ..Board::default() };
        b.push(n("a", Class::Final, "A: готово.", 1));
        assert!(b.sayable(1000, &[]));
        assert_eq!(tk(&mut b, 1000, None).unwrap(), "A: готово.", "первый итог после тишины — сразу");
        b.push(n("b", Class::Final, "B: готово.", 2));
        b.push(n("c", Class::Restore, "C: перезапустила зависшего агента.", 3));
        assert!(!b.sayable(1100, &[]), "«Ты тут? Есть новости» не врёт: take ничего не даст");
        assert_eq!(tk(&mut b, 1100, None), None);
        assert!(b.sayable(1600, &[]));
        assert_eq!(tk(&mut b, 1600, None).unwrap(), "C: перезапустила зависшего агента. B: готово.");
        b.push(n("d", Class::Final, "D: готово.", 4));
        assert_eq!(tk(&mut b, 1700, None), None, "окно считается от прошлой пачки");
    }

    #[test]
    fn now_lane_ignores_the_budget_and_the_answer_wait() {
        let mut b = Board { every: 600, ..Board::default() };
        b.push(n("a", Class::Final, "A: готово.", 1));
        tk(&mut b, 1000, None);
        b.push(n("q", Class::Question, "Q: нужен ответ?", 2));
        assert_eq!(tk(&mut b, 1001, None).unwrap(), "Q: нужен ответ?", "вопрос — сразу, бюджет его не держит");
        b.push(n("u", Class::Urgent, "U: связь не держится.", 3));
        b.push(n("w", Class::Now, "W: нужна папка.", 4));
        assert!(b.sayable(1002, &[]));
        assert_eq!(tk(&mut b, 1002, None).unwrap(), "U: связь не держится. W: нужна папка.", "срочное и «нужно» — и пока ждём ответа");
    }

    #[test]
    fn now_is_answered_and_survives_rescan() {
        let mut b = Board::default();
        b.push(n("w", Class::Now, "W: нужна папка.", 1));
        b.retain_sessions(|_| false);
        assert!(b.sayable(10, &[]), "rescan «нужно» не снимает");
        b.answered("w");
        assert!(b.sayable(10, &[]), "новый вопрос агента или конец хода снимают только прежний вопрос");
        b.prompted("w");
        assert_eq!(tk(&mut b, 10, None), None, "ответ пользователя снимает");
    }

    #[test]
    fn long_return_lifts_the_budget() {
        for long in [false, true] {
            let mut b = Board { every: 600, ..Board::default() };
            b.push(n("a", Class::Final, "A: готово.", 1));
            tk(&mut b, 1000, None);
            b.push(n("b", Class::Final, "B: готово.", 2));
            b.returned(900, long);
            assert_eq!(b.sayable(1100, &[]), long, "приветствие вводит в курс сразу (long={long})");
        }
    }

    #[test]
    fn summary_is_not_held_by_the_budget() {
        let mut b = Board { every: 600, ..Board::default() };
        b.push(n("a", Class::Restore, "A: перезапустила зависшего агента.", 1));
        tk(&mut b, 1000, None);
        b.push(n(SUMMARY, Class::Final, "agent-speak — всё влито.", 2));
        assert_eq!(tk(&mut b, 1010, None).unwrap(), "agent-speak — всё влито.");
    }

    #[test]
    fn every_zero_is_no_budget() {
        let mut b = Board::default();
        for (i, s) in ["a", "b", "c"].into_iter().enumerate() {
            b.push(n(s, Class::Final, &format!("{s}: готово."), i as u64));
            assert!(tk(&mut b, 1000 + i as u64, None).is_some());
        }
    }

    #[test]
    fn batch_of_finals_is_one_line_by_code() {
        let imp = |s: &str, line: &str, born| News { important: true, ..n(s, Class::Final, line, born) };
        let mut b = Board::default();
        b.push(n("a", Class::Final, "datebook: готово.", 1));
        b.push(n("b", Class::Final, "omarchy-mail: тесты прошли.", 2));
        assert_eq!(tk(&mut b, 10, None).unwrap(), "Закончили: datebook, omarchy-mail.");
        b.push(n("a", Class::Final, "datebook: готово.", 3));
        b.push(imp("c", "agent-speak: нужно решить, куда пушить.", 4));
        b.push(n("d", Class::Final, "omarchy-mail: готово.", 5));
        b.push(n("e", Class::Final, "omarchy-mail: готово.", 6));
        let c = b.take(20, &[]);
        assert_eq!(line(&c), "agent-speak: нужно решить, куда пушить. Закончили ещё: datebook, omarchy-mail.", "важное — своей фразой, имена — без повторов");
        assert_eq!(c.iter().map(|c| (c.class, c.project.as_str())).collect::<Vec<_>>(), [(Some(Class::Final), "agent-speak"), (Some(Class::Final), "")], "у склейки имён нет «проект: »");
        b.push(imp("c", "agent-speak: нужно решить, куда пушить.", 7));
        b.push(n("a", Class::Final, "datebook: готово.", 8));
        assert_eq!(tk(&mut b, 30, None).unwrap(), "agent-speak: нужно решить, куда пушить. Закончил ещё: datebook.");
        // глагол — по числу имён, а не правило числительных: 21 проект — «Закончили»
        for i in 0..21 {
            b.push(n(&format!("s{i}"), Class::Final, &format!("p{i}: готово."), 40 + i));
        }
        assert!(tk(&mut b, 40, None).unwrap().starts_with("Закончили: p0, p1,"));
    }

    #[test]
    fn git_facts_follow_the_final_only_alarming_in_a_batch() {
        let git = |commits, dirty| Facts { commits, dirty, unpushed: Some(2) };
        let with = |s: &str, line: &str, born, important, facts| News { important, facts, ..n(s, Class::Final, line, born) };
        let mut b = Board::default();
        b.push(with("a", "datebook: готово.", 1, false, git(1, 0)));
        assert_eq!(sp(&mut b, 10).unwrap(), "datebook: готово. Закоммичено.", "один итог — все факты; «не запушено» — только в окне");
        b.push(with("a", "datebook: готово.", 2, false, git(1, 0)));
        b.push(with("b", "agent-speak: эхо починено.", 3, false, git(1, 3)));
        b.push(with("e", "agent-speak: орб готов.", 4, false, git(0, 3)));
        b.push(with("c", "omarchy-mail: нужно решить, куда пушить.", 5, true, git(0, 1)));
        b.push(with("d", "site: выкатка готова, нужна проверка.", 6, true, git(2, 0)));
        assert_eq!(sp(&mut b, 20).unwrap(), "omarchy-mail: нужно решить, куда пушить. Не закоммичен 1 файл. site: выкатка готова, нужна проверка. \
            Закончили ещё: datebook, agent-speak. В agent-speak не закоммичены 3 файла.", "одна фраза на репозиторий");
    }

    #[test]
    fn recap_speaks_its_own_phrase_and_stays_out_of_the_summary() {
        let board = || {
            let mut b = Board::default();
            b.push(n("a", Class::Final, "datebook: готово.", 1));
            b.push(n("b", Class::Final, "agent-speak: эхо починено.", 2));
            b.push(n("c", Class::Final, "omarchy-mail: тесты зелёные.", 3));
            b.push(n("q", Class::Question, "site: куда пушить?", 4));
            b
        };
        let recap = |s: &str| (s != "c").then(|| format!("Чиним {s}."));
        let mut b = board();
        assert_eq!(b.recap(recap), ["a", "b"], "вопрос не трогается");
        assert_eq!(sp(&mut b, 10).unwrap(), "site: куда пушить?");
        b.answered("q");
        b.asked = None;
        assert_eq!(sp(&mut b, 20).unwrap(), "datebook: Чиним a. agent-speak: Чиним b. Закончил ещё: omarchy-mail.", "recap — своей фразой");
        let mut b = board();
        let got = b.recap(recap);
        let rest: Vec<String> = b.take_finals(&got).iter().map(News::line).collect();
        assert_eq!(rest, ["omarchy-mail: тесты зелёные."], "на сводку — только итоги без recap");
        assert_eq!(b.finals(), 2);
        let mut b = Board::default();
        b.push(News { important: true, ..n("d", Class::Final, "site: нужно решить, куда пушить.", 5) });
        assert_eq!(b.recap(recap), ["d"], "важный итог с recap — тоже не в сводку Haiku");
        assert_eq!(sp(&mut b, 10).unwrap(), "site: нужно решить, куда пушить.", "фраза Haiku с тем, что нужно от пользователя, остаётся");
    }

    #[test]
    fn chunks_carry_class_and_project() {
        let mut b = Board::default();
        b.push(n("q", Class::Question, "plug: куда пушить?", 1));
        b.push(n("x", Class::Question, "x: ещё?", 2));
        b.push(n("u", Class::Urgent, "plug: связь не держится.", 3));
        let c = b.take(10, &[]);
        assert_eq!(c, [
            Chunk { class: Some(Class::Urgent), project: "plug".into(), text: "связь не держится.".into() },
            Chunk { class: Some(Class::Question), project: "plug".into(), text: "куда пушить?".into() },
            Chunk::oma("После этого ещё один вопрос."),
        ]);
    }

    #[test]
    fn cue_and_tone_by_class() {
        use Class::*;
        let got: Vec<_> = [Urgent, Now, Restore, Question, Final].into_iter().map(|c| (c.cue(), c.tone())).collect();
        assert_eq!(got, [(Some(Cue::Help), "alarm"), (Some(Cue::Ask), "ask"), (None, ""), (Some(Cue::Ask), "ask"), (Some(Cue::Done), "done")]);
    }
}
