import QtQuick
import Quickshell
import qs.Commons
import qs.Ui
import "@PLUGIN_DIR@" as Plugin
import "@PLUGIN_DIR@/Model.js" as Model
import "@PLUGIN_DIR@/I18n.js" as I18n

// predmaxim.agent-speak among the bar's indicators: lit while speaking, paused
// or in auto mode, otherwise only when the group is hovered. Left click
// toggles the settings window (the hidden widget, Panel.qml); right click
// pauses or resumes, and only while there is something to pause.
// keep-custom-widgets.sh copies this file into the predmaxim.indicators clone
// as indicators/AgentSpeak.qml and fills in @PLUGIN_DIR@.
BarIndicator {
  id: root

  readonly property var tr: I18n.translator(I18n.textLanguage(function(name) { return Quickshell.env(name) }))
  readonly property var look: Model.view(link.speech)

  active: look.lit
  activeText: look.icon
  inactiveText: look.icon
  activeTooltipText: root.tr(look.tip, look.arg)
  inactiveTooltipText: root.tr(look.tip, look.arg)

  onPressed: function(button) {
    if (button === Qt.RightButton) {
      if (Model.busy(link.speech)) link.send(Model.cmd("pause"))
    } else if (button === Qt.LeftButton) {
      Quickshell.execDetached(["omarchy-shell", "predmaxim.agent-speak", "toggle"])
    }
  }

  Plugin.Link { id: link }
}
