# Spec 001 — Fixed monthly expenses

Status: accepted · 2026-08-22

## Problem

Fixed monthly expenses (rent, subscriptions, instalments) are known in advance
and easy to forget. Existing tools are either full budgeting apps — too heavy,
they want every transaction — or a note that goes stale. What is actually
needed is a *checklist per month*: what falls due, what is already paid, how
much is left.

## Scope

In: the recurring charge set, per-month payment tracking, a TUI to manage it,
an Omarchy bar widget to see it without opening anything.

Out: variable spending, income, budgets, bank import, FX conversion,
multi-user, forecasting. If it needs a transaction ledger, it is not paybar.

## Surfaces

### 1. CLI

Defined in [`CONTRACT.md`](../../CONTRACT.md). It is the substrate: the TUI
calls the same functions, and the widget shells out to `--json`.

### 2. TUI (`paybar` with no args)

```
  August 2026                        ARS  75,000 / 120,000        3 pending

  x   05  Rent                home           ARS  90,000.00
  !   10  Internet            home           ARS   8,500.00
      15  Spotify             fun            ARS   3,900.00
      20  Gym                 health         ARS  17,600.00
      28  Claude              work           USD      20.00

  [███████████████░░░░░░░░░]  62%
```

Layout: a header line (period · totals per currency · pending count), the
expense list, a progress bar per primary currency, a one-line key hint.

Keys — deliberately the same muscle memory as tobar:

| Key | Action |
|---|---|
| `space` / `p` | toggle paid for the shown period |
| `a` | add expense |
| `e` | edit selected |
| `x` | delete selected |
| `t` | archive / restore selected |
| `h` / `l` | previous / next period |
| `Tab` | show archived |
| `j` / `k` | move selection |
| `q` | quit |

Adding and editing use an inline field prompt, not a modal form: one line at
the bottom of the screen, `Esc` cancels. Same interaction as tobar's add.

### 3. Omarchy bar widget

Plugin id `matiaslapolla.paybar`, kind `bar-widget`, installed to
`~/.config/omarchy/plugins/matiaslapolla.paybar/`.

Bar label: a nerd-font glyph plus the pending count, and — when the bar is
horizontal — the primary currency's outstanding amount. Nothing pending
renders the glyph alone in the dim foreground; anything overdue renders it in
`bar.urgent`.

Popup panel (left click): the current period's list, one row per expense with
due date, name, amount and a paid toggle; a total row; month stepping. Rows
are actionable — clicking one runs `paybar pay`/`unpay` and re-reads. This is
parity with the TUI minus add/edit/delete, which belong in the TUI.

## Visual style

The TUI matches Omarchy by **using only ANSI indexed colors** (0–15 plus
`reset`), never RGB literals. The terminal already carries the active Omarchy
theme, so the TUI re-themes itself for free when `omarchy theme set` runs.
Concretely: `paid` uses green, `overdue` uses red, the selection uses the
terminal's reverse video, and everything else is default foreground with
`dim` for secondary text.

The QML widget matches by composing `qs.Ui` components (`Panel`,
`PanelSectionHeader`, `PanelSeparator`, `PanelActionButton`) and reading
colors from `bar.foreground` / `bar.urgent` / `Color.accent`, never from
hardcoded hex.

## Acceptance

- `paybar add "Rent" 90000 --day 5` then `paybar ls` shows it as due.
- `paybar pay 1` then `paybar ls` shows it paid; `paybar ls --period <prev>`
  still shows it pending for the previous month.
- An expense with `--day 31` resolves to the 28th in February.
- `paybar status --json` is valid JSON with all keys present on an empty db.
- Two currencies produce two entries in `totals`, never a blended sum.
- `omarchy theme set gruvbox` re-themes the TUI on next launch with no code change.
