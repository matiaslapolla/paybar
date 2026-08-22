use chrono::NaiveDate;

use crate::db::{Entry, Status, Total, primary_currency, totals};
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
pub fn print_status(entries: &[Entry], period: Period) {
    let totals = totals(entries);
    if totals.is_empty() {
        println!("{period}\tnothing due");
        return;
    }
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

fn total_json(t: &Total) -> String {
    format!(
        "{{\"currency\":{},\"dueCents\":{},\"paidCents\":{}}}",
        json_string(&t.currency),
        t.due_cents,
        t.paid_cents
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
/// distinguish "absent" from "none".
pub fn json(entries: &[Entry], period: Period, today: NaiveDate, with_items: bool) -> String {
    let totals = totals(entries);
    let pending = entries.iter().filter(|e| e.status != Status::Paid).count();
    let overdue = entries.iter().filter(|e| e.status == Status::Overdue).count();
    let totals_json = totals.iter().map(total_json).collect::<Vec<_>>().join(",");
    let mut out = format!(
        "{{\"period\":\"{period}\",\"today\":\"{today}\",\"pending\":{pending},\
         \"overdue\":{overdue},\"primaryCurrency\":{},\"totals\":[{totals_json}]",
        json_opt_string(primary_currency(entries).as_deref())
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
    use crate::db::Expense;

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

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 8, 22).unwrap()
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
        let out = json(&[entry()], Period::parse("2026-08").unwrap(), today(), false);
        assert_eq!(
            out,
            "{\"period\":\"2026-08\",\"today\":\"2026-08-22\",\"pending\":0,\"overdue\":0,\
             \"primaryCurrency\":\"ARS\",\"totals\":[{\"currency\":\"ARS\",\
             \"dueCents\":9000000,\"paidCents\":9000000}]}"
        );
    }

    /// An empty database is an answer, not a special case: same keys, zeroes.
    #[test]
    fn an_empty_period_still_produces_every_key() {
        let out = json(&[], Period::parse("2026-08").unwrap(), today(), true);
        assert_eq!(
            out,
            "{\"period\":\"2026-08\",\"today\":\"2026-08-22\",\"pending\":0,\"overdue\":0,\
             \"primaryCurrency\":null,\"totals\":[],\"items\":[]}"
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
}
