//! Пересказ хода агента: лёгкая модель → короткая фраза для голоса.

use crate::gitfacts::{self, Facts};
use crate::learn;
use crate::text::sentences;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Ask {
    Progress,
    Final,
    /// конец хода, который начал не пользователь, а отчёт помощника агента (оркестратор): звучит, только если important
    Step,
    Question,
    /// сводка итогов нескольких агентов после долгого отсутствия
    Summary,
    /// сторож: агент давно молчит — завис или работа долгая по природе (important — завис)
    Hung,
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
    /// папка агента: итогу — факты git (done_facts, только Claude); None — без фактов
    pub cwd: Option<String>,
    /// начало хода — последний промпт пользователя, эпоха, с
    pub since: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Digest {
    pub session: String,
    pub project: String,
    pub ask: Ask,
    pub important: bool,
    pub say: String,
    /// git после хода — говорит код, не модель
    pub facts: Facts,
}

/// Символов текста агента в промпте — хвост: итог хода в конце.
pub const MAX_INPUT: usize = 6000;
pub const WORKERS: usize = 3;
/// Слов в пересказе — около 12 секунд речи; модель иногда пишет больше, чем просят.
pub const MAX_SAY_WORDS: usize = 30;
/// Слов в сводке нескольких итогов — длиннее одного пересказа.
pub const MAX_SUMMARY_WORDS: usize = 45;

// Тон — тот же, что у мозга Омы (дизайн 2026-11-12, «Характер»): новость с результата, о себе — в женском роде, без шуток.
const COMMON: &str = "Ты — Ома и рассказываешь пользователю голосом, что сделал его ИИ-агент. Говори по-русски, как живой коллега: коротко, простыми словами, без канцелярита («осуществлено», «произведено», «в рамках», «данный»). \
Начинай с результата или с того, что нужно от пользователя, — не со слова «агент»: «всё влито, нужны решения по выкатке», «тесты падают на сборке», «нужно решить, куда пушить». Пиши безличными и страдательными формами («влито», «проверено», «не проверено»). Кто делал, не называй, если и так ясно; если без этого нельзя — «агент», в третьем лице; не «я сделал» и не «ты сделал». \
К пользователю — на «ты» и без родовых окончаний: с «ты» не бывает прошедшего времени и слов «рад», «готов», «должен» — не «ты слушал», «видела», «готов ли», а «во время чтения», «слышно ли», «получилось ли». \
Риск, который виден в тексте агента, — не проверено, не запушено, удаление, выкатка, миграция, — назови прямо, с причиной в двух-трёх словах, можно со своим советом: «запуск не проверен — я бы прогнала тесты»; о себе — в женском роде. Риска, которого нет в тексте агента, не придумывай. Без шуток: это новости. \
Не называй переменные, функции, классы, файлы, пути, команды и код — только смысл: что сделано, каким способом в общих словах, что дальше. \
Кого или чего ждёт агент — бери из его текста. Что агент ждёт пользователя, говори, только если агент прямо обращается к пользователю: просит ответить, решить, проверить. «Жду ревью», «жду исполнителей» без обращения — агент ждёт своих помощников. Не приписывай пользователю действий, о которых агент не просит. \
В словах, где ударение легко прочесть неверно, — омографы (замо́к и за́мок), редкие слова, имена и названия — ставь знак ударения U+0301 сразу после ударной гласной. \
Без markdown и списков. Ответ — строго одна строка JSON: {\"say\": \"…\", \"important\": true или false}.";

/// git — у итога есть факты git: о коммитах и пуше скажет код, модель молчит.
pub fn prompt(job: &Job, git: bool) -> String {
    let kind = match job.ask {
        Ask::Final if git => "Агент закончил ход. Одной фразой, не длиннее 20 слов: что сделано и что требует внимания пользователя — оставшиеся проблемы, советы агента, что не проверено. О коммитах и пуше, в том числе «не запушено» из рисков выше, не говори — это скажет код. important — true, если пользователю нужно что-то решить или сделать.",
        Ask::Final => "Агент закончил ход. Одной фразой, не длиннее 20 слов: что сделано и что требует внимания пользователя — оставшиеся проблемы, советы агента, что не запушено или не проверено. important — true, если пользователю нужно что-то решить или сделать.",
        Ask::Step => "Агент закончил ход, который начал не пользователь, а отчёт его помощника: это переписка агента со своими помощниками. Одной фразой, не длиннее 20 слов: что сделано. important — true, только если вся работа, о которой просил пользователь, закончена или агент прямо обращается к пользователю (решить, ответить, проверить, ошибка, которую он не обойдёт); ход переписки с помощниками («ревьювер принял ответ», «исполнитель закрывает находку», «отправил на ревью») — false.",
        Ask::Question => "Агент ждёт ответа пользователя. Одной фразой, не длиннее 25 слов. Начни с того, что нужно решить: «нужен ответ: …» — суть вопроса общими словами; совет агента, если он есть, — после. Варианты ответа не перечисляй. Вопросов несколько — их число и суть каждого в двух-трёх словах. Вопрос важный (архитектура, данные, деньги, необратимые действия) — в тех же 25 словах, как агент предлагает это сделать. important — true.",
        Ask::Summary => "Ниже итоги нескольких агентов, по строке на проект: «проект: итог». Сожми их в две-три фразы, не длиннее 40 слов, по проекту: «проект — итог»; главное — что требует внимания пользователя. important — true.",
        Ask::Hung => "Агент работает, но давно молчит. Ниже его последний вызов инструмента и экран терминала. important — true, если похоже на зависание: вызов, который обычно идёт секунды или минуты, или пустое ожидание без признаков работы; false — если работа долгая по природе (сборка, тесты, установка, загрузка, сон, ожидание сервера или таймера) или на экране видно, что идёт повтор запроса. say — причина, не длиннее 8 слов.",
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
    // факты git — кодом, без модели, до неё: за 5–15 с ответа модели пользователь может начать новый ход, его правки — не этого;
    // вне цикла событий: git в большом репозитории — сотни мс
    let facts = job.cwd.as_deref().and_then(|c| gitfacts::collect(std::path::Path::new(c), job.since));
    // пустой текст пересказывать нечего — модель не зовём
    let said = if job.text.trim().is_empty() { None } else { ask(&prompt(&job, facts.is_some())).and_then(|o| parse(&o, if job.ask == Ask::Summary { MAX_SUMMARY_WORDS } else { MAX_SAY_WORDS })) };
    // сломанный claude (вход, PATH, лимит) иначе виден только по запасным фразам
    if said.is_none() && !job.text.trim().is_empty() {
        eprintln!("agent-speak: пересказ не удался ({}): {}", job.session, if job.fallback.is_some() { "запасная фраза" } else { "молчу" });
    }
    // вопрос важен и без модели: пользователя ждут
    let (important, say) = said.or_else(|| job.fallback.map(|f| (job.ask == Ask::Question, f)))?;
    Some(Digest { session: job.session, project: job.project, ask: job.ask, important, say, facts: facts.unwrap_or_default() })
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
    #[test]
    fn helper_step_prompt_silences_chatter() {
        let job = Job { session: "s".into(), project: "p".into(), ask: Ask::Step, text: "Ревьювер принял ответ.".into(), context: String::new(), fallback: None, cwd: None, since: 0 };
        let p = prompt(&job, false);
        assert!(p.contains("помощник") && p.contains("false"), "переписку с помощниками модель не считает важной");
    }

    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    fn job(ask: Ask, text: &str, fallback: Option<&str>) -> Job {
        Job { session: "s".into(), project: "plug".into(), ask, text: text.into(), context: "почини орб".into(), fallback: fallback.map(String::from), cwd: None, since: 0 }
    }

    #[test]
    fn prompt_forbids_inventing_user_wait() {
        assert!(prompt(&job(Ask::Final, "Жду ревью 7.", None), false).contains("Не приписывай пользователю действий"));
    }

    #[test]
    fn prompt_starts_with_result_in_omas_tone() {
        // в журнале 2026-10-25…11-12 треть фраз — «проект: Агент сделал…»: новость с результата, не с «агента»
        let p = prompt(&job(Ask::Final, "Готово.", None), false);
        assert!(p.contains("Начинай с результата") && !p.contains("(«агент сделал»)"), "{p}");
        assert!(p.contains("всё влито, нужны решения по выкатке") && p.contains("не «я сделал» и не «ты сделал»"));
        assert!(p.contains("без канцелярита") && p.contains("не придумывай") && p.contains("о себе — в женском роде"));
        assert!(p.contains("Без шуток"), "юмора в новостях нет: Haiku повторяет одни приёмы");
        assert!(prompt(&job(Ask::Question, "x", None), false).contains("Начни с того, что нужно решить"));
        assert!(prompt(&job(Ask::Summary, "a: x", None), false).contains("«проект — итог»"));
    }

    /// Последние ответы агентов из транскриптов Claude Code этой машины (только чтение): не в репозитории — это чужая работа.
    fn recent_agent_texts(n: usize) -> Vec<String> {
        let home = std::path::PathBuf::from(std::env::var("HOME").unwrap()).join(".claude/projects");
        let mut files: Vec<(std::time::SystemTime, std::path::PathBuf)> = std::fs::read_dir(&home).unwrap().flatten()
            .filter(|d| !d.file_name().to_string_lossy().contains("agent-speak-oma")) // чаты самой Омы — не агенты
            .flat_map(|d| std::fs::read_dir(d.path()).into_iter().flatten().flatten())
            .filter(|f| f.path().extension().is_some_and(|e| e == "jsonl"))
            .filter_map(|f| Some((f.metadata().ok()?.modified().ok()?, f.path())))
            .collect();
        files.sort_by(|a, b| b.0.cmp(&a.0));
        files.into_iter().filter_map(|(_, p)| {
            let text = crate::source::claude::last_turn(&std::fs::read_to_string(p).ok()?).join("\n");
            (text.split_whitespace().count() > 15).then_some(text)
        }).take(n).collect()
    }

    #[test]
    #[ignore = "живой claude (Haiku) и транскрипты ~/.claude/projects"]
    fn probe_result_first() {
        // смотреть глазами: начало с результата, не «Агент…», нет «ты сделал», шуток и риска, которого нет в тексте агента
        let texts = recent_agent_texts(8);
        let mut agent_first = 0;
        for text in &texts {
            let j = Job { context: String::new(), ..job(Ask::Final, text, None) }; // задачи пользователя нет: «почини орб» модель вплетает в новость
            let say = digest(j, crate::learn::ask_claude).map(|d| d.say).unwrap_or_default();
            agent_first += say.starts_with("Агент") as usize;
            println!("{:.80}\n  => {say}\n", text.replace('\n', " "));
        }
        println!("с «Агент» в начале: {agent_first} из {}", texts.len());
    }

    #[test]
    fn prompt_asks_for_stress_marks() {
        assert!(prompt(&job(Ask::Final, "Готово.", None), false).contains("U+0301"));
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
        let p = prompt(&job(Ask::Final, &long, None), false);
        assert!(p.contains("Не называй переменные"));
        assert!(p.contains("\"say\"") && p.contains("\"important\""));
        assert!(p.contains("почини орб"));
        assert!(p.contains("КОНЕЦ"));                       // хвост, а не начало
        assert!(p.chars().count() < MAX_INPUT + 3000);  // правила — около 2,5 тыс. знаков, текст агента — только хвост
        assert!(prompt(&job(Ask::Question, "x", None), false).contains("ждёт ответа"));
        assert!(prompt(&job(Ask::Progress, "x", None), false).contains("какой шаг закрыт"));
        assert!(p.contains("не длиннее 20 слов") && p.contains("без родовых окончаний"));
        assert!(prompt(&job(Ask::Question, "x", None), false).contains("Варианты ответа не перечисляй"));
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
        assert_eq!(d, Digest { session: "s".into(), project: "plug".into(), ask: Ask::Final, important: true, say: "Фича готова.".into(), facts: Facts::default() });
    }

    #[test]
    fn final_with_cwd_gets_git_facts_and_model_keeps_off_git() {
        let dir = gitfacts::tests::repo("digest");
        std::fs::write(dir.join("a"), "2").unwrap();
        let j = |cwd: Option<&std::path::Path>| Job { cwd: cwd.map(|c| c.to_string_lossy().into()), since: gitfacts::tests::now() - 100, ..job(Ask::Final, "Сделал.", Some("готово")) };
        let git = prompt(&j(Some(&dir)), true);
        assert!(git.contains("О коммитах и пуше, в том числе «не запушено» из рисков выше, не говори — это скажет код") && !git.contains("что не запушено"), "{git}");
        assert!(prompt(&j(None), false).contains("что не запушено"), "без фактов — как раньше");
        let ok = |_: &str| Some("{\"say\":\"Фича готова.\"}".to_string());
        assert_eq!(digest(j(Some(&dir)), ok).unwrap().facts.voice(), "Не закоммичен 1 файл.");
        assert_eq!(digest(j(None), ok).unwrap().facts, Facts::default());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn git_facts_are_taken_before_the_model_and_choose_its_prompt() {
        let dir = gitfacts::tests::repo("before-model");
        let j = |cwd: &std::path::Path| Job { cwd: Some(cwd.to_string_lossy().into()), since: gitfacts::tests::now() - 100, ..job(Ask::Final, "Сделал.", None) };
        // пока модель думает, пользователь начал новый ход: его правка — не факт прошлого
        let d = digest(j(&dir), |p| {
            assert!(p.contains("это скажет код"), "факты есть — о пуше говорит код");
            std::fs::write(dir.join("новый ход"), "1").unwrap();
            Some("{\"say\":\"Фича готова.\"}".into())
        }).unwrap();
        assert_eq!(d.facts, Facts::default(), "git опрошен до модели");
        // не репозиторий — фактов нет: «не запушено» остаётся модели
        let none = std::env::temp_dir().join(format!("oma-digest-norepo-{}", std::process::id()));
        std::fs::create_dir_all(&none).unwrap();
        let d = digest(j(&none), |p| {
            assert!(p.contains("что не запушено") && !p.contains("это скажет код"), "{p}");
            Some("{\"say\":\"Фича готова.\"}".into())
        }).unwrap();
        assert_eq!(d.facts, Facts::default());
        let _ = (std::fs::remove_dir_all(dir), std::fs::remove_dir_all(none));
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
        let p = prompt(&job(Ask::Summary, "plug: Фича готова.\nsite: Тесты упали.", None), false);
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

    #[test]
    fn hung_prompt_asks_hang_verdict() {
        let p = prompt(&job(Ask::Hung, "Тишина 12 мин. Вызов: Bash sleep 600.", None), false);
        assert!(p.contains("похоже на зависание") && p.contains("Тишина 12 мин"), "{p}");
    }
}
