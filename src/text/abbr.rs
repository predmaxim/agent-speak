//! Сокращения и буквы: «т. е.», «#42», «v5», кириллические аббревиатуры (ТЗ), одиночные буквы («пункт б.»).

use crate::text::terms::{join_letters, Terms};

/// До разбиения на предложения (иначе «т. е.» режется по точке) и до чисел.
pub fn abbreviations(s: &str) -> String {
    let c: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < c.len() {
        let prev = if i == 0 { None } else { Some(c[i - 1]) };
        let fresh = prev.is_none_or(|p| !p.is_alphanumeric());
        if fresh {
            let after_number = c[..i].iter().rev().find(|x| !x.is_whitespace()).is_some_and(char::is_ascii_digit);
            if let Some((full, can_end, len)) = short_phrase(&c[i..], after_number) {
                out.push_str(&full);
                i += len;
                // «т. е.» стоит внутри фразы; точку после «т. д.»/«т. п.» оставляем, если дальше новое предложение
                let rest = c[i..].iter().find(|x| !x.is_whitespace());
                if can_end && rest.is_none_or(|x| x.is_uppercase()) {
                    out.push('.');
                }
                continue;
            }
            // «#42» → «номер 42»; «##3» и «#fff» не трогаем
            if c[i] == '#' && prev != Some('#') && c.get(i + 1).is_some_and(char::is_ascii_digit) {
                out.push_str("номер ");
                i += 1;
                continue;
            }
        }
        out.push(c[i]);
        // «v5» → «v 5», «5GB» → «5 GB»: иначе число приклеивается к букве; «0x1F», «mp3v2» не трогаем
        if c[i].is_ascii_alphabetic() && c.get(i + 1).is_some_and(char::is_ascii_digit) && !prev.is_some_and(|p| p.is_ascii_digit())
            || c[i].is_ascii_digit() && c.get(i + 1).is_some_and(char::is_ascii_alphabetic)
                && !prev.is_some_and(|p| p.is_ascii_alphabetic()) && !c.get(i + 2).is_some_and(char::is_ascii_digit)
        {
            out.push(' ');
        }
        i += 1;
    }
    out
}

/// Сокращения с точкой: образец (пробел в нём необязателен), слова, может ли сокращение закончить предложение.
const SHORT: [(&str, &str, bool); 7] = [
    ("т. е.", "то есть", false), ("т. к.", "так как", false), ("т. д.", "так далее", true), ("т. п.", "тому подобное", true),
    ("см.", "смотри", false), ("напр.", "например", false), ("тыс.", "тысяч", true),
];

/// Сокращение (без учёта регистра) → слова, может ли закончить предложение, длина совпадения в символах.
fn short_phrase(c: &[char], after_number: bool) -> Option<(String, bool, usize)> {
    'next: for (pat, full, can_end) in SHORT {
        if after_number && pat == "см." {
            continue; // «10 см.» — сантиметры
        }
        let mut j = 0;
        for p in pat.chars() {
            if c.get(j).is_some_and(|x| x.to_lowercase().next() == Some(p)) {
                j += 1;
            } else if p != ' ' {
                continue 'next;
            }
        }
        // первая буква кириллицей — 2 байта
        let full = if c[0].is_uppercase() { full[..2].to_uppercase() + &full[2..] } else { full.to_string() };
        return Some((full, can_end, j));
    }
    None
}

/// Внутри предложения, до чисел: аббревиатуры ТЗ/ИИ → названия букв, одиночные «б», «в» перед знаком → название.
pub fn cyrillic(s: &str, terms: &Terms) -> String {
    let c: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < c.len() {
        if !is_cyr(c[i]) {
            out.push(c[i]);
            i += 1;
            continue;
        }
        let start = i;
        while i < c.len() && is_cyr(c[i]) {
            i += 1;
        }
        let word: String = c[start..i].iter().collect();
        let prev = if start == 0 { None } else { Some(c[start - 1]) };
        let next = c.get(i).copied();
        // дефис рядом — часть составного слова («ТЗ-шник», «из-за»)
        let joined = prev == Some('-') || next == Some('-');
        let n = i - start;
        if joined {
            out.push_str(&word);
        } else if (2..=5).contains(&n) && word.chars().all(char::is_uppercase) {
            match terms.get(&word) {
                Some(p) => out.push_str(p),
                None => out.push_str(&join_letters(&word.chars().map(cyr_letter).collect::<Vec<_>>())),
            }
        } else if n == 1 && next.is_none_or(|x| !x.is_alphanumeric() && !x.is_whitespace() && x != '-')
            && prev.is_none_or(char::is_whitespace)
            && !c[..start].iter().rev().find(|x| !x.is_whitespace()).is_some_and(char::is_ascii_digit)
        {
            let name = cyr_letter(c[start]);
            out.push_str(if "аеёиоуыэюя".contains(c[start]) { &word } else { name });
        } else {
            out.push_str(&word);
        }
    }
    out
}

fn is_cyr(c: char) -> bool {
    ('\u{400}'..='\u{4FF}').contains(&c)
}

fn cyr_letter(c: char) -> &'static str {
    match c.to_lowercase().next().unwrap_or(c) {
        'а' => "а", 'б' => "бэ", 'в' => "вэ", 'г' => "гэ", 'д' => "дэ", 'е' => "е", 'ё' => "ё", 'ж' => "жэ",
        'з' => "зэ", 'и' => "и", 'й' => "и краткое", 'к' => "ка", 'л' => "эль", 'м' => "эм", 'н' => "эн",
        'о' => "о", 'п' => "пэ", 'р' => "эр", 'с' => "эс", 'т' => "тэ", 'у' => "у", 'ф' => "эф", 'х' => "ха",
        'ц' => "цэ", 'ч' => "че", 'ш' => "ша", 'щ' => "ща", 'ъ' => "твёрдый знак", 'ы' => "ы",
        'ь' => "мягкий знак", 'э' => "э", 'ю' => "ю", 'я' => "я", _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t() -> Terms {
        Terms::from_str("гост\tгост\n")
    }

    #[test]
    fn short_phrases() {
        assert_eq!(abbreviations("Это, т. е. так, и т.д. и т. п."), "Это, то есть так, и так далее и тому подобное.");
        assert_eq!(abbreviations("и т. д. Потом"), "и так далее. Потом");
        assert_eq!(abbreviations("Т. Е. да"), "То есть да");
        assert_eq!(abbreviations("Не работает, т.к. нет прав, см. выше, напр. в файле."), "Не работает, так как нет прав, смотри выше, например в файле.");
        assert_eq!(abbreviations("См. выше: 5 тыс. строк, 10 см. Потом 3 тыс. Всё"), "Смотри выше: 5 тысяч строк, 10 см. Потом 3 тысяч. Всё");
    }

    #[test]
    fn hash_number() {
        assert_eq!(abbreviations("PR #42 и ##3"), "PR номер 42 и ##3");
        assert_eq!(abbreviations("цвет #fff"), "цвет #fff");
    }

    #[test]
    fn latin_glued_to_digit() {
        assert_eq!(abbreviations("версии v5 и x86, utf8"), "версии v 5 и x 86, utf 8");
        assert_eq!(abbreviations("3D и 2FA, 5GB, 10x"), "3 D и 2 FA, 5 GB, 10 x");
        assert_eq!(abbreviations("0x1F и mp3v2"), "0x1F и mp 3v2");
    }

    #[test]
    fn cyrillic_acronyms() {
        let s = cyrillic("По ТЗ это делает ИИ, а ПР закрыт.", &t());
        assert_eq!(s, "По тэ зэ это делает и и, а пэ эр закрыт.");
        assert_eq!(cyrillic("МВД и ЙЪ", &t()), "эм вэ дэ и и краткое твёрдый знак");
    }

    #[test]
    fn acronym_dictionary_first_and_boundaries() {
        assert_eq!(cyrillic("ГОСТ есть", &t()), "гост есть");
        assert_eq!(cyrillic("Но НЕ ВСЕГДА, ДА", &t()), "Но эн е ВСЕГДА, дэ а");
        assert_eq!(cyrillic("ТЗ-шник", &t()), "ТЗ-шник");
        assert!(cyrillic("ВНИМАНИЕ тут", &t()).starts_with("ВНИМАНИЕ"));
        assert!(cyrillic("А Я тут", &t()).starts_with("А Я"));
    }

    #[test]
    fn single_letters_only_before_punctuation() {
        assert!(!cyrillic("пункт б.", &t()).contains(" б."));
        assert!(cyrillic("пункт б.", &t()).ends_with("бэ."));
        assert!(cyrillic("вариант в, затем", &t()).contains("вариант вэ,"));
        assert_eq!(cyrillic("в доме, к другу, с ним", &t()), "в доме, к другу, с ним");
        assert_eq!(cyrillic("в 2020 г.", &t()), "в 2020 г.");
        assert_eq!(cyrillic("пункт а.", &t()), "пункт а.");
    }
}
