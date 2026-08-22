# paybar

Fixed monthly expenses, two surfaces, one SQLite file. Sibling to
[`tobar`](https://github.com/matiaslapolla/tobar).

- `core/` — Rust binary `paybar`: CLI verbs + Ratatui TUI (run `paybar` with no args).
- `shell/` — Omarchy bar widget (Quickshell QML): what is still unpaid, with a popup to settle it.
- `CONTEXT.md` — the domain model.
- `CONTRACT.md` — the schema and command surface both surfaces conform to.

Not a budgeting app. It answers one question — *what falls due this month and
what is left to pay* — and refuses the rest: no variable spending, no income,
no bank import, no FX conversion.

## Data

Single SQLite file at `~/.local/share/paybar/expenses.db` (override with
`PAYBAR_DB`), WAL mode. Open it from anywhere: `sqlite3 ~/.local/share/paybar/expenses.db`.

## Build & install

```sh
# CLI + TUI
cd core && cargo build --release && cp target/release/paybar ~/.local/bin/

# Bar widget
cd shell && ./install.sh
```

## CLI

```
paybar                                  # TUI
paybar add Rent 90000 --day 5 --category home
paybar add Claude 20 --day 28 --currency usd --category work
paybar ls [--period YYYY-MM] [--pending|--paid|--all] [--archived]
paybar pay <id> [--period YYYY-MM] [--amount <a>]
paybar unpay <id> [--period YYYY-MM]
paybar edit <id> [--name|--amount|--day|--category|--no-category|--currency|--archive|--restore]
paybar rm <id>
paybar status [--period YYYY-MM] [--json]
```

### Example session

```sh
$ paybar add Rent 90000 --day 5 --category home
1	2026-08-05	ARS 90,000.00        # id, resolved due date, amount

$ paybar add Claude 20 --day 28 --currency usd --category work
2	2026-08-28	USD 20.00

$ paybar ls                              # pending is the default
1	!	2026-08-05	Rent	home	ARS 90,000.00
2	 	2026-08-28	Claude	work	USD 20.00

$ paybar pay 1
1	2026-08	ARS 90,000.00

$ paybar status
ARS 90,000.00 / 90,000.00 · 0 pending, 0 overdue
USD 0.00 / 20.00 · 1 pending, 0 overdue

$ paybar ls --period 2026-07             # last month is untouched
1	!	2026-07-05	Rent	home	ARS 90,000.00
```

Columns are `id`, mark (`x` paid, `!` overdue, ` ` due), due date, name,
category, amount. `paybar --help` shows the same examples.

Amounts use `.` as the decimal separator; `,` and `_` are ignored as digit
grouping. More than two decimals is an error, not a rounding. Currency defaults
to `$PAYBAR_CURRENCY`, itself defaulting to `ARS`; totals are grouped by
currency and never converted between them.

A due day past the end of a month is clamped, never rolled forward: `--day 31`
falls on the 28th in February, so the charge stays in the month it belongs to.

### TUI keys

`space`/`p` pay · `a` add · `e` edit · `t` archive · `x` delete · `h`/`l` month · `Tab` archived · `j`/`k` move · `q` quit

The add and edit prompt takes one line: `<name> <amount> <day> [category]`. It
is parsed from the end, so a name may contain spaces without any quoting.

The TUI uses only ANSI indexed colors, so the terminal's palette is the
palette: `omarchy theme set <name>` re-themes it with no code change.
