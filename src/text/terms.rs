//! Словарь произношений латинских слов + транслитерация для незнакомых.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};

pub struct Terms {
    map: HashMap<String, String>,
    path: Option<PathBuf>,
}

impl Terms {
    pub fn load(path: &Path) -> Terms {
        let s = std::fs::read_to_string(path).unwrap_or_default();
        Terms { path: Some(path.to_path_buf()), ..Terms::from_str(&s) }
    }

    pub fn from_str(s: &str) -> Terms {
        let map = s
            .lines()
            .filter_map(|l| l.split_once('\t'))
            .filter_map(|(w, p)| Some((w.trim().to_lowercase(), speakable(p.trim())?)))
            .filter(|(w, _)| !w.is_empty())
            .collect();
        Terms { map, path: None }
    }

    pub fn contains(&self, word: &str) -> bool {
        self.map.contains_key(&word.to_lowercase())
    }

    pub fn get(&self, word: &str) -> Option<&str> {
        self.map.get(&word.to_lowercase()).map(String::as_str)
    }

    pub fn add(&mut self, word: &str, pron: &str) {
        let w = word.to_lowercase();
        if let Some(p) = &self.path {
            if let Err(e) = append_line(p, &format!("{w}\t{pron}")) {
                eprintln!("agent-speak: не записал словарь: {e}");
            }
        }
        self.map.insert(w, pron.to_string());
    }

    pub fn apply(&self, sentence: &str) -> (String, Vec<String>) {
        let mut out = String::new();
        let mut unknown: Vec<String> = Vec::new();
        let mut word = String::new();
        let flush = |word: &mut String, out: &mut String, unknown: &mut Vec<String>| {
            if word.is_empty() {
                return;
            }
            let w = word.to_lowercase();
            match self.map.get(&w) {
                Some(p) => out.push_str(&spoken(&w, p)),
                None => {
                    out.push_str(&speak_unknown(word));
                    if !unknown.contains(&w) {
                        unknown.push(w);
                    }
                }
            }
            word.clear();
        };
        let chars: Vec<char> = sentence.chars().collect();
        for (i, &c) in chars.iter().enumerate() {
            if c.is_ascii_alphabetic() {
                word.push(c);
                continue;
            }
            // точка между латинскими словами (file.lua) → «точка»
            let between = c == '.' && !word.is_empty() && chars.get(i + 1).is_some_and(char::is_ascii_alphabetic);
            flush(&mut word, &mut out, &mut unknown);
            if between {
                out.push_str(" точка ");
            } else if (c == '-' || c == '_') && chars.get(i + 1).is_some_and(char::is_ascii_alphabetic) {
                out.push(' ');
            } else {
                out.push(c);
            }
        }
        flush(&mut word, &mut out, &mut unknown);
        (out, unknown)
    }
}

fn append_line(p: &Path, line: &str) -> std::io::Result<()> {
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let needs_nl = std::fs::read(p).is_ok_and(|b| !b.is_empty() && !b.ends_with(b"\n"));
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(p)?;
    if needs_nl {
        f.write_all(b"\n")?;
    }
    writeln!(f, "{line}")
}

/// Аббревиатура (все заглавные ≤5 или без гласных ≤4) — по буквам, иначе транслитерация.
fn speak_unknown(word: &str) -> String {
    let lower = word.to_lowercase();
    let upper = word.len() >= 2 && word.len() <= 5 && word.chars().all(|c| c.is_ascii_uppercase());
    let no_vowels = lower.len() <= 4 && !lower.chars().any(|c| "aeiouy".contains(c));
    if upper || no_vowels {
        join_letters(&lower.chars().map(letter).collect::<Vec<_>>())
    } else {
        transliterate(&lower)
    }
}

/// Названия букв → одна строка. Дефис Silero склеивает («пи-ар» → «пивар»), запятая рвёт на слова —
/// лучше всего пробел (замер: /tmp/tts-spike/sep, латинские аббревиатуры 13/13 против 9/13 с дефисом).
pub fn join_letters(names: &[&str]) -> String {
    names.join(" ")
}

/// Словарное «эн-пи-эм» — по буквам, если частей столько же, сколько букв (или частей в их названиях: «w» — «дабл-ю»):
/// дефис → пробел. Слоги «+ан-тро-пик», «а-пи» и составное «веб-ст+орм» не трогаем — Silero читает их слитно.
fn spoken(word: &str, pron: &str) -> String {
    let parts = pron.split('-').count();
    let names: usize = word.chars().map(|c| letter(c.to_ascii_lowercase()).split('-').count()).sum();
    if parts >= 2 && (parts == word.chars().count() || parts == names) {
        pron.replace('-', " ")
    } else {
        pron.to_string()
    }
}

/// Произношение, которое Silero прочтёт как написано: одно слово из русских букв, «-» и «+» перед гласной.
/// Пояснения модели, пробелы и чужие буквы («кѝ» Silero читает как «к») — None; «+» не перед гласной убирается.
pub fn speakable(pron: &str) -> Option<String> {
    let ru = |c: char| matches!(c, 'а'..='я' | 'А'..='Я' | 'ё' | 'Ё');
    let c: Vec<char> = pron.chars().collect();
    if !c.iter().any(|&x| ru(x)) || !c.iter().all(|&x| ru(x) || x == '+' || x == '-') {
        return None;
    }
    let stressed = |i: usize| c.get(i + 1).is_some_and(|v| "аеёиоуыэюяАЕЁИОУЫЭЮЯ".contains(*v));
    Some(c.iter().enumerate().filter(|&(i, &x)| x != '+' || stressed(i)).map(|(_, &x)| x).collect())
}

fn letter(c: char) -> &'static str {
    match c {
        'a' => "эй", 'b' => "би", 'c' => "си", 'd' => "ди", 'e' => "и", 'f' => "эф", 'g' => "джи",
        'h' => "эйч", 'i' => "ай", 'j' => "джей", 'k' => "кей", 'l' => "эл", 'm' => "эм", 'n' => "эн",
        'o' => "оу", 'p' => "пи", 'q' => "кью", 'r' => "ар", 's' => "эс", 't' => "ти", 'u' => "ю",
        'v' => "ви", 'w' => "дабл-ю", 'x' => "экс", 'y' => "уай", _ => "зед",
    }
}

// ponytail: побуквенные правила с диграфами — грубо («каче»), словарь и фоновая модель исправляют
pub fn transliterate(word: &str) -> String {
    let w: Vec<char> = word.to_lowercase().chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < w.len() {
        let pair: String = w[i..(i + 2).min(w.len())].iter().collect();
        let di = match pair.as_str() {
            "sh" => Some("ш"), "ch" => Some("ч"), "th" => Some("т"), "ph" => Some("ф"), "zh" => Some("ж"),
            "kh" => Some("х"), "ts" => Some("ц"), "oo" => Some("у"), "ee" => Some("и"), "ya" => Some("я"),
            "yo" => Some("йо"), "yu" => Some("ю"), "ck" => Some("к"), "qu" => Some("кв"),
            _ => None,
        };
        if let Some(d) = di {
            out.push_str(d);
            i += 2;
            continue;
        }
        let next = w.get(i + 1).copied();
        out.push_str(match w[i] {
            'a' => "а", 'b' => "б", 'c' if matches!(next, Some('e' | 'i' | 'y')) => "с", 'c' => "к",
            'd' => "д", 'e' => "е", 'f' => "ф", 'g' => "г", 'h' => "х", 'i' => "и", 'j' => "дж",
            'k' => "к", 'l' => "л", 'm' => "м", 'n' => "н", 'o' => "о", 'p' => "п", 'q' => "к",
            'r' => "р", 's' => "с", 't' => "т", 'u' => "у", 'v' => "в", 'w' => "в", 'x' => "кс",
            'y' if i == 0 => "й", 'y' => "и", 'z' => "з", _ => "",
        });
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t() -> Terms {
        Terms::from_str("hyprland\tхайпрлэнд\nnpm\tэн-пи-эм\n")
    }

    #[test]
    fn known_replaced_case_insensitive() {
        let (s, unknown) = t().apply("Правка в Hyprland через NPM.");
        assert_eq!(s, "Правка в хайпрлэнд через эн пи эм.");
        assert!(unknown.is_empty());
    }

    #[test]
    fn unknown_transliterated_and_reported() {
        let (s, unknown) = t().apply("Используем serde и Tokio.");
        assert_eq!(s, "Используем серде и токио.");
        assert_eq!(unknown, vec!["serde", "tokio"]);
    }

    #[test]
    fn abbreviation_spelled() {
        let (s, unknown) = t().apply("Через SSH и gpg.");
        assert_eq!(s, "Через эс эс эйч и джи пи джи.");
        assert_eq!(unknown, vec!["ssh", "gpg"]);
    }

    #[test]
    fn dictionary_hyphen_only_between_letter_names() {
        let t = Terms::from_str("vrr\tви-+ар-+ар\nwebstorm\tвеб-ст+орм\njson\tджей-сон\n");
        assert_eq!(t.apply("vrr webstorm json").0, "ви +ар +ар веб-ст+орм джей-сон");
    }

    #[test]
    fn syllables_not_split_into_words() {
        let t = Terms::from_str("anthropic\t+ан-тро-пик\nhw\tэйч-дабл-+ю\napi\tа-пи\n");
        assert_eq!(t.apply("Anthropic, hw, API").0, "+ан-тро-пик, эйч дабл +ю, а-пи");
    }

    #[test]
    fn only_speakable_entries_loaded() {
        let t = Terms::from_str(
            "key\tкѝ\nacpi\t+а-сі-пі-+ай\nstdin\tстандартный вход (или \"стдин\")\nbuttongroup\tбатон+гр+уп или бат+онгр+уп\n\
             plugins\tп+лагинс\ntfs\tте-эф-++эс\nnpm\tэн-пи-эм\n",
        );
        assert!(!t.contains("key") && !t.contains("acpi") && !t.contains("stdin") && !t.contains("buttongroup"));
        assert_eq!(t.get("plugins"), Some("плагинс"), "«+» не перед гласной — не ударение");
        assert_eq!(t.get("tfs"), Some("те-эф-+эс"));
        assert_eq!(t.get("npm"), Some("эн-пи-эм"));
    }

    #[test]
    fn file_name_parts() {
        let (s, _) = t().apply("Файл hyprland.lua.");
        assert_eq!(s, "Файл хайпрлэнд точка луа.");
    }

    #[test]
    fn translit_digraphs() {
        assert_eq!(transliterate("shell"), "шелл");
        assert_eq!(transliterate("cache"), "каче");
        assert_eq!(transliterate("city"), "сити");
        assert_eq!(transliterate("box"), "бокс");
    }

    #[test]
    fn add_persists() {
        let p = std::env::temp_dir().join(format!("terms-{}.tsv", std::process::id()));
        let _ = std::fs::remove_file(&p);
        let mut a = Terms::load(&p);
        a.add("Tokio", "токио");
        let b = Terms::load(&p);
        assert!(b.contains("tokio"));
        std::fs::remove_file(&p).unwrap();
    }

    #[test]
    fn add_after_file_without_trailing_newline_and_missing_dir() {
        let d = std::env::temp_dir().join(format!("terms-dir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let p = d.join("sub/terms.tsv");
        let mut a = Terms::load(&p);
        a.add("one", "раз"); // каталога нет
        std::fs::write(&p, "one\tраз").unwrap(); // без \n в конце
        a.add("two", "два");
        let b = Terms::load(&p);
        assert!(b.contains("one") && b.contains("two"));
        std::fs::remove_dir_all(&d).unwrap();
    }
}
