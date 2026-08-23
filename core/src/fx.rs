use std::time::Duration;

use anyhow::{Context, Result, bail};
use chrono::{Local, NaiveDateTime};
use rusqlite::{Connection, OptionalExtension, params};

use crate::db::{DATE_FMT, Entry, Total, now_string, primary_currency, totals};

/// A rate relates exactly one pair, in one direction, read from one place.
/// Everything else in paybar stays currency-agnostic.
pub const BASE: &str = "USD";
pub const QUOTE: &str = "ARS";

/// dolarapi publishes the dollar in seven places at once. Which one is a
/// judgement call, so it is the user's; an unknown name is an error rather
/// than a silent fallback to a number they did not choose.
pub const CASAS: [&str; 7] =
    ["oficial", "blue", "bolsa", "contadoconliqui", "cripto", "mayorista", "tarjeta"];

const DEFAULT_CASA: &str = "blue";
const DEFAULT_TTL_SECS: i64 = 3600;
const TIMEOUT: Duration = Duration::from_millis(2500);

pub struct Config {
    pub enabled: bool,
    pub casa: String,
    pub ttl_secs: i64,
}

impl Config {
    pub fn from_env() -> Result<Config> {
        let enabled = match std::env::var("PAYBAR_FX") {
            Ok(v) => !matches!(v.trim().to_lowercase().as_str(), "off" | "0" | "false" | "no"),
            Err(_) => true,
        };
        let casa = match std::env::var("PAYBAR_FX_CASA") {
            Ok(v) if !v.trim().is_empty() => v.trim().to_lowercase(),
            _ => DEFAULT_CASA.to_string(),
        };
        if !CASAS.contains(&casa.as_str()) {
            bail!("unknown casa {casa:?}; expected one of {}", CASAS.join(", "));
        }
        let ttl_secs = match std::env::var("PAYBAR_FX_TTL") {
            Ok(v) if !v.trim().is_empty() => {
                v.trim().parse::<i64>().with_context(|| format!("PAYBAR_FX_TTL: {v:?}"))?
            }
            _ => DEFAULT_TTL_SECS,
        };
        if ttl_secs < 0 {
            bail!("PAYBAR_FX_TTL cannot be negative");
        }
        Ok(Config { enabled, casa, ttl_secs })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Rate {
    pub casa: String,
    pub base: String,
    pub quote: String,
    /// Centavos of `quote` for one unit of `base`. Blue at 1550.00 is 155_000.
    pub rate_centavos: i64,
    /// When paybar read it, local time.
    pub fetched_at: String,
    /// When the source says it last moved. Passed through, never parsed.
    pub source_updated_at: Option<String>,
    /// The last refresh failed and this is the previous answer.
    pub stale: bool,
}

impl Rate {
    pub fn age_secs(&self, now: NaiveDateTime) -> i64 {
        match NaiveDateTime::parse_from_str(&self.fetched_at, DATE_FMT) {
            Ok(t) => (now - t).num_seconds().max(0),
            Err(_) => i64::MAX,
        }
    }

    /// "@ blue 1,550.00", plus its age once it has stopped being current.
    /// A converted number nobody can attribute is worse than none at all, so
    /// every surface that renders a conversion renders this next to it.
    pub fn label(&self, now: NaiveDateTime) -> String {
        let age =
            if self.stale { format!(" ({} old)", self.age_label(now)) } else { String::new() };
        format!("@ {} {}{}", self.casa, crate::money::format_cents(self.rate_centavos), age)
    }

    /// "4m", "3h", "2d" — enough to judge whether to trust it, no more.
    pub fn age_label(&self, now: NaiveDateTime) -> String {
        let secs = self.age_secs(now);
        if secs >= 86_400 {
            format!("{}d", secs / 86_400)
        } else if secs >= 3600 {
            format!("{}h", secs / 3600)
        } else {
            format!("{}m", secs / 60)
        }
    }
}

// ---- conversion -------------------------------------------------------------

/// Cents of `from` to cents of `to`, or `None` when the pair is not the one
/// this rate describes.
///
/// Integer throughout and half-up at the end: the annotation is approximate by
/// construction, but it should not also drift.
pub fn convert_cents(amount_cents: i64, from: &str, to: &str, rate_centavos: i64) -> Option<i64> {
    if rate_centavos <= 0 || from == to {
        return None;
    }
    let n = amount_cents as i128;
    let rate = rate_centavos as i128;
    let scaled = match (from, to) {
        (BASE, QUOTE) => n * rate,
        (QUOTE, BASE) => {
            // (n / rate) * 100, kept in one expression so the division rounds once.
            n * 100 * 100
        }
        _ => return None,
    };
    let divisor: i128 = match (from, to) {
        (BASE, QUOTE) => 100,
        _ => rate * 100,
    };
    Some(div_round_half_up(scaled, divisor) as i64)
}

fn div_round_half_up(n: i128, d: i128) -> i128 {
    let sign = if (n < 0) != (d < 0) { -1 } else { 1 };
    let (n, d) = (n.abs(), d.abs());
    sign * ((n + d / 2) / d)
}

/// The rate a period needs, or `None` when it needs none: a month in a single
/// currency, or one whose currencies this rate does not relate, never touches
/// the network.
pub fn needed_for(totals: &[Total], primary: Option<&str>) -> bool {
    let Some(primary) = primary else { return false };
    let other = match primary {
        QUOTE => BASE,
        BASE => QUOTE,
        _ => return false,
    };
    totals.iter().any(|t| t.currency == other)
}

// ---- cache ------------------------------------------------------------------

pub fn cached(conn: &Connection, casa: &str) -> Result<Option<Rate>> {
    let row = conn
        .query_row(
            "SELECT rate_centavos, fetched_at, source_updated_at FROM fx_rates
             WHERE casa = ?1 AND base = ?2 AND quote = ?3",
            params![casa, BASE, QUOTE],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?)),
        )
        .optional()?;
    Ok(row.map(|(rate_centavos, fetched_at, source_updated_at)| Rate {
        casa: casa.to_string(),
        base: BASE.to_string(),
        quote: QUOTE.to_string(),
        rate_centavos,
        fetched_at,
        source_updated_at,
        stale: false,
    }))
}

pub fn store(conn: &Connection, rate: &Rate) -> Result<()> {
    conn.execute(
        "INSERT INTO fx_rates (casa, base, quote, rate_centavos, fetched_at, source_updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(casa, base, quote) DO UPDATE SET
           rate_centavos = excluded.rate_centavos,
           fetched_at = excluded.fetched_at,
           source_updated_at = excluded.source_updated_at",
        params![
            rate.casa,
            rate.base,
            rate.quote,
            rate.rate_centavos,
            rate.fetched_at,
            rate.source_updated_at
        ],
    )?;
    Ok(())
}

// ---- source -----------------------------------------------------------------

/// `venta`, not `compra`: converting an obligation denominated in dollars asks
/// what it costs to obtain them, which is the sell side.
fn parse_body(body: &str) -> Result<(i64, Option<String>)> {
    let v: serde_json::Value = serde_json::from_str(body).context("dolarapi returned non-JSON")?;
    let venta = v.get("venta").context("dolarapi response has no `venta`")?;
    let raw = match venta {
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::String(s) => s.clone(),
        _ => bail!("`venta` is neither a number nor a string"),
    };
    let centavos = parse_centavos(&raw)?;
    if centavos <= 0 {
        bail!("`venta` is not a positive rate: {raw}");
    }
    let source = v.get("fechaActualizacion").and_then(|s| s.as_str()).map(str::to_string);
    Ok((centavos, source))
}

/// A decimal to centavos, half-up past two places.
///
/// Deliberately not `money::parse_cents`: that one rejects extra precision
/// because it guards what a human typed, where silent rounding loses money.
/// This guards a quoted rate, where refusing 1545.333 would lose the whole
/// annotation over a third decimal nobody asked about.
fn parse_centavos(raw: &str) -> Result<i64> {
    let s = raw.trim();
    let (whole, frac) = match s.split_once('.') {
        Some((w, f)) => (w, f),
        None => (s, ""),
    };
    if whole.is_empty() || !whole.chars().all(|c| c.is_ascii_digit()) {
        bail!("not a rate: {raw:?}");
    }
    if !frac.chars().all(|c| c.is_ascii_digit()) {
        bail!("not a rate: {raw:?}");
    }
    let whole: i64 = whole.parse().with_context(|| format!("not a rate: {raw:?}"))?;
    let mut padded = format!("{frac:0<3}");
    padded.truncate(3);
    let thousandths: i64 = padded.parse().unwrap_or(0);
    Ok(whole * 100 + (thousandths + 5) / 10)
}

fn fetch(casa: &str) -> Result<(i64, Option<String>)> {
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        .user_agent("paybar")
        .build();
    let agent: ureq::Agent = config.into();
    let url = format!("https://dolarapi.com/v1/dolares/{casa}");
    let body = agent.get(&url).call()?.body_mut().read_to_string()?;
    parse_body(&body)
}

/// The rate to render, refreshing when the cached copy aged past the TTL.
///
/// Never fails and never blocks a command: a fetch that fails falls back to the
/// cache marked stale, and an empty cache falls back to no annotation at all.
/// A missing exchange rate is not an error in a program about fixed expenses.
pub fn resolve(conn: &Connection, cfg: &Config, force: bool) -> Option<Rate> {
    if !cfg.enabled {
        return None;
    }
    let cached = cached(conn, &cfg.casa).ok().flatten();
    let now = Local::now().naive_local();
    let fresh_enough = match &cached {
        Some(r) => r.age_secs(now) < cfg.ttl_secs,
        None => false,
    };
    if fresh_enough && !force {
        return cached;
    }
    match fetch(&cfg.casa) {
        Ok((rate_centavos, source_updated_at)) => {
            let rate = Rate {
                casa: cfg.casa.clone(),
                base: BASE.to_string(),
                quote: QUOTE.to_string(),
                rate_centavos,
                fetched_at: now_string(),
                source_updated_at,
                stale: false,
            };
            let _ = store(conn, &rate);
            Some(rate)
        }
        Err(_) => cached.map(|mut r| {
            r.stale = true;
            r
        }),
    }
}

/// The rate a listing needs, or `None` when it needs none.
///
/// A misconfigured casa is an error rather than a silent fall back to a rate
/// the user did not choose. A *missing* rate is not an error at all: a
/// fixed-expense tracker that cannot reach the internet still has every answer
/// that matters, minus one annotation.
pub fn for_entries(conn: &Connection, entries: &[Entry], force: bool) -> Result<Option<Rate>> {
    let cfg = Config::from_env()?;
    if !cfg.enabled {
        return Ok(None);
    }
    if !needed_for(&totals(entries), primary_currency(entries).as_deref()) {
        return Ok(None);
    }
    Ok(resolve(conn, &cfg, force))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rate() -> Rate {
        Rate {
            casa: "blue".into(),
            base: BASE.into(),
            quote: QUOTE.into(),
            rate_centavos: 155_000,
            fetched_at: "2026-08-23T18:00:00".into(),
            source_updated_at: Some("2026-08-23T21:00:00.000Z".into()),
            stale: false,
        }
    }

    fn total(currency: &str) -> Total {
        Total { currency: currency.into(), due_cents: 0, paid_cents: 0 }
    }

    #[test]
    fn usd_converts_to_ars_at_the_quoted_rate() {
        // USD 1,140.00 at blue 1550 is ARS 1,767,000.00.
        assert_eq!(convert_cents(114_000, BASE, QUOTE, 155_000), Some(176_700_000));
    }

    #[test]
    fn ars_converts_back_to_usd() {
        assert_eq!(convert_cents(176_700_000, QUOTE, BASE, 155_000), Some(114_000));
    }

    #[test]
    fn a_rate_with_centavos_survives_the_round_trip() {
        // bolsa at 1545.30
        assert_eq!(convert_cents(100, BASE, QUOTE, 154_530), Some(154_530));
    }

    #[test]
    fn conversion_declines_pairs_it_does_not_describe() {
        assert_eq!(convert_cents(100, "EUR", QUOTE, 155_000), None);
        assert_eq!(convert_cents(100, BASE, BASE, 155_000), None);
        assert_eq!(convert_cents(100, BASE, QUOTE, 0), None);
    }

    #[test]
    fn rounding_is_half_up_not_truncation() {
        assert_eq!(div_round_half_up(15, 10), 2);
        assert_eq!(div_round_half_up(14, 10), 1);
        assert_eq!(div_round_half_up(-15, 10), -2);
    }

    /// A month in one currency has nothing to relate, and must not reach for
    /// the network to discover that.
    #[test]
    fn a_rate_is_needed_only_when_two_related_currencies_meet() {
        assert!(!needed_for(&[total("ARS")], Some("ARS")));
        assert!(!needed_for(&[], None));
        assert!(!needed_for(&[total("ARS"), total("EUR")], Some("ARS")));
        assert!(needed_for(&[total("ARS"), total("USD")], Some("ARS")));
        assert!(needed_for(&[total("ARS"), total("USD")], Some("USD")));
    }

    #[test]
    fn the_response_is_read_from_venta() {
        let body = r#"{"moneda":"USD","casa":"blue","nombre":"Blue","compra":1530,
                       "venta":1550,"fechaActualizacion":"2026-08-23T21:00:00.000Z"}"#;
        let (centavos, source) = parse_body(body).unwrap();
        assert_eq!(centavos, 155_000);
        assert_eq!(source.as_deref(), Some("2026-08-23T21:00:00.000Z"));
    }

    #[test]
    fn a_fractional_quote_keeps_its_centavos() {
        let body = r#"{"venta":1545.3}"#;
        assert_eq!(parse_body(body).unwrap().0, 154_530);
    }

    /// Rejecting a third decimal would lose the annotation over a digit the
    /// user never typed and cannot fix.
    #[test]
    fn extra_precision_in_a_quote_is_rounded_not_rejected() {
        assert_eq!(parse_centavos("1586.084").unwrap(), 158_608);
        assert_eq!(parse_centavos("1586.085").unwrap(), 158_609);
        assert_eq!(parse_centavos("1550").unwrap(), 155_000);
        assert_eq!(parse_centavos("1550.5").unwrap(), 155_050);
    }

    #[test]
    fn a_garbage_body_is_an_error_not_a_zero_rate() {
        assert!(parse_body("not json").is_err());
        assert!(parse_body(r#"{"compra":1530}"#).is_err());
        assert!(parse_body(r#"{"venta":"abc"}"#).is_err());
        assert!(parse_body(r#"{"venta":0}"#).is_err());
    }

    #[test]
    fn a_label_names_the_casa_and_only_admits_an_age_when_stale() {
        let now = NaiveDateTime::parse_from_str("2026-08-23T21:00:00", DATE_FMT).unwrap();
        assert_eq!(rate().label(now), "@ blue 1,550.00");
        let mut r = rate();
        r.stale = true;
        assert_eq!(r.label(now), "@ blue 1,550.00 (3h old)");
    }

    #[test]
    fn age_is_reported_in_the_coarsest_useful_unit() {
        let now = |s: &str| NaiveDateTime::parse_from_str(s, DATE_FMT).unwrap();
        assert_eq!(rate().age_label(now("2026-08-23T18:04:00")), "4m");
        assert_eq!(rate().age_label(now("2026-08-23T21:00:00")), "3h");
        assert_eq!(rate().age_label(now("2026-08-25T18:00:00")), "2d");
    }

    #[test]
    fn an_unparseable_timestamp_reads_as_infinitely_old() {
        let mut r = rate();
        r.fetched_at = "whenever".into();
        assert_eq!(r.age_secs(Local::now().naive_local()), i64::MAX);
    }
}
