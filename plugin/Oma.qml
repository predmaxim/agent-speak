import QtQuick
import Quickshell.Io
import "." as Plugin
import "Model.js" as Model

// predmaxim.agent-speak as Oma: a panel kept loaded from the shell's start.
// One connection to agent-speakd feeds the orb with its bubbles and the window.
Item {
  id: root

  property var bubbles: []

  Plugin.Link {
    id: link
    onBubble: function(b) { root.bubbles = Model.pushBubble(root.bubbles, b, Date.now()) }
  }

  // The stack goes as a whole BUBBLE_TTL after its newest bubble; quiet mode keeps it.
  Timer {
    interval: 1000
    repeat: true
    running: root.bubbles.length > 0
    onTriggered: {
      var kept = Model.expire(root.bubbles, Date.now(), link.speech.quiet)
      if (kept !== root.bubbles) root.bubbles = kept
    }
  }

  // omarchy-shell predmaxim.oma toggle | talk | dialog
  IpcHandler {
    target: "predmaxim.oma"
    function toggle(): void { omaWindow.toggle() }
    function talk(): void { link.send(Model.omaCmd({ do: "talk" })) }
    function dialog(): void { link.send(Model.omaCmd({ do: "dialog" })) }
  }

  Plugin.Orb {
    link: link
    bubbles: root.bubbles
    onDismissed: function(key) { root.bubbles = Model.dismiss(root.bubbles, key) }
    onWindowToggled: omaWindow.toggle()
  }

  // Qualified: a bare `Window` is QtQuick's.
  Plugin.Window {
    id: omaWindow
    link: link
  }
}
