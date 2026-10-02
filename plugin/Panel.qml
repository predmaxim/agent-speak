import QtQuick
import Quickshell
import Quickshell.Wayland
import qs.Commons
import qs.Ui
import "."
import "Model.js" as Model
import "I18n.js" as I18n

// predmaxim.agent-speak: the settings window of agent-speakd. The bar icon is
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

  onOpenedChanged: if (opened) Qt.callLater(function() { escCatcher.forceActiveFocus() })

  // Own subscription while the window is open.
  Link { id: link; wanted: root.opened }

  // A modal in the middle of the screen, like predmaxim.todo: a click on the
  // dimmed screen or Esc closes it.
  PanelWindow {
    id: modal
    screen: root.QsWindow.window ? root.QsWindow.window.screen : null
    visible: root.opened
    color: Color.menu.scrim
    exclusionMode: ExclusionMode.Ignore
    anchors { top: true; bottom: true; left: true; right: true }
    WlrLayershell.namespace: "predmaxim-agent-speak"
    WlrLayershell.layer: WlrLayer.Overlay
    WlrLayershell.keyboardFocus: visible ? WlrKeyboardFocus.Exclusive : WlrKeyboardFocus.None

    MouseArea { anchors.fill: parent; onClicked: root.close() }

    Item {
      id: escCatcher
      focus: true
      Keys.onEscapePressed: root.close()
    }

    BorderSurface {
      id: card
      anchors.centerIn: parent
      width: Math.min(Style.space(480), modal.width - Style.space(80))
      height: Math.min(column.implicitHeight + card.contentTopInset + card.contentBottomInset, modal.height * 0.85)
      color: Color.popups.background
      borderSpec: Border.surfaceSpec("popups", "border", Color.popups.border, Math.max(1, Style.space(2)))
      padding: Style.spacing.panelPadding
      radius: Style.cornerRadius

      MouseArea { anchors.fill: parent }   // clicks on the card stay on it

      Column {
        id: column
        anchors.fill: parent
        anchors.topMargin: card.contentTopInset
        anchors.rightMargin: card.contentRightInset
        anchors.bottomMargin: card.contentBottomInset
        anchors.leftMargin: card.contentLeftInset
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
            foreground: root.bar.foreground
            fontFamily: root.bar.fontFamily
            onClicked: link.send(Model.cmd("pause"))
          }

          Button {
            text: root.tr("Stop")
            enabled: Model.busy(root.speech)
            opacity: enabled ? 1 : 0.4
            bordered: true
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
                  width: chipLabel.implicitWidth + Style.spacing.rowPaddingX * 2
                  height: chipLabel.implicitHeight + Style.spacing.xl
                  current: root.speech[group.modelData.key] === chip.modelData.value
                  hasCursor: chipMouse.containsMouse
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
