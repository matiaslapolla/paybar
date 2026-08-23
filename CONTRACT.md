# paybar — shared contract (v1)

Both surfaces (Rust `core/`, QML `shell/`) MUST conform to this file. Changes
here require updating both.

## Database

- Path: `~/.local/share/paybar/expenses.db`, overridable via env `PAYBAR_DB`.
- Every connection sets: `PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000; PRAGMA foreign_keys=ON;`
- Writers use short transactions only.
- **Rust owns DDL/migrations** (via `PRAGMA user_version`). No other surface
  runs DDL; the QML plugin never opens the file at all.

## Schema (user_version = 2)

```sql
CREATE TABLE expenses (
  id           INTEGER PRIMARY KEY,
  name         TEXT NOT NULL,
  amount_cents INTEGER NOT NULL,            -- expected charge, in cents
  currency     TEXT NOT NULL,               -- ISO-4217-ish code, uppercase
  due_day      INTEGER NOT NULL,            -- 1..31, clamped to period length
  category     TEXT,
  active       INTEGER NOT NULL DEFAULT 1,  -- 0 = archived
  sort_order   INTEGER NOT NULL DEFAULT 0,
  created_at   TEXT NOT NULL                -- "YYYY-MM-DDTHH:MM:SS" local
);

CREATE TABLE payments (
  id           INTEGER PRIMARY KEY,
  expense_id   INTEGER NOT NULL REFERENCES expenses(id) ON DELETE CASCADE,
  period       TEXT NOT NULL,               -- "YYYY-MM"
  paid_at      TEXT NOT NULL,               -- "YYYY-MM-DDTHH:MM:SS" local
  amount_cents INTEGER NOT NULL,            -- what was actually paid
  UNIQUE(expense_id, period)
);

CREATE INDEX idx_payments_period ON payments(period);

-- v2. A cache, not a ledger: no converted amount is stored anywhere, and
-- dropping this table returns paybar to its v1 behaviour exactly.
CREATE TABLE fx_rates (
  casa              TEXT NOT NULL,               -- dolarapi casa slug
  base              TEXT NOT NULL,               -- "USD"
  quote             TEXT NOT NULL,               -- "ARS"
  rate_centavos     INTEGER NOT NULL,            -- centavos of quote per 1 base
  fetched_at        TEXT NOT NULL,               -- "YYYY-MM-DDTHH:MM:SS" local
  source_updated_at TEXT,                        -- passed through, never parsed
  PRIMARY KEY (casa, base, quote)
);
```

## Semantics

- Default listing order: `sort_order ASC, due_day ASC, id ASC`.
- `sort_order` for a new expense = `MAX(sort_order)+1` (or 0 when empty).
- Status is derived per period, never stored: `paid` when a payment row exists;
  otherwise `overdue` when the clamped due date is strictly before today and
  the period is not in the future; otherwise `due`.
- Due date = `min(due_day, days_in_month(period))`. Clamped, never rolled over.
- `pay` on an already-paid (expense, period) updates `paid_at`/`amount_cents`
  rather than inserting a second row.
- Archived (`active = 0`) expenses are excluded from every period listing and
  from totals, but their past payments survive.
- Totals are grouped by currency and never summed. This is an invariant, not a
  default: no surface prints a single total spanning currencies, and there is
  no flag that makes one.
- A rate may *annotate* a total — a subordinate figure saying what it is worth
  in the primary currency — and must always be rendered with the casa and rate
  that produced it. It never replaces a total and is never stored on one.

## CLI surface (`paybar`, built from `core/`)

```
paybar                                  # launches TUI
paybar add <name> <amount> --day <n> [--category <c>] [--currency <c>]
paybar ls [--period YYYY-MM] [--pending | --paid | --all] [--archived]
paybar pay <id> [--period YYYY-MM] [--amount <a>]
paybar unpay <id> [--period YYYY-MM]
paybar edit <id> [--name <n>] [--amount <a>] [--day <n>] [--category <c>|--no-category]
                 [--currency <c>] [--archive | --restore]
paybar rm <id>
paybar status [--period YYYY-MM]
```

- `<amount>` is a plain decimal using `.` as the decimal separator (`45900`,
  `12.99`). `_` and `,` are stripped as digit grouping. It is parsed to cents
  with exactly two decimal places; more precision is an error, not a rounding.
- `--currency` defaults to `$PAYBAR_CURRENCY`, itself defaulting to `ARS`.
- Conversion is configured by environment only: `PAYBAR_FX` (`off` disables),
  `PAYBAR_FX_CASA` (one of `oficial`, `blue`, `bolsa`, `contadoconliqui`,
  `cripto`, `mayorista`, `tarjeta`; default `blue`), `PAYBAR_FX_TTL` (seconds a
  fetched rate stays fresh; default 3600). An unknown casa is an error in any
  command that resolves a rate; a fetch that fails is not. `PAYBAR_FX=off`
  validates neither casa nor TTL, because nothing downstream will read them.
- `--period` defaults to the current period everywhere.
- `add` prints the created row id and the resolved due date.
- `ls` defaults to `--pending`. Columns: `id`, mark (`x` paid, `!` overdue,
  ` ` due), due date, name, category, amount.
- `--pending`/`--paid`/`--all` choose which statuses to show; `--archived` is
  an orthogonal flag that additionally includes retired expenses. Conflating
  the two would make "everything" quietly mean "everything including things I
  stopped paying".
- `status` prints one line per currency: `<currency> <paid> / <total> · <n> pending, <m> overdue`.

### `--json`

`ls` and `status` accept `--json` and emit a single JSON object on stdout.
This is the only interface the QML plugin uses.

```json
{
  "period": "2026-08",
  "today": "2026-08-22",
  "pending": 3,
  "overdue": 1,
  "primaryCurrency": "ARS",
  "fx": {
    "casa": "blue",
    "base": "USD",
    "quote": "ARS",
    "rateCentavos": 155000,
    "fetchedAt": "2026-08-23T18:04:11",
    "sourceUpdatedAt": "2026-08-23T21:00:00.000Z",
    "stale": false
  },
  "totals": [
    { "currency": "ARS", "dueCents": 12000000, "paidCents": 7500000,
      "approxCents": null }
  ],
  "items": [
    {
      "id": 1,
      "name": "Rent",
      "category": "home",
      "currency": "ARS",
      "amountCents": 9000000,
      "paidCents": 9000000,
      "dueDate": "2026-08-05",
      "status": "paid"
    }
  ]
}
```

- `status --json` omits `items`; `ls --json` includes them.
- `fx` is `null` — never absent — when conversion is disabled, when no rate is
  available, and when the period does not need one. `approxCents` is
  `dueCents` converted to `primaryCurrency`, and is `null` for the primary
  currency itself and whenever `fx` is `null`. Surfaces render it; they never
  compute it, so rounding has one implementation.
- `paidCents` has no converted counterpart, deliberately. It was paid at
  whatever rate applied that day; restating it at today's would not be an
  approximation but a wrong number.
- `stale` is derived at emit time from the rate's age against the TTL, not
  remembered from the fetch — the same rule as an expense's status. A rate held
  on screen goes stale where it sits.
- `status` is field-stable: keys are always present, even at zero.
- Exit code is 0 with an empty `items` array when nothing matches; a non-zero
  exit means the command failed, never "nothing to show".

## Cross-process refresh

- SQLite has no cross-process notification. The QML plugin re-runs
  `paybar status --json` on a configurable interval (default 60 s), on popup
  open, and after any mutation it triggers itself.
- The TUI re-queries on every user action.
- A rate refreshes as a side effect of those reads: a command that needs one
  fetches iff the cached copy aged past `PAYBAR_FX_TTL`. There is no separate
  schedule for it — it rides the refreshes above, so the widget's own poll and
  popup-open both carry it.
- The TUI resolves at launch, on `r`, and once when stepping into a month that
  turns out to need a rate — never on its idle tick, because a fetch blocks and
  a redraw must not. A failed attempt is not retried until `r`, so an offline
  session cannot stall on every keypress.
- A fetch never changes an exit code. On failure the cached rate is served with
  `"stale": true`; with no cached rate, `fx` is `null`. Non-zero still means
  the command failed, never "no rate".
