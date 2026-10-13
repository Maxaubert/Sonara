//! Numbers as words (#294): cardinals, ordinals, years and digits, written
//! the way they are said, with British "and" ("one hundred and twelve") and
//! spaces instead of hyphens ("twenty six"), so no engine reads them again.

const ONES: [&str; 20] = [
    "zero",
    "one",
    "two",
    "three",
    "four",
    "five",
    "six",
    "seven",
    "eight",
    "nine",
    "ten",
    "eleven",
    "twelve",
    "thirteen",
    "fourteen",
    "fifteen",
    "sixteen",
    "seventeen",
    "eighteen",
    "nineteen",
];
const TENS: [&str; 10] = [
    "", "", "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety",
];
const SCALES: [&str; 7] = [
    "",
    "thousand",
    "million",
    "billion",
    "trillion",
    "quadrillion",
    "quintillion",
];

/// 0..=99 as words.
fn below_hundred(n: u64) -> String {
    if n < 20 {
        ONES[n as usize].to_string()
    } else if n.is_multiple_of(10) {
        TENS[(n / 10) as usize].to_string()
    } else {
        format!("{} {}", TENS[(n / 10) as usize], ONES[(n % 10) as usize])
    }
}

/// 1..=999 as words ("one hundred and twelve").
fn below_thousand(n: u64) -> String {
    let (h, rest) = (n / 100, n % 100);
    match (h, rest) {
        (0, r) => below_hundred(r),
        (h, 0) => format!("{} hundred", ONES[h as usize]),
        (h, r) => format!("{} hundred and {}", ONES[h as usize], below_hundred(r)),
    }
}

/// A whole number as words: "one thousand and twenty four".
pub fn cardinal(n: u64) -> String {
    if n == 0 {
        return "zero".into();
    }
    let mut groups = Vec::new();
    let mut m = n;
    while m > 0 {
        groups.push(m % 1000);
        m /= 1000;
    }
    let mut parts = Vec::new();
    for (i, g) in groups.iter().enumerate().rev() {
        if *g == 0 {
            continue;
        }
        let words = below_thousand(*g);
        // "one thousand and twenty four": "and" before a last group under
        // a hundred that follows a higher one.
        let words = if i == 0 && *g < 100 && groups.len() > 1 {
            format!("and {words}")
        } else {
            words
        };
        parts.push(if i == 0 {
            words
        } else {
            format!("{words} {}", SCALES[i])
        });
    }
    parts.join(" ")
}

/// Digits (with any leading zeros) as a whole number, else one by one
/// when too long.
pub fn cardinal_str(digits: &str) -> String {
    match digits.parse::<u64>() {
        Ok(n) => cardinal(n),
        Err(_) => spell_digits(digits),
    }
}

/// "first", "twenty first", "one hundred and twelfth".
pub fn ordinal(n: u64) -> String {
    let words = cardinal(n);
    let (head, last) = match words.rsplit_once(' ') {
        Some((h, l)) => (format!("{h} "), l.to_string()),
        None => (String::new(), words.clone()),
    };
    let last = match last.as_str() {
        "one" => "first".into(),
        "two" => "second".into(),
        "three" => "third".into(),
        "five" => "fifth".into(),
        "eight" => "eighth".into(),
        "nine" => "ninth".into(),
        "twelve" => "twelfth".into(),
        w if w.ends_with('y') => format!("{}ieth", &w[..w.len() - 1]),
        w => format!("{w}th"),
    };
    format!("{head}{last}")
}

/// A year as said: "nineteen ninety nine", "nineteen oh five", "two
/// thousand and five", "twenty twenty six"; other numbers as cardinals.
pub fn year(y: u64) -> String {
    if !(1000..=2099).contains(&y) {
        return cardinal(y);
    }
    if (2000..2010).contains(&y) || y.is_multiple_of(1000) {
        return cardinal(y);
    }
    let (hi, lo) = (y / 100, y % 100);
    let lo = match lo {
        0 => "hundred".to_string(),
        1..=9 => format!("oh {}", ONES[lo as usize]),
        _ => below_hundred(lo),
    };
    format!("{} {lo}", below_hundred(hi))
}

/// A plural of a number word: "nineties", "twenties", "thousands".
pub fn plural(words: &str) -> String {
    match words.strip_suffix('y') {
        Some(stem) => format!("{stem}ies"),
        None => format!("{words}s"),
    }
}

/// Each digit as a word: "zero zero seven".
pub fn spell_digits(s: &str) -> String {
    s.chars()
        .filter_map(|c| c.to_digit(10))
        .map(|d| ONES[d as usize])
        .collect::<Vec<_>>()
        .join(" ")
}

/// A decimal: "three point one four", "point five", "minus two point five".
pub fn decimal(int_part: &str, frac: &str) -> String {
    let whole = if int_part.is_empty() {
        String::new()
    } else {
        format!("{} ", cardinal_str(&int_part.replace(',', "")))
    };
    if frac.is_empty() {
        return whole.trim_end().to_string();
    }
    format!("{whole}point {}", spell_digits(frac))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_are_said_with_british_and_and_no_hyphens() {
        assert_eq!(cardinal(0), "zero");
        assert_eq!(cardinal(26), "twenty six");
        assert_eq!(cardinal(112), "one hundred and twelve");
        assert_eq!(cardinal(1024), "one thousand and twenty four");
        assert_eq!(cardinal(3500), "three thousand five hundred");
        assert_eq!(
            cardinal(1_234_567),
            "one million two hundred and thirty four thousand five hundred and sixty seven"
        );
        assert_eq!(ordinal(1), "first");
        assert_eq!(ordinal(21), "twenty first");
        assert_eq!(ordinal(12), "twelfth");
        assert_eq!(ordinal(100), "one hundredth");
        assert_eq!(ordinal(112), "one hundred and twelfth");
        assert_eq!(ordinal(30), "thirtieth");
        assert_eq!(year(1999), "nineteen ninety nine");
        assert_eq!(year(1905), "nineteen oh five");
        assert_eq!(year(2000), "two thousand");
        assert_eq!(year(2005), "two thousand and five");
        assert_eq!(year(2026), "twenty twenty six");
        assert_eq!(year(1900), "nineteen hundred");
        assert_eq!(plural(&year(1990)), "nineteen nineties");
        assert_eq!(spell_digits("007"), "zero zero seven");
        assert_eq!(decimal("3", "14"), "three point one four");
        assert_eq!(decimal("", "5"), "point five");
    }
}
