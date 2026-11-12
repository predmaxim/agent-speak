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
        // подчёркивание заголовка «=====» (setext) — разметка, не «равно равно»
        if in_code || l.is_empty() || l.chars().all(|c| c == '=') {
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
        // замена знака словом («C++,» → «C плюс плюс ,») оставляет пробел перед пунктуацией
        let l = l.split_whitespace().collect::<Vec<_>>().join(" ").replace(" ,", ",");
        if l.is_empty() {
            continue;
        }
        out.push(end_sentence(l));
    }
    out.join("\n")
}

/// Текст для показа (лента агента, пузыри): без разметки markdown, но без замен для озвучки.
pub fn plain(raw: &str) -> String {
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
        let l = strip_marker(l).replace("**", "").replace("__", "").replace("~~", "").replace('`', "");
        out.push(l.split_whitespace().collect::<Vec<_>>().join(" "));
    }
    out.join(" ")
}

/// Префиксы ключей — из правил gitleaks (MIT, https://github.com/gitleaks/gitleaks/blob/master/config/gitleaks.toml);
/// длинные раньше своих начал (sk-ant- раньше sk-).
const KEY_PREFIXES: [&str; 23] = [
    "github_pat_", "ghp_", "gho_", "ghu_", "ghs_", "ghr_", "glpat-", "sk-ant-", "sk-proj-", "sk-", "xoxb-", "xoxp-", "xoxa-",
    "xoxs-", "AKIA", "ASIA", "AIza", "ya29.", "hf_", "npm_", "pypi-", "dop_v1_", "shpat_",
];
/// Имя, после которого `=` или `:` идёт секрет: password=…, DB_PASSWORD: …, "api_key": "…".
const SECRET_NAMES: [&str; 6] = ["password", "passwd", "secret", "token", "api_key", "apikey"];
const HIDDEN: &str = "•••";

/// Секреты в показываемом тексте (пузыри): ключ с известным префиксом — «ghp_…», JWT — «eyJ…», значение после
/// password=, token: и т. п. — «•••», блок закрытого ключа — до конца текста. Хэши, UUID и пути не трогаются:
/// случайные строки по энтропии не ищем — это ложно ловит хэши и base64.
pub fn mask_secrets(text: &str) -> String {
    let text = cut_private_key(text);
    let mut out: Vec<String> = Vec::new();
    let mut want = Want::Nothing;
    for w in text.split(' ') {
        match want {
            Want::Value => {
                out.push(HIDDEN.into());
                want = Want::Nothing;
                continue;
            }
            Want::Separator if w == "=" || w == ":" => {
                out.push(w.into());
                want = Want::Value;
                continue;
            }
            _ => {}
        }
        let (w, next) = assignment(w);
        want = next;
        out.push(mask_keys(&w));
    }
    out.join(" ")
}

/// Что ждёт следующее слово после имени секрета.
#[derive(Clone, Copy)]
enum Want {
    Nothing,
    Separator, // «password» — дальше «=» или «:»
    Value,     // «password:» — дальше значение
}

/// Имя секрета в слове: «token=abc» → «token=•••»; «token:» — значение в следующем слове, «token» — сначала знак.
fn assignment(w: &str) -> (String, Want) {
    let named = |s: &str| {
        let s = s.trim_end_matches(['"', '\'']);
        let start = s.char_indices().rev().take_while(|&(_, c)| c.is_ascii_alphanumeric() || c == '_').last().map_or(s.len(), |(i, _)| i);
        let name = s[start..].to_ascii_lowercase();
        SECRET_NAMES.iter().any(|n| name.ends_with(n))
    };
    for (p, _) in w.char_indices().filter(|&(_, c)| c == '=' || c == ':') {
        if named(&w[..p]) {
            let head = &w[..=p];
            return if w[p + 1..].trim_matches(['"', '\'']).is_empty() { (head.into(), Want::Value) } else { (format!("{head}{HIDDEN}"), Want::Nothing) };
        }
    }
    let bare = w.trim_matches(['"', '\'']);
    let next = if !bare.is_empty() && bare.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && named(bare) { Want::Separator } else { Want::Nothing };
    (w.into(), next)
}

/// Ключи с известным префиксом и JWT — в начале слова или после кавычки, скобки, «=», «:», «,» (не внутри пути).
fn mask_keys(w: &str) -> String {
    let mut out = String::new();
    let mut i = 0;
    while let Some(c) = w[i..].chars().next() {
        if i == 0 || w[..i].ends_with(['"', '\'', '(', '[', '{', '<', '=', ':', ',']) {
            let rest = &w[i..];
            let len = rest.find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))).unwrap_or(rest.len());
            let token = rest[..len].trim_end_matches('.');
            if let Some(shown) = key_prefix(token) {
                out.push_str(shown);
                out.push('…');
                i += token.len();
                continue;
            }
        }
        out.push(c);
        i += c.len_utf8();
    }
    out
}

/// Префикс, который остаётся от ключа; не ключ — None. Тело ключа — от 16 знаков и с цифрой: npm_config_cache, sk-learn — не ключи;
/// у ключа AWS (AKIA, ASIA) — ровно 16 знаков [A-Z2-7], как в правиле gitleaks: цифры в нём может не быть.
fn key_prefix(token: &str) -> Option<&'static str> {
    if token.starts_with("eyJ") && token.split('.').count() >= 3 && token.split('.').all(|s| !s.is_empty()) {
        return Some("eyJ");
    }
    KEY_PREFIXES.into_iter().find(|p| token.starts_with(p)).filter(|p| {
        let body = &token[p.len()..];
        if matches!(*p, "AKIA" | "ASIA") {
            return body.len() == 16 && body.bytes().all(|b| b.is_ascii_uppercase() || (b'2'..=b'7').contains(&b));
        }
        body.len() >= 16 && body.bytes().any(|b| b.is_ascii_digit())
    })
}

/// Блок «-----BEGIN … PRIVATE KEY-----» (и PGP «… PRIVATE KEY BLOCK-----») и всё после него → заголовок и «…».
fn cut_private_key(text: &str) -> String {
    const BEGIN: &str = "-----BEGIN ";
    let mut from = 0;
    while let Some(i) = text[from..].find(BEGIN) {
        let head = from + i + BEGIN.len();
        if let Some(j) = text[head..].find("-----").filter(|&j| text[head..head + j].trim_end_matches(" BLOCK").ends_with("PRIVATE KEY")) {
            return format!("{}…", &text[..head + j + 5]);
        }
        from = head;
    }
    text.into()
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
    let s = s.replace("**", "").replace("__", "").replace("~~", "").replace('*', "");
    let s = s.replace('→', " в ").replace('←', " из ").replace('·', ",");
    let s = SAY.iter().fold(s, |s, (sign, said)| s.replace(sign, said));
    s.chars().filter(|c| !is_emoji(*c)).collect()
}

/// Знаков нет в алфавите Silero: модель выбросила бы их молча и склеила слова («да/нет» → «данет»).
/// «+» у неё — ударение, а в тексте агента это плюс («C++», «+5»); ударение из U+0301 ставится позже.
/// Составные операторы — раньше одиночного «=»: иначе «!=» рвал бы фразу на «!» и терял «не».
const SAY: [(&str, &str); 16] = [
    ("!=", " не равно "), (">=", " не меньше "), ("<=", " не больше "), ("=>", " в "), ("==", " равно "), ("=", " равно "), ("≠", " не равно "), ("≥", " не меньше "), ("≤", " не больше "),
    ("≈", " около "), ("~", " около "), ("×", " на "), ("№", " номер "), ("−", " минус "), ("+", " плюс "), ("/", " "),
];

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
    strip_line(c).to_string()
}

fn url_domain(u: &str) -> Option<String> {
    let u = u.strip_prefix("https://").or_else(|| u.strip_prefix("http://"))?;
    Some(u.split('/').next().unwrap_or(u).to_string())
}

/// Последний непустой сегмент пути без суффикса `:строка`.
fn path_name(p: &str) -> String {
    strip_line(p.trim_end_matches('/').rsplit('/').next().unwrap_or(p)).to_string()
}

/// «x.rs:120», «x.rs:120:5» → «x.rs»; «Player::write» не трогаем.
fn strip_line(s: &str) -> &str {
    match s.rsplit_once(':') {
        Some((head, n)) if !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) => strip_line(head),
        _ => s,
    }
}

/// Имя файла с расширением: «x.rs», «Model.js»; «.claude», «1.0» — нет.
fn has_ext(name: &str) -> bool {
    name.rsplit_once('.').is_some_and(|(a, e)| !a.is_empty() && e.bytes().all(|b| b.is_ascii_alphanumeric()) && e.bytes().any(|b| b.is_ascii_alphabetic()))
}

/// Относительный путь «src/x.rs:120»: у последней части «:строка» или расширение; «и/или», «МБ/с» — не путь.
/// При одной «/» слева должна быть папка: не файл и без заглавных — «TypeScript/Node.js», «README.md/CHANGELOG.md» — перечисление.
fn is_rel_path(t: &str) -> bool {
    let parts: Vec<&str> = strip_line(t).trim_end_matches('/').split('/').collect();
    let dir = parts.len() > 2 || !has_ext(parts[0]) && !parts[0].chars().any(char::is_uppercase);
    t.contains('/') && (strip_line(t).len() < t.len() || has_ext(&path_name(t)) && dir)
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

/// Слово вне код-спанов: URL → домен, путь (абсолютный или относительный с именем файла) → имя файла,
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
    } else if (core.starts_with('/') || core.starts_with("~/")) && core[1..].contains('/') || is_rel_path(core) {
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
    fn plain_strips_markdown_keeps_text() {
        assert_eq!(plain("- **Орб** справа, `orb_corner`\n## Итог\n```\nls\n```\n1. Готово"), "Орб справа, orb_corner Итог Готово");
    }

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
        assert_eq!(clean("и/или"), "и или.");
    }

    #[test]
    fn relative_paths_become_file_name() {
        assert_eq!(clean("Правка в src/speaker.rs:120 и ветке feature/oma-x."), "Правка в speaker.rs и ветке feature oma-x.");
        assert_eq!(clean("Смотри (docs/rules.md), и/или Да/нет, МБ/с, 0.5/1.0"), "Смотри (rules.md), и или Да нет, МБ с, 0.5 1.0.");
        assert_eq!(clean("Правка в ./x.rs, plugin/Model.js и .claude/settings.json"), "Правка в x.rs, Model.js и settings.json.");
        // слева от «/» не папка — слово с заглавной или файл: это перечисление, а не путь
        assert_eq!(clean("На TypeScript/Node.js, см. README.md/CHANGELOG.md"), "На TypeScript Node.js, см. README.md CHANGELOG.md.");
    }

    #[test]
    fn code_span_drops_only_line_suffix() {
        assert_eq!(clean("Метод `Player::write` в `speaker.rs:12:5`"), "Метод Player::write в speaker.rs.");
    }

    #[test]
    fn symbols_outside_silero_alphabet_spoken() {
        assert_eq!(
            clean("Да/нет, МБ/с, два×три, №5, ≥ 10, ≤2, −3, ~20, ≈7, a = b, x == y, a≠b"),
            "Да нет, МБ с, два на три, номер 5, не меньше 10, не больше 2, минус 3, около 20, около 7, a равно b, x равно y, a не равно b."
        );
    }

    #[test]
    fn compound_operators_keep_their_meaning() {
        // по одиночному «=» «!=» рвал бы фразу на «!» и терял «не», «>=» давал бы «> равно»
        assert_eq!(clean("x != y, a >= 1, b <= 2, a => b, x == y"), "x не равно y, a не меньше 1, b не больше 2, a в b, x равно y.");
    }

    #[test]
    fn strikethrough_and_setext_underline_are_markup() {
        assert_eq!(clean("~~старое~~ новое, ~20"), "старое новое, около 20.");
        assert_eq!(clean("Заголовок\n=====\nТекст"), "Заголовок.\nТекст.");
        assert_eq!(plain("~~старое~~ новое"), "старое новое");
    }

    #[test]
    fn literal_plus_is_said() {
        assert_eq!(clean("C++ и +5, a + b"), "C плюс плюс и плюс 5, a плюс b.");
    }

    #[test]
    fn hashes_become_gap() {
        assert_eq!(clean("Коммит 9db1e2b, x86_64 и mp3 v2"), format!("Коммит {GAP}, {GAP} и mp3 v2."));
        let s = super::super::sentences::drop_gutted(super::super::sentences::split(&clean("Коммит 9db1e2b готов, всё ок.")));
        assert_eq!(s, vec!["Коммит готов, всё ок."]);
        let s = super::super::sentences::drop_gutted(super::super::sentences::split(&clean("Смотри a3f9c2.")));
        assert!(s.is_empty());
    }

    #[test]
    fn mask_secrets_table() {
        for (raw, want) in [
            ("нужен токен ghp_abcdefghijklmnopqrstuvwxyz0123456789, дать доступ?", "нужен токен ghp_…, дать доступ?"),
            ("ключ sk-ant-api03-AbCdEf1234567890abcdef и sk-proj-Zx9yW8vU7tS6rQ5pO4nM.", "ключ sk-ant-… и sk-proj-…."),
            ("(AKIAIOSFODNN7EXAMPLE) AIzaSyA1234567890abcdefghijklmnopqrstu", "(AKIA…) AIza…"),
            ("hf_AbCdEfGhIj1234567890KlMnOpQr glpat-x1y2z3a4b5c6d7e8f9g0 xoxb-1234567890-abcdefghij", "hf_… glpat-… xoxb-…"),
            ("jwt eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.sig_abc-123 тут", "jwt eyJ… тут"),
            ("password=hunter2 и DB_PASSWORD: s3cret", "password=••• и DB_PASSWORD: •••"),
            ("api_key = abc123 GITHUB_TOKEN=ghp_abcdefghijklmnopqrstuvwxyz0123456789", "api_key = ••• GITHUB_TOKEN=•••"),
            (r#"{"token": "xyz", "x": 1} и https://h.io/cb?token=abc&next=1"#, r#"{"token": ••• "x": 1} и https://h.io/cb?token=•••"#),
            ("вот: -----BEGIN OPENSSH PRIVATE KEY----- b3BlbnNzaC1rZXk -----END OPENSSH PRIVATE KEY----- всё", "вот: -----BEGIN OPENSSH PRIVATE KEY-----…"),
            ("-----BEGIN PGP PRIVATE KEY BLOCK----- lQOYBGM -----END PGP PRIVATE KEY BLOCK-----", "-----BEGIN PGP PRIVATE KEY BLOCK-----…"),
            // ключ AWS — ровно 16 знаков [A-Z2-7] после AKIA/ASIA, цифры может не быть
            ("AKIAABCDEFGHIJKLMNOP и ASIAQRSTUVWXYZ234567", "AKIA… и ASIA…"),
        ] {
            assert_eq!(mask_secrets(raw), want, "{raw}");
        }
        // не секреты: хэши, UUID, пути, имена переменных и слова с похожим началом
        for raw in [
            "коммит 5493482a1b2c3d4e5f60718293a4b5c6d7e8f901 и 9db1e2b",
            "id 123e4567-e89b-12d3-a456-426614174000",
            "/home/u/.cache/hf_hub_2024_models_files/x и ./sk-learn-0123456789abcdef",
            "npm_config_cache_directory sk-learn ASIA AKIA max_tokens: 500 token_count=5",
            "ASIAPACIFICREGIONS AKIAABCDEFGHIJKLMNOPQ ASIA_PACIFIC_REGION_NAMES",
            "-----BEGIN CERTIFICATE----- MIIB -----END CERTIFICATE-----",
            "eyJhbGciOiJIUzI1NiJ9 без подписи",
        ] {
            assert_eq!(mask_secrets(raw), raw);
        }
    }
}
