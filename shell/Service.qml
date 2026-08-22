import QtQuick
import Quickshell
import Quickshell.Io
import "Model.js" as Model

// The only thing in this plugin that talks to `paybar`. Everything else renders
// what lands here.
//
// The database is never opened directly: clamping, status derivation, cents
// parsing, and currency grouping already exist once in the Rust binary, and a
// second implementation in QML would be a second set of rules to keep in sync.
Item {
  id: root

  property var settings: ({})

  // "" | "missing-binary" | "failed"
  property string error: ""
  property string lastError: ""
  // The last refresh failed but earlier data is still on screen. A transient
  // error must never blank the bar.
  property bool stale: false
  property bool loaded: false

  property var snapshot: Model.emptySnapshot()
  /// "" means the current month, whatever that is when the query runs.
  property string period: ""

  readonly property var items: Model.readList(snapshot.items)
  readonly property int pending: snapshot.pending
  readonly property int overdue: snapshot.overdue
  readonly property string primaryCurrency: snapshot.primaryCurrency
  readonly property int outstandingCents: Model.outstanding(snapshot, primaryCurrency)
  readonly property string periodLabel: Model.periodLabel(snapshot.period)

  property string binaryPath: ""
  readonly property bool ready: binaryPath !== ""

  function setting(name, fallback) {
    var value = settings ? settings[name] : undefined
    return value === undefined || value === null ? fallback : value
  }

  function intSetting(name, fallback, min, max) {
    var n = parseInt(String(setting(name, fallback)), 10)
    if (!isFinite(n)) n = fallback
    return Math.max(min, Math.min(max, n))
  }

  // Fixed expenses change monthly, not minutely. Polling hard would buy
  // nothing; the popup refreshes on open, which is when it matters.
  readonly property int refreshIntervalSec: intSetting("refreshIntervalSec", 300, 30, 3600)
  readonly property int missingBinaryBackoffSec: 900

  function resolveBinary() {
    if (!probeProcess.running) probeProcess.running = true
  }

  function refresh() {
    if (!ready) { resolveBinary(); return }
    if (listProcess.running) return
    var args = [binaryPath, "ls", "--all", "--json"]
    if (period !== "") args = args.concat(["--period", period])
    listProcess.command = args
    listProcess.running = true
  }

  function stepPeriod(delta) {
    period = Model.stepPeriod(snapshot.period, delta)
    refresh()
  }

  function currentPeriod() {
    period = ""
    refresh()
  }

  // Mutations re-read rather than editing the local model: the database is the
  // truth and it is one process spawn away.
  function runAndRefresh(args) {
    if (!ready || mutateProcess.running) return
    mutateProcess.command = [binaryPath].concat(args)
    mutateProcess.running = true
  }

  function togglePaid(item) {
    if (!item) return
    var verb = item.paidCents === null ? "pay" : "unpay"
    var args = [verb, String(item.id)]
    if (snapshot.period !== "") args = args.concat(["--period", snapshot.period])
    runAndRefresh(args)
  }

  function applyList(raw) {
    var result = Model.parseSnapshot(raw)
    if (!result.ok) {
      error = "failed"
      lastError = result.error
      stale = loaded
      return
    }
    snapshot = result.snapshot
    loaded = true
    error = ""
    lastError = ""
    stale = false
  }

  Timer {
    id: refreshTimer
    interval: (root.error === "missing-binary" ? root.missingBinaryBackoffSec : root.refreshIntervalSec) * 1000
    repeat: true
    running: true
    triggeredOnStart: true
    onTriggered: root.ready ? root.refresh() : root.resolveBinary()
  }

  // Resolving the path once keeps every other call an argv array with no shell
  // in the middle.
  Process {
    id: probeProcess
    running: false
    command: ["sh", "-c", "command -v paybar"]
    stdout: StdioCollector { id: probeOut; waitForEnd: true }
    onExited: function(exitCode) {
      var path = String(probeOut.text || "").trim().split("\n")[0]
      if (exitCode === 0 && path !== "") {
        root.binaryPath = path
        root.error = ""
        root.refresh()
      } else {
        root.binaryPath = ""
        root.error = "missing-binary"
        root.lastError = "paybar is not on PATH"
      }
    }
  }

  Process {
    id: listProcess
    running: false
    command: []
    stdout: StdioCollector { id: listOut; waitForEnd: true }
    stderr: StdioCollector { id: listErr; waitForEnd: true }
    onExited: function(exitCode) {
      if (exitCode === 0) {
        root.applyList(String(listOut.text || ""))
        return
      }
      root.error = "failed"
      root.lastError = String(listErr.text || "").trim() || ("paybar exited " + exitCode)
      root.stale = root.loaded
    }
  }

  Process {
    id: mutateProcess
    running: false
    command: []
    stderr: StdioCollector { id: mutateErr; waitForEnd: true }
    onExited: function(exitCode) {
      if (exitCode !== 0) {
        root.lastError = String(mutateErr.text || "").trim() || ("paybar exited " + exitCode)
      }
      root.refresh()
    }
  }
}
