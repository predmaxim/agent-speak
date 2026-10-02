import QtQuick
import Quickshell
import Quickshell.Io
import "Model.js" as Model

// The connection to agent-speakd: subscribes on connect and keeps the latest
// state line in `speech`; commands go out with send(). No daemon (or it
// restarted) — `speech` is Model.OFFLINE and a retry runs every 5 s.
// Shared by Indicator.qml (in the predmaxim.indicators clone) and Panel.qml.
Item {
  id: link

  property bool wanted: true
  property var speech: Model.OFFLINE
  property bool up: false      // the live socket is connected
  property int attempt: 0      // a failed Socket never reconnects, so each retry builds a new one

  function send(line) {
    if (up && sockLoader.item) {
      sockLoader.item.write(line)
      sockLoader.item.flush()
    }
  }

  visible: false
  onWantedChanged: if (!wanted) { up = false; speech = Model.OFFLINE }

  Loader {
    id: sockLoader
    active: link.wanted
    sourceComponent: sockComponent
    property int generation: link.attempt
    onGenerationChanged: { active = false; active = link.wanted }
  }

  Component {
    id: sockComponent
    Socket {
      path: Quickshell.env("XDG_RUNTIME_DIR") + "/agent-speak.sock"
      connected: true
      onConnectedChanged: {
        link.up = connected
        if (connected) { write(Model.cmd("subscribe")); flush() }
        else link.speech = Model.OFFLINE
      }
      parser: SplitParser {
        onRead: function(line) {
          var s = Model.parse(line)
          if (s) link.speech = s
        }
      }
    }
  }

  Timer {
    interval: 5000
    repeat: true
    running: link.wanted && !link.up
    onTriggered: link.attempt++
  }
}
