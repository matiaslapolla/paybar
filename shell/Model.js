.pragma library

// Pure helpers over the `paybar --json` surface. No QML types, so the parsing
// and formatting rules can be reasoned about without a running shell.

function emptySnapshot() {
  return {
    period: "",
    today: "",
    pending: 0,
    overdue: 0,
    primaryCurrency: "",
    totals: [],
    items: []
  }
}

/// Parse `paybar ls --json` / `paybar status --json`. Returns a result object
/// rather than throwing, because every caller has to render the failure anyway.
function parseSnapshot(raw) {
  var text = String(raw || "").trim()
  if (text === "") return { ok: true, snapshot: emptySnapshot() }
  var parsed
  try {
    parsed = JSON.parse(text)
  } catch (e) {
    return { ok: false, error: "paybar returned unparseable JSON" }
  }
  if (!parsed || typeof parsed !== "object" || parsed.length !== undefined) {
    return { ok: false, error: "expected a JSON object" }
  }
  var snapshot = emptySnapshot()
  snapshot.period = String(parsed.period || "")
  snapshot.today = String(parsed.today || "")
  snapshot.pending = Number(parsed.pending) || 0
  snapshot.overdue = Number(parsed.overdue) || 0
  snapshot.primaryCurrency = parsed.primaryCurrency ? String(parsed.primaryCurrency) : ""
  snapshot.totals = readList(parsed.totals).map(function(t) {
    return {
      currency: String(t.currency || ""),
      dueCents: Number(t.dueCents) || 0,
      paidCents: Number(t.paidCents) || 0
    }
  })
  snapshot.items = readList(parsed.items).map(function(i) {
    return {
      id: Number(i.id),
      name: String(i.name || ""),
      category: i.category ? String(i.category) : "",
      currency: String(i.currency || ""),
      amountCents: Number(i.amountCents) || 0,
      paidCents: i.paidCents === null || i.paidCents === undefined ? null : Number(i.paidCents),
      dueDate: String(i.dueDate || ""),
      status: String(i.status || "due")
    }
  })
  return { ok: true, snapshot: snapshot }
}

// Array-like rather than Array: a value that travels through a QML `var`
// property arrives as QVariantList, for which Array.isArray() is false and
// .map() is absent. Duck-typing on length is what survives the trip.
function readList(value) {
  if (!value || !value.length) return []
  var out = []
  for (var i = 0; i < value.length; i++) out.push(value[i])
  return out
}

/// Cents to a grouped decimal, matching the Rust formatter: 4590000 -> "45,900.00".
function formatCents(cents) {
  var n = Number(cents) || 0
  var neg = n < 0
  var abs = Math.abs(n)
  var whole = Math.floor(abs / 100)
  var frac = abs % 100
  var digits = String(whole)
  var grouped = ""
  for (var i = 0; i < digits.length; i++) {
    if (i > 0 && (digits.length - i) % 3 === 0) grouped += ","
    grouped += digits[i]
  }
  return (neg ? "-" : "") + grouped + "." + (frac < 10 ? "0" : "") + frac
}

/// What is still owed, per currency. A paid item owes nothing even when the
/// amount actually charged differed from the expected one.
function outstanding(snapshot, currency) {
  var total = 0
  var items = readList(snapshot.items)
  for (var i = 0; i < items.length; i++) {
    var it = items[i]
    if (currency && it.currency !== currency) continue
    if (it.paidCents === null) total += it.amountCents
  }
  return total
}

/// The month label a human reads: "2026-08" -> "August 2026".
var MONTHS = ["January", "February", "March", "April", "May", "June",
              "July", "August", "September", "October", "November", "December"]

function periodLabel(period) {
  var parts = String(period || "").split("-")
  if (parts.length !== 2) return String(period || "")
  var m = parseInt(parts[1], 10)
  if (!(m >= 1 && m <= 12)) return String(period)
  return MONTHS[m - 1] + " " + parts[0]
}

/// Step a "YYYY-MM" period without a Date object, so a month with fewer days
/// than today can never shift the result.
function stepPeriod(period, delta) {
  var parts = String(period || "").split("-")
  if (parts.length !== 2) return period
  var y = parseInt(parts[0], 10)
  var m = parseInt(parts[1], 10) + delta
  while (m < 1) { m += 12; y -= 1 }
  while (m > 12) { m -= 12; y += 1 }
  return y + "-" + (m < 10 ? "0" : "") + m
}

/// "2026-08-05" -> "05". The day is all the row needs; the month is the header.
function dayOf(dueDate) {
  var parts = String(dueDate || "").split("-")
  return parts.length === 3 ? parts[2] : ""
}
