use std::time::Duration;

use anyhow::{Context, Result, bail};
use chrono::{Local, NaiveDateTime};
use rusqlite::{Connection, OptionalExtension, params};

use crate::db::{DATE_FMT, Entry, Total, now_string, primary_currency, totals};

/// A rate relates exactly one pair, in one direction, read from one place.
/// Everything else in paybar stays currency-agnostic.
const BASE: &str = "USD";
const QUOTE: &str = "ARS";

/// dolarapi publishes the dollar in seven places at once. Which one is a
/// judgement call, so it is the user's; an unknown name is an error rather
/// than a silent fallback to a number they did not choose.
const CASAS: [&str; 7] =
    ["oficial", "blue", "bolsa", "contadoconliqui", "cripto", "mayorista", "tarjeta"];

const DEFAULT_CASA: &str = "blue";
const DEFAULT_TTL_SECS: i64 = 3600;
const TIMEOUT: Duration = Duration::from_millis(2500);

struct Config {
    enabled: bool,
    casa: String,
    ttl_secs: i64,
}

impl Config {
    fn from_env() -> Result<Config> {
        let enabled = match std::env::var("PAYBAR_FX") {
            Ok(v) => !matches!(v.trim().to_lowercase().as_str(), "off" | "0" | "false" | "no"),
            Err(_) => true,
        };
        if !enabled {
            // Nothing downstream will read the casa or the TTL, so nothing
            // downstream should be able to reject them either.
            return Ok(Config {
                enabled,
                casa: DEFAULT_CASA.to_string(),
                ttl_secs: DEFAULT_TTL_SECS,
            });
        }
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
    /// How long this rate was meant to stay fresh. Carried along so every
    /// surface derives staleness the same way rather than trusting a flag set
    /// at fetch time — a rate held on screen for hours goes stale where it
    /// sits, and a stored boolean would not notice.
    pub ttl_secs: i64,
}

impl Rate {
    /// Still within the TTL it was fetched under. Derived, never stored.
    pub fn is_current(&self, now: NaiveDateTime) -> bool {
        self.age_secs(now) < self.ttl_secs
    }

    fn age_secs(&self, now: NaiveDateTime) -> i64 {
        match NaiveDateTime::parse_from_str(&self.fetched_at, DATE_FMT) {
            Ok(t) => (now - t).num_seconds().max(0),
            Err(_) => i64::MAX,
        }
    }

    /// "@ blue 1,550.00", plus its age once it has stopped being current.
    /// A converted number nobody can attribute is worse than none at all, so
    /// every surface that renders a conversion renders this next to it.
    pub fn label(&self, now: NaiveDateTime) -> String {
        let age = if self.is_current(now) {
            String::new()
        } else {
            format!(" ({} old)", self.age_label(now))
        };
        format!("@ {} {}{}", self.casa, crate::money::format_cents(self.rate_centavos), age)
    }

    /// The whole annotation bar the indentation each surface picks for itself.
    /// One implementation, so the attribution cannot drift between the CLI and
    /// the TUI the way it would if each formatted its own.
    pub fn annotation(&self, approx_cents: i64, primary: &str, now: NaiveDateTime) -> String {
        format!(
            "\u{2248} {} {} due {}",
            primary,
            crate::money::format_cents(approx_cents),
            self.label(now)
        )
    }

    /// "4m", "3h", "2d" — enough to judge whether to trust it, no more.
    fn age_label(&self, now: NaiveDateTime) -> String {
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
    // Both directions as one numerator/denominator pair, so the rounding
    // happens once and an unrelated pair cannot fall through to a wildcard
    // that silently assumes the other direction's divisor.
    let (numerator, denominator) = match (from, to) {
        (BASE, QUOTE) => (n * rate, 100),
        (QUOTE, BASE) => (n * 10_000, rate * 100),
        _ => return None,
    };
    Some(div_round_half_up(numerator, denominator) as i64)
}

fn div_round_half_up(n: i128, d: i128) -> i128 {
    let sign = if (n < 0) != (d < 0) { -1 } else { 1 };
    let (n, d) = (n.abs(), d.abs());
    sign * ((n + d / 2) / d)
}

/// Whether a period has two currencies this rate can relate. A month in a
/// single currency, or one holding a currency outside the pair, never touches
/// the network to discover it has nothing to convert.
fn needed_for(totals: &[Total], primary: Option<&str>) -> bool {
    let Some(primary) = primary else { return false };
    let other = match primary {
        QUOTE => BASE,
        BASE => QUOTE,
        _ => return false,
    };
    totals.iter().any(|t| t.currency == other)
}

/// What a total is worth in the primary currency, or `None` when there is
/// nothing to say: no rate, the primary currency itself, or a pair this rate
/// does not describe.
///
/// Only the *due* total converts. What was actually paid was paid at whatever
/// rate applied that day, and restating it at today's is not an approximation
/// but a wrong number.
pub fn approx_for(rate: Option<&Rate>, total: &Total, primary: Option<&str>) -> Option<i64> {
    let (primary, rate) = (primary?, rate?);
    convert_cents(total.due_cents, &total.currency, primary, rate.rate_centavos)
}

// ---- cache ------------------------------------------------------------------

fn cached(conn: &Connection, cfg: &Config) -> Result<Option<Rate>> {
    let row = conn
        .query_row(
            "SELECT rate_centavos, fetched_at, source_updated_at FROM fx_rates
             WHERE casa = ?1 AND base = ?2 AND quote = ?3",
            params![cfg.casa, BASE, QUOTE],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?)),
        )
        .optional()?;
    Ok(row.map(|(rate_centavos, fetched_at, source_updated_at)| Rate {
        casa: cfg.casa.clone(),
        base: BASE.to_string(),
        quote: QUOTE.to_string(),
        rate_centavos,
        fetched_at,
        source_updated_at,
        ttl_secs: cfg.ttl_secs,
    }))
}

fn store(conn: &Connection, rate: &Rate) -> Result<()> {
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
fn resolve(conn: &Connection, cfg: &Config, force: bool) -> Option<Rate> {
    if !cfg.enabled {
        return None;
    }
    let cached = cached(conn, cfg).ok().flatten();
    let now = Local::now().naive_local();
    if !force && cached.as_ref().is_some_and(|r| r.is_current(now)) {
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
                ttl_secs: cfg.ttl_secs,
            };
            let _ = store(conn, &rate);
            Some(rate)
        }
        // The previous answer, which `is_current` will already report as stale:
        // reaching this arm means the cache had aged past its TTL.
        Err(_) => cached,
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
            ttl_secs: 3600,
        }
    }

    fn at(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, DATE_FMT).unwrap()
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
        assert_eq!(convert_cents(154_530, QUOTE, BASE, 154_530), Some(100));
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

    /// Staleness is derived from age, not remembered from the fetch: a rate
    /// left on screen goes stale where it sits, and a stored flag would not
    /// notice.
    #[test]
    fn a_label_admits_an_age_only_once_the_ttl_has_passed() {
        assert_eq!(rate().label(at("2026-08-23T18:30:00")), "@ blue 1,550.00");
        assert_eq!(rate().label(at("2026-08-23T21:00:00")), "@ blue 1,550.00 (3h old)");
        assert!(rate().is_current(at("2026-08-23T18:59:00")));
        assert!(!rate().is_current(at("2026-08-23T19:01:00")));
    }

    #[test]
    fn an_annotation_carries_the_figure_and_its_attribution() {
        assert_eq!(
            rate().annotation(176_700_000, "ARS", at("2026-08-23T18:30:00")),
            "\u{2248} ARS 1,767,000.00 due @ blue 1,550.00"
        );
    }

    /// Only the due total converts. Restating what was already paid at today's
    /// rate would not be an approximation, it would be a wrong number.
    #[test]
    fn approx_for_converts_the_due_total_and_leaves_the_primary_alone() {
        let usd = Total { currency: "USD".into(), due_cents: 114_000, paid_cents: 114_000 };
        let ars = Total { currency: "ARS".into(), due_cents: 9_000_000, paid_cents: 0 };
        assert_eq!(approx_for(Some(&rate()), &usd, Some("ARS")), Some(176_700_000));
        assert_eq!(approx_for(Some(&rate()), &ars, Some("ARS")), None);
        assert_eq!(approx_for(None, &usd, Some("ARS")), None);
        assert_eq!(approx_for(Some(&rate()), &usd, None), None);
    }

    /// `off` means off: nothing downstream reads the casa, so nothing
    /// downstream may reject it either.
    #[test]
    fn disabling_conversion_stops_the_casa_being_validated() {
        unsafe {
            std::env::set_var("PAYBAR_FX", "off");
            std::env::set_var("PAYBAR_FX_CASA", "garbage");
        }
        let cfg = Config::from_env().unwrap();
        assert!(!cfg.enabled);
        unsafe {
            std::env::set_var("PAYBAR_FX", "on");
        }
        assert!(Config::from_env().is_err());
        unsafe {
            std::env::remove_var("PAYBAR_FX");
            std::env::remove_var("PAYBAR_FX_CASA");
        }
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
