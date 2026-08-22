# Plan 001 — Fixed monthly expenses

Implements [spec 001](../specs/001-fixed-expenses.md). Branch: `feat/core`, then `feat/shell-widget`.

## Phase 1 — core data layer (`core/src/db.rs`, `core/src/money.rs`, `core/src/period.rs`)

1. `period.rs`: `Period` newtype over `(year, month)`, parse/format `YYYY-MM`,
   `days_in_month`, `clamp_day`, `prev`/`next`, `is_future`. Unit tests cover
   February, leap years, December→January rollover.
2. `money.rs`: parse a decimal string to `i64` cents (rejecting >2 decimals),
   format cents with grouping. Unit tests cover `45900`, `12.99`, `1_000`,
   `1,000`, and the `12.999` rejection.
3. `db.rs`: open/migrate (`user_version`), CRUD for expenses, `pay`/`unpay`
   with upsert semantics, and one `period_view(period)` query returning rows
   with derived status. Tests run against a `tempfile` db.

Gate: `cargo test` green.

## Phase 2 — CLI (`core/src/main.rs`, `core/src/output.rs`)

4. clap subcommands per CONTRACT.md, `after_help` examples mirroring tobar's.
5. `output.rs`: the table renderer and the JSON serializer. JSON shape is
   asserted by a test against the CONTRACT example.

Gate: `cargo test` green; manual `paybar add/ls/pay/status --json` round-trip.

## Phase 3 — TUI (`core/src/tui.rs`)

6. Ratatui app: list + header + progress bar + inline prompt. ANSI indexed
   colors only — a lint-style test greps the module for `Color::Rgb` and fails
   if present.
7. Keys per spec, period stepping, archived toggle.

Gate: `cargo test` green; launch and drive it by hand.

## Phase 4 — Omarchy plugin (`shell/`)

8. `manifest.json` (`matiaslapolla.paybar`, `bar-widget`, `defaultSection:
   right`, settings schema for `refreshIntervalSec` and `showAmount`).
9. `BarWidget.qml` — `Quickshell.Io.Process` running `paybar status --json`
   on the interval; label + urgent colouring.
10. `Panel.qml` — the popup list built from `paybar ls --json`, row clicks
    invoking `paybar pay|unpay` and re-reading.
11. `install.sh` — symlink `shell/` into `~/.config/omarchy/plugins/`, then
    `omarchy-shell shell rescanPlugins` and `omarchy plugin enable`.

Gate: widget renders in the bar, popup toggles a payment, theme switch keeps it legible.

## Phase 5 — packaging

12. `README.md` with build/install, mirroring tobar's.
13. Hyprland keybind suggestion for launching the TUI (documented, not installed).
