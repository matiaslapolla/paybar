use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use chrono::{Local, NaiveDate};
use rusqlite::{Connection, params};

use crate::period::Period;

pub const DATE_FMT: &str = "%Y-%m-%dT%H:%M:%S";

#[derive(Debug, Clone, PartialEq)]
pub struct Expense {
    pub id: i64,
    pub name: String,
    pub amount_cents: i64,
    pub currency: String,
    pub due_day: u32,
    pub category: Option<String>,
    pub active: bool,
    pub sort_order: i64,
    pub created_at: String,
}

/// An expense seen through one period: the same row, plus what happened to it
/// that month. Status is derived here and stored nowhere.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub expense: Expense,
    pub paid_cents: Option<i64>,
    pub due_date: NaiveDate,
    pub status: Status,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Paid,
    Overdue,
    Due,
}

impl Status {
    pub fn as_str(&self) -> &'static str {
        match self {
            Status::Paid => "paid",
            Status::Overdue => "overdue",
            Status::Due => "due",
        }
    }

    pub fn mark(&self) -> &'static str {
        match self {
            Status::Paid => "x",
            Status::Overdue => "!",
            Status::Due => " ",
        }
    }
}

impl Entry {
    /// What is still owed on this entry. A paid entry owes nothing even when
    /// the amount actually paid differed from the expected one.
    pub fn outstanding_cents(&self) -> i64 {
        match self.paid_cents {
            Some(_) => 0,
            None => self.expense.amount_cents,
        }
    }
}

pub fn default_currency() -> String {
    std::env::var("PAYBAR_CURRENCY")
        .ok()
        .filter(|c| !c.trim().is_empty())
        .map(|c| c.trim().to_uppercase())
        .unwrap_or_else(|| "ARS".to_string())
}

pub fn db_path() -> Result<PathBuf> {
    if let Some(p) = std::env::var_os("PAYBAR_DB") {
        return Ok(PathBuf::from(p));
    }
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home).join(".local/share/paybar/expenses.db"))
}

pub fn open() -> Result<Connection> {
    open_at(&db_path()?)
}

pub fn open_at(path: &Path) -> Result<Connection> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let conn = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.busy_timeout(Duration::from_millis(5000))?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    migrate(&conn)?;
    Ok(conn)
}

fn migrate(conn: &Connection) -> Result<()> {
    let mut version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version == 0 {
        conn.execute_batch(
            "BEGIN;
             CREATE TABLE expenses (
               id           INTEGER PRIMARY KEY,
               name         TEXT NOT NULL,
               amount_cents INTEGER NOT NULL,
               currency     TEXT NOT NULL,
               due_day      INTEGER NOT NULL,
               category     TEXT,
               active       INTEGER NOT NULL DEFAULT 1,
               sort_order   INTEGER NOT NULL DEFAULT 0,
               created_at   TEXT NOT NULL
             );
             CREATE TABLE payments (
               id           INTEGER PRIMARY KEY,
               expense_id   INTEGER NOT NULL REFERENCES expenses(id) ON DELETE CASCADE,
               period       TEXT NOT NULL,
               paid_at      TEXT NOT NULL,
               amount_cents INTEGER NOT NULL,
               UNIQUE(expense_id, period)
             );
             CREATE INDEX idx_payments_period ON payments(period);
             PRAGMA user_version = 1;
             COMMIT;",
        )?;
        version = 1;
    }
    if version == 1 {
        // A cache, not a ledger. Nothing converted is stored anywhere; dropping
        // this table returns paybar to its pre-FX behaviour exactly.
        conn.execute_batch(
            "BEGIN;
             CREATE TABLE fx_rates (
               casa              TEXT NOT NULL,
               base              TEXT NOT NULL,
               quote             TEXT NOT NULL,
               rate_centavos     INTEGER NOT NULL,
               fetched_at        TEXT NOT NULL,
               source_updated_at TEXT,
               PRIMARY KEY (casa, base, quote)
             );
             PRAGMA user_version = 2;
             COMMIT;",
        )?;
        version = 2;
    }
    debug_assert_eq!(version, 2);
    Ok(())
}

pub fn now_string() -> String {
    Local::now().format(DATE_FMT).to_string()
}

fn row_to_expense(row: &rusqlite::Row) -> rusqlite::Result<Expense> {
    Ok(Expense {
        id: row.get(0)?,
        name: row.get(1)?,
        amount_cents: row.get(2)?,
        currency: row.get(3)?,
        due_day: row.get::<_, i64>(4)? as u32,
        category: row.get(5)?,
        active: row.get::<_, i64>(6)? != 0,
        sort_order: row.get(7)?,
        created_at: row.get(8)?,
    })
}

const COLS: &str =
    "id, name, amount_cents, currency, due_day, category, active, sort_order, created_at";

pub fn add_expense(
    conn: &mut Connection,
    name: &str,
    amount_cents: i64,
    currency: &str,
    due_day: u32,
    category: Option<&str>,
) -> Result<i64> {
    if name.trim().is_empty() {
        bail!("an expense needs a name");
    }
    if !(1..=31).contains(&due_day) {
        bail!("due day must be between 1 and 31, got {due_day}");
    }
    let tx = conn.transaction()?;
    let sort_order: i64 = tx.query_row(
        "SELECT COALESCE(MAX(sort_order) + 1, 0) FROM expenses",
        [],
        |r| r.get(0),
    )?;
    tx.execute(
        "INSERT INTO expenses (name, amount_cents, currency, due_day, category, sort_order, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            name.trim(),
            amount_cents,
            currency.to_uppercase(),
            due_day as i64,
            category,
            sort_order,
            now_string()
        ],
    )?;
    let id = tx.last_insert_rowid();
    tx.commit()?;
    Ok(id)
}

pub fn get_expense(conn: &Connection, id: i64) -> Result<Expense> {
    let sql = format!("SELECT {COLS} FROM expenses WHERE id = ?1");
    conn.query_row(&sql, params![id], row_to_expense)
        .map_err(|_| anyhow::anyhow!("no expense with id {id}"))
}

pub fn delete_expense(conn: &Connection, id: i64) -> Result<()> {
    let n = conn.execute("DELETE FROM expenses WHERE id = ?1", params![id])?;
    if n == 0 {
        bail!("no expense with id {id}");
    }
    Ok(())
}

pub enum CategoryChange {
    Keep,
    Set(String),
    Clear,
}

pub enum ActiveChange {
    Keep,
    Archive,
    Restore,
}

pub struct Edit<'a> {
    pub name: Option<&'a str>,
    pub amount_cents: Option<i64>,
    pub currency: Option<&'a str>,
    pub due_day: Option<u32>,
    pub category: CategoryChange,
    pub active: ActiveChange,
}

pub fn edit_expense(conn: &Connection, id: i64, edit: Edit<'_>) -> Result<()> {
    let current = get_expense(conn, id)?;
    if let Some(day) = edit.due_day
        && !(1..=31).contains(&day)
    {
        bail!("due day must be between 1 and 31, got {day}");
    }
    let category = match edit.category {
        CategoryChange::Keep => current.category.clone(),
        CategoryChange::Set(c) => Some(c),
        CategoryChange::Clear => None,
    };
    let active = match edit.active {
        ActiveChange::Keep => current.active,
        ActiveChange::Archive => false,
        ActiveChange::Restore => true,
    };
    conn.execute(
        "UPDATE expenses SET name = ?1, amount_cents = ?2, currency = ?3, due_day = ?4,
         category = ?5, active = ?6 WHERE id = ?7",
        params![
            edit.name.unwrap_or(&current.name).trim(),
            edit.amount_cents.unwrap_or(current.amount_cents),
            edit.currency.unwrap_or(&current.currency).to_uppercase(),
            edit.due_day.unwrap_or(current.due_day) as i64,
            category,
            active as i64,
            id
        ],
    )?;
    Ok(())
}

/// Paying an already-paid (expense, period) updates the record rather than
/// inserting a second one: the question "was this month's rent paid" has one
/// answer, and a duplicate row would make it two.
pub fn pay(conn: &Connection, id: i64, period: Period, amount_cents: Option<i64>) -> Result<i64> {
    let expense = get_expense(conn, id)?;
    let amount = amount_cents.unwrap_or(expense.amount_cents);
    conn.execute(
        "INSERT INTO payments (expense_id, period, paid_at, amount_cents) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(expense_id, period)
         DO UPDATE SET paid_at = excluded.paid_at, amount_cents = excluded.amount_cents",
        params![id, period.to_string(), now_string(), amount],
    )?;
    Ok(amount)
}

pub fn unpay(conn: &Connection, id: i64, period: Period) -> Result<()> {
    get_expense(conn, id)?;
    conn.execute(
        "DELETE FROM payments WHERE expense_id = ?1 AND period = ?2",
        params![id, period.to_string()],
    )?;
    Ok(())
}

pub struct View {
    pub include_archived: bool,
    pub only_pending: bool,
    pub only_paid: bool,
}

impl View {
    pub fn all() -> Self {
        View { include_archived: false, only_pending: false, only_paid: false }
    }
}

/// Every expense in one period, with its payment and derived status.
///
/// `today` is passed in rather than read from the clock so status is a pure
/// function of its inputs and testable without freezing time.
pub fn period_view(
    conn: &Connection,
    period: Period,
    today: NaiveDate,
    view: &View,
) -> Result<Vec<Entry>> {
    let where_active = if view.include_archived { "1=1" } else { "e.active = 1" };
    let sql = format!(
        "SELECT e.id, e.name, e.amount_cents, e.currency, e.due_day, e.category, \
                e.active, e.sort_order, e.created_at, p.amount_cents \
         FROM expenses e \
         LEFT JOIN payments p ON p.expense_id = e.id AND p.period = ?1 \
         WHERE {where_active} \
         ORDER BY e.sort_order ASC, e.due_day ASC, e.id ASC"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(params![period.to_string()], |row| {
            let expense = row_to_expense(row)?;
            let paid_cents: Option<i64> = row.get(9)?;
            Ok((expense, paid_cents))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let entries = rows
        .into_iter()
        .map(|(expense, paid_cents)| {
            let due_date = period.day(expense.due_day);
            let status = if paid_cents.is_some() {
                Status::Paid
            } else if due_date < today {
                Status::Overdue
            } else {
                Status::Due
            };
            Entry { expense, paid_cents, due_date, status }
        })
        .filter(|e| {
            if view.only_pending {
                e.status != Status::Paid
            } else if view.only_paid {
                e.status == Status::Paid
            } else {
                true
            }
        })
        .collect();
    Ok(entries)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Total {
    pub currency: String,
    pub due_cents: i64,
    pub paid_cents: i64,
}

/// Totals grouped by currency. paybar does no FX conversion, so two currencies
/// produce two totals rather than one blended number that means nothing.
pub fn totals(entries: &[Entry]) -> Vec<Total> {
    let mut out: Vec<Total> = Vec::new();
    for e in entries {
        let slot = match out.iter_mut().find(|t| t.currency == e.expense.currency) {
            Some(t) => t,
            None => {
                out.push(Total {
                    currency: e.expense.currency.clone(),
                    due_cents: 0,
                    paid_cents: 0,
                });
                out.last_mut().expect("just pushed")
            }
        };
        slot.due_cents += e.expense.amount_cents;
        slot.paid_cents += e.paid_cents.unwrap_or(0);
    }
    out.sort_by(|a, b| a.currency.cmp(&b.currency));
    out
}

/// The currency the bar shows when it has room for only one: whichever covers
/// the most expenses, ties broken alphabetically so the answer is stable.
pub fn primary_currency(entries: &[Entry]) -> Option<String> {
    let mut counts: Vec<(String, usize)> = Vec::new();
    for e in entries {
        match counts.iter_mut().find(|(c, _)| *c == e.expense.currency) {
            Some((_, n)) => *n += 1,
            None => counts.push((e.expense.currency.clone(), 1)),
        }
    }
    counts.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    counts.into_iter().next().map(|(c, _)| c)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let conn = open_at(&dir.path().join("expenses.db")).unwrap();
        (dir, conn)
    }

    fn p(s: &str) -> Period {
        Period::parse(s).unwrap()
    }

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    fn seed(conn: &mut Connection) -> (i64, i64) {
        let rent = add_expense(conn, "Rent", 9_000_000, "ARS", 5, Some("home")).unwrap();
        let claude = add_expense(conn, "Claude", 2000, "usd", 28, Some("work")).unwrap();
        (rent, claude)
    }

    #[test]
    fn migration_sets_user_version_and_schema() {
        let (_dir, conn) = temp_db();
        let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, 2);
        let mode: String = conn.query_row("PRAGMA journal_mode", [], |r| r.get(0)).unwrap();
        assert_eq!(mode.to_lowercase(), "wal");
    }

    #[test]
    fn migration_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("expenses.db");
        drop(open_at(&path).unwrap());
        let conn = open_at(&path).unwrap();
        let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, 2);
    }

    #[test]
    fn currency_is_stored_uppercase() {
        let (_dir, mut conn) = temp_db();
        let (_, claude) = seed(&mut conn);
        assert_eq!(get_expense(&conn, claude).unwrap().currency, "USD");
    }

    #[test]
    fn add_rejects_a_nameless_expense_or_an_impossible_day() {
        let (_dir, mut conn) = temp_db();
        assert!(add_expense(&mut conn, "  ", 100, "ARS", 5, None).is_err());
        assert!(add_expense(&mut conn, "x", 100, "ARS", 0, None).is_err());
        assert!(add_expense(&mut conn, "x", 100, "ARS", 32, None).is_err());
    }

    /// Status is derived per period: the same expense is overdue in one month
    /// and merely due in the next, with nothing stored to say so.
    #[test]
    fn status_is_derived_from_the_period_and_today() {
        let (_dir, mut conn) = temp_db();
        seed(&mut conn);
        let today = d(2026, 8, 22);

        let v = period_view(&conn, p("2026-08"), today, &View::all()).unwrap();
        assert_eq!(v[0].status, Status::Overdue); // rent, due the 5th
        assert_eq!(v[1].status, Status::Due); // claude, due the 28th

        let future = period_view(&conn, p("2026-09"), today, &View::all()).unwrap();
        assert!(future.iter().all(|e| e.status == Status::Due));
    }

    #[test]
    fn paying_is_scoped_to_one_period() {
        let (_dir, mut conn) = temp_db();
        let (rent, _) = seed(&mut conn);
        pay(&conn, rent, p("2026-08"), None).unwrap();

        let aug = period_view(&conn, p("2026-08"), d(2026, 8, 22), &View::all()).unwrap();
        assert_eq!(aug[0].status, Status::Paid);
        assert_eq!(aug[0].paid_cents, Some(9_000_000));

        let jul = period_view(&conn, p("2026-07"), d(2026, 8, 22), &View::all()).unwrap();
        assert_eq!(jul[0].status, Status::Overdue);
        assert_eq!(jul[0].paid_cents, None);
    }

    #[test]
    fn paying_twice_updates_instead_of_duplicating() {
        let (_dir, mut conn) = temp_db();
        let (rent, _) = seed(&mut conn);
        pay(&conn, rent, p("2026-08"), None).unwrap();
        pay(&conn, rent, p("2026-08"), Some(9_500_000)).unwrap();
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM payments", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
        let v = period_view(&conn, p("2026-08"), d(2026, 8, 22), &View::all()).unwrap();
        assert_eq!(v[0].paid_cents, Some(9_500_000));
        // The expected amount is untouched by what was actually paid.
        assert_eq!(v[0].expense.amount_cents, 9_000_000);
    }

    #[test]
    fn unpay_is_idempotent_and_still_checks_the_id() {
        let (_dir, mut conn) = temp_db();
        let (rent, _) = seed(&mut conn);
        unpay(&conn, rent, p("2026-08")).unwrap();
        pay(&conn, rent, p("2026-08"), None).unwrap();
        unpay(&conn, rent, p("2026-08")).unwrap();
        assert!(unpay(&conn, 999, p("2026-08")).is_err());
    }

    #[test]
    fn a_due_day_past_the_month_end_is_clamped() {
        let (_dir, mut conn) = temp_db();
        let id = add_expense(&mut conn, "Loan", 100_000, "ARS", 31, None).unwrap();
        let v = period_view(&conn, p("2026-02"), d(2026, 2, 1), &View::all()).unwrap();
        assert_eq!(v.iter().find(|e| e.expense.id == id).unwrap().due_date, d(2026, 2, 28));
    }

    #[test]
    fn archived_expenses_leave_the_listing_but_keep_their_history() {
        let (_dir, mut conn) = temp_db();
        let (rent, _) = seed(&mut conn);
        pay(&conn, rent, p("2026-07"), None).unwrap();
        edit_expense(
            &conn,
            rent,
            Edit {
                name: None,
                amount_cents: None,
                currency: None,
                due_day: None,
                category: CategoryChange::Keep,
                active: ActiveChange::Archive,
            },
        )
        .unwrap();

        let v = period_view(&conn, p("2026-08"), d(2026, 8, 22), &View::all()).unwrap();
        assert!(v.iter().all(|e| e.expense.id != rent));
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM payments", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);

        let with_archived = period_view(
            &conn,
            p("2026-08"),
            d(2026, 8, 22),
            &View { include_archived: true, ..View::all() },
        )
        .unwrap();
        assert!(with_archived.iter().any(|e| e.expense.id == rent));
    }

    #[test]
    fn deleting_an_expense_takes_its_payments_with_it() {
        let (_dir, mut conn) = temp_db();
        let (rent, _) = seed(&mut conn);
        pay(&conn, rent, p("2026-08"), None).unwrap();
        delete_expense(&conn, rent).unwrap();
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM payments", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 0);
        assert!(delete_expense(&conn, rent).is_err());
    }

    #[test]
    fn pending_and_paid_views_partition_the_period() {
        let (_dir, mut conn) = temp_db();
        let (rent, _) = seed(&mut conn);
        pay(&conn, rent, p("2026-08"), None).unwrap();
        let today = d(2026, 8, 22);
        let pending = period_view(
            &conn,
            p("2026-08"),
            today,
            &View { only_pending: true, ..View::all() },
        )
        .unwrap();
        let paid = period_view(
            &conn,
            p("2026-08"),
            today,
            &View { only_paid: true, ..View::all() },
        )
        .unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(paid.len(), 1);
        assert_eq!(paid[0].expense.id, rent);
    }

    /// Two currencies are two totals. Blending them would produce a number
    /// that does not mean anything.
    #[test]
    fn totals_are_grouped_by_currency_never_blended() {
        let (_dir, mut conn) = temp_db();
        let (rent, _) = seed(&mut conn);
        pay(&conn, rent, p("2026-08"), None).unwrap();
        let v = period_view(&conn, p("2026-08"), d(2026, 8, 22), &View::all()).unwrap();
        assert_eq!(
            totals(&v),
            vec![
                Total { currency: "ARS".into(), due_cents: 9_000_000, paid_cents: 9_000_000 },
                Total { currency: "USD".into(), due_cents: 2000, paid_cents: 0 },
            ]
        );
    }

    #[test]
    fn primary_currency_is_the_most_common_one() {
        let (_dir, mut conn) = temp_db();
        seed(&mut conn);
        add_expense(&mut conn, "Internet", 850_000, "ARS", 10, None).unwrap();
        let v = period_view(&conn, p("2026-08"), d(2026, 8, 22), &View::all()).unwrap();
        assert_eq!(primary_currency(&v).as_deref(), Some("ARS"));
        assert_eq!(primary_currency(&[]), None);
    }

    #[test]
    fn outstanding_ignores_a_payment_that_differed_from_the_amount() {
        let (_dir, mut conn) = temp_db();
        let (rent, _) = seed(&mut conn);
        pay(&conn, rent, p("2026-08"), Some(1)).unwrap();
        let v = period_view(&conn, p("2026-08"), d(2026, 8, 22), &View::all()).unwrap();
        assert_eq!(v[0].outstanding_cents(), 0);
        assert_eq!(v[1].outstanding_cents(), 2000);
    }

    #[test]
    fn paybar_db_env_override() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("custom.db");
        unsafe { std::env::set_var("PAYBAR_DB", &path) };
        assert_eq!(db_path().unwrap(), path);
        unsafe { std::env::remove_var("PAYBAR_DB") };
    }
}
