pub mod abbr;
pub mod clean;
pub mod numbers;
pub mod sentences;
pub mod terms;

use terms::Terms;

/// Сырой текст агента → предложения для синтеза + незнакомые латинские слова.
/// Haiku ставит ударение комбинирующим знаком U+0301 после гласной; Silero ждёт «+» перед ней.
pub fn plus_stress(p: &str) -> String {
    let mut v: Vec<char> = Vec::new();
    for c in p.chars() {
        if c == '\u{301}' && !v.is_empty() {
            v.insert(v.len() - 1, '+');
        } else {
            v.push(c);
        }
    }
    v.into_iter().collect()
}

pub fn prepare(raw: &str, terms: &Terms) -> (Vec<String>, Vec<String>) {
    let cleaned = abbr::abbreviations(&clean::clean(&plus_stress(raw)));
    let mut out = Vec::new();
    let mut unknown = Vec::new();
    for s in sentences::drop_gutted(sentences::split(&cleaned)) {
        let (s, u) = terms.apply(&numbers::numbers_to_words(&abbr::cyrillic(&s, terms)));
        for w in u {
            if !unknown.contains(&w) {
                unknown.push(w);
            }
        }
        out.push(s);
    }
    (out, unknown)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepare_keeps_model_stress() {
        let (s, _) = prepare("Открой замо\u{301}к и все\u{301}.", &Terms::from_str(""));
        assert_eq!(s, vec!["Открой зам+ок и вс+е."]);
    }

    #[test]
    fn prepare_end_to_end() {
        let t = Terms::from_str("hyprland\tхайпрлэнд\n");
        let (s, u) = prepare("## Готово\nПоправил `~/.config/hypr/hyprland.lua`, 3 правки → Hyprland.\n```\nx\n```", &t);
        assert_eq!(s, vec!["Готово.", "Поправил хайпрлэнд точка луа, три правки в хайпрлэнд."]);
        assert_eq!(u, vec!["lua"]);
    }

    #[test]
    fn prepare_abbreviations_and_letters() {
        let t = Terms::from_str("pr\tп+и-+ар\n");
        let (s, _) = prepare("По ТЗ, т. е. так: PR #42, версия v5, пункт б. Всё и т. д.", &t);
        assert_eq!(s, vec!["По тэ зэ, то есть так: п+и +ар номер сорок два, версия ви пять, пункт бэ.", "Всё и так далее."]);
    }
}
