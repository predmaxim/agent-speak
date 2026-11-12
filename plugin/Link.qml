import QtQuick
import Quickshell
import Quickshell.Io
import "Model.js" as Model

// The connection to agent-speakd: subscribes on connect and keeps the latest
// state line in `speech`, the agent list in `agents`, the permission cards in
// `permissions` and the deferred orders in `later`; bubbles, chats, the memory page, the
// orb's ripples and a shown agent come as signals; commands go out with send(). No daemon (or it
// restarted) — `speech` is Model.OFFLINE and a retry runs every 5 s.
Item {
  id: link

  property bool wanted: true
  property var speech: Model.OFFLINE
  signal bubble(var b)
  signal chat(var c)
  signal chatDelta(var d)
  signal chats(var list)
  signal ripple(var r)   // {tone: "final" | "question" | "status"}: news came, Oma does not say it now
  signal memory(var m)   // {prefs, days}: Oma's answer for the window's memory page
  signal agentShown(var name)   // `agent-speak show` brought an agent to the front (its project)

  property var agents: []
  property var permissions: []
  property var later: []
  property bool up: false      // the live socket is connected
  property int attempt: 0      // a failed Socket never reconnects, so each retry builds a new one

  // Reconnect now (the service was just turned on), not at the next 5 s retry.
  function retry() { if (!up) attempt++ }

  // Oma as a whole (the service): on, the link reconnects at once instead of
  // in up to 5 s. `agent-speak on` returns when systemctl does: with the
  // unit's Type=notify, once the daemon's socket is there.
  function power(on) {
    powerProc.on = on
    powerProc.running = true
  }

  Process {
    id: powerProc
    property bool on: false
    command: [Quickshell.env("HOME") + "/.local/bin/agent-speak", on ? "on" : "off"]
    onExited: if (on) link.retry()
  }

  // No connection: nothing to queue it for (the service may stay off), the
  // journal says what was dropped; the window and the orb show Oma off.
  function send(line) {
    if (up && sockLoader.item) {
      sockLoader.item.write(line)
      sockLoader.item.flush()
    } else console.warn("predmaxim.oma: no agent-speakd, not sent:", line.trim())
  }

  visible: false
  onWantedChanged: if (!wanted) { up = false; forget() }

  // No daemon: nothing it said holds.
  function forget() {
    speech = Model.OFFLINE
    agents = []
    permissions = []
    later = []
  }

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
        else link.forget()
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
          else if (m.type === "chats") link.chats(m.value)
          else if (m.type === "ripple") link.ripple(m.value)
          else if (m.type === "memory") link.memory(m.value)
          else if (m.type === "shown") link.agentShown(m.value)
          else if (m.type === "permissions") link.permissions = m.value
          else if (m.type === "later") link.later = m.value
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
