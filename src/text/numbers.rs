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
    let digits_at = |i: usize| chars.get(i..).map_or(0, |r| r.iter().take_while(|c| c.is_ascii_digit()).count());
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if !chars[i].is_ascii_digit() {
            // «-3» — минус; «5-7», «2026-11» — дефис
            if chars[i] == '-' && (i == 0 || chars[i - 1].is_whitespace() || chars[i - 1] == '(') && digits_at(i + 1) > 0 {
                out.push_str("минус ");
            } else {
                out.push(chars[i]);
            }
            i += 1;
            continue;
        }
        // целое; разряды по три цифры через пробел или запятую — одно число («1 000», «1,000,000»)
        let n = digits_at(i);
        let mut int: String = chars[i..i + n].iter().collect();
        i += n;
        if n <= 3 && !int.starts_with('0') {
            while matches!(chars.get(i), Some(' ' | ',')) && digits_at(i + 1) == 3 {
                int.extend(&chars[i + 1..i + 4]);
                i += 4;
            }
        }
        out.push_str(&int_words(&int));
        // дальше через одиночные '.' или ',' (версия/десятичное) и время «12:30»
        loop {
            let n = digits_at(i + 1);
            match chars.get(i) {
                Some('.' | ',') if n > 0 => out.push_str(" точка "),
                Some(':') if n == 2 => out.push(' '),
                _ => break,
            }
            out.push_str(&int_words(&chars[i + 1..i + 1 + n].iter().collect::<String>()));
            i += 1 + n;
        }
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

/// Ведущий ноль («05» в «0.05», код «0502») — по цифрам, иначе ноль потеряется.
fn int_words(digits: &str) -> String {
    let n: u64 = match digits.parse() {
        Ok(n) if n < 1_000_000_000 && !(digits.len() > 1 && digits.starts_with('0')) => n,
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
    fn leading_zeros_kept() {
        assert_eq!(numbers_to_words("Порог 0.05 и 0.5"), "Порог ноль точка ноль пять и ноль точка пять");
        assert_eq!(numbers_to_words("код 0502"), "код ноль пять ноль два");
        assert_eq!(numbers_to_words("2.10"), "два точка десять");
    }

    #[test]
    fn thousand_separators() {
        assert_eq!(
            numbers_to_words("1 000 строк, 1,000,000 байт, 13 984 токена"),
            "одна тысяча строк, один миллион байт, тринадцать тысяч девятьсот восемьдесят четыре токена"
        );
        assert_eq!(numbers_to_words("1, 2, 3 и 1,8 с"), "один, два, три и один точка восемь с");
        assert_eq!(numbers_to_words("1 000,5 и 2024 100"), "одна тысяча точка пять и две тысячи двадцать четыре сто");
    }

    #[test]
    fn time() {
        assert_eq!(numbers_to_words("в 12:30 и 9:05"), "в двенадцать тридцать и девять ноль пять");
        assert_eq!(numbers_to_words("3:1"), "три:один");
    }

    #[test]
    fn minus_before_number() {
        assert_eq!(numbers_to_words("-3 и (-2), 5-7, 2026-11"), "минус три и (минус два), пять-семь, две тысячи двадцать шесть-одиннадцать");
    }

    #[test]
    fn sentence_end_dot_kept() {
        assert_eq!(numbers_to_words("Итого 3."), "Итого три.");
    }
}
