pub mod abbr;
pub mod clean;
pub mod numbers;
pub mod sentences;
pub mod terms;

use terms::Terms;

/// Русское склонение по числу: один / два-четыре / пять (1 агент, 2 агента, 5 агентов, 11 агентов, 21 агент).
pub fn plural(n: usize, forms: [&'static str; 3]) -> &'static str {
    match (n % 10, n % 100) {
        (1, r) if r != 11 => forms[0],
        (2..=4, r) if !(12..=14).contains(&r) => forms[1],
        _ => forms[2],
    }
}

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
    // ударение — после clean: там буквальный «+» становится словом «плюс»
    let cleaned = abbr::abbreviations(&plus_stress(&clean::clean(raw)));
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
    fn plural_forms() {
        let f = ["агент", "агента", "агентов"];
        let got: Vec<&str> = [0, 1, 2, 4, 5, 11, 12, 14, 21, 22, 25, 101, 111, 112, 1001].iter().map(|&n| plural(n, f)).collect();
        assert_eq!(got, ["агентов", "агент", "агента", "агента", "агентов", "агентов", "агентов", "агентов", "агент", "агента", "агентов", "агент", "агентов", "агентов", "агент"]);
    }

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
    fn prepare_drops_every_asterisk() {
        // в Silero v5_6 «*» включает фокус, а непарная «**» — ValueError: фраза молча пропала бы
        let (s, _) = prepare("**Готово** *важно* a * b, ** хвост *", &Terms::from_str(""));
        assert!(!s.is_empty() && s.iter().all(|x| !x.contains('*')), "{s:?}");
    }

    #[test]
    fn prepare_plus_is_not_stress() {
        let (s, _) = prepare("Стало +5 строк на C++, ударе\u{301}ние на месте.", &Terms::from_str(""));
        assert_eq!(s, vec!["Стало плюс пять строк на си плюс плюс, удар+ение на месте."]);
    }

    #[test]
    fn prepare_short_forms_keep_sentence() {
        let (s, _) = prepare("Не работает, т.к. нет прав, см. выше, напр. в файле.", &Terms::from_str(""));
        assert_eq!(s, vec!["Не работает, так как нет прав, смотри выше, например в файле."]);
    }

    #[test]
    fn prepare_numbers() {
        let t = Terms::from_str("gb\tгигаб+айт\n");
        let (s, _) = prepare("Порог 0.05, всего 1\u{202f}000 строк к 12:30, файл 5GB, код E0502.", &t);
        assert_eq!(s, vec!["Порог ноль точка ноль пять, всего одна тысяча строк к двенадцать тридцать, файл пять гигаб+айт, код е ноль пять ноль два."]);
    }

    #[test]
    fn prepare_abbreviations_and_letters() {
        let t = Terms::from_str("pr\tп+и-+ар\n");
        let (s, _) = prepare("По ТЗ, т. е. так: PR #42, версия v5, пункт б. Всё и т. д.", &t);
        assert_eq!(s, vec!["По тэ зэ, то есть так: п+и +ар номер сорок два, версия ви пять, пункт бэ.", "Всё и так далее."]);
    }
}
