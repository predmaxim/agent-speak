//! Ответы и задания агентам от Омы: как передать (текстом или клавишами меню), куда, запуск нового агента.
//! Зовёт Ома через CLI `agent-speak tell | menu | launch | projects`: исход нужен её мозгу сразу. tell исполняет служба в своём
//! потоке действий, в одной очереди с действиями сторожа: две записи в одну панель не перемешаются.

use crate::mux::{self, Key, Step};
use crate::source::{claude, tail::tail_of};
use serde_json::Value;

/// Пауза после цифры пункта меню: TUI открывает поле ввода не мгновенно.
const MENU_PAUSE: u64 = 500;
/// После «Chat about this» агент получает отказ и начинает ход — ответ уйдёт следующей репликой.
const DECLINE_PAUSE: u64 = 1500;
const PLAN_FEEDBACK: &str = "Tell Claude what to change";

/// Одной строкой: Enter внутри текста отправил бы задание по частям (Orca, поле меню).
// shortcut: структура длинного задания теряется; начнут путаться — проверить многострочную вставку в Orca
pub fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn digit(n: usize) -> Result<Step, String> {
    u8::try_from(n).ok().filter(|d| (1..=9).contains(d)).map(|d| Step::Key(Key::Digit(d))).ok_or_else(|| format!("нет пункта меню {n}"))
}

/// Номер пункта с этим текстом на экране: цифры в начале строки после «❯» и пробелов.
fn item_of(screen: &str, label: &str) -> Option<usize> {
    screen.lines().filter(|l| l.contains(label)).find_map(|l| {
        let l = l.trim_start_matches(|c: char| c.is_whitespace() || c == '❯');
        l.split_once('.').and_then(|(n, _)| n.parse().ok())
    })
}

/// Строка «❯ n. текст» или «  n. текст» → (n, текст).
fn numbered(l: &str) -> Option<(usize, &str)> {
    let (num, rest) = l.trim_start_matches(|c: char| c.is_whitespace() || c == '❯').split_once(". ")?;
    Some((num.parse().ok()?, rest.trim()))
}

/// Текст пункта n меню на экране: последняя строка «n. текст» — меню внизу, выше бывает нумерованный план.
fn item_text(screen: &str, n: usize) -> Option<&str> {
    screen.lines().rev().find_map(|l| numbered(l).filter(|(k, _)| *k == n).map(|(_, t)| t))
}

/// Пункты меню на экране по порядку, как их видит пользователь: «n. текст» снизу вверх (меню — внизу), номера подряд
/// с 1, строка каждого — ниже предыдущего (нумерованный план над меню — не пункты); без курсора «❯» меню нет — пусто.
fn menu(screen: &str) -> Vec<String> {
    if !menu_on(screen) {
        return Vec::new();
    }
    let lines: Vec<&str> = screen.lines().collect();
    let mut items: Vec<String> = Vec::new();
    let mut after = None;
    for n in 1.. {
        let found = lines.iter().enumerate().rev().find_map(|(i, l)| numbered(l).filter(|(k, _)| *k == n).map(|(_, t)| (i, t)));
        match found {
            Some((i, t)) if after.is_none_or(|a| i > a) => {
                items.push(t.to_string());
                after = Some(i);
            }
            _ => break,
        }
    }
    items
}

/// Пункты меню агента session с экрана его терминала — только чтение, ничего не нажимается (`agent-speak menu`):
/// Ома называет в вопросе подтверждения тот пункт, что увидит пользователь (у плана — «Yes, and use auto mode»).
pub fn menu_of(session: &str) -> Result<Vec<String>, String> {
    let live = crate::agents::scan_all().into_iter().find(|l| l.id == session).ok_or("агента нет среди живых")?;
    let pane = mux::locate(live.pid).ok_or("агент не в Orca и не в herdr — экран не прочитать")?;
    let items = menu(&mux::screen(&pane)?);
    if items.is_empty() {
        return Err("на экране агента нет меню".into());
    }
    Ok(items)
}

/// Цифра пункта n — только если на экране у него ожидаемый текст: устаревший или двойной клик, другое меню
/// (разрешение вместо вопроса) — отказ, а не чужой выбор.
fn press(screen: &str, n: usize, want: &str) -> Result<Step, String> {
    match item_text(screen, n) {
        Some(t) if t == want.trim() => digit(n),
        Some(t) => Err(format!("на экране пункт {n} — «{t}», а не «{want}»: меню сменилось, не жму")),
        None => Err(format!("не вижу на экране пункта {n} «{want}» — не жму")),
    }
}

/// Свой ответ в меню: пункт с полем ввода (его цифра — key), текст без Enter, Enter.
fn typed(key: Step, text: &str) -> Vec<Step> {
    vec![key, Step::Pause(MENU_PAUSE), Step::Type(text.into()), Step::Key(Key::Enter)]
}

/// Как передать ответ агенту: вызов, который ждёт пользователя (меню), — клавишами; иначе — текстом в поле ввода.
/// option — пункт меню (0 — свой ответ текстом); expect — его метка, как её видел спросивший (пусто — не сверять).
/// Цифра жмётся, только если меню на экране то самое: метка пункта совпала с вариантом вызова, у плана — пункт
/// одобрения. Второе в ответе — метка нажатого пункта с экрана (у плана «Yes, and use auto mode» — Ома её зачитывает),
/// пусто — пункт не выбирали. Экран не читается только у ответа текстом без вызова в полёте.
pub fn steps(call: Option<&Value>, text: &str, option: usize, expect: &str, screen: impl FnOnce() -> Result<String, String>) -> Result<(Vec<Step>, String), String> {
    let text = text.trim();
    match call.and_then(|c| c["name"].as_str()) {
        Some("AskUserQuestion") => {
            let qs = call.unwrap()["input"]["questions"].as_array().cloned().unwrap_or_default();
            let n = qs.first().and_then(|q| q["options"].as_array()).map_or(0, Vec::len);
            let single = qs.len() == 1 && qs[0]["multiSelect"] != true;
            match option {
                _ if !single && text.is_empty() => Err("на такой вопрос — ответ текстом".into()),
                // несколько вопросов или выбор нескольких: меню отклоняем («Chat about this»), ответ — репликой
                _ if !single => Ok((vec![press(&screen()?, n + 2, CHAT_ABOUT)?, Step::Pause(DECLINE_PAUSE), Step::Text(text.into())], String::new())),
                0 if text.is_empty() => Err("пустой ответ".into()),
                0 => Ok((typed(press(&screen()?, n + 1, TYPE_SOMETHING)?, text), String::new())),
                o if o <= n => {
                    let label = qs[0]["options"][o - 1]["label"].as_str().unwrap_or_default().to_string();
                    if !expect.is_empty() && expect != label {
                        return Err(format!("вариант {o} — «{label}», а не «{expect}»: вопрос сменился, не жму"));
                    }
                    Ok((vec![press(&screen()?, o, &label)?], label))
                }
                o => Err(format!("нет варианта {o}")),
            }
        }
        Some("ExitPlanMode") => {
            let screen = screen()?;
            match option {
                1 => {
                    let shown = item_text(&screen, 1).filter(|t| t.starts_with("Yes")).ok_or(NO_PLAN_MENU)?;
                    if !expect.is_empty() && expect != shown {
                        return Err(format!("на экране пункт 1 — «{shown}», а не «{expect}»: не жму"));
                    }
                    Ok((vec![digit(1)?], shown.to_string()))
                }
                0 if !text.is_empty() => item_of(&screen, PLAN_FEEDBACK).map_or_else(|| Err(NO_PLAN_MENU.into()), |i| Ok((typed(digit(i)?, text), String::new()))),
                0 => Err("пустой ответ".into()),
                o => Err(format!("нет варианта {o}")),
            }
        }
        _ if text.is_empty() => Err("пустой текст".into()),
        // вызов в полёте может ждать разрешения: в меню Enter выбрал бы подсвеченный пункт («Yes»)
        Some(_) if menu_on(&screen()?) => Err("агент ждёт разрешения в терминале — ответь там".into()),
        _ => Ok((vec![Step::Text(text.into())], String::new())),
    }
}

/// На экране меню Claude Code: строка с курсором «❯» на пункте с номером.
fn menu_on(screen: &str) -> bool {
    screen.lines().any(|l| l.trim_start().strip_prefix('❯').is_some_and(|r| {
        let r = r.trim_start();
        r.split_once('.').is_some_and(|(n, _)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
    }))
}

const NO_PLAN_MENU: &str = "не вижу меню одобрения плана";
/// Пункты меню вопроса Claude Code после вариантов (живая проверка, Claude Code 2.1.310).
const TYPE_SOMETHING: &str = "Type something.";
const CHAT_ABOUT: &str = "Chat about this";

/// Передать агенту session текст или пункт меню (expect — его метка, пусто — не сверять); Ok — метка нажатого пункта.
/// Зовётся из потока действий службы (`mux::Work::Other`): чтение экрана, сверка и нажатие — без чужих записей между ними.
pub fn tell(session: &str, option: usize, text: &str, expect: &str) -> Result<String, String> {
    let live = crate::agents::scan_all().into_iter().find(|l| l.id == session).ok_or("агента нет среди живых")?;
    let pane = mux::locate(live.pid).ok_or("агент не в Orca и не в herdr — передать не могу")?;
    // у Codex меню не разбираем — всегда текст
    let call = if live.agent == "claude" { claude::in_flight(&tail_of(&live.transcript.to_string_lossy(), |t| claude::has_record(t, false))) } else { None };
    let (steps, chosen) = steps(call.as_ref(), &one_line(text), option, expect, || mux::screen(&pane))?;
    mux::perform(&pane, &steps)?;
    Ok(chosen)
}

/// Мультиплексор по имени: orca или herdr.
fn named(name: &str) -> Result<&'static dyn mux::Mux, String> {
    mux::adapters().into_iter().find(|m| m.name() == name).ok_or_else(|| format!("нет терминала {name} — есть orca и herdr"))
}

/// Папки, где мультиплексор where_ умеет запустить агента; его ошибка (Orca закрыта) — как есть.
pub fn projects(where_: &str) -> Result<Vec<String>, String> {
    let mut v = named(where_)?.projects()?;
    v.sort();
    v.dedup();
    Ok(v)
}

/// Новый агент Claude в известной папке path мультиплексора where_ с заданием text.
pub fn launch(path: &str, text: &str, where_: &str) -> Result<(), String> {
    let m = named(where_)?;
    if !m.projects()?.iter().any(|x| x == path) {
        return Err(m.no_project());
    }
    m.launch(path, &one_line(text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mux::{Key, Step};
    use serde_json::{json, Value};

    fn no_screen() -> Result<String, String> {
        Err("экран не нужен".into())
    }

    /// AskUserQuestion: один вопрос из n вариантов.
    fn ask(n: usize, multi: bool) -> Value {
        let options: Vec<Value> = (1..=n).map(|i| json!({"label": format!("вариант {i}"), "description": "пояснение"})).collect();
        json!({"name": "AskUserQuestion", "input": {"questions": [{"question": "Что?", "header": "Что", "multiSelect": multi, "options": options}]}})
    }

    /// Экран вопроса с n вариантами — раскладка из живой проверки (Claude Code 2.1.310): метка строкой «N. метка»,
    /// описание — строкой ниже, «Type something.» и «Chat about this» за чертой.
    fn ask_screen(n: usize, multi: bool) -> String {
        let mark = if multi { "[ ] " } else { "" };
        let mut s = String::from(" ☐ Что\n\nЧто?\n\n");
        for i in 1..=n {
            s += &format!("{}{i}. {mark}вариант {i}\n     пояснение\n", if i == 1 { "❯ " } else { "  " });
        }
        s += &format!("  {}. {mark}Type something{}\n────\n  {}. Chat about this\n\nEnter to select", n + 1, if multi { "" } else { "." }, n + 2);
        s
    }

    /// Экран одобрения плана из живой проверки (Claude Code 2.1.310).
    const PLAN_SCREEN: &str = "   Claude has written up a plan and is ready to execute. Would you like to proceed?
   ❯ 1. Yes, and use auto mode
     2. Yes, manually approve edits
     3. Tell Claude what to change
        shift+tab to approve with this feedback
   ctrl+g to edit in VS Code · ~/.claude/plans/x.md";

    fn ok(r: Result<(Vec<Step>, String), String>) -> Vec<Step> {
        r.unwrap().0
    }

    #[test]
    fn option_picks_digit_of_single_question() {
        let screen = || Ok(ask_screen(2, false));
        assert_eq!(steps(Some(&ask(2, false)), "", 2, "", screen).unwrap(), (vec![Step::Key(Key::Digit(2))], "вариант 2".to_string()));
        assert_eq!(ok(steps(Some(&ask(2, false)), "", 2, "вариант 2", screen)), [Step::Key(Key::Digit(2))]);
        assert!(steps(Some(&ask(2, false)), "", 3, "", screen).unwrap_err().contains("нет варианта 3"));
    }

    #[test]
    fn option_is_pressed_only_on_the_menu_it_was_meant_for() {
        // устаревший или двойной клик: на экране уже другой вопрос — цифра выбрала бы чужой вариант
        let screen = || Ok(ask_screen(2, false));
        assert!(steps(Some(&ask(2, false)), "", 1, "Красный", screen).unwrap_err().contains("не жму"), "ждали другой вариант");
        let other = || Ok(ask_screen(2, false).replace("1. вариант 1", "1. Удалить всё"));
        assert!(steps(Some(&ask(2, false)), "", 1, "", other).unwrap_err().contains("не жму"), "транскрипт и экран о разном");
        let permission = || Ok(" Do you want to proceed?\n ❯ 1. Yes\n   2. Yes, and don't ask again\n   3. No".to_string());
        assert!(steps(Some(&ask(2, false)), "", 1, "", permission).is_err(), "меню разрешения вместо вопроса — не «1. Yes»");
        assert!(steps(Some(&ask(2, false)), "", 1, "", no_screen).is_err(), "экран не прочитан — не жмём вслепую");
    }

    #[test]
    fn free_text_answer_goes_to_type_something() {
        assert_eq!(ok(steps(Some(&ask(2, false)), "Зелёный, как трава", 0, "", || Ok(ask_screen(2, false)))),
            [Step::Key(Key::Digit(3)), Step::Pause(500), Step::Type("Зелёный, как трава".into()), Step::Key(Key::Enter)]);
        let shifted = || Ok(ask_screen(3, false));
        assert!(steps(Some(&ask(2, false)), "Зелёный", 0, "", shifted).unwrap_err().contains("не жму"), "пункт 3 — не «Type something»");
    }

    #[test]
    fn several_questions_or_multiselect_decline_then_text() {
        // «Chat about this» первого вопроса (N+2), затем ответ обычной репликой
        let want = [Step::Key(Key::Digit(5)), Step::Pause(1500), Step::Text("яблоко и слива".into())];
        assert_eq!(ok(steps(Some(&ask(3, true)), "яблоко и слива", 0, "", || Ok(ask_screen(3, true)))), want);
        let mut two = ask(3, false);
        two["input"]["questions"] = json!([two["input"]["questions"][0].clone(), two["input"]["questions"][0].clone()]);
        assert_eq!(ok(steps(Some(&two), "яблоко и слива", 0, "", || Ok(ask_screen(3, false)))), want);
        assert!(steps(Some(&ask(3, true)), "", 1, "", || Ok(ask_screen(3, true))).is_err(), "такой ответ — только текстом");
        assert!(steps(Some(&ask(3, true)), "яблоко", 0, "", || Ok(ask_screen(4, true))).is_err(), "пункт 5 — не «Chat about this»");
    }

    #[test]
    fn plan_approve_names_the_option_from_the_screen() {
        let plan = json!({"name": "ExitPlanMode", "input": {"plan": "создать файл"}});
        let screen = || Ok(PLAN_SCREEN.to_string());
        // одобрение включает и auto mode: исход называет пункт с экрана, Ома зачитывает его
        assert_eq!(steps(Some(&plan), "", 1, "", screen).unwrap(), (vec![Step::Key(Key::Digit(1))], "Yes, and use auto mode".to_string()));
        assert_eq!(ok(steps(Some(&plan), "", 1, "Yes, and use auto mode", screen)), [Step::Key(Key::Digit(1))]);
        assert!(steps(Some(&plan), "", 1, "Yes", screen).unwrap_err().contains("не жму"), "ждали другой пункт — метка целиком");
        assert_eq!(ok(steps(Some(&plan), "Назови файл b.txt", 0, "", screen)),
            [Step::Key(Key::Digit(3)), Step::Pause(500), Step::Type("Назови файл b.txt".into()), Step::Key(Key::Enter)]);
        assert!(steps(Some(&plan), "", 1, "", || Ok("❯ ".into())).unwrap_err().contains("меню"), "меню нет на экране — не жмём вслепую");
        assert!(steps(Some(&plan), "правки", 0, "", || Ok("❯ ".into())).unwrap_err().contains("меню"));
        let numbered = || Ok(format!("План:\n1. Удалить старое\n2. Написать новое\n{PLAN_SCREEN}"));
        assert_eq!(steps(Some(&plan), "", 1, "", numbered).unwrap().1, "Yes, and use auto mode", "пункт меню, а не строка плана выше");
    }

    #[test]
    fn no_menu_is_plain_text_and_empty_is_error() {
        let bash = json!({"name": "Bash", "input": {"command": "cargo test"}});
        let prompt = || Ok("✻ Running…\n❯ \n  ⏵⏵ auto mode on".to_string());
        assert_eq!(steps(Some(&bash), "займись тестами", 2, "", prompt).unwrap(), (vec![Step::Text("займись тестами".into())], String::new()));
        assert_eq!(ok(steps(None, "займись тестами", 0, "", no_screen)), [Step::Text("займись тестами".into())]);
        assert!(steps(None, " ", 0, "", no_screen).is_err());
    }

    #[test]
    fn permission_menu_is_refused() {
        // меню разрешения: Enter выбрал бы подсвеченное «Yes» — и «нет, не удаляй» запустило бы rm
        let bash = json!({"name": "Bash", "input": {"command": "rm -rf build"}});
        let menu = || Ok(" Do you want to proceed?\n ❯ 1. Yes\n   2. Yes, and don't ask again\n   3. No".to_string());
        assert!(steps(Some(&bash), "Нет, не удаляй", 0, "", menu).unwrap_err().contains("разрешения"));
        assert!(steps(Some(&bash), "дальше", 0, "", no_screen).is_err(), "экран не прочитан — не пишем вслепую");
    }

    #[test]
    fn menu_lists_options_as_on_screen() {
        // Ома называет в вопросе подтверждения пункт с экрана: одобрение плана включает и auto mode
        assert_eq!(menu(PLAN_SCREEN), ["Yes, and use auto mode", "Yes, manually approve edits", "Tell Claude what to change"]);
        let numbered = format!("План:\n1. Удалить старое\n2. Написать новое\n3. Проверить\n4. Закоммитить\n{PLAN_SCREEN}");
        assert_eq!(menu(&numbered).len(), 3, "строки плана выше меню — не пункты");
        assert_eq!(menu(&ask_screen(2, false)), ["вариант 1", "вариант 2", "Type something.", "Chat about this"]);
        assert!(menu("Готово:\n1. тесты\n2. доки\n❯ ").is_empty(), "нумерованный текст без курсора меню — не меню");
        assert!(menu("").is_empty());
    }

    #[test]
    fn task_text_is_one_line() {
        assert_eq!(one_line("Сделай так:\n1. тесты\n\n2. доки  "), "Сделай так: 1. тесты 2. доки");
    }

    #[test]
    fn tell_unknown_session_is_error() {
        assert!(tell("нет-такой-сессии", 0, "привет", "").unwrap_err().contains("нет среди живых"));
        assert!(menu_of("нет-такой-сессии").unwrap_err().contains("нет среди живых"));
    }
}
