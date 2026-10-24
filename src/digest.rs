//! Пересказ хода агента: лёгкая модель → короткая фраза для голоса.

use crate::learn;
use crate::text::sentences;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Ask {
    Progress,
    Final,
    Question,
    /// сводка итогов нескольких агентов после долгого отсутствия
    Summary,
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
/// Слов в пересказе — около 12 секунд речи; модель иногда пишет больше, чем просят.
pub const MAX_SAY_WORDS: usize = 30;
/// Слов в сводке нескольких итогов — длиннее одного пересказа.
pub const MAX_SUMMARY_WORDS: usize = 45;

const COMMON: &str = "Ты — Ома, помощница, которая пересказывает пользователю голосом работу ИИ-агента. Пиши по-русски, коротко, простыми словами. \
Об агенте говори в третьем лице («агент сделал»), к пользователю — на «ты» и без родовых окончаний: с «ты» не бывает прошедшего времени и слов «рад», «готов», «должен» — не «ты слушал», «видела», «готов ли», а «во время чтения», «слышно ли», «получилось ли». \
Не называй переменные, функции, классы, файлы, пути, команды и код — только смысл: что сделано, каким способом в общих словах, что дальше. \
Кого или чего ждёт агент — бери из его текста. Что агент ждёт пользователя, говори, только если агент прямо обращается к пользователю: просит ответить, решить, проверить. «Жду ревью», «жду исполнителей» без обращения — агент ждёт своих помощников. Не приписывай пользователю действий, о которых агент не просит. \
В словах, где ударение легко прочесть неверно, — омографы (замо́к и за́мок), редкие слова, имена и названия — ставь знак ударения U+0301 сразу после ударной гласной. \
Без markdown и списков. Ответ — строго одна строка JSON: {\"say\": \"…\", \"important\": true или false}.";

pub fn prompt(job: &Job) -> String {
    let kind = match job.ask {
        Ask::Final => "Агент закончил ход. Одной фразой, не длиннее 20 слов: что сделано и что требует внимания пользователя — оставшиеся проблемы, советы агента, что не запушено или не проверено. important — true, если пользователю нужно что-то решить или сделать.",
        Ask::Question => "Агент ждёт ответа пользователя. Одной фразой, не длиннее 25 слов: суть вопроса общими словами и совет агента, если он есть. Варианты ответа не перечисляй. Вопросов несколько — их число и суть каждого в двух-трёх словах. Вопрос важный (архитектура, данные, деньги, необратимые действия) — в тех же 25 словах, как агент предлагает это сделать. important — true.",
        Ask::Summary => "Ниже итоги нескольких агентов, по строке на проект: «проект: итог». Сожми их в две-три фразы, не длиннее 40 слов: назови проекты и главное, что требует внимания пользователя. important — true.",
        Ask::Progress => "Агент ещё работает; ниже его сообщения за последнюю минуту. Одной фразой, не длиннее 15 слов: какой шаг закрыт и что дальше. important — true, только если пользователю стоит узнать это сейчас: проблема, смена плана, неожиданный результат.",
    };
    let skip = job.text.chars().count().saturating_sub(MAX_INPUT);
    let tail: String = job.text.chars().skip(skip).collect();
    format!("{COMMON} {kind}\n\nЗадача пользователя: {}\n\nТекст агента:\n{tail}", job.context)
}

/// (important, say) из ответа модели; она может обернуть JSON в ```-ограду или фразу.
pub fn parse(out: &str, max: usize) -> Option<(bool, String)> {
    let v: serde_json::Value = serde_json::from_str(out.get(out.find('{')?..=out.rfind('}')?)?).ok()?;
    let say = v["say"].as_str()?.trim();
    (!say.is_empty()).then(|| (v["important"].as_bool().unwrap_or(false), clip(say, max)))
}

/// Не длиннее max слов: целые предложения, сколько влезет; первое само длиннее — обрезать по слову с «…».
fn clip(say: &str, max: usize) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut words = 0;
    for s in sentences::split(say) {
        words += s.split_whitespace().count();
        if words > max {
            break;
        }
        out.push(s);
    }
    if out.is_empty() {
        let head: Vec<&str> = say.split_whitespace().take(max).collect();
        return format!("{}…", head.join(" ").trim_end_matches([',', ';', ':', '—', '-']).trim_end());
    }
    out.join(" ")
}

pub fn digest(job: Job, ask: impl Fn(&str) -> Option<String>) -> Option<Digest> {
    // пустой текст пересказывать нечего — модель не зовём
    let said = if job.text.trim().is_empty() { None } else { ask(&prompt(&job)).and_then(|o| parse(&o, if job.ask == Ask::Summary { MAX_SUMMARY_WORDS } else { MAX_SAY_WORDS })) };
    // сломанный claude (вход, PATH, лимит) иначе виден только по запасным фразам
    if said.is_none() && !job.text.trim().is_empty() {
        eprintln!("agent-speak: пересказ не удался ({}): {}", job.session, if job.fallback.is_some() { "запасная фраза" } else { "молчу" });
    }
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
    fn prompt_forbids_inventing_user_wait() {
        assert!(prompt(&job(Ask::Final, "Жду ревью 7.", None)).contains("Не приписывай пользователю действий"));
    }

    #[test]
    fn prompt_asks_for_stress_marks() {
        assert!(prompt(&job(Ask::Final, "Готово.", None)).contains("U+0301"));
    }

    #[test]
    #[ignore = "живой claude (Haiku)"]
    fn probe_waiting_for_helpers() {
        let texts = ["Исполнитель на проверке запущен. Лишний временный файл удалён. Жду ревью 7.", "Жду правки обеих спек и исполнителей партии 1."];
        for text in texts {
            for _ in 0..3 {
                let d = digest(job(Ask::Final, text, None), crate::learn::ask_claude);
                println!("{text:.40} => {:?}", d.map(|d| d.say));
            }
        }
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
        assert!(p.contains("не длиннее 20 слов") && p.contains("без родовых окончаний"));
        assert!(prompt(&job(Ask::Question, "x", None)).contains("Варианты ответа не перечисляй"));
    }

    #[test]
    fn clip_keeps_whole_sentences_within_limit() {
        assert_eq!(clip("Коротко.", MAX_SAY_WORDS), "Коротко.");
        let c = clip(&"Раз два три четыре пять. ".repeat(10), MAX_SAY_WORDS); // 50 слов
        assert_eq!((c.split_whitespace().count(), c.ends_with('.')), (30, true));
        let c = clip(&format!("{} — хвост", "слово ".repeat(40)), MAX_SAY_WORDS); // одно предложение длиннее предела
        assert_eq!((c.split_whitespace().count(), c.ends_with("слово…")), (MAX_SAY_WORDS, true));
    }

    #[test]
    fn parse_accepts_fenced_json_rejects_prose_and_empty() {
        assert_eq!(parse("```json\n{\"say\": \"Экран готов.\", \"important\": false}\n```", MAX_SAY_WORDS), Some((false, "Экран готов.".into())));
        assert_eq!(parse("{\"say\":\"Вопрос.\",\"important\":true}", MAX_SAY_WORDS), Some((true, "Вопрос.".into())));
        assert_eq!(parse("{\"say\":\"Без важности.\"}", MAX_SAY_WORDS), Some((false, "Без важности.".into())));
        assert_eq!(parse("Sure! Here is the summary.", MAX_SAY_WORDS), None);
        assert_eq!(parse("{\"say\": \"  \", \"important\": true}", MAX_SAY_WORDS), None);
        let long = format!("{{\"say\": \"{}\"}}", "Раз два три четыре пять. ".repeat(10));
        assert_eq!(parse(&long, MAX_SAY_WORDS).unwrap().1.split_whitespace().count(), 30); // длинный ответ модели обрезан
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
    fn summary_prompt_lists_finals() {
        let p = prompt(&job(Ask::Summary, "plug: Фича готова.\nsite: Тесты упали.", None));
        assert!(p.contains("Сожми их в две-три фразы") && p.contains("plug: Фича готова.") && p.contains("site: Тесты упали."));
    }

    #[test]
    fn summary_clipped_at_45_words() {
        let long = format!("{{\"say\": \"{}\"}}", "Раз два три четыре пять. ".repeat(12)); // 60 слов
        assert_eq!(parse(&long, MAX_SUMMARY_WORDS).unwrap().1.split_whitespace().count(), 45);
    }

    #[test]
    fn summary_falls_back_to_joined_finals() {
        let d = digest(job(Ask::Summary, "a: x\nb: y", Some("a: x. b: y")), |_| None).unwrap();
        assert_eq!(d.say, "a: x. b: y");
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
