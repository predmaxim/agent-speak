//! Фразы, которые говорит код, а не модель, и их варианты (дизайн 2026-11-12, «result-first»; таблицы утвердил пользователь).
//! Варианты звучат по кругу, по порядку, не случайно; после запуска — первый.

use std::collections::HashMap;

/// Сколько раз звучал каждый круг; ключ — первый вариант.
#[derive(Default)]
pub struct Rotor(HashMap<&'static str, usize>);

impl Rotor {
    pub fn next(&mut self, variants: &'static [&'static str]) -> &'static str {
        let n = self.0.entry(variants[0]).or_default();
        *n += 1;
        variants[(*n - 1) % variants.len()]
    }
}

/// Род Омы — по голосу (config::gender): шаблон `{м|ж}` → форма её рода, "m" — мужская, иначе женская («Понял{|а}»,
/// «пош{ёл|ла}»). Подстановки без «|» («{n}», «{что}») не трогаются: шаблон разрешают до подстановки, чужой текст — никогда.
/// Тот же синтаксис — gendered в python/agent_voice/conversation.py.
pub fn gendered(t: &str, g: &str) -> String {
    let mut out = String::with_capacity(t.len());
    let mut rest = t;
    while let Some(i) = rest.find('{') {
        let Some(j) = rest[i..].find('}').map(|j| i + j) else { break };
        let (head, inner) = rest[..j].rsplit_once('{').unwrap(); // ближайшая к «}» «{»: внутри шаблона скобок нет
        out.push_str(head);
        match inner.split_once('|') {
            Some((m, f)) if !f.contains('|') => out.push_str(if g == "m" { m } else { f }),
            _ => out.push_str(&rest[head.len()..=j]),
        }
        rest = &rest[j + 1..];
    }
    out + rest
}

/// «Ты тут?»: «Эй» — с ним Silero читает «ты тут» вопросом (первый вариант пользователь выбрал из десяти). Ответ на любой
/// вариант должен пройти is_answer Омы («да», «тут», «давай»…); слов и основ ответа (ANSWER_WORDS, ANSWER_STEMS в conversation.py)
/// в вопросе не должно быть, кроме «тут»: ответ, начатый под эхо вопроса, иначе срезается как эхо (тест Python
/// test_every_are_you_there_variant_hears_the_answer_under_its_echo). В первых двух, выбранных пользователем, есть «новости».
pub const HERE: &[&str] = &["Эй, ты тут? Есть новости.", "Эй, послушаешь новости?", "Эй, ты тут? Есть вести."];
/// Первая фраза после долгого отсутствия, перед обзором доски.
pub const WELCOME: &[&str] = &["С возвращением.", "Снова привет.", "Привет ещё раз."];
/// Начало обзора доски после приветствия: «Пока тебя не было: …».
pub const AWAY: &[&str] = &["Пока тебя не было", "За это время", "Тем временем"];
/// Итог хода, когда модель пересказа не ответила.
pub const DONE: &[&str] = &["готово", "ход закончен", "сделано"];
/// После вопроса на доске ждут ещё вопросы; {n} — «один вопрос», «2 вопроса».
pub const MORE: &[&str] = &["После этого ещё {n}.", "Дальше ещё {n}.", "В очереди ещё {n}."];

// Сторож сделал сам — фраза после «проект: ».
/// Перезапустила после своего Ctrl+C.
pub const REVIVED: &[&str] = &["перезапустила зависшего агента.", "агент висел — перезапустила.", "подняла агента заново, он висел."];
pub const SUB_STOPPED: &[&str] = &["прервала зависшего сабагента, агент запустит его заново.", "сабагент висел — прервала.", "остановила сабагента, агент перезапустит."];
pub const STOPPED: &[&str] = &["прервала зависший ход, попросила продолжить.", "агент висел — прервала, попросила продолжить.", "сбросила зависший ход и попросила идти дальше."];
/// Перезапустила упавшего посреди хода.
pub const RESUMED: &[&str] = &["перезапустила агента, он закрылся посреди хода.", "агент вылетел посреди хода — перезапустила.", "подняла агента, он закрылся посреди хода."];
pub const WOKE: &[&str] = &["разбудила агента: лимит сброшен.", "лимит сброшен, агента разбудила.", "после сброса лимита разбудила агента."];
pub const CONTINUED: &[&str] = &["связь оборвалась, попросила агента продолжить.", "после обрыва связи попросила продолжить.", "был обрыв связи — попросила агента продолжить."];
pub const NUDGED: &[&str] = &["написала агенту: его помощник молчит.", "помощник агента затих — сказала агенту.", "напомнила агенту про молчащего помощника."];
/// Лимит кончился: одна фраза на все агенты; сброс неизвестен — без «, сброс в {t}».
pub const LIMIT: &[&str] = &["Кончился лимит Claude, сброс в {t}.", "Лимит Claude исчерпан, сброс в {t}.", "Claude упёрся в лимит, сброс в {t}."];
/// После «проект: »; {что} — «нужен вход в Claude».
pub const API: &[&str] = &["ошибка API — {что}.", "API отказал — {что}.", "Claude ответил ошибкой — {что}."];
/// Хвост срочных фраз сторожа: нужен пользователь. Без родовых окончаний, не «посмотри сам».
pub const END: &[&str] = &["загляни, пожалуйста", "нужна твоя помощь", "посмотри, пожалуйста"];

/// Круги фраз сторожа: доска склеивает одинаковые новости разных проектов, какой бы вариант ни выпал каждому.
const WATCH: [&[&str]; 8] = [REVIVED, SUB_STOPPED, STOPPED, RESUMED, WOKE, CONTINUED, NUDGED, API];

/// Фраза сторожа (без «проект: ») любым вариантом → первым, хвост END → первым хвостом; иная фраза — как есть.
/// Ключ склейки доски: «A, B: лимит сброшен…» вместо двух фраз подряд.
pub fn canon(said: &str) -> String {
    let s = END[1..].iter().fold(said.to_string(), |s, e| s.replace(e, END[0]));
    for t in WATCH {
        for v in t {
            let Some((pre, post)) = v.split_once("{что}") else {
                if s == *v {
                    return t[0].to_string();
                }
                continue;
            };
            if s.len() >= pre.len() + post.len() && s.starts_with(pre) && s.ends_with(post) {
                return t[0].replace("{что}", &s[pre.len()..s.len() - post.len()]);
            }
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gendered_picks_the_form_of_the_voice() {
        assert_eq!(gendered("Понял{|а}, пош{ёл|ла}.", "m"), "Понял, пошёл.");
        assert_eq!(gendered("Понял{|а}, пош{ёл|ла}.", "f"), "Поняла, пошла.");
        assert_eq!(gendered("сам{|а} — {что}, ещё {n}", "m"), "сам — {что}, ещё {n}", "подстановки без «|» не трогаются");
        assert_eq!(gendered("без шаблона {", "f"), "без шаблона {");
        assert_eq!(gendered("{x{a|b}}", "m"), "{xa}", "шаблон — ближайшие скобки, как регулярка в Python");
    }

    #[test]
    fn rotor_goes_round_in_order_per_table() {
        let mut r = Rotor::default();
        let got: Vec<&str> = (0..4).map(|_| r.next(HERE)).collect();
        assert_eq!(got, [HERE[0], HERE[1], HERE[2], HERE[0]], "по порядку, четвёртый — снова первый");
        assert_eq!(r.next(WELCOME), WELCOME[0], "у другой таблицы свой круг");
    }

    #[test]
    fn tables_have_three_variants_and_here_asks() {
        for t in [HERE, WELCOME, AWAY, DONE, MORE, LIMIT, END].iter().chain(WATCH.iter()) {
            assert_eq!(t.len(), 3, "{t:?}");
        }
        assert!(HERE.iter().all(|v| v.contains('?')), "«Ты тут?» — вопрос в каждом варианте");
    }

    #[test]
    fn canon_maps_any_variant_to_first() {
        assert_eq!(canon(REVIVED[1]), REVIVED[0]);
        assert_eq!(canon("API отказал — нужен вход в Claude."), "ошибка API — нужен вход в Claude.");
        assert_ne!(canon("API отказал — нужен вход в Claude."), canon("ошибка API — запрос отклонён."), "разные ошибки не склеиваются");
        assert_eq!(canon("связь с Claude не держится — нужна твоя помощь."), "связь с Claude не держится — загляни, пожалуйста.");
        assert_eq!(canon("Фича готова."), "Фича готова.");
    }
}
