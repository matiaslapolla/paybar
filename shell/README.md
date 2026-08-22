# paybar — Omarchy bar widget

What is still unpaid this month, in the bar, with a popup to tick items off.

```
                                             󰇁 12,400.00

┌──────────────────────────────────────────────┐
│  ‹            August 2026                 ›  │
│  ☑  05  Rent      home         ARS 90,000.00 │
│  ☐  10  Internet  home          ARS 8,500.00 │
│  ☐  15  Spotify   fun           ARS 3,900.00 │
│  ☐  28  Claude    work              USD 20.00│
├──────────────────────────────────────────────┤
│  ARS               90,000.00 / 102,400.00    │
│  USD                     0.00 / 20.00        │
│              space pay · h/l month · r refresh│
└──────────────────────────────────────────────┘
```

## Install

```sh
./install.sh
```

Symlinks this directory to `~/.config/omarchy/plugins/matiaslapolla.paybar`,
rescans plugins, and enables the widget. Move it with
`omarchy bar move matiaslapolla.paybar --section left`.

It needs the `paybar` binary on `PATH`:

```sh
cd ../core && cargo build --release && cp target/release/paybar ~/.local/bin/
```

> **Editing the QML needs a shell restart.** Quickshell caches compiled QML per
> plugin, so `rescanPlugins` re-reads a plugin without recompiling changed
> files. Use `omarchy restart shell` after a code change.

## How it talks to paybar

It does not open the database. It runs `paybar ls --all --json` through
`Quickshell.Io.Process` and renders the result.

Clamping a due day to a short month, deriving paid/overdue/due, parsing cents,
grouping by currency — all of that already exists once in the Rust binary. A
second implementation in QML would be a second set of rules to keep in step.

## Interactions

**Bar** — left click toggles the popup, middle click refreshes.

The label is the outstanding amount in the primary currency, or the pending
count when `showAmount` is off. Nothing outstanding shows a dim check; anything
overdue turns the label urgent. The widget never disappears.

**Popup**

| Key | Action |
|---|---|
| `space` / `p` | mark the row paid, or undo it |
| `h` / `l` | previous / next month |
| `j` / `k` | move the cursor |
| `r` | refresh |
| `Esc` | close |

Clicking a row toggles it. Unlike a task list there is no second verb competing
for the click, so splitting the target would only make it harder to hit.

Browsing another month is a popup-only mode: closing snaps back to the current
month, because the bar label is read at a glance with no month beside it and
would otherwise claim March's total is this month's.

Adding, editing, and deleting live in the TUI. A bar popup is the wrong place
to type an amount.

## Settings

| key | default | meaning |
|---|---|---|
| `refreshIntervalSec` | 300 | poll cadence, 30–3600. Fixed expenses change monthly, not minutely. |
| `showAmount` | true | outstanding amount vs. pending count |

## Troubleshooting

**Bar shows `󰇁 !`** — `paybar` is not on `PATH`. The widget backs off to a
15-minute poll rather than spawning a doomed process every five minutes.

**"Showing the last known month"** — `paybar` exited non-zero. The last good
data stays on screen; the message carries stderr.
