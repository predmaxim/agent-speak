pub mod clean;
pub mod numbers;
pub mod sentences;
pub mod terms;

use terms::Terms;

/// Сырой текст агента → предложения для синтеза + незнакомые латинские слова.
pub fn prepare(raw: &str, terms: &Terms) -> (Vec<String>, Vec<String>) {
    let cleaned = clean::clean(raw);
    let mut out = Vec::new();
    let mut unknown = Vec::new();
    for s in sentences::drop_gutted(sentences::split(&cleaned)) {
        let (s, u) = terms.apply(&numbers::numbers_to_words(&s));
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
    fn prepare_end_to_end() {
        let t = Terms::from_str("hyprland\tхайпрлэнд\n");
        let (s, u) = prepare("## Готово\nПоправил `~/.config/hypr/hyprland.lua`, 3 правки → Hyprland.\n```\nx\n```", &t);
        assert_eq!(s, vec!["Готово.", "Поправил хайпрлэнд точка луа, три правки в хайпрлэнд."]);
        assert_eq!(u, vec!["lua"]);
    }
}
