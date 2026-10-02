//! Markdown, код, пути, ссылки, таблицы → читаемый текст. Вырезанное — пометка GAP.

pub const GAP: char = '\u{1}';

pub fn clean(raw: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut in_code = false;
    for line in raw.lines() {
        let l = line.trim();
        if l.starts_with("```") {
            in_code = !in_code;
            continue;
        }
        if in_code || l.is_empty() {
            continue;
        }
        let l = if l.starts_with('|') {
            match table_row(l) {
                Some(r) => r,
                None => continue,
            }
        } else {
            strip_marker(l).to_string()
        };
        let l = inline(&l);
        let l = l.split_whitespace().collect::<Vec<_>>().join(" ");
        if l.is_empty() {
            continue;
        }
        out.push(end_sentence(l));
    }
    out.join("\n")
}

/// Строка таблицы → «a, b, c»; разделитель |---| → None.
fn table_row(l: &str) -> Option<String> {
    let cells: Vec<&str> = l.trim_matches('|').split('|').map(str::trim).collect();
    if cells.iter().all(|c| c.chars().all(|ch| matches!(ch, '-' | ':' | ' '))) {
        return None;
    }
    Some(cells.into_iter().filter(|c| !c.is_empty()).collect::<Vec<_>>().join(", "))
}

/// Заголовок, пункт списка, цитата → без маркера.
fn strip_marker(l: &str) -> &str {
    let l = l.trim_start_matches('#').trim_start_matches('>').trim_start();
    for m in ["- ", "* ", "+ "] {
        if let Some(r) = l.strip_prefix(m) {
            return r;
        }
    }
    let digits = l.chars().take_while(char::is_ascii_digit).count();
    if digits > 0 {
        if let Some(r) = l[digits..].strip_prefix(". ") {
            return r;
        }
    }
    l
}

fn inline(l: &str) -> String {
    let mut s = String::new();
    let mut rest = l;
    while let Some(i) = rest.find('`') {
        s.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        match after.find('`') {
            Some(j) => {
                s.push_str(&code_span(&after[..j]));
                rest = &after[j + 1..];
            }
            None => {
                rest = after;
            }
        }
    }
    s.push_str(rest);
    let s = links(&s);
    let s = s.replace("**", "").replace("__", "").replace('*', "");
    let s = s.replace('→', " в ").replace('←', " из ").replace('·', ",");
    s.chars().filter(|c| !is_emoji(*c)).collect()
}

/// `код`: URL → домен; путь → имя файла; команда/выражение → GAP; одно слово → как есть.
fn code_span(c: &str) -> String {
    let c = c.trim();
    if let Some(d) = url_domain(c) {
        return d;
    }
    if c.contains('/') && !c.contains(' ') {
        return path_name(c);
    }
    if c.contains(' ') || c.chars().any(|ch| "=|$;(){}<>&\"'".contains(ch)) {
        return GAP.to_string();
    }
    c.split(':').next().unwrap_or(c).to_string()
}

fn url_domain(u: &str) -> Option<String> {
    let u = u.strip_prefix("https://").or_else(|| u.strip_prefix("http://"))?;
    Some(u.split('/').next().unwrap_or(u).to_string())
}

/// Последний непустой сегмент пути без суффикса `:строка`.
fn path_name(p: &str) -> String {
    let name = p.trim_end_matches('/').rsplit('/').next().unwrap_or(p);
    name.split(':').next().unwrap_or(name).to_string()
}

/// [текст](url) → текст; голый URL → домен.
fn links(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find("](") {
        let open = rest[..i].rfind('[');
        let close = rest[i + 2..].find(')');
        match (open, close) {
            (Some(o), Some(c)) => {
                out.push_str(&rest[..o]);
                out.push_str(&rest[o + 1..i]);
                rest = &rest[i + 2 + c + 1..];
            }
            _ => break,
        }
    }
    out.push_str(rest);
    out.split(' ').map(word).collect::<Vec<_>>().join(" ")
}

/// Слово вне код-спанов: URL → домен, абсолютный путь → имя файла,
/// хеш/идентификатор из букв и цифр (≥6) → GAP; окружающая пунктуация сохраняется.
fn word(w: &str) -> String {
    let lead_end = w.len() - w.trim_start_matches(['(', '[', '{', '"', '\'', '«']).len();
    let core_end = w.trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']', '}', '"', '\'', '»']).len();
    if core_end <= lead_end {
        return w.to_string();
    }
    let (lead, core, trail) = (&w[..lead_end], &w[lead_end..core_end], &w[core_end..]);
    let new = if let Some(d) = url_domain(core) {
        d
    } else if (core.starts_with('/') || core.starts_with("~/")) && core[1..].contains('/') {
        path_name(core)
    } else if is_ident(core) {
        GAP.to_string()
    } else {
        return w.to_string();
    };
    format!("{lead}{new}{trail}")
}

fn is_ident(t: &str) -> bool {
    t.len() >= 6
        && t.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && t.chars().any(|c| c.is_ascii_alphabetic())
        && t.chars().any(|c| c.is_ascii_digit())
}

fn is_emoji(c: char) -> bool {
    matches!(c as u32, 0x1F000..=0x1FAFF | 0x2600..=0x27BF | 0xFE0F | 0x200D)
}

fn end_sentence(mut l: String) -> String {
    if !l.ends_with(['.', '!', '?', '…', ':']) {
        l.push('.');
    }
    l
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_block_removed() {
        let t = clean("Смотри:\n```rust\nfn main() {}\n```\nГотово");
        assert!(!t.contains("fn main"));
        assert!(t.contains("Готово."));
    }

    #[test]
    fn path_becomes_file_name() {
        assert_eq!(clean("Правка в `~/.config/hypr/hyprland.lua:42`"), "Правка в hyprland.lua.");
    }

    #[test]
    fn shell_code_is_gap() {
        assert_eq!(clean("Запусти `cargo test --all`"), format!("Запусти {GAP}."));
    }

    #[test]
    fn short_code_kept_as_term() {
        assert_eq!(clean("Используем `serde`"), "Используем serde.");
    }

    #[test]
    fn link_and_url() {
        assert_eq!(clean("Смотри [доку](https://docs.rs/x)"), "Смотри доку.");
        assert_eq!(clean("Тут https://github.com/a/b есть"), "Тут github.com есть.");
    }

    #[test]
    fn table_rows_read() {
        let t = clean("| Голос | Где |\n|---|---|\n| Silero | офлайн |");
        assert_eq!(t, "Голос, Где.\nSilero, офлайн.");
    }

    #[test]
    fn list_heading_bold() {
        assert_eq!(clean("## Итог\n- **первое** дело\n1. второе"), "Итог.\nпервое дело.\nвторое.");
    }

    #[test]
    fn emoji_and_arrow() {
        assert_eq!(clean("Hyprland → хайпрлэнд 🔊"), "Hyprland в хайпрлэнд.");
    }

    #[test]
    fn only_code_gives_nothing() {
        let t = clean("```\nls -la\n```");
        assert!(super::super::sentences::split(&t).is_empty());
    }

    #[test]
    fn urls_in_backticks_and_punctuation() {
        assert_eq!(clean("Открой `https://github.com/a/b`"), "Открой github.com.");
        assert_eq!(clean("Тут (https://x.com/a) есть"), "Тут (x.com) есть.");
        assert_eq!(clean("Смотри https://x.com/a, там"), "Смотри x.com, там.");
        assert_eq!(clean("Смотри https://x.com/a."), "Смотри x.com.");
    }

    #[test]
    fn bare_and_trailing_slash_paths() {
        assert_eq!(clean("Правка в /etc/hypr/hyprland.lua:42 готова"), "Правка в hyprland.lua готова.");
        assert_eq!(clean("Правка в ~/.config/a.toml, ок"), "Правка в a.toml, ок.");
        assert_eq!(clean("Папка `src/text/`"), "Папка text.");
        assert_eq!(clean("и/или"), "и/или.");
    }

    #[test]
    fn hashes_become_gap() {
        assert_eq!(clean("Коммит 9db1e2b, x86_64 и mp3 v2"), format!("Коммит {GAP}, {GAP} и mp3 v2."));
        let s = super::super::sentences::drop_gutted(super::super::sentences::split(&clean("Коммит 9db1e2b готов, всё ок.")));
        assert_eq!(s, vec!["Коммит готов, всё ок."]);
        let s = super::super::sentences::drop_gutted(super::super::sentences::split(&clean("Смотри a3f9c2.")));
        assert!(s.is_empty());
    }
}
