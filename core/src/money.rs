use anyhow::{Result, bail};

/// Amounts are integer cents, never floats: a fixed expense is compared and
/// summed every month, and binary floating point cannot hold 12.99 exactly.

/// Parse a plain decimal to cents. `_` and `,` are stripped as digit grouping,
/// `.` is the decimal separator.
///
/// More than two decimal places is an error rather than a rounding: silently
/// turning 12.999 into 13.00 is the kind of helpfulness that loses money.
pub fn parse_cents(raw: &str) -> Result<i64> {
    let cleaned: String = raw.chars().filter(|c| *c != '_' && *c != ',').collect();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() {
        bail!("amount is empty");
    }
    let (sign, digits) = match cleaned.strip_prefix('-') {
        Some(rest) => (-1, rest),
        None => (1, cleaned.strip_prefix('+').unwrap_or(cleaned)),
    };

    let (whole, frac) = match digits.split_once('.') {
        Some((w, f)) => (w, f),
        None => (digits, ""),
    };
    if whole.is_empty() && frac.is_empty() {
        bail!("not an amount: {raw:?}");
    }
    if !whole.chars().all(|c| c.is_ascii_digit()) || !frac.chars().all(|c| c.is_ascii_digit()) {
        bail!("not an amount: {raw:?}");
    }
    if frac.len() > 2 {
        bail!("amount has more than two decimals: {raw:?}");
    }

    let whole: i64 = if whole.is_empty() { 0 } else { whole.parse()? };
    let frac: i64 = match frac.len() {
        0 => 0,
        1 => frac.parse::<i64>()? * 10,
        _ => frac.parse::<i64>()?,
    };
    Ok(sign * (whole * 100 + frac))
}

/// Cents to a grouped decimal: 4590000 -> "45,900.00".
pub fn format_cents(cents: i64) -> String {
    let neg = cents < 0;
    let abs = cents.unsigned_abs();
    let whole = abs / 100;
    let frac = abs % 100;

    let digits = whole.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(c);
    }
    format!("{}{}.{:02}", if neg { "-" } else { "" }, grouped, frac)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_shapes_people_type() {
        assert_eq!(parse_cents("45900").unwrap(), 4_590_000);
        assert_eq!(parse_cents("12.99").unwrap(), 1299);
        assert_eq!(parse_cents("12.9").unwrap(), 1290);
        assert_eq!(parse_cents("12.").unwrap(), 1200);
        assert_eq!(parse_cents(".5").unwrap(), 50);
        assert_eq!(parse_cents("1_000").unwrap(), 100_000);
        assert_eq!(parse_cents("1,000.50").unwrap(), 100_050);
        assert_eq!(parse_cents("  20  ").unwrap(), 2000);
        assert_eq!(parse_cents("-8.25").unwrap(), -825);
    }

    /// Rounding here would silently change the number the user typed.
    #[test]
    fn extra_precision_is_rejected_not_rounded() {
        assert!(parse_cents("12.999").is_err());
        assert!(parse_cents("0.001").is_err());
    }

    #[test]
    fn rejects_things_that_are_not_amounts() {
        assert!(parse_cents("").is_err());
        assert!(parse_cents("abc").is_err());
        assert!(parse_cents("12.3.4").is_err());
        assert!(parse_cents("1 000").is_err());
        assert!(parse_cents(".").is_err());
    }

    #[test]
    fn formats_with_thousands_grouping() {
        assert_eq!(format_cents(4_590_000), "45,900.00");
        assert_eq!(format_cents(1299), "12.99");
        assert_eq!(format_cents(0), "0.00");
        assert_eq!(format_cents(5), "0.05");
        assert_eq!(format_cents(100_000_000), "1,000,000.00");
        assert_eq!(format_cents(-825), "-8.25");
    }

    #[test]
    fn parse_and_format_round_trip() {
        for s in ["45,900.00", "12.99", "0.00", "1,000,000.00"] {
            assert_eq!(format_cents(parse_cents(s).unwrap()), s);
        }
    }
}
