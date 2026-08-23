import QtQuick
import QtQuick.Controls
import Quickshell
import Quickshell.Io
import qs.Commons
import qs.Ui
import "Model.js" as Model

// Fixed monthly expenses in the Omarchy bar.
//
// The bar answers "how much of this month is still unpaid"; the popup answers
// "which ones, and let me tick them off". Adding, editing, and deleting live in
// the TUI — a bar popup is the wrong place to type an amount.
Panel {
  id: root
  moduleName: "matiaslapolla.paybar"
  ipcTarget: "matiaslapolla.paybar"
  manageIpc: false

  // Nerd Font money glyph.
  readonly property string glyph: "\uf0d6"

  // qs.Ui.Panel is a popup host, not a bar widget: unlike BarWidget it brings
  // neither the bar geometry nor a size, and a widget with no implicit width
  // lays out zero-wide and never appears.
  readonly property bool vertical: bar ? bar.vertical : false
  implicitWidth: button.implicitWidth
  implicitHeight: button.implicitHeight

  readonly property color foreground: bar ? bar.foreground : Color.foreground
  readonly property color urgent: bar ? bar.urgent : Color.urgent
  readonly property color dim: Qt.darker(foreground, 1.55)
  readonly property string fontFamily: bar ? bar.fontFamily : Style.font.family
  readonly property color hoverFill: bar ? Style.hoverFillFor(bar.foreground, Color.accent) : "transparent"
  readonly property color selectedFill: bar ? Style.selectedFillFor(bar.foreground, Color.accent) : "transparent"

  property int cursorIndex: 0
  property bool cursorActive: false

  readonly property bool showAmount: setting("showAmount", true) !== false

  // ---- bar label -----------------------------------------------------------

  readonly property string barLabel: {
    if (paybar.error === "missing-binary") return glyph + " !"
    if (!paybar.loaded) return glyph
    if (paybar.pending === 0) return glyph + " \u2713"
    if (showAmount && paybar.primaryCurrency !== "") {
      return glyph + " " + Model.formatCents(paybar.outstandingCents)
    }
    return glyph + " " + paybar.pending
  }

  readonly property string verticalLabel: paybar.pending > 0 ? String(paybar.pending) : glyph

  readonly property string tooltip: {
    if (paybar.error === "missing-binary") return "paybar is not on PATH"
    if (!paybar.loaded) return "paybar"
    if (paybar.pending === 0) return paybar.periodLabel + " — all settled"
    var line = paybar.periodLabel + " — " + paybar.pending + " pending"
    if (paybar.overdue > 0) line += ", " + paybar.overdue + " overdue"
    return line
  }

  // ---- cursor --------------------------------------------------------------

  function clampCursor() {
    var n = paybar.items.length
    cursorIndex = n === 0 ? 0 : Math.max(0, Math.min(cursorIndex, n - 1))
  }

  function moveCursor(dx, dy) {
    var n = paybar.items.length
    if (n === 0) return
    cursorIndex = (cursorIndex + dy + n) % n
  }

  function selectedItem() {
    var n = paybar.items.length
    if (n === 0) return null
    return paybar.items[Math.max(0, Math.min(cursorIndex, n - 1))]
  }

  // Browsing another month is a popup-only mode. Closing snaps back to the
  // current one, because the bar label is read at a glance with no month next
  // to it — left on March it would quietly claim to be this month's total.
  onOpenedChanged: {
    cursorActive = false
    cursorIndex = 0
    paybar.currentPeriod()
    if (opened) Qt.callLater(function() { keyCatcher.forceActiveFocus() })
  }

  Service {
    id: paybar
    settings: root.settings
  }

  Connections {
    target: paybar
    function onSnapshotChanged() { root.clampCursor() }
  }

  IpcHandler {
    target: root.ipcTarget
    function open(): void { root.open() }
    function close(): void { root.close() }
    function show(): void { root.open() }
    function hide(): void { root.close() }
    function toggle(): void { root.toggle() }
    function refresh(): string { paybar.refresh(); return "ok" }
  }

  WidgetButton {
    id: button
    anchors.fill: parent
    bar: root.bar
    text: root.vertical ? root.verticalLabel : root.barLabel
    tooltipText: root.tooltip
    active: paybar.overdue > 0 || paybar.error === "missing-binary"
    // Nothing outstanding is background information, not a headline.
    dimmed: paybar.loaded && paybar.pending === 0

    onPressed: function(b) {
      if (b === Qt.MiddleButton) paybar.refresh()
      else root.toggle()
    }
  }

  KeyboardPanel {
    id: panel
    anchorItem: button
    owner: root
    bar: root.bar
    open: root.opened
    focusTarget: keyCatcher
    contentWidth: panel.fittedContentWidth(Style.space(420))
    contentHeight: panel.fittedContentHeight(column.implicitHeight, Style.space(560))

    PanelKeyCatcher {
      id: keyCatcher
      anchors.fill: parent

      onMoveRequested: function(dx, dy) {
        if (dx !== 0) { paybar.stepPeriod(dx); return }
        if (!root.cursorActive) { root.cursorActive = true; return }
        root.moveCursor(dx, dy)
      }
      onActivateRequested: if (root.cursorActive) paybar.togglePaid(root.selectedItem())
      onCloseRequested: root.close()
      onTabRequested: function(direction) { root.switchPanel(direction) }
      onTextKey: function(t) {
        var k = String(t || "").toLowerCase()
        if (k === " " || k === "p") paybar.togglePaid(root.selectedItem())
        else if (k === "h") paybar.stepPeriod(-1)
        else if (k === "l") paybar.stepPeriod(1)
        else if (k === "r") paybar.refresh()
      }

      Flickable {
        id: panelFlick
        anchors.fill: parent
        contentWidth: width
        contentHeight: column.implicitHeight
        clip: true
        boundsBehavior: Flickable.StopAtBounds
        flickableDirection: Flickable.VerticalFlick
        interactive: contentHeight > height
        ScrollBar.vertical: ScrollBar { policy: ScrollBar.AsNeeded }

        Column {
          id: column
          width: panelFlick.width
          spacing: Style.space(8)

          // ---- header: month stepper ------------------------------------

          Item {
            width: parent.width
            height: monthLabel.implicitHeight + Style.space(4)
            visible: paybar.loaded

            Text {
              id: prevMonth
              anchors.left: parent.left
              anchors.verticalCenter: parent.verticalCenter
              text: "‹"
              color: prevMouse.containsMouse ? root.foreground : root.dim
              font.family: root.fontFamily
              font.pixelSize: Style.font.title
              MouseArea {
                id: prevMouse
                anchors.fill: parent
                anchors.margins: -Style.space(6)
                hoverEnabled: true
                cursorShape: Qt.PointingHandCursor
                onClicked: paybar.stepPeriod(-1)
              }
            }

            Text {
              id: monthLabel
              anchors.centerIn: parent
              text: paybar.periodLabel
              color: root.foreground
              font.family: root.fontFamily
              font.pixelSize: Style.font.subtitle
            }

            Text {
              id: nextMonth
              anchors.right: parent.right
              anchors.verticalCenter: parent.verticalCenter
              text: "›"
              color: nextMouse.containsMouse ? root.foreground : root.dim
              font.family: root.fontFamily
              font.pixelSize: Style.font.title
              MouseArea {
                id: nextMouse
                anchors.fill: parent
                anchors.margins: -Style.space(6)
                hoverEnabled: true
                cursorShape: Qt.PointingHandCursor
                onClicked: paybar.stepPeriod(1)
              }
            }
          }

          // ---- failure and empty states ---------------------------------

          Text {
            width: parent.width
            visible: paybar.error === "missing-binary"
            text: "paybar is not on PATH.\nBuild it with `cargo build --release` and copy it to ~/.local/bin."
            wrapMode: Text.WordWrap
            color: root.urgent
            font.family: root.fontFamily
            font.pixelSize: Style.font.bodySmall
          }

          Text {
            width: parent.width
            visible: paybar.stale
            text: "Showing the last known month — " + paybar.lastError
            wrapMode: Text.WordWrap
            color: root.urgent
            font.family: root.fontFamily
            font.pixelSize: Style.font.caption
          }

          Text {
            width: parent.width
            visible: paybar.loaded && paybar.items.length === 0
            text: "No fixed expenses yet — add them with `paybar add`."
            wrapMode: Text.WordWrap
            color: root.dim
            font.family: root.fontFamily
            font.pixelSize: Style.font.bodySmall
          }

          // ---- expense rows -----------------------------------------------

          Repeater {
            model: paybar.items

            delegate: Item {
              id: row
              required property var modelData
              required property int index

              readonly property bool paid: modelData.status === "paid"
              readonly property bool overdue: modelData.status === "overdue"
              readonly property bool hasCursor: root.cursorActive && root.cursorIndex === index

              width: column.width
              height: Math.max(Style.space(24), nameText.implicitHeight + Style.space(8))

              Rectangle {
                anchors.fill: parent
                radius: Style.cornerRadius
                color: row.hasCursor ? root.selectedFill
                     : (rowMouse.containsMouse ? root.hoverFill : "transparent")
              }

              // The whole row toggles: unlike a task list there is no second
              // verb competing for the click, so splitting the target would
              // only make it harder to hit.
              MouseArea {
                id: rowMouse
                anchors.fill: parent
                hoverEnabled: true
                cursorShape: Qt.PointingHandCursor
                onClicked: {
                  root.cursorActive = true
                  root.cursorIndex = row.index
                  paybar.togglePaid(row.modelData)
                }
              }

              Text {
                id: mark
                anchors.left: parent.left
                anchors.leftMargin: Style.space(6)
                anchors.verticalCenter: parent.verticalCenter
                width: Style.space(16)
                text: row.paid ? "\uf14a" : "\uf096"
                color: row.paid ? root.dim : (row.overdue ? root.urgent : root.foreground)
                font.family: root.fontFamily
                font.pixelSize: Style.font.icon
              }

              Text {
                id: dayText
                anchors.left: mark.right
                anchors.leftMargin: Style.space(6)
                anchors.verticalCenter: parent.verticalCenter
                text: Model.dayOf(row.modelData.dueDate)
                color: row.overdue ? root.urgent : root.dim
                font.family: root.fontFamily
                font.pixelSize: Style.font.caption
              }

              Text {
                id: nameText
                anchors.left: dayText.right
                anchors.leftMargin: Style.space(10)
                anchors.right: amountText.left
                anchors.rightMargin: Style.space(8)
                anchors.verticalCenter: parent.verticalCenter
                text: row.modelData.name
                     + (row.modelData.category === "" ? "" : "  " + row.modelData.category)
                elide: Text.ElideRight
                color: row.paid ? root.dim : root.foreground
                font.family: root.fontFamily
                font.pixelSize: Style.font.body
              }

              Text {
                id: amountText
                anchors.right: parent.right
                anchors.rightMargin: Style.space(6)
                anchors.verticalCenter: parent.verticalCenter
                text: row.modelData.currency + " " + Model.formatCents(row.modelData.amountCents)
                color: row.paid ? root.dim : root.foreground
                font.family: root.fontFamily
                font.pixelSize: Style.font.caption
              }
            }
          }

          PanelSeparator {
            width: parent.width
            foreground: root.foreground
            visible: paybar.loaded && paybar.items.length > 0
          }

          // ---- totals, one row per currency -------------------------------
          //
          // Never summed across currencies. A total in another currency gets a
          // subordinate line saying what it is worth and at which rate; that
          // annotation decorates one total and never replaces the two above it.

          Repeater {
            model: Model.readList(paybar.snapshot.totals)

            delegate: Column {
              required property var modelData
              readonly property string approx:
                Model.approxLabel(modelData, paybar.fx, paybar.primaryCurrency)

              width: column.width

              Item {
                width: parent.width
                height: totalLabel.implicitHeight + Style.space(4)

                Text {
                  id: totalLabel
                  anchors.left: parent.left
                  anchors.leftMargin: Style.space(6)
                  anchors.verticalCenter: parent.verticalCenter
                  text: modelData.currency
                  color: root.dim
                  font.family: root.fontFamily
                  font.pixelSize: Style.font.caption
                }

                Text {
                  anchors.right: parent.right
                  anchors.rightMargin: Style.space(6)
                  anchors.verticalCenter: parent.verticalCenter
                  text: Model.formatCents(modelData.paidCents) + " / " + Model.formatCents(modelData.dueCents)
                  color: root.foreground
                  font.family: root.fontFamily
                  font.pixelSize: Style.font.caption
                }
              }

              Text {
                width: parent.width - Style.space(12)
                x: Style.space(6)
                visible: approx !== ""
                horizontalAlignment: Text.AlignRight
                elide: Text.ElideLeft
                text: approx
                color: Qt.darker(root.dim, 1.2)
                font.family: root.fontFamily
                font.pixelSize: Style.font.caption
              }
            }
          }

          Text {
            width: parent.width
            visible: paybar.loaded
            horizontalAlignment: Text.AlignRight
            text: "space pay · h/l month · r refresh"
            color: Qt.darker(root.dim, 1.2)
            font.family: root.fontFamily
            font.pixelSize: Style.font.caption
          }
        }
      }
    }
  }
}
