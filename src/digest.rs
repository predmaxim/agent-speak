//! Пересказ хода агента: лёгкая модель → короткая фраза для голоса.

use crate::learn;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Ask {
    Progress,
    Final,
    Question,
}

#[derive(Clone, Debug)]
pub struct Job {
    pub session: String,
    pub project: String,
    pub ask: Ask,
    pub text: String,
    /// последний промпт пользователя этой сессии
    pub context: String,
    /// фраза без модели; None — без модели молчим
    pub fallback: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Digest {
    pub session: String,
    pub project: String,
    pub ask: Ask,
    pub important: bool,
    pub say: String,
}

/// Символов текста агента в промпте — хвост: итог хода в конце.
pub const MAX_INPUT: usize = 6000;
pub const WORKERS: usize = 3;

const COMMON: &str = "Ты — Ома, помощница, которая пересказывает пользователю голосом работу ИИ-агента. Пиши по-русски, коротко, простыми словами. \
Не называй переменные, функции, классы, файлы, пути, команды и код — только смысл: что сделано, каким способом в общих словах, что дальше. \
Без markdown и списков. Ответ — строго одна строка JSON: {\"say\": \"…\", \"important\": true или false}.";

pub fn prompt(job: &Job) -> String {
    let kind = match job.ask {
        Ask::Final => "Агент закончил ход. Одной-двумя фразами: что сделано и что требует внимания пользователя — оставшиеся проблемы, советы агента, что не запушено или не проверено. important — true, если пользователю нужно что-то решить или сделать.",
        Ask::Question => "Агент ждёт ответа пользователя. Перескажи суть вопроса общими словами и совет агента, если он есть. Если вопрос важный (архитектура, данные, деньги, необратимые действия), добавь в общих словах, как агент предлагает это сделать. important — true.",
        Ask::Progress => "Агент ещё работает; ниже его сообщения за последнюю минуту. Одной фразой: какой шаг закрыт и что дальше. important — true, только если пользователю стоит узнать это сейчас: проблема, смена плана, неожиданный результат.",
    };
    let skip = job.text.chars().count().saturating_sub(MAX_INPUT);
    let tail: String = job.text.chars().skip(skip).collect();
    format!("{COMMON} {kind}\n\nЗадача пользователя: {}\n\nТекст агента:\n{tail}", job.context)
}

/// (important, say) из ответа модели; она может обернуть JSON в ```-ограду или фразу.
pub fn parse(out: &str) -> Option<(bool, String)> {
    let v: serde_json::Value = serde_json::from_str(out.get(out.find('{')?..=out.rfind('}')?)?).ok()?;
    let say = v["say"].as_str()?.trim();
    (!say.is_empty()).then(|| (v["important"].as_bool().unwrap_or(false), say.to_string()))
}

pub fn digest(job: Job, ask: impl Fn(&str) -> Option<String>) -> Option<Digest> {
    // пустой текст пересказывать нечего — модель не зовём
    let said = if job.text.trim().is_empty() { None } else { ask(&prompt(&job)).and_then(|o| parse(&o)) };
    // вопрос важен и без модели: пользователя ждут
    let (important, say) = said.or_else(|| job.fallback.map(|f| (job.ask == Ask::Question, f)))?;
    Some(Digest { session: job.session, project: job.project, ask: job.ask, important, say })
}

pub fn spawn(out: Sender<Digest>) -> Sender<Job> {
    spawn_with(out, learn::ask_claude)
}

pub fn spawn_with(out: Sender<Digest>, ask: fn(&str) -> Option<String>) -> Sender<Job> {
    let (tx, rx) = mpsc::channel::<Job>();
    let rx = Arc::new(Mutex::new(rx));
    for _ in 0..WORKERS {
        let (rx, out) = (rx.clone(), out.clone());
        std::thread::spawn(move || loop {
            // замок только на ожидание задачи: в `while let` он дожил бы до конца работы и воркеры шли бы по одному
            let Ok(job) = rx.lock().unwrap().recv() else { return };
            if let Some(d) = digest(job, ask) {
                if out.send(d).is_err() {
                    return;
                }
            }
        });
    }
    tx
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    fn job(ask: Ask, text: &str, fallback: Option<&str>) -> Job {
        Job { session: "s".into(), project: "plug".into(), ask, text: text.into(), context: "почини орб".into(), fallback: fallback.map(String::from) }
    }

    #[test]
    fn prompt_has_rules_context_and_tail() {
        let long = format!("{}КОНЕЦ", "а".repeat(MAX_INPUT * 2));
        let p = prompt(&job(Ask::Final, &long, None));
        assert!(p.contains("Не называй переменные"));
        assert!(p.contains("\"say\"") && p.contains("\"important\""));
        assert!(p.contains("почини орб"));
        assert!(p.contains("КОНЕЦ"));                       // хвост, а не начало
        assert!(p.chars().count() < MAX_INPUT + 2000);
        assert!(prompt(&job(Ask::Question, "x", None)).contains("ждёт ответа"));
        assert!(prompt(&job(Ask::Progress, "x", None)).contains("какой шаг закрыт"));
    }

    #[test]
    fn parse_accepts_fenced_json_rejects_prose_and_empty() {
        assert_eq!(parse("```json\n{\"say\": \"Экран готов.\", \"important\": false}\n```"), Some((false, "Экран готов.".into())));
        assert_eq!(parse("{\"say\":\"Вопрос.\",\"important\":true}"), Some((true, "Вопрос.".into())));
        assert_eq!(parse("{\"say\":\"Без важности.\"}"), Some((false, "Без важности.".into())));
        assert_eq!(parse("Sure! Here is the summary."), None);
        assert_eq!(parse("{\"say\": \"  \", \"important\": true}"), None);
    }

    #[test]
    fn digest_uses_model_answer() {
        let d = digest(job(Ask::Final, "Сделал.", Some("готово")), |_| Some("{\"say\":\"Фича готова.\",\"important\":true}".into())).unwrap();
        assert_eq!(d, Digest { session: "s".into(), project: "plug".into(), ask: Ask::Final, important: true, say: "Фича готова.".into() });
    }

    #[test]
    fn digest_falls_back_on_prose() {
        let d = digest(job(Ask::Question, "Удалить?", Some("Клод задал вопрос")), |_| Some("Sure!".into())).unwrap();
        assert_eq!((d.say.as_str(), d.important), ("Клод задал вопрос", true)); // вопрос без модели — всё равно важен
        let d = digest(job(Ask::Final, "Сделал.", Some("готово")), |_| None).unwrap();
        assert_eq!((d.say.as_str(), d.important), ("готово", false));
        assert_eq!(digest(job(Ask::Progress, "шаг", None), |_| None), None);
        assert_eq!(digest(job(Ask::Final, "  ", Some("готово")), |_| panic!("пустой текст — модель не зовём")).unwrap().say, "готово");
    }

    #[test]
    fn spawn_runs_at_most_three_at_once() {
        static NOW: AtomicUsize = AtomicUsize::new(0);
        static MAX: AtomicUsize = AtomicUsize::new(0);
        fn slow(_: &str) -> Option<String> {
            let n = NOW.fetch_add(1, Ordering::SeqCst) + 1;
            MAX.fetch_max(n, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(50));
            NOW.fetch_sub(1, Ordering::SeqCst);
            Some("{\"say\":\"ок\"}".into())
        }
        let (tx, rx) = mpsc::channel();
        let jobs = spawn_with(tx, slow);
        for _ in 0..6 { jobs.send(job(Ask::Final, "т", None)).unwrap(); }
        for _ in 0..6 { rx.recv_timeout(Duration::from_secs(5)).unwrap(); }
        assert!(MAX.load(Ordering::SeqCst) <= WORKERS);
    }
}
