import QtQuick
import Quickshell
import qs.Commons
import qs.Ui
import "."
import "Model.js" as Model
import "I18n.js" as I18n

// predmaxim.agent-speak: the settings panel of agent-speakd. The bar icon is
// Indicator.qml in the predmaxim.indicators clone; this widget stays in the
// layout hidden, for the IPC toggle. Settings live in the daemon's
// config.toml: a choice goes out as `set`, and the window lights what the
// daemon confirms in its next state line, not what was clicked.
Panel {
  id: root
  moduleName: "predmaxim.agent-speak"
  ipcTarget: "predmaxim.agent-speak"

  readonly property var tr: I18n.translator(I18n.textLanguage(function(name) { return Quickshell.env(name) }))
  readonly property var speech: link.speech
  readonly property var look: Model.view(root.speech)
  readonly property var sections: Model.sections(root.tr)

  function choose(key, value, sample) {
    link.send(Model.set(key, value))
    if (sample) link.send(Model.say(Model.SAMPLE))
  }

  visible: false
  implicitWidth: 0
  implicitHeight: 0

  // One cursor for mouse and keyboard: section `curSec`, chip `curIdx`.
  property bool cursorActive: false
  property int curSec: 0
  property int curIdx: 0

  function rowCount(sec) { return root.sections[sec] ? root.sections[sec].options.length : 0 }

  function setCursor(sec, idx) {
    root.cursorActive = true
    root.curSec = sec
    root.curIdx = idx
  }

  // curSec -1 is the Pause/Stop row above the chips (shown while the daemon runs).
  function moveCursor(dx, dy) {
    if (!root.speech.running || root.sections.length === 0) return
    if (!root.cursorActive) { root.setCursor(0, 0); return }
    var sec = Math.max(-1, Math.min(root.sections.length - 1, root.curSec + dy))
    var n = sec < 0 ? 2 : root.rowCount(sec)
    root.setCursor(sec, Math.max(0, Math.min(n - 1, root.curIdx + dx)))
  }

  function activate() {
    if (!root.cursorActive || !root.speech.running) return
    if (root.curSec < 0) {
      if (Model.busy(root.speech)) link.send(Model.cmd(root.curIdx === 0 ? "pause" : "stop"))
      return
    }
    var sec = root.sections[root.curSec]
    var opt = sec ? sec.options[root.curIdx] : null
    if (opt) root.choose(sec.key, opt.value, sec.sample)
  }

  // The rows vanish with the daemon: drop the cursor with them.
  onSpeechChanged: if (!speech.running) cursorActive = false

  function back() { root.close() }

  onOpenedChanged: {
    if (opened) {
      anchor = findAnchor()
      cursorActive = false
      moveGate.reset()
    }
  }

  PointerMoveGate { id: moveGate; referenceItem: column }

  // Own subscription while the window is open.
  Link { id: link; wanted: root.opened }

  // The panel hangs off the icon in the indicator group (Indicator.qml, id
  // "AgentSpeak" there); while the group is collapsed the icon has no width,
  // so it hangs off the group itself.
  property Item anchor: root
  // The plugin bar API only lists our own widgets, so walk the bar window
  // we live in (same screen) for the icon, else the group.
  function find(item, test) {
    if (!item) return null
    if (test(item)) return item
    for (var i = 0; i < item.children.length; i++) {
      var hit = root.find(item.children[i], test)
      if (hit) return hit
    }
    return null
  }
  function findAnchor() {
    var top = root.QsWindow.window ? root.QsWindow.window.contentItem : null
    var group = root.find(top, function(it) { return "revealInactiveIndicators" in it && "indicatorEntries" in it })
    return root.find(group, function(it) { return it.moduleName === "AgentSpeak" && it.visible && it.width > 0 && it.opacity > 0 }) || group || root
  }

  KeyboardPanel {
    id: panel
    anchorItem: root.anchor
    owner: root
    bar: root.bar
    open: root.opened
    focusTarget: keyCatcher
    contentWidth: panel.fittedContentWidth(Style.space(480))
    contentHeight: panel.fittedContentHeight(column.implicitHeight)

    PanelKeyCatcher {
      id: keyCatcher
      anchors.fill: parent
      onMoveRequested: function(dx, dy) { root.moveCursor(dx, dy) }
      onActivateRequested: root.activate()
      onCloseRequested: root.back()
      onTabRequested: function(direction) { root.switchPanel(direction) }

      Column {
        id: column
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.top: parent.top
        spacing: Style.space(14)

        PanelHero {
          title: root.tr("Speech")
          meta: root.tr(root.look.tip, root.look.arg)
          foreground: root.bar.foreground
          fontFamily: root.bar.fontFamily
          iconComponent: Text {
            text: root.look.icon
            color: root.bar.foreground
            font.family: root.bar.fontFamily
            font.pixelSize: Style.font.display
          }
        }

        Row {
          visible: root.speech.running
          spacing: Style.space(8)

          Button {
            text: root.speech.paused ? root.tr("Resume") : root.tr("Pause")
            enabled: Model.busy(root.speech)
            opacity: enabled ? 1 : 0.4
            bordered: true
            hasCursor: root.cursorActive && root.curSec === -1 && root.curIdx === 0
            foreground: root.bar.foreground
            fontFamily: root.bar.fontFamily
            onClicked: link.send(Model.cmd("pause"))
          }

          Button {
            text: root.tr("Stop")
            enabled: Model.busy(root.speech)
            opacity: enabled ? 1 : 0.4
            bordered: true
            hasCursor: root.cursorActive && root.curSec === -1 && root.curIdx === 1
            foreground: root.bar.foreground
            fontFamily: root.bar.fontFamily
            onClicked: link.send(Model.cmd("stop"))
          }
        }

        PanelSeparator { foreground: root.bar.foreground }

        // No daemon: what to run instead of the selectors.
        Column {
          visible: !root.speech.running
          width: parent.width
          spacing: Style.space(6)

          Text {
            textFormat: Text.PlainText
            text: root.tr("Speech service is not running")
            color: root.bar.foreground
            font.family: root.bar.fontFamily
            font.pixelSize: Style.font.body
            font.bold: true
          }

          Text {
            textFormat: Text.PlainText
            text: "systemctl --user start agent-speakd"
            color: Qt.darker(root.bar.foreground, 1.4)
            font.family: root.bar.fontFamily
            font.pixelSize: Style.font.caption
          }
        }

        // Selectors: the current option lit like the headphones mode in predmaxim.audio.
        Repeater {
          model: root.speech.running ? root.sections : []

          Column {
            id: group
            required property var modelData
            required property int index
            width: column.width
            spacing: Style.space(6)

            PanelSectionHeader {
              text: group.modelData.caption
              foreground: root.bar.foreground
              fontFamily: root.bar.fontFamily
            }

            Flow {
              width: parent.width
              spacing: Style.space(6)

              Repeater {
                model: group.modelData.options

                CursorSurface {
                  id: chip
                  required property var modelData
                  required property int index
                  width: chipLabel.implicitWidth + Style.spacing.rowPaddingX * 2
                  height: chipLabel.implicitHeight + Style.spacing.xl
                  current: root.speech[group.modelData.key] === chip.modelData.value
                  hasCursor: root.cursorActive && root.curSec === group.index && root.curIdx === chip.index
                  foreground: root.bar.foreground

                  Text {
                    id: chipLabel
                    anchors.centerIn: parent
                    textFormat: Text.PlainText
                    text: chip.modelData.label
                    color: root.bar.foreground
                    font.family: root.bar.fontFamily
                    font.pixelSize: Style.font.body
                    font.bold: chip.current
                  }

                  MouseArea {
                    id: chipMouse
                    anchors.fill: parent
                    hoverEnabled: true
                    cursorShape: Qt.PointingHandCursor
                    onPositionChanged: function(mouse) {
                      if (moveGate.moved(chipMouse, mouse)) root.setCursor(group.index, chip.index)
                    }
                    onClicked: root.choose(group.modelData.key, chip.modelData.value, group.modelData.sample)
                  }
                }
              }
            }
          }
        }
      }
    }
  }
}
