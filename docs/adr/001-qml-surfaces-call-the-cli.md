# ADR 001 — QML surfaces call the CLI, never the database

Status: accepted · 2026-08-22
Applies to: paybar `shell/`, and the same decision in [tobar](https://github.com/matiaslapolla/tobar) `shell/`

## Context

Both paybar and tobar are one SQLite file with several surfaces over it. The
Rust binary and the Swift menu bar app each open the file directly and conform
to a shared `CONTRACT.md`. Adding an Omarchy bar widget raised the question a
third time: does the Quickshell plugin open the database too?

The rules worth protecting are not in the schema. They are in the code around
it: focus exclusivity, ordering, English date parsing, `#tag` extraction,
clamping a due day to a short month, deriving paid/overdue/due, parsing cents
without rounding, grouping totals by currency. A surface that opens the file
inherits the tables and none of that.

## Decision

QML surfaces do not open the database. They spawn the Rust binary with
`--json` and render the result. Mutations run the same binary and then re-read;
they never edit a local model optimistically.

`CONTRACT.md` states it as a rule: **only Rust and Swift touch the file.**

## Consequences

**Good.** There is exactly one implementation of every rule. Quick-add in a
popup sends its text verbatim to `add`, so a date phrase or a tag parses
identically to the terminal — there is no second parser to drift. The widget
stays a view: its failure modes are "binary missing" and "binary failed",
neither of which can corrupt anything. Schema changes need no QML change at
all, only a CLI change, which is where they belong.

**Cost.** A process spawn per refresh. At tobar's 30 s and paybar's 5 min
against a ~1 ms binary this is not a cost worth engineering around; if it ever
became one, the fix is a longer interval, not a second database client.

**Cost.** The widget needs the binary on `PATH`, a dependency the Swift app
does not have. This is rendered as a first-class state with instructions rather
than left to fail silently, and the poll backs off while it is missing.

## Alternatives rejected

**A QML SQLite binding.** Quickshell has no binding worth depending on, and
taking one would put a second reader on the WAL with none of the semantics.

**A shared library (FFI).** Correct in principle, disproportionate for two
personal tools: it adds a build artifact, a C ABI, and a versioning problem to
save a process spawn nobody can perceive.

**Duplicating just the "simple" rules in JS.** They are not simple — the tag
grammar and the due-day clamp are exactly the kind of rule that looks trivial
and drifts. Duplicating any of them makes the contract advisory rather than
enforced.
