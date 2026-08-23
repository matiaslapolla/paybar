use chrono::{NaiveDate, NaiveDateTime};

use crate::db::{Entry, Status, Total, primary_currency, totals};
use crate::fx::{self, Rate};
use crate::money::format_cents;
use crate::period::Period;

/// Human table. Columns: id, mark, due date, name, category, amount.
pub fn print_entries(entries: &[Entry]) {
    for e in entries {
        println!(
            "{}\t{}\t{}\t{}\t{}\t{} {}",
            e.expense.id,
            e.status.mark(),
            e.due_date,
            e.expense.name,
            e.expense.category.as_deref().unwrap_or(""),
            e.expense.currency,
            format_cents(e.expense.amount_cents)
        );
    }
}

/// One line per currency. Nothing at all still prints a line, because "you owe
/// nothing this month" is an answer and a blank screen is not.
pub fn print_status(entries: &[Entry], period: Period, rate: Option<&Rate>, now: NaiveDateTime) {
    let totals = totals(entries);
    if totals.is_empty() {
        println!("{period}\tnothing due");
        return;
    }
    let primary = primary_currency(entries);
    // Counts are per currency, not global: a global count printed next to a
    // currency total reads as that currency's count, and would be a lie.
    for t in &totals {
        let of_currency = entries.iter().filter(|e| e.expense.currency == t.currency);
        let pending = of_currency.clone().filter(|e| e.status != Status::Paid).count();
        let overdue = of_currency.filter(|e| e.status == Status::Overdue).count();
        println!(
            "{} {} / {} · {} pending, {} overdue",
            t.currency,
            format_cents(t.paid_cents),
            format_cents(t.due_cents),
            pending,
            overdue
        );
        // Indented under the total it belongs to, and subordinate to it: the
        // two figures above are exact, this one is derived and dated.
        if let (Some(approx), Some(primary), Some(rate)) =
            (fx::approx_for(rate, t, primary.as_deref()), primary.as_deref(), rate)
        {
            println!("    {}", rate.annotation(approx, primary, now));
        }
    }
}

// ---- JSON -------------------------------------------------------------------
//
// Hand-rolled rather than pulling serde in for two fixed shapes. The example in
// CONTRACT.md is the specification, and a test asserts against it.

fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

fn json_string(s: &str) -> String {
    format!("\"{}\"", esc(s))
}

fn json_opt_string(s: Option<&str>) -> String {
    s.map(json_string).unwrap_or_else(|| "null".to_string())
}

fn json_opt_int(n: Option<i64>) -> String {
    n.map(|n| n.to_string()).unwrap_or_else(|| "null".to_string())
}

fn total_json(t: &Total, approx: Option<i64>) -> String {
    format!(
        "{{\"currency\":{},\"dueCents\":{},\"paidCents\":{},\"approxCents\":{}}}",
        json_string(&t.currency),
        t.due_cents,
        t.paid_cents,
        json_opt_int(approx)
    )
}

/// The rate itself travels alongside the converted figures, so a surface can
/// show what it was without recomputing anything. `stale` is derived here
/// rather than read off the rate, for the same reason an expense's status is.
fn fx_json(rate: &Rate, now: NaiveDateTime) -> String {
    format!(
        "{{\"casa\":{},\"base\":{},\"quote\":{},\"rateCentavos\":{},\"fetchedAt\":{},\
         \"sourceUpdatedAt\":{},\"stale\":{}}}",
        json_string(&rate.casa),
        json_string(&rate.base),
        json_string(&rate.quote),
        rate.rate_centavos,
        json_string(&rate.fetched_at),
        json_opt_string(rate.source_updated_at.as_deref()),
        !rate.is_current(now)
    )
}

fn entry_json(e: &Entry) -> String {
    format!(
        "{{\"id\":{},\"name\":{},\"category\":{},\"currency\":{},\"amountCents\":{},\
         \"paidCents\":{},\"dueDate\":\"{}\",\"status\":{}}}",
        e.expense.id,
        json_string(&e.expense.name),
        json_opt_string(e.expense.category.as_deref()),
        json_string(&e.expense.currency),
        e.expense.amount_cents,
        e.paid_cents.map(|c| c.to_string()).unwrap_or_else(|| "null".to_string()),
        e.due_date,
        json_string(e.status.as_str())
    )
}

/// Field-stable: every key is present even at zero, so a caller never has to
/// distinguish "absent" from "none". `fx` is `null` rather than missing when
/// there is no rate, for the same reason.
pub fn json(
    entries: &[Entry],
    period: Period,
    today: NaiveDate,
    now: NaiveDateTime,
    with_items: bool,
    rate: Option<&Rate>,
) -> String {
    let totals = totals(entries);
    let primary = primary_currency(entries);
    let pending = entries.iter().filter(|e| e.status != Status::Paid).count();
    let overdue = entries.iter().filter(|e| e.status == Status::Overdue).count();
    let totals_json = totals
        .iter()
        .map(|t| total_json(t, fx::approx_for(rate, t, primary.as_deref())))
        .collect::<Vec<_>>()
        .join(",");
    let mut out = format!(
        "{{\"period\":\"{period}\",\"today\":\"{today}\",\"pending\":{pending},\
         \"overdue\":{overdue},\"primaryCurrency\":{},\"fx\":{},\"totals\":[{totals_json}]",
        json_opt_string(primary.as_deref()),
        rate.map(|r| fx_json(r, now)).unwrap_or_else(|| "null".to_string())
    );
    if with_items {
        let items = entries.iter().map(entry_json).collect::<Vec<_>>().join(",");
        out.push_str(&format!(",\"items\":[{items}]"));
    }
    out.push('}');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{DATE_FMT, Expense};

    fn entry() -> Entry {
        Entry {
            expense: Expense {
                id: 1,
                name: "Rent".into(),
                amount_cents: 9_000_000,
                currency: "ARS".into(),
                due_day: 5,
                category: Some("home".into()),
                active: true,
                sort_order: 0,
                created_at: "2026-08-01T10:00:00".into(),
            },
            paid_cents: Some(9_000_000),
            due_date: NaiveDate::from_ymd_opt(2026, 8, 5).unwrap(),
            status: Status::Paid,
        }
    }

    fn usd_entry() -> Entry {
        let mut e = entry();
        e.expense.id = 2;
        e.expense.name = "Claude".into();
        e.expense.currency = "USD".into();
        e.expense.amount_cents = 114_000;
        e.paid_cents = None;
        e.status = Status::Due;
        e
    }

    fn rate() -> Rate {
        Rate {
            casa: "blue".into(),
            base: "USD".into(),
            quote: "ARS".into(),
            rate_centavos: 155_000,
            fetched_at: "2026-08-23T18:00:00".into(),
            source_updated_at: Some("2026-08-23T21:00:00.000Z".into()),
            ttl_secs: 3600,
        }
    }

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 8, 22).unwrap()
    }

    fn now() -> NaiveDateTime {
        NaiveDateTime::parse_from_str("2026-08-23T18:04:00", DATE_FMT).unwrap()
    }

    /// Past the rate's one-hour TTL.
    fn much_later() -> NaiveDateTime {
        NaiveDateTime::parse_from_str("2026-08-23T21:00:00", DATE_FMT).unwrap()
    }

    #[test]
    fn item_json_matches_the_contract_example() {
        assert_eq!(
            entry_json(&entry()),
            "{\"id\":1,\"name\":\"Rent\",\"category\":\"home\",\"currency\":\"ARS\",\
             \"amountCents\":9000000,\"paidCents\":9000000,\"dueDate\":\"2026-08-05\",\
             \"status\":\"paid\"}"
        );
    }

    #[test]
    fn status_json_omits_items_and_keeps_every_other_key() {
        let out = json(&[entry()], Period::parse("2026-08").unwrap(), today(), now(), false, None);
        assert_eq!(
            out,
            "{\"period\":\"2026-08\",\"today\":\"2026-08-22\",\"pending\":0,\"overdue\":0,\
             \"primaryCurrency\":\"ARS\",\"fx\":null,\"totals\":[{\"currency\":\"ARS\",\
             \"dueCents\":9000000,\"paidCents\":9000000,\"approxCents\":null}]}"
        );
    }

    /// An empty database is an answer, not a special case: same keys, zeroes.
    #[test]
    fn an_empty_period_still_produces_every_key() {
        let out = json(&[], Period::parse("2026-08").unwrap(), today(), now(), true, None);
        assert_eq!(
            out,
            "{\"period\":\"2026-08\",\"today\":\"2026-08-22\",\"pending\":0,\"overdue\":0,\
             \"primaryCurrency\":null,\"fx\":null,\"totals\":[],\"items\":[]}"
        );
    }

    #[test]
    fn a_missing_category_is_null_not_an_empty_string() {
        let mut e = entry();
        e.expense.category = None;
        e.paid_cents = None;
        e.status = Status::Due;
        assert!(entry_json(&e).contains("\"category\":null"));
        assert!(entry_json(&e).contains("\"paidCents\":null"));
    }

    #[test]
    fn quotes_in_a_name_are_escaped() {
        let mut e = entry();
        e.expense.name = "say \"hi\"".into();
        assert!(entry_json(&e).contains("\"name\":\"say \\\"hi\\\"\""));
    }

    // ---- fx ----------------------------------------------------------------

    #[test]
    fn fx_json_carries_the_rate_that_produced_the_conversion() {
        assert_eq!(
            fx_json(&rate(), now()),
            "{\"casa\":\"blue\",\"base\":\"USD\",\"quote\":\"ARS\",\"rateCentavos\":155000,\
             \"fetchedAt\":\"2026-08-23T18:00:00\",\
             \"sourceUpdatedAt\":\"2026-08-23T21:00:00.000Z\",\"stale\":false}"
        );
    }

    /// The primary currency is already the number it converts to; annotating it
    /// with itself would be noise.
    #[test]
    fn only_the_non_primary_total_gets_an_approximation() {
        let entries = [entry(), usd_entry()];
        let out = json(&entries, Period::parse("2026-08").unwrap(), today(), now(), false, Some(&rate()));
        assert!(out.contains(
            "{\"currency\":\"ARS\",\"dueCents\":9000000,\"paidCents\":9000000,\"approxCents\":null}"
        ));
        assert!(out.contains(
            "{\"currency\":\"USD\",\"dueCents\":114000,\"paidCents\":0,\"approxCents\":176700000}"
        ));
    }

    #[test]
    fn without_a_rate_every_approximation_is_null() {
        let entries = [entry(), usd_entry()];
        let out = json(&entries, Period::parse("2026-08").unwrap(), today(), now(), false, None);
        assert!(out.contains("\"fx\":null"));
        assert_eq!(out.matches("\"approxCents\":null").count(), 2);
    }

    /// A rate held past its TTL is still worth showing, but only with its age
    /// attached — and `stale` in the payload has to agree with the text.
    #[test]
    fn an_aged_rate_says_so_in_both_the_text_and_the_json() {
        let entries = [entry(), usd_entry()];
        let period = Period::parse("2026-08").unwrap();
        assert!(json(&entries, period, today(), now(), false, Some(&rate())).contains("\"stale\":false"));
        assert!(
            json(&entries, period, today(), much_later(), false, Some(&rate()))
                .contains("\"stale\":true")
        );
    }
}
