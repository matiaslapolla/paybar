mod db;
mod fx;
mod money;
mod output;
mod period;
mod tui;

use anyhow::{Result, bail};
use chrono::Local;
use clap::{Parser, Subcommand};

use db::{ActiveChange, CategoryChange, Edit, View};
use money::{format_cents, parse_cents};
use output::{json, print_entries, print_status};
use period::Period;

const EXAMPLES: &str = "\
Examples:
  paybar                                     open the TUI
  paybar add Rent 90000 --day 5              a fixed charge due on the 5th
  paybar add Claude 20 --day 28 --currency usd --category work
  paybar ls                                  what is still pending this month
  paybar ls --all                            paid and pending together
  paybar ls --archived                       include expenses you retired
  paybar ls --period 2026-07                 look at another month
  paybar pay 1                               settle expense 1 for this month
  paybar pay 1 --amount 92500                record what was actually charged
  paybar unpay 1                             undo it
  paybar edit 1 --amount 95000               the rent went up
  paybar edit 1 --archive                    stop carrying it into new months
  paybar rm 4                                delete it and its history
  paybar status                              one line per currency
  paybar status --json                       what the bar widget reads

Amounts use `.` as the decimal separator; `,` and `_` are ignored as grouping.
Currency defaults to $PAYBAR_CURRENCY, itself defaulting to ARS.

Totals stay grouped by currency. When a month holds both USD and ARS, the
non-primary total is annotated with what it is worth at a quoted rate:
  PAYBAR_FX=off       turn the annotation off entirely
  PAYBAR_FX_CASA      oficial|blue|bolsa|contadoconliqui|cripto|mayorista|tarjeta (blue)
  PAYBAR_FX_TTL       seconds a fetched rate stays fresh (3600)";

#[derive(Parser)]
#[command(name = "paybar", about = "fixed monthly expenses", after_help = EXAMPLES)]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Add a fixed monthly expense
    Add {
        name: String,
        amount: String,
        /// Day of the month it falls due (1-31; clamped to shorter months)
        #[arg(long)]
        day: u32,
        #[arg(long)]
        category: Option<String>,
        #[arg(long)]
        currency: Option<String>,
    },
    /// List a period's expenses
    Ls {
        #[arg(long)]
        period: Option<String>,
        #[arg(long, conflicts_with_all = ["paid", "all"])]
        pending: bool,
        #[arg(long, conflicts_with = "all")]
        paid: bool,
        /// Paid and pending together
        #[arg(long)]
        all: bool,
        /// Also show archived expenses
        #[arg(long)]
        archived: bool,
        #[arg(long)]
        json: bool,
    },
    /// Record an expense as settled for a period
    Pay {
        id: i64,
        #[arg(long)]
        period: Option<String>,
        /// What was actually charged, when it differs from the expected amount
        #[arg(long)]
        amount: Option<String>,
    },
    /// Undo a payment
    Unpay {
        id: i64,
        #[arg(long)]
        period: Option<String>,
    },
    /// Change an expense
    Edit {
        id: i64,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        amount: Option<String>,
        #[arg(long)]
        day: Option<u32>,
        #[arg(long, conflicts_with = "no_category")]
        category: Option<String>,
        #[arg(long)]
        no_category: bool,
        #[arg(long)]
        currency: Option<String>,
        #[arg(long, conflicts_with = "restore")]
        archive: bool,
        #[arg(long)]
        restore: bool,
    },
    /// Delete an expense and its payment history
    Rm { id: i64 },
    /// One line per currency: paid, total, and what is still pending
    Status {
        #[arg(long)]
        period: Option<String>,
        #[arg(long)]
        json: bool,
    },
}

fn resolve_period(raw: Option<String>) -> Result<Period> {
    match raw {
        Some(s) => Period::parse(&s),
        None => Ok(Period::of(Local::now().date_naive())),
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let Some(cmd) = cli.cmd else {
        return tui::run();
    };
    let mut conn = db::open()?;
    let today = Local::now().date_naive();

    match cmd {
        Cmd::Add { name, amount, day, category, currency } => {
            let cents = parse_cents(&amount)?;
            let currency = currency.unwrap_or_else(db::default_currency);
            let id = db::add_expense(
                &mut conn,
                &name,
                cents,
                &currency,
                day,
                category.as_deref(),
            )?;
            let period = Period::of(today);
            println!("{}\t{}\t{} {}", id, period.day(day), currency.to_uppercase(), format_cents(cents));
        }
        Cmd::Ls { period, pending, paid, all, archived, json: as_json } => {
            let period = resolve_period(period)?;
            // Two orthogonal axes: which statuses to show, and whether
            // archived expenses count. --all widens the first, --archived the
            // second; conflating them made "everything" quietly mean
            // "everything including things I retired".
            let view = View {
                include_archived: archived,
                only_pending: pending || !(paid || all),
                only_paid: paid,
            };
            let entries = db::period_view(&conn, period, today, &view)?;
            if as_json {
                let rate = fx::for_entries(&conn, &entries, false)?;
                println!("{}", json(&entries, period, today, true, rate.as_ref()));
            } else {
                // The table has no totals line, so there is nothing to annotate
                // and no reason to reach for the network.
                print_entries(&entries);
            }
        }
        Cmd::Pay { id, period, amount } => {
            let period = resolve_period(period)?;
            let amount = amount.as_deref().map(parse_cents).transpose()?;
            let paid = db::pay(&conn, id, period, amount)?;
            let expense = db::get_expense(&conn, id)?;
            println!("{}\t{}\t{} {}", id, period, expense.currency, format_cents(paid));
        }
        Cmd::Unpay { id, period } => {
            let period = resolve_period(period)?;
            db::unpay(&conn, id, period)?;
        }
        Cmd::Edit {
            id, name, amount, day, category, no_category, currency, archive, restore,
        } => {
            let amount_cents = amount.as_deref().map(parse_cents).transpose()?;
            let category_change = match (&category, no_category) {
                (Some(c), _) => CategoryChange::Set(c.clone()),
                (None, true) => CategoryChange::Clear,
                (None, false) => CategoryChange::Keep,
            };
            let active_change = match (archive, restore) {
                (true, _) => ActiveChange::Archive,
                (_, true) => ActiveChange::Restore,
                _ => ActiveChange::Keep,
            };
            let nothing_to_do = name.is_none()
                && amount_cents.is_none()
                && day.is_none()
                && currency.is_none()
                && matches!(category_change, CategoryChange::Keep)
                && matches!(active_change, ActiveChange::Keep);
            if nothing_to_do {
                bail!("nothing to edit: pass --name, --amount, --day, --category, --no-category, --currency, --archive, or --restore");
            }
            db::edit_expense(
                &conn,
                id,
                Edit {
                    name: name.as_deref(),
                    amount_cents,
                    currency: currency.as_deref(),
                    due_day: day,
                    category: category_change,
                    active: active_change,
                },
            )?;
        }
        Cmd::Rm { id } => db::delete_expense(&conn, id)?,
        Cmd::Status { period, json: as_json } => {
            let period = resolve_period(period)?;
            let entries = db::period_view(&conn, period, today, &View::all())?;
            let rate = fx::for_entries(&conn, &entries, false)?;
            if as_json {
                println!("{}", json(&entries, period, today, false, rate.as_ref()));
            } else {
                print_status(&entries, period, rate.as_ref(), Local::now().naive_local());
            }
        }
    }
    Ok(())
}
