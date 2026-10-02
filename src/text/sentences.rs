//! Разбиение на предложения и выброс «обрубков» после очистки.

use crate::text::clean::GAP;

pub fn split(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        let chars: Vec<char> = line.chars().collect();
        let mut start = 0;
        for i in 0..chars.len() {
            let end = matches!(chars[i], '.' | '!' | '?' | '…');
            let next_space = chars.get(i + 1).is_none_or(|c| c.is_whitespace());
            if end && next_space {
                push(&mut out, &chars[start..=i]);
                start = i + 1;
            }
        }
        push(&mut out, &chars[start..]);
    }
    out
}

fn push(out: &mut Vec<String>, s: &[char]) {
    let s: String = s.iter().collect::<String>().trim().to_string();
    if s.chars().any(|c| c.is_alphanumeric()) {
        out.push(s);
    }
}

pub fn drop_gutted(sentences: Vec<String>) -> Vec<String> {
    sentences
        .into_iter()
        .filter_map(|s| {
            let gaps = s.matches(GAP).count();
            let words = s.split_whitespace().filter(|w| w.chars().any(char::is_alphanumeric) && !w.contains(GAP)).count();
            if gaps >= 2 || (gaps == 1 && words < 3) {
                return None;
            }
            let s = s.replace(GAP, "");
            Some(s.split_whitespace().collect::<Vec<_>>().join(" ").replace(" .", "."))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::clean::GAP;

    #[test]
    fn splits_on_punctuation_and_lines() {
        assert_eq!(split("Раз. Два! Три?\nЧетыре"), vec!["Раз.", "Два!", "Три?", "Четыре"]);
    }

    #[test]
    fn version_not_split() {
        assert_eq!(split("Версия 2.1.284 стоит."), vec!["Версия 2.1.284 стоит."]);
    }

    #[test]
    fn gutted_dropped_short_whole_kept() {
        let s = vec![
            "Готово.".to_string(),
            format!("Запусти {GAP} и {GAP}."),
            format!("Смотри {GAP}."),
            format!("Поправил {GAP} в конфиге Hyprland."),
        ];
        assert_eq!(drop_gutted(s), vec!["Готово.", "Поправил в конфиге Hyprland."]);
    }
}
