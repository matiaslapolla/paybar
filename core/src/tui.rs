use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use chrono::{Local, NaiveDate};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, List, ListItem, ListState, Paragraph},
};
use rusqlite::Connection;

use crate::db::{self, ActiveChange, CategoryChange, Edit, Entry, Status, View};
use crate::fx::{self, Rate};
use crate::money::{format_cents, parse_cents};
use crate::period::Period;

// Only ANSI indexed colors are used anywhere in this module. The terminal
// already carries the active Omarchy theme, so a themed palette costs nothing
// and `omarchy theme set` re-themes the TUI with no code change. A test below
// enforces the rule against this file's own source.
const DIM: Style = Style::new().fg(Color::DarkGray);
const PAID: Style = Style::new().fg(Color::Green);
const OVERDUE: Style = Style::new().fg(Color::Red);
/// One step below DIM, for the annotation: the totals it sits beside are
/// exact, and it is derived, approximate and dated.
const FAINT: Style = Style::new().fg(Color::DarkGray).add_modifier(Modifier::DIM);

pub enum Mode {
    Normal,
    /// Inline single-line prompt. `editing` is Some(id) when rewriting an
    /// existing expense, None when adding.
    Input { buf: String, editing: Option<i64> },
    Confirm(i64),
}

pub enum Action {
    Add(String),
    Rewrite(i64, String),
    TogglePaid(i64),
    ToggleArchive(i64),
    Delete(i64),
}

/// The four fields an expense needs, parsed from one line.
#[derive(Debug, PartialEq)]
pub struct Spec {
    pub name: String,
    pub amount_cents: i64,
    pub due_day: u32,
    pub category: Option<String>,
}

/// Parse `<name> <amount> <day> [category]` from the end, so a name may
/// contain spaces without any quoting: "Internet fibre 8500 10" works.
///
/// A trailing non-numeric token is the category; the two numbers before it are
/// the day and the amount; whatever remains in front is the name.
pub fn parse_spec(line: &str) -> Result<Spec> {
    let mut tokens: Vec<&str> = line.split_whitespace().collect();
    if tokens.len() < 3 {
        bail!("need at least: <name> <amount> <day>");
    }
    let category = if tokens
        .last()
        .is_some_and(|t| t.parse::<u32>().is_err() && parse_cents(t).is_err())
    {
        tokens.pop().map(|c| c.to_string())
    } else {
        None
    };
    if tokens.len() < 3 {
        bail!("need at least: <name> <amount> <day>");
    }
    let day: u32 = tokens
        .pop()
        .expect("checked length")
        .parse()
        .map_err(|_| anyhow::anyhow!("day must be a number between 1 and 31"))?;
    if !(1..=31).contains(&day) {
        bail!("day must be between 1 and 31, got {day}");
    }
    let amount_cents = parse_cents(tokens.pop().expect("checked length"))?;
    let name = tokens.join(" ");
    if name.trim().is_empty() {
        bail!("an expense needs a name");
    }
    Ok(Spec { name, amount_cents, due_day: day, category })
}

/// The inverse, for prefilling the edit prompt.
pub fn spec_line(entry: &Entry) -> String {
    let mut line = format!(
        "{} {} {}",
        entry.expense.name,
        format_cents(entry.expense.amount_cents).replace(',', ""),
        entry.expense.due_day
    );
    if let Some(c) = &entry.expense.category {
        line.push(' ');
        line.push_str(c);
    }
    line
}

pub struct App {
    pub entries: Vec<Entry>,
    pub period: Period,
    pub today: NaiveDate,
    pub show_archived: bool,
    pub sel: usize,
    pub mode: Mode,
    pub quit: bool,
    /// Set when a prompt could not be parsed, cleared on the next keypress.
    pub error: Option<String>,
    /// Resolved at launch, on `r`, and once when a month first turns out to
    /// need one — never on the idle tick, because a fetch blocks and a redraw
    /// is not the place for one.
    pub rate: Option<Rate>,
    /// The user pressed `r`. Named for the request, not for the rate: whether
    /// the rate itself is stale is derived from its age.
    pub refresh_requested: bool,
    /// An attempt already failed. Only `r` retries, so stepping through months
    /// offline cannot stall on every keypress.
    pub rate_unavailable: bool,
}

impl App {
    pub fn new(today: NaiveDate) -> Self {
        App {
            entries: Vec::new(),
            period: Period::of(today),
            today,
            show_archived: false,
            sel: 0,
            mode: Mode::Normal,
            quit: false,
            error: None,
            rate: None,
            refresh_requested: false,
            rate_unavailable: false,
        }
    }

    pub fn selected(&self) -> Option<&Entry> {
        self.entries.get(self.sel)
    }

    pub fn clamp_sel(&mut self) {
        self.sel = self.sel.min(self.entries.len().saturating_sub(1));
    }

    pub fn view(&self) -> View {
        View { include_archived: self.show_archived, only_pending: false, only_paid: false }
    }

    /// Pure state transition; returns a db action for the caller to apply.
    pub fn on_key(&mut self, key: KeyEvent) -> Option<Action> {
        self.error = None;
        match &mut self.mode {
            Mode::Normal => self.on_key_normal(key),
            Mode::Input { buf, editing } => match key.code {
                KeyCode::Esc => {
                    self.mode = Mode::Normal;
                    None
                }
                KeyCode::Enter => {
                    let text = buf.trim().to_string();
                    let editing = *editing;
                    if text.is_empty() {
                        self.mode = Mode::Normal;
                        return None;
                    }
                    // Validate before leaving the prompt, so a typo does not
                    // discard everything that was typed.
                    if let Err(e) = parse_spec(&text) {
                        self.error = Some(e.to_string());
                        return None;
                    }
                    self.mode = Mode::Normal;
                    Some(match editing {
                        Some(id) => Action::Rewrite(id, text),
                        None => Action::Add(text),
                    })
                }
                KeyCode::Backspace => {
                    buf.pop();
                    None
                }
                KeyCode::Char(c) => {
                    buf.push(c);
                    None
                }
                _ => None,
            },
            Mode::Confirm(id) => {
                let id = *id;
                match key.code {
                    KeyCode::Char('y') => {
                        self.mode = Mode::Normal;
                        Some(Action::Delete(id))
                    }
                    KeyCode::Char('n') | KeyCode::Esc => {
                        self.mode = Mode::Normal;
                        None
                    }
                    _ => None,
                }
            }
        }
    }

    fn on_key_normal(&mut self, key: KeyEvent) -> Option<Action> {
        match key.code {
            KeyCode::Char('q') => {
                self.quit = true;
                None
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.sel += 1;
                self.clamp_sel();
                None
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.sel = self.sel.saturating_sub(1);
                None
            }
            KeyCode::Char('h') | KeyCode::Left => {
                self.period = self.period.prev();
                self.sel = 0;
                None
            }
            KeyCode::Char('l') | KeyCode::Right => {
                self.period = self.period.next();
                self.sel = 0;
                None
            }
            KeyCode::Tab => {
                self.show_archived = !self.show_archived;
                self.clamp_sel();
                None
            }
            KeyCode::Char('a') => {
                self.mode = Mode::Input { buf: String::new(), editing: None };
                None
            }
            KeyCode::Char('e') => {
                let e = self.selected()?;
                self.mode = Mode::Input { buf: spec_line(e), editing: Some(e.expense.id) };
                None
            }
            KeyCode::Char(' ') | KeyCode::Char('p') => {
                Some(Action::TogglePaid(self.selected()?.expense.id))
            }
            KeyCode::Char('r') => {
                self.refresh_requested = true;
                None
            }
            KeyCode::Char('t') => Some(Action::ToggleArchive(self.selected()?.expense.id)),
            KeyCode::Char('x') => {
                self.mode = Mode::Confirm(self.selected()?.expense.id);
                None
            }
            _ => None,
        }
    }
}

pub fn apply(conn: &mut Connection, period: Period, action: Action) -> Result<()> {
    match action {
        Action::Add(line) => {
            let spec = parse_spec(&line)?;
            db::add_expense(
                conn,
                &spec.name,
                spec.amount_cents,
                &db::default_currency(),
                spec.due_day,
                spec.category.as_deref(),
            )?;
        }
        Action::Rewrite(id, line) => {
            let spec = parse_spec(&line)?;
            db::edit_expense(
                conn,
                id,
                Edit {
                    name: Some(&spec.name),
                    amount_cents: Some(spec.amount_cents),
                    currency: None,
                    due_day: Some(spec.due_day),
                    category: match &spec.category {
                        Some(c) => CategoryChange::Set(c.clone()),
                        None => CategoryChange::Clear,
                    },
                    active: ActiveChange::Keep,
                },
            )?;
        }
        Action::TogglePaid(id) => {
            let paid: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM payments WHERE expense_id = ?1 AND period = ?2)",
                rusqlite::params![id, period.to_string()],
                |r| r.get(0),
            )?;
            if paid {
                db::unpay(conn, id, period)?
            } else {
                db::pay(conn, id, period, None)?;
            }
        }
        Action::ToggleArchive(id) => {
            let expense = db::get_expense(conn, id)?;
            db::edit_expense(
                conn,
                id,
                Edit {
                    name: None,
                    amount_cents: None,
                    currency: None,
                    due_day: None,
                    category: CategoryChange::Keep,
                    active: if expense.active { ActiveChange::Archive } else { ActiveChange::Restore },
                },
            )?;
        }
        Action::Delete(id) => db::delete_expense(conn, id)?,
    }
    Ok(())
}

fn refresh(app: &mut App, conn: &Connection) -> Result<()> {
    app.entries = db::period_view(conn, app.period, app.today, &app.view())?;
    app.clamp_sel();
    Ok(())
}

fn progress_bar(paid: i64, due: i64, width: usize) -> String {
    if due <= 0 {
        return "\u{2591}".repeat(width);
    }
    let filled = ((paid.max(0) as f64 / due as f64) * width as f64).round() as usize;
    let filled = filled.min(width);
    format!("{}{}", "\u{2588}".repeat(filled), "\u{2591}".repeat(width - filled))
}

fn ui(f: &mut Frame, app: &App) {
    let [header_area, list_area, status_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(f.area());

    // ---- header: period, then one total per currency -----------------------
    let totals = db::totals(&app.entries);
    let pending = app.entries.iter().filter(|e| e.status != Status::Paid).count();
    let primary = db::primary_currency(&app.entries);
    let mut header = vec![Span::raw(format!(" {}", app.period.label()))];
    for t in &totals {
        header.push(Span::styled(
            format!(
                "    {} {} / {}",
                t.currency,
                format_cents(t.paid_cents),
                format_cents(t.due_cents)
            ),
            DIM,
        ));
        if let (Some(rate), Some(primary)) = (app.rate.as_ref(), primary.as_deref())
            && let Some(approx) = fx::approx_for(app.rate.as_ref(), t, Some(primary))
        {
            header.push(Span::styled(
                format!("  {}", rate.annotation(approx, primary, Local::now().naive_local())),
                FAINT,
            ));
        }
    }
    header.push(Span::styled(
        format!("    {pending} pending"),
        if app.entries.iter().any(|e| e.status == Status::Overdue) { OVERDUE } else { DIM },
    ));
    f.render_widget(Paragraph::new(Line::from(header)), header_area);

    // ---- expense rows ------------------------------------------------------
    let name_width = app
        .entries
        .iter()
        .map(|e| e.expense.name.chars().count())
        .max()
        .unwrap_or(0)
        .max(8);
    let items: Vec<ListItem> = app
        .entries
        .iter()
        .map(|e| {
            let style = match e.status {
                Status::Paid => PAID,
                Status::Overdue => OVERDUE,
                Status::Due => Style::new(),
            };
            let mut spans = vec![
                Span::styled(format!(" {} ", e.status.mark()), style),
                Span::styled(format!("{:02}  ", e.due_date.format("%d")), DIM),
                Span::raw(format!("{:width$}  ", e.expense.name, width = name_width)),
                Span::styled(
                    format!("{:10}", e.expense.category.as_deref().unwrap_or("")),
                    DIM,
                ),
                Span::raw(format!(
                    "{} {:>12}",
                    e.expense.currency,
                    format_cents(e.expense.amount_cents)
                )),
            ];
            if !e.expense.active {
                spans.push(Span::styled("  archived", DIM));
            }
            ListItem::new(Line::from(spans))
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::bordered()
                .border_style(DIM)
                .title(Span::styled(" paybar ", DIM)),
        )
        // REVERSED rather than a background colour: it inverts whatever the
        // terminal theme already is, so the selection is legible in every theme.
        .highlight_style(Style::new().add_modifier(Modifier::REVERSED));
    let mut state = ListState::default();
    if !app.entries.is_empty() {
        state.select(Some(app.sel));
    }
    f.render_stateful_widget(list, list_area, &mut state);

    // ---- status line -------------------------------------------------------
    let status: Line = match (&app.mode, &app.error) {
        (_, Some(err)) => Line::from(vec![
            Span::styled(" ! ", OVERDUE),
            Span::styled(err.clone(), OVERDUE),
        ]),
        (Mode::Input { buf, editing }, _) => Line::from(vec![
            Span::styled(
                if editing.is_some() { " edit: " } else { " add: " },
                DIM,
            ),
            Span::raw(buf.clone()),
            Span::styled("\u{2588}", DIM),
            Span::styled("   <name> <amount> <day> [category]", DIM),
        ]),
        (Mode::Confirm(_), _) => Line::from(vec![
            Span::raw(" delete "),
            Span::raw(
                app.selected()
                    .map(|e| format!("\"{}\"", e.expense.name))
                    .unwrap_or_default(),
            ),
            Span::styled("? y/n", DIM),
        ]),
        (Mode::Normal, _) => {
            let primary = totals.first();
            let bar = primary
                .map(|t| {
                    format!(
                        " [{}] {}%  ",
                        progress_bar(t.paid_cents, t.due_cents, 20),
                        if t.due_cents > 0 {
                            (t.paid_cents * 100 / t.due_cents).clamp(0, 100)
                        } else {
                            0
                        }
                    )
                })
                .unwrap_or_else(|| " ".to_string());
            Line::from(vec![
                Span::styled(bar, DIM),
                Span::styled(
                    "space pay \u{00b7} a add \u{00b7} e edit \u{00b7} t archive \u{00b7} x delete \u{00b7} h/l month \u{00b7} tab archived \u{00b7} r rate \u{00b7} q quit",
                    DIM,
                ),
            ])
        }
    };
    f.render_widget(Paragraph::new(status), status_area);
}

pub fn run() -> Result<()> {
    let mut conn = db::open()?;
    let mut app = App::new(Local::now().date_naive());
    refresh(&mut app, &conn)?;
    app.rate = fx::for_entries(&conn, &app.entries, false)?;

    let mut terminal = ratatui::init();
    let result = (|| -> Result<()> {
        let mut last_tick = Instant::now();
        while !app.quit {
            terminal.draw(|f| ui(f, &app))?;
            if event::poll(Duration::from_millis(250))? {
                if let Event::Key(key) = event::read()?
                    && key.kind == KeyEventKind::Press
                {
                    let period = app.period;
                    if let Some(action) = app.on_key(key)
                        && let Err(e) = apply(&mut conn, period, action)
                    {
                        app.error = Some(e.to_string());
                    }
                    refresh(&mut app, &conn)?;
                    // `r` retries whatever failed and says so; stepping into a
                    // month that turns out to hold two currencies resolves once
                    // and then gives up until asked again, so an offline
                    // session cannot stall on every keypress.
                    let stepped_into_a_mixed_month =
                        period != app.period && app.rate.is_none() && !app.rate_unavailable;
                    if app.refresh_requested || stepped_into_a_mixed_month {
                        match fx::for_entries(&conn, &app.entries, app.refresh_requested) {
                            Ok(rate) => {
                                app.rate_unavailable = rate.is_none();
                                if rate.is_some() {
                                    app.rate = rate;
                                }
                            }
                            Err(e) => {
                                app.rate_unavailable = true;
                                app.error = Some(e.to_string());
                            }
                        }
                        app.refresh_requested = false;
                    }
                    last_tick = Instant::now();
                }
            } else if last_tick.elapsed() >= Duration::from_secs(2) {
                refresh(&mut app, &conn)?;
                last_tick = Instant::now();
            }
        }
        Ok(())
    })();
    ratatui::restore();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 8, 22).unwrap()
    }

    fn temp_db() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open_at(&dir.path().join("expenses.db")).unwrap();
        (dir, conn)
    }

    fn app_with(conn: &Connection) -> App {
        let mut a = App::new(today());
        refresh(&mut a, conn).unwrap();
        a
    }

    // ---- parsing ----------------------------------------------------------

    #[test]
    fn parses_the_four_fields() {
        assert_eq!(
            parse_spec("Rent 90000 5 home").unwrap(),
            Spec {
                name: "Rent".into(),
                amount_cents: 9_000_000,
                due_day: 5,
                category: Some("home".into())
            }
        );
    }

    /// Parsing from the end is what lets a name contain spaces with no quoting.
    #[test]
    fn a_name_may_contain_spaces() {
        let s = parse_spec("Internet fibre 8500 10").unwrap();
        assert_eq!(s.name, "Internet fibre");
        assert_eq!(s.amount_cents, 850_000);
        assert_eq!(s.due_day, 10);
        assert_eq!(s.category, None);
    }

    #[test]
    fn rejects_lines_that_are_missing_a_field_or_out_of_range() {
        assert!(parse_spec("Rent 90000").is_err());
        assert!(parse_spec("Rent 90000 0").is_err());
        assert!(parse_spec("Rent 90000 32").is_err());
        assert!(parse_spec("90000 5 home").is_err());
        assert!(parse_spec("").is_err());
    }

    #[test]
    fn spec_line_round_trips_through_the_parser() {
        let (_dir, mut conn) = temp_db();
        db::add_expense(&mut conn, "Internet fibre", 850_000, "ARS", 10, Some("home")).unwrap();
        let entries = db::period_view(&conn, Period::of(today()), today(), &View::all()).unwrap();
        let line = spec_line(&entries[0]);
        let spec = parse_spec(&line).unwrap();
        assert_eq!(spec.name, "Internet fibre");
        assert_eq!(spec.amount_cents, 850_000);
        assert_eq!(spec.due_day, 10);
        assert_eq!(spec.category.as_deref(), Some("home"));
    }

    // ---- keys -------------------------------------------------------------

    #[test]
    fn add_flow_produces_an_action() {
        let (_dir, conn) = temp_db();
        let mut a = app_with(&conn);
        a.on_key(key(KeyCode::Char('a')));
        for c in "Rent 90000 5 home".chars() {
            assert!(a.on_key(key(KeyCode::Char(c))).is_none());
        }
        match a.on_key(key(KeyCode::Enter)) {
            Some(Action::Add(text)) => assert_eq!(text, "Rent 90000 5 home"),
            _ => panic!("expected an Add action"),
        }
    }

    /// A typo must not throw away everything that was typed.
    #[test]
    fn an_unparseable_line_keeps_the_prompt_open() {
        let (_dir, conn) = temp_db();
        let mut a = app_with(&conn);
        a.on_key(key(KeyCode::Char('a')));
        for c in "Rent 90000".chars() {
            a.on_key(key(KeyCode::Char(c)));
        }
        assert!(a.on_key(key(KeyCode::Enter)).is_none());
        assert!(a.error.is_some());
        assert!(matches!(a.mode, Mode::Input { .. }));
        // Correcting it and submitting works.
        for c in " 5".chars() {
            a.on_key(key(KeyCode::Char(c)));
        }
        assert!(matches!(a.on_key(key(KeyCode::Enter)), Some(Action::Add(_))));
    }

    #[test]
    fn empty_input_is_a_noop_and_esc_cancels() {
        let (_dir, conn) = temp_db();
        let mut a = app_with(&conn);
        a.on_key(key(KeyCode::Char('a')));
        assert!(a.on_key(key(KeyCode::Enter)).is_none());
        assert!(matches!(a.mode, Mode::Normal));
        a.on_key(key(KeyCode::Char('a')));
        a.on_key(key(KeyCode::Char('z')));
        assert!(a.on_key(key(KeyCode::Esc)).is_none());
        assert!(matches!(a.mode, Mode::Normal));
    }

    #[test]
    fn edit_prefills_the_current_values() {
        let (_dir, mut conn) = temp_db();
        db::add_expense(&mut conn, "Rent", 9_000_000, "ARS", 5, Some("home")).unwrap();
        let mut a = app_with(&conn);
        a.on_key(key(KeyCode::Char('e')));
        match &a.mode {
            Mode::Input { buf, editing } => {
                assert_eq!(buf, "Rent 90000.00 5 home");
                assert_eq!(*editing, Some(1));
            }
            _ => panic!("expected the input prompt"),
        }
    }

    #[test]
    fn month_stepping_moves_the_period_and_resets_the_cursor() {
        let (_dir, conn) = temp_db();
        let mut a = app_with(&conn);
        assert_eq!(a.period, Period::parse("2026-08").unwrap());
        a.on_key(key(KeyCode::Char('h')));
        assert_eq!(a.period, Period::parse("2026-07").unwrap());
        a.on_key(key(KeyCode::Char('l')));
        a.on_key(key(KeyCode::Char('l')));
        assert_eq!(a.period, Period::parse("2026-09").unwrap());
    }

    #[test]
    fn delete_needs_confirmation() {
        let (_dir, mut conn) = temp_db();
        db::add_expense(&mut conn, "Rent", 9_000_000, "ARS", 5, None).unwrap();
        let mut a = app_with(&conn);
        assert!(a.on_key(key(KeyCode::Char('x'))).is_none());
        assert!(a.on_key(key(KeyCode::Char('n'))).is_none());
        a.on_key(key(KeyCode::Char('x')));
        assert!(matches!(a.on_key(key(KeyCode::Char('y'))), Some(Action::Delete(1))));
    }

    #[test]
    fn keys_are_safe_on_an_empty_list() {
        let (_dir, conn) = temp_db();
        let mut a = app_with(&conn);
        for c in [' ', 'e', 'x', 't', 'j', 'k'] {
            assert!(a.on_key(key(KeyCode::Char(c))).is_none());
        }
        assert!(!a.quit);
        a.on_key(key(KeyCode::Char('q')));
        assert!(a.quit);
    }

    // ---- actions against the database -------------------------------------

    #[test]
    fn toggling_paid_is_scoped_to_the_shown_period() {
        let (_dir, mut conn) = temp_db();
        db::add_expense(&mut conn, "Rent", 9_000_000, "ARS", 5, None).unwrap();
        let mut a = app_with(&conn);

        let action = a.on_key(key(KeyCode::Char(' '))).unwrap();
        apply(&mut conn, a.period, action).unwrap();
        refresh(&mut a, &conn).unwrap();
        assert_eq!(a.entries[0].status, Status::Paid);

        // The previous month is untouched.
        a.on_key(key(KeyCode::Char('h')));
        refresh(&mut a, &conn).unwrap();
        assert_eq!(a.entries[0].status, Status::Overdue);

        // Toggling again in the original month clears it.
        a.on_key(key(KeyCode::Char('l')));
        refresh(&mut a, &conn).unwrap();
        let action = a.on_key(key(KeyCode::Char(' '))).unwrap();
        apply(&mut conn, a.period, action).unwrap();
        refresh(&mut a, &conn).unwrap();
        assert_eq!(a.entries[0].status, Status::Overdue);
    }

    #[test]
    fn archiving_removes_it_from_the_listing_until_tab() {
        let (_dir, mut conn) = temp_db();
        db::add_expense(&mut conn, "Gym", 1_760_000, "ARS", 20, None).unwrap();
        let mut a = app_with(&conn);
        let action = a.on_key(key(KeyCode::Char('t'))).unwrap();
        apply(&mut conn, a.period, action).unwrap();
        refresh(&mut a, &conn).unwrap();
        assert!(a.entries.is_empty());

        a.on_key(key(KeyCode::Tab));
        refresh(&mut a, &conn).unwrap();
        assert_eq!(a.entries.len(), 1);
        assert!(!a.entries[0].expense.active);
    }

    #[test]
    fn add_and_rewrite_reach_the_database() {
        let (_dir, mut conn) = temp_db();
        let mut a = app_with(&conn);
        apply(&mut conn, a.period, Action::Add("Rent 90000 5 home".into())).unwrap();
        refresh(&mut a, &conn).unwrap();
        assert_eq!(a.entries[0].expense.name, "Rent");
        assert_eq!(a.entries[0].expense.amount_cents, 9_000_000);

        apply(&mut conn, a.period, Action::Rewrite(1, "Rent 95000 8".into())).unwrap();
        refresh(&mut a, &conn).unwrap();
        assert_eq!(a.entries[0].expense.amount_cents, 9_500_000);
        assert_eq!(a.entries[0].expense.due_day, 8);
        // Dropping the category from the line clears it.
        assert_eq!(a.entries[0].expense.category, None);
    }

    // ---- presentation -----------------------------------------------------

    #[test]
    fn progress_bar_fills_proportionally() {
        assert_eq!(progress_bar(0, 100, 4), "░░░░");
        assert_eq!(progress_bar(100, 100, 4), "████");
        assert_eq!(progress_bar(50, 100, 4), "██░░");
        // Nothing due is an empty bar, not a divide by zero.
        assert_eq!(progress_bar(0, 0, 4), "░░░░");
        // Overpaying does not overflow the bar.
        assert_eq!(progress_bar(500, 100, 4), "████");
    }

    /// The rule that makes the TUI match whatever Omarchy theme is active:
    /// only ANSI indexed colors, so the terminal palette is the palette.
    #[test]
    fn no_truecolor_literals_in_this_module() {
        let src = include_str!("tui.rs");
        for needle in ["Color::Rgb", "Color::Indexed"] {
            let uses = src.matches(needle).count();
            let mentions_in_this_test = src.matches(&format!("\"{needle}\"")).count();
            assert_eq!(
                uses, mentions_in_this_test,
                "{needle} would pin the TUI to one palette instead of the terminal's"
            );
        }
    }
}
