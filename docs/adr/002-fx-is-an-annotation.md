# ADR 002 — An exchange rate annotates a total, it never blends one

Status: accepted · 2026-08-23
Supersedes: the "no exchange rates anywhere" absolute in Spec 001

## Context

paybar shipped with a hard rule, stated in `CONTRACT.md`, `CONTEXT.md`, the
README and a test named `totals_are_grouped_by_currency_never_blended`:

> Totals are grouped by currency. paybar never converts between currencies.

The rule was written against a specific failure: budgeting apps that add
`ARS 585,196` to `USD 1,140` at some rate they picked, print one number, and
leave the user unable to say what it means or when it was true. That failure is
real and the rule prevented it.

But a month that is 60% USD by value renders as two totals with no relationship
between them, and the user pays rent in one of the two. Refusing to relate them
is not neutrality — it just moves the arithmetic to the user's head, where it
happens at a rate they half-remember.

There is also no such thing as *the* USD/ARS rate here. On 2026-08-23 dolarapi
reported `oficial` 1520, `blue` 1550, `bolsa` 1545.3, `CCL` 1589.7, `cripto`
1586.08 and `tarjeta` 1976. Any single number printed without saying which one
it is, and when it was read, is the failure the original rule was protecting
against.

## Decision

The grouping invariant stays. It is not weakened, flagged, or made optional.

A rate is rendered as an **annotation**: a second, visually subordinate line
attached to a non-primary total, always carrying its casa and its value, and
its age when stale.

```
USD 1,140.00 / 1,140.00 · 0 pending, 0 overdue
                          ≈ ARS 1,767,000.00 @ blue 1,550.00
```

Concretely:

- No surface ever prints a single total spanning currencies. There is no
  `TOTAL` row, and no `--blend` flag to add one later.
- No converted amount is written to `expenses` or `payments`. The only thing
  persisted is the rate itself, in its own table, as a cache.
- Every rendered conversion names its casa and its rate. A number the user
  cannot attribute is not shown.
- One casa, globally configured. Per-expense rates are Spec 002's explicit
  non-goal.

## Consequences

**Good.** The question "how much is this month" gets an answer, and the answer
is auditable at a glance — casa, rate, staleness. The `≈` and the indentation
make the epistemic status visible: this is derived, approximate, and dated,
unlike the two totals above it which are exact.

**Good.** Because nothing converted is stored, deleting `fx_rates` or setting
`PAYBAR_FX=off` returns the program to its previous behaviour byte for byte.
The feature is removable.

**Cost.** paybar now has a network dependency, in a program that had none. It
is contained: lazy, TTL-gated, 2.5 s timeout, skipped entirely for
single-currency months, and unable to fail a command — a fetch that fails falls
back to cache, and a cache that is empty falls back to no annotation. But
`open()` is no longer purely local, and that is a real change in the program's
character.

**Cost.** The default casa is a judgement call baked into the binary. `blue`
is wrong for card subscriptions, which clear near `tarjeta` — roughly 27%
higher. The annotation is approximate by construction and says which rate it
used, which is the honest version of a number that cannot be exactly right.

## Alternatives rejected

**A blended `TOTAL` line.** The thing the original rule existed to prevent.
It produces a number no counterparty will ever charge, and it hides the two
figures that are actually true behind one that is not.

**A `--blend` flag offering both.** Two contracts, two renderings per surface,
and a stated invariant that is really a default. An invariant with a flag is
not an invariant.

**Keeping the absolute ban.** Defensible, and it was the status quo for a
reason. Rejected because the ban was on *blending*, and it had been written
down as a ban on *relating* — a stronger rule than the problem required.

**A bundled rate table, or a manual `paybar fx set <rate>`.** No network, and
therefore no staleness to manage. Rejected because a manually entered rate goes
stale silently, which is worse than an annotation that admits its age; and in
this economy a table shipped in the binary is wrong within days.

**Per-expense casas.** More accurate and materially more machinery — a schema
column, a migration, flags on `add`/`edit`, a per-row rate in three surfaces —
in service of a figure that is labelled approximate. Left in Spec 002 as a
revisit-if-it-hurts.
