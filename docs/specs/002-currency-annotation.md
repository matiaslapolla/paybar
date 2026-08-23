# Spec 002 — Currency annotation

Status: accepted · 2026-08-23

## Problem

Spec 001 put FX conversion out of scope, and every surface says so: totals are
grouped by currency and never summed. That rule is still right — a blended
number is not a number anyone owes — but it leaves a real question unanswered.
A month holding `ARS 585,196` and `USD 1,140` reads as two unrelated facts,
and the second one is the larger of the two in the only currency the rent is
paid in. "How much is this month, really" has no answer on screen.

## Scope

In: a single USD↔ARS rate, fetched from a public API, cached, and rendered as
an *annotation* next to a total that is already grouped.

Out: per-expense rates, historical rates, converting what was actually paid,
storing a converted amount, currencies beyond a single configured pair,
arbitrage between casas, and any arithmetic that produces one blended total.

## Decision summary

Recorded in [ADR 002](../adr/002-fx-is-an-annotation.md). In short: the
grouping invariant survives. A rate decorates a total; it never replaces one,
and nothing converted is ever written to `expenses` or `payments`.

## Source

[dolarapi.com](https://dolarapi.com) — public, keyless, free, no registration.
`GET https://dolarapi.com/v1/dolares/{casa}` returns:

```json
{"moneda":"USD","casa":"blue","nombre":"Blue","compra":1530,"venta":1550,
 "fechaActualizacion":"2026-08-23T21:00:00.000Z"}
```

**`venta` is the rate used.** Converting an obligation denominated in USD into
ARS asks what it costs to obtain those dollars, which is the sell side. Using
`compra` would flatter every total.

**`casa` is configurable and defaults to `blue`.** Argentina prices the dollar
in seven places at once, and on 2026-08-23 they spanned `oficial` 1520 to
`tarjeta` 1976 — a 30% spread. There is no neutral choice, so the choice is
the user's; `blue` is the default because it is the reference most people
carry in their head. The valid set is fixed at `oficial`, `blue`, `bolsa`,
`contadoconliqui`, `cripto`, `mayorista`, `tarjeta`; anything else is an error
at read time, not a silent fallback.

## Configuration

| Variable | Default | Meaning |
|---|---|---|
| `PAYBAR_FX` | `on` | `off` disables conversion everywhere |
| `PAYBAR_FX_CASA` | `blue` | which dolarapi casa to read |
| `PAYBAR_FX_TTL` | `3600` | seconds a cached rate stays fresh |

## Behaviour

**The rate is cached in the database** (`fx_rates`, schema v2) with the time it
was fetched and the timestamp the source itself reported. Nothing else in the
database learns about currencies.

**Refresh is lazy and demand-driven.** A rate is fetched only when a command
needs one and the cached copy is older than the TTL. It is needed only when a
period actually holds more than one currency — an all-ARS month never touches
the network. This is what makes "it updates when I open the widget" true
without a daemon: the widget refreshes on popup open, the TUI on launch, and
each refresh runs a command that fetches iff the cache went stale.

**The network is never load-bearing.** One 2.5 s global timeout covers
connect, send and receive together, so the worst case is bounded whatever
stalls. On
any failure — offline, DNS, 500, garbage body — the cached rate is used and
marked `stale`. With no cached rate at all, the annotation is simply absent.
A failed fetch never changes an exit code: `status --json` exits 0 with
`"fx": null` exactly as it exits 0 with an empty `items`.

**Conversion is integer arithmetic** in `i128`, from cents to cents, half-up.
The rate itself is stored as centavos per unit (blue 1550.00 → `155000`), so a
rate with two decimals like `bolsa` 1545.3 survives intact.

**The target is the primary currency** — already defined in `CONTEXT.md` as the
one covering the most active expenses. Totals in the primary currency get no
annotation; there is nothing to say.

## Surfaces

### `status`

```
ARS 585,196.00 / 585,196.00 · 0 pending, 0 overdue
USD   1,140.00 /   1,140.00 · 0 pending, 0 overdue
    ≈ ARS 1,767,000.00 @ blue 1,550.00
```

The annotation is its own line, indented, and attached to the total above it.
A stale rate appends its age: `@ blue 1,550.00 (3h old)`.

### `ls --json` / `status --json`

Two additions, both field-stable:

- a top-level `fx` object, `null` when disabled, unavailable, or unnecessary;
- `approxCents` on each entry of `totals`, `null` for the primary currency and
  whenever `fx` is `null`.

```json
"fx": {
  "casa": "blue",
  "base": "USD",
  "quote": "ARS",
  "rateCentavos": 155000,
  "fetchedAt": "2026-08-23T18:04:11",
  "sourceUpdatedAt": "2026-08-23T21:00:00.000Z",
  "stale": false
}
```

`approxCents` is computed in Rust and shipped, rather than left to each surface
to multiply. Same reason the QML plugin does not open the database: rounding is
a rule, and rules live in one place.

### TUI

The header keeps one line per currency and appends `≈ ARS … @ casa rate` to
non-primary ones. `r` forces a refresh that bypasses the TTL — the one key
added, and the only place the TUI fetches other than launch, because a fetch
blocks and the event loop must not.

### Widget

No new settings and no new process spawns. It already calls `ls --all --json`
on popup open; the annotation arrives in that payload and renders under the
totals block, dim, right-aligned. Age is rendered as `(stale)` rather than a
duration: the plugin does not parse timestamps, and the distinction that
matters at a glance is current versus not. A stale rate is not an error state —
the bar never blanks over FX.

## Out of scope, deliberately

**Per-expense casas.** The Terreno at USD 1,000 is not bought at the same price
as a USD 20 subscription billed to a card, and modelling that honestly means a
column, a migration, a flag on `add` and `edit`, and a per-row rate in every
surface. It buys accuracy in an annotation that is explicitly approximate.
Revisit if the single rate proves misleading in practice.

**Rate history.** `payments.amount_cents` already records what was actually
charged. That is the number that matters after the fact; a remembered rate
would compete with it.
