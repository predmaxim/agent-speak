//! Ответы и задания агентам от Омы: как передать (текстом или клавишами меню), куда, запуск нового агента.
//! Зовёт Ома через CLI `agent-speak tell | launch | projects`: исход нужен её мозгу сразу.

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

/// Свой ответ в меню: пункт с полем ввода, текст без Enter, Enter.
fn typed(item: usize, text: &str) -> Result<Vec<Step>, String> {
    Ok(vec![digit(item)?, Step::Pause(MENU_PAUSE), Step::Type(text.into()), Step::Key(Key::Enter)])
}

/// Как передать ответ агенту: вызов, который ждёт пользователя (меню), — клавишами; иначе — текстом в поле ввода.
/// option — пункт меню (0 — свой ответ текстом); screen читается у плана (набор пунктов зависит от режима)
/// и у прочих вызовов в полёте (не ждёт ли разрешения), у вопроса с вариантами — нет.
pub fn steps(call: Option<&Value>, text: &str, option: usize, screen: impl FnOnce() -> Result<String, String>) -> Result<Vec<Step>, String> {
    let text = text.trim();
    match call.and_then(|c| c["name"].as_str()) {
        Some("AskUserQuestion") => {
            let qs = call.unwrap()["input"]["questions"].as_array().cloned().unwrap_or_default();
            let n = qs.first().and_then(|q| q["options"].as_array()).map_or(0, Vec::len);
            if qs.len() == 1 && qs[0]["multiSelect"] != true {
                match option {
                    0 if text.is_empty() => Err("пустой ответ".into()),
                    0 => typed(n + 1, text),
                    o if o <= n => Ok(vec![digit(o)?]),
                    o => Err(format!("нет варианта {o}")),
                }
            } else if text.is_empty() {
                Err("на такой вопрос — ответ текстом".into())
            } else {
                // несколько вопросов или выбор нескольких: меню отклоняем («Chat about this»), ответ — репликой
                Ok(vec![digit(n + 2)?, Step::Pause(DECLINE_PAUSE), Step::Text(text.into())])
            }
        }
        Some("ExitPlanMode") => {
            let screen = screen()?;
            match option {
                1 if screen.contains("1. Yes") => Ok(vec![digit(1)?]),
                0 if !text.is_empty() => item_of(&screen, PLAN_FEEDBACK).map_or_else(|| Err(NO_PLAN_MENU.into()), |i| typed(i, text)),
                0 => Err("пустой ответ".into()),
                1 => Err(NO_PLAN_MENU.into()),
                o => Err(format!("нет варианта {o}")),
            }
        }
        _ if text.is_empty() => Err("пустой текст".into()),
        // вызов в полёте может ждать разрешения: в меню Enter выбрал бы подсвеченный пункт («Yes»)
        Some(_) if menu_on(&screen()?) => Err("агент ждёт разрешения в терминале — ответь там".into()),
        _ => Ok(vec![Step::Text(text.into())]),
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

/// Передать агенту session текст или пункт меню.
// shortcut: CLI и поток действий сторожа могут писать в один терминал одновременно; начнёт мешать — вести через демон
pub fn tell(session: &str, option: usize, text: &str) -> Result<(), String> {
    let live = crate::agents::scan_all().into_iter().find(|l| l.id == session).ok_or("агента нет среди живых")?;
    let pane = mux::locate(live.pid).ok_or("агент не в Orca и не в herdr — передать не могу")?;
    // у Codex меню не разбираем — всегда текст
    let call = if live.agent == "claude" { claude::in_flight(&tail_of(&live.transcript.to_string_lossy())) } else { None };
    let steps = steps(call.as_ref(), &one_line(text), option, || mux::screen(&pane))?;
    mux::perform(&pane, &steps)
}

/// Папки, где можно запустить агента, — по всем адаптерам; адаптер с ошибкой не даёт ничего.
pub fn projects() -> Vec<String> {
    let mut v: Vec<String> = mux::adapters().iter().flat_map(|m| m.projects().unwrap_or_default()).collect();
    v.sort();
    v.dedup();
    v
}

/// Новый агент Claude в известной папке path с заданием text.
pub fn launch(path: &str, text: &str) -> Result<(), String> {
    let m = mux::adapters().into_iter().find(|m| m.projects().is_ok_and(|p| p.iter().any(|x| x == path)));
    m.ok_or("этого проекта нет в Orca — добавь его, и я запущу")?.launch(path, &one_line(text))
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
        let options: Vec<Value> = (1..=n).map(|i| json!({"label": format!("вариант {i}"), "description": ""})).collect();
        json!({"name": "AskUserQuestion", "input": {"questions": [{"question": "Что?", "header": "Что", "multiSelect": multi, "options": options}]}})
    }

    /// Экран одобрения плана из живой проверки (Claude Code 2.1.310).
    const PLAN_SCREEN: &str = "   Claude has written up a plan and is ready to execute. Would you like to proceed?
   ❯ 1. Yes, and use auto mode
     2. Yes, manually approve edits
     3. Tell Claude what to change
        shift+tab to approve with this feedback
   ctrl+g to edit in VS Code · ~/.claude/plans/x.md";

    #[test]
    fn option_picks_digit_of_single_question() {
        assert_eq!(steps(Some(&ask(2, false)), "", 2, no_screen).unwrap(), [Step::Key(Key::Digit(2))]);
        assert!(steps(Some(&ask(2, false)), "", 3, no_screen).unwrap_err().contains("нет варианта 3"));
    }

    #[test]
    fn free_text_answer_goes_to_type_something() {
        assert_eq!(steps(Some(&ask(2, false)), "Зелёный, как трава", 0, no_screen).unwrap(),
            [Step::Key(Key::Digit(3)), Step::Pause(500), Step::Type("Зелёный, как трава".into()), Step::Key(Key::Enter)]);
    }

    #[test]
    fn several_questions_or_multiselect_decline_then_text() {
        // «Chat about this» первого вопроса (N+2), затем ответ обычной репликой
        let want = [Step::Key(Key::Digit(5)), Step::Pause(1500), Step::Text("яблоко и слива".into())];
        assert_eq!(steps(Some(&ask(3, true)), "яблоко и слива", 0, no_screen).unwrap(), want);
        let mut two = ask(3, false);
        two["input"]["questions"] = json!([two["input"]["questions"][0].clone(), two["input"]["questions"][0].clone()]);
        assert_eq!(steps(Some(&two), "яблоко и слива", 0, no_screen).unwrap(), want);
        assert!(steps(Some(&ask(3, true)), "", 1, no_screen).is_err(), "такой ответ — только текстом");
    }

    #[test]
    fn plan_approve_and_feedback_by_screen() {
        let plan = json!({"name": "ExitPlanMode", "input": {"plan": "создать файл"}});
        assert_eq!(steps(Some(&plan), "", 1, || Ok(PLAN_SCREEN.into())).unwrap(), [Step::Key(Key::Digit(1))]);
        assert_eq!(steps(Some(&plan), "Назови файл b.txt", 0, || Ok(PLAN_SCREEN.into())).unwrap(),
            [Step::Key(Key::Digit(3)), Step::Pause(500), Step::Type("Назови файл b.txt".into()), Step::Key(Key::Enter)]);
        assert!(steps(Some(&plan), "", 1, || Ok("❯ ".into())).unwrap_err().contains("меню"), "меню нет на экране — не жмём вслепую");
        assert!(steps(Some(&plan), "правки", 0, || Ok("❯ ".into())).unwrap_err().contains("меню"));
    }

    #[test]
    fn no_menu_is_plain_text_and_empty_is_error() {
        let bash = json!({"name": "Bash", "input": {"command": "cargo test"}});
        let prompt = || Ok("✻ Running…\n❯ \n  ⏵⏵ auto mode on".to_string());
        assert_eq!(steps(Some(&bash), "займись тестами", 2, prompt).unwrap(), [Step::Text("займись тестами".into())]);
        assert_eq!(steps(None, "займись тестами", 0, no_screen).unwrap(), [Step::Text("займись тестами".into())]);
        assert!(steps(None, " ", 0, no_screen).is_err());
    }

    #[test]
    fn permission_menu_is_refused() {
        // меню разрешения: Enter выбрал бы подсвеченное «Yes» — и «нет, не удаляй» запустило бы rm
        let bash = json!({"name": "Bash", "input": {"command": "rm -rf build"}});
        let menu = || Ok(" Do you want to proceed?\n ❯ 1. Yes\n   2. Yes, and don't ask again\n   3. No".to_string());
        assert!(steps(Some(&bash), "Нет, не удаляй", 0, menu).unwrap_err().contains("разрешения"));
        assert!(steps(Some(&bash), "дальше", 0, no_screen).is_err(), "экран не прочитан — не пишем вслепую");
    }

    #[test]
    fn task_text_is_one_line() {
        assert_eq!(one_line("Сделай так:\n1. тесты\n\n2. доки  "), "Сделай так: 1. тесты 2. доки");
    }

    #[test]
    fn tell_unknown_session_is_error() {
        assert!(tell("нет-такой-сессии", 0, "привет").unwrap_err().contains("нет среди живых"));
    }
}
