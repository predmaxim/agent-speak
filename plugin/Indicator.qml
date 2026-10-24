import QtQuick
import Quickshell
import qs.Commons
import qs.Ui
import "@PLUGIN_DIR@" as Plugin
import "@PLUGIN_DIR@/Model.js" as Model
import "@PLUGIN_DIR@/I18n.js" as I18n

// predmaxim.agent-speak (Oma) among the bar's indicators: shown only when the
// group is hovered — while Oma runs, the orb shows her. LMB — Oma's window
// (off: its power switch turns her on); RMB — sound on/off, or Oma on when
// she is off. keep-custom-widgets.sh copies this file into the
// predmaxim.indicators clone as indicators/Oma.qml and fills in ../../predmaxim.agent-speak.
BarIndicator {
  id: root

  readonly property var tr: I18n.translator(I18n.textLanguage(function(name) { return Quickshell.env(name) }))
  readonly property bool running: link.speech.running === true

  active: false
  activeText: "\u{F05CB}"
  inactiveText: "\u{F05CB}"
  inactiveTooltipText: !running ? root.tr("Oma is off") : link.speech.quiet ? root.tr("Oma: sound off") : root.tr("Oma")

  onPressed: function(button) {
    if (button !== Qt.RightButton) Quickshell.execDetached(["omarchy-shell", "predmaxim.oma", "toggle"])
    else if (!running) Quickshell.execDetached([Quickshell.env("HOME") + "/.local/bin/agent-speak", "on"])
    else link.send(Model.set("quiet_override", link.speech.quiet ? "off" : "on"))
  }

  Plugin.Link { id: link }
}
