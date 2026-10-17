import QtQuick
import Quickshell
import Quickshell.Io
import "Model.js" as Model

// The connection to agent-speakd: subscribes on connect and keeps the latest
// state line in `speech` and the agent list in `agents`; bubbles and chats
// come as signals; commands go out with send(). No daemon (or it restarted) —
// `speech` is Model.OFFLINE and a retry runs every 5 s.
Item {
  id: link

  property bool wanted: true
  property var speech: Model.OFFLINE
  signal bubble(var b)
  signal chat(var c)
  signal chatDelta(var d)
  signal chats(var list)

  property var agents: []
  property bool up: false      // the live socket is connected
  property int attempt: 0      // a failed Socket never reconnects, so each retry builds a new one

  function send(line) {
    if (up && sockLoader.item) {
      sockLoader.item.write(line)
      sockLoader.item.flush()
    }
  }

  visible: false
  onWantedChanged: if (!wanted) { up = false; speech = Model.OFFLINE; agents = [] }

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
        else { link.speech = Model.OFFLINE; link.agents = [] }
      }
      parser: SplitParser {
        onRead: function(line) {
          var m = Model.parseLine(line)
          if (!m) return
          if (m.type === "state") link.speech = m.value
          else if (m.type === "agents") link.agents = m.value
          else if (m.type === "bubble") link.bubble(m.value)
          else if (m.type === "chat") link.chat(m.value)
          else if (m.type === "chat_delta") link.chatDelta(m.value)
          else link.chats(m.value)
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
