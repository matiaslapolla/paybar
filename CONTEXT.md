# paybar — domain model

Fixed monthly expenses, three surfaces, one SQLite file. Sibling project to
[`tobar`](https://github.com/matiaslapolla/tobar); same shape, same conventions.

## Vocabulary

**Expense** — a recurring fixed charge the user owes every month: rent, a
subscription, a loan instalment. It has a `name`, an `amount`, a `due_day`
(day of the month it falls on) and an optional `category`. An expense is
`active` or archived; archiving keeps its history without carrying it into
future periods.

**Period** — one calendar month, written `YYYY-MM`. Every question paybar
answers is scoped to a period: "what do I owe in 2026-08", "what did I pay in
2026-07". The *current period* is the month of today's local date.

**Payment** — the record that one expense was settled for one period. At most
one payment per (expense, period); paying twice is a no-op, not a duplicate.
The paid amount is recorded separately from the expense amount, because the
real charge often differs from the expected one (FX, rate changes).

**Status** — an expense's state *within a period*, derived, never stored:
- `paid` — a payment exists for that period.
- `overdue` — no payment, and the period's due date is in the past.
- `due` — no payment, and the due date has not arrived yet.

`overdue` only exists for the current and past periods; a future period is
always `due` or `paid`.

**Due date** — `due_day` resolved against a period's length. `due_day = 31` in
February resolves to the 28th/29th: the day is *clamped*, never rolled into the
next month. An expense is never silently skipped.

**Amount** — stored as an integer number of **cents**, never a float. Rendered
with a currency code. Each expense carries its own `currency`, and totals are
always **grouped by currency**. Two currencies means two totals, never one
blended number — see [ADR 002](docs/adr/002-fx-is-an-annotation.md).

**Primary currency** — the currency whose total the bar widget shows when
space allows only one. Defaults to the currency of the largest number of
active expenses. It is also what an annotation converts *to*.

**Casa** — which of the several simultaneous dollar prices a rate is read at:
`oficial`, `blue`, `bolsa`, `contadoconliqui`, `cripto`, `mayorista`,
`tarjeta`. They differ by tens of percent, so a converted figure is meaningless
without the casa attached to it.

**Rate** — the `venta` price of one unit of USD in ARS centavos, at one casa,
read from dolarapi and cached with the time it was read. A rate is never
applied to an expense or a payment; it exists to produce an **annotation**.

**Annotation** — a derived, approximate, dated figure printed *next to* a
total: what that total is worth in the primary currency. It is subordinate by
construction — marked `≈`, always carrying its casa and rate, and admitting its
age once stale. It decorates one total; it never merges two.

## Surfaces

- `core/` — Rust binary `paybar`: CLI verbs + Ratatui TUI. Owns the schema.
- `shell/` — Omarchy shell plugin: bar widget + popup panel (Quickshell QML).
- `CONTRACT.md` — the schema and command surface both sides conform to.

QML surfaces never open the database. They shell out to `paybar ... --json`,
so parsing, clamping, and status derivation live in exactly one place.
