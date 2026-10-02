//! Числа → слова, именительный падеж (склонение по контексту правилами ненадёжно).

const ONES: [&str; 20] = [
    "ноль", "один", "два", "три", "четыре", "пять", "шесть", "семь", "восемь", "девять", "десять",
    "одиннадцать", "двенадцать", "тринадцать", "четырнадцать", "пятнадцать", "шестнадцать",
    "семнадцать", "восемнадцать", "девятнадцать",
];
const TENS: [&str; 10] = ["", "", "двадцать", "тридцать", "сорок", "пятьдесят", "шестьдесят", "семьдесят", "восемьдесят", "девяносто"];
const HUNDREDS: [&str; 10] = ["", "сто", "двести", "триста", "четыреста", "пятьсот", "шестьсот", "семьсот", "восемьсот", "девятьсот"];

pub fn numbers_to_words(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if !chars[i].is_ascii_digit() {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        // группа: цифры, разделённые одиночными '.' или ',' (версия/десятичное)
        let mut parts = vec![String::new()];
        while i < chars.len() {
            let c = chars[i];
            if c.is_ascii_digit() {
                parts.last_mut().unwrap().push(c);
            } else if (c == '.' || c == ',') && chars.get(i + 1).is_some_and(char::is_ascii_digit) {
                parts.push(String::new());
            } else {
                break;
            }
            i += 1;
        }
        let words: Vec<String> = parts.iter().map(|p| int_words(p)).collect();
        out.push_str(&words.join(" точка "));
        // процент: "50%" или "50 %"
        let mut j = i;
        if chars.get(j) == Some(&' ') {
            j += 1;
        }
        if chars.get(j) == Some(&'%') {
            out.push_str(" процентов");
            i = j + 1;
        }
    }
    out
}

fn int_words(digits: &str) -> String {
    let n: u64 = match digits.parse() {
        Ok(n) if n < 1_000_000_000 => n,
        _ => return digits.chars().map(|c| ONES[c.to_digit(10).unwrap() as usize]).collect::<Vec<_>>().join(" "),
    };
    if n == 0 {
        return "ноль".into();
    }
    let mut w: Vec<String> = Vec::new();
    let millions = n / 1_000_000;
    let thousands = n / 1000 % 1000;
    let rest = n % 1000;
    if millions > 0 {
        w.push(triple(millions, false));
        w.push(plural(millions, "миллион", "миллиона", "миллионов").into());
    }
    if thousands > 0 {
        w.push(triple(thousands, true));
        w.push(plural(thousands, "тысяча", "тысячи", "тысяч").into());
    }
    if rest > 0 {
        w.push(triple(rest, false));
    }
    w.retain(|s| !s.is_empty());
    w.join(" ")
}

/// 1..999; feminine — для тысяч («одна», «две»).
fn triple(n: u64, feminine: bool) -> String {
    let mut w = Vec::new();
    let h = (n / 100) as usize;
    let t = (n % 100) as usize;
    if h > 0 {
        w.push(HUNDREDS[h].to_string());
    }
    let (tens, ones) = if t < 20 { (0, t) } else { (t / 10, t % 10) };
    if tens > 0 {
        w.push(TENS[tens].to_string());
    }
    if ones > 0 {
        w.push(match (ones, feminine) {
            (1, true) => "одна".into(),
            (2, true) => "две".into(),
            _ => ONES[ones].to_string(),
        });
    }
    w.join(" ")
}

fn plural(n: u64, one: &'static str, few: &'static str, many: &'static str) -> &'static str {
    let (d, h) = (n % 10, n % 100);
    if d == 1 && h != 11 {
        one
    } else if (2..=4).contains(&d) && !(12..=14).contains(&h) {
        few
    } else {
        many
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integers() {
        assert_eq!(numbers_to_words("0"), "ноль");
        assert_eq!(numbers_to_words("42 файла"), "сорок два файла");
        assert_eq!(numbers_to_words("13984"), "тринадцать тысяч девятьсот восемьдесят четыре");
        assert_eq!(numbers_to_words("2001"), "две тысячи один");
        assert_eq!(numbers_to_words("1000000"), "один миллион");
    }

    #[test]
    fn decimals_versions_percent() {
        assert_eq!(numbers_to_words("1,8 с"), "один точка восемь с");
        assert_eq!(numbers_to_words("2.1.284"), "два точка один точка двести восемьдесят четыре");
        assert_eq!(numbers_to_words("50 %"), "пятьдесят процентов");
        assert_eq!(numbers_to_words("7%"), "семь процентов");
    }

    #[test]
    fn sentence_end_dot_kept() {
        assert_eq!(numbers_to_words("Итого 3."), "Итого три.");
    }
}
