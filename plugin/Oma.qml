import QtQuick
import Quickshell
import Quickshell.Io
import "." as Plugin
import "Model.js" as Model

// predmaxim.agent-speak as Oma: a panel kept loaded from the shell's start.
// One connection to agent-speakd feeds the orb with its bubbles and the window.
Item {
  id: root

  property var bubbles: []
  // The phrase being spoken, apart from the TTL'd stack: Oma drops it. No
  // draft is null, never undefined (Model.escWanted tests !== null).
  property var draft: null
  // Esc is Oma's while a phrase can be cancelled and a second after a cancel.
  readonly property bool escWanted: Model.escWanted(root.draft, link.speech, escHold.running)
  onEscWantedChanged: escBind.apply()

  // The open window shows it all itself: no bubbles show or pile up meanwhile.
  Plugin.Link {
    id: link
    onBubble: function(b) {
      if (b.id === "draft") root.draft = b.drop ? null : b
      else if (!omaWindow.shown) root.bubbles = Model.pushBubble(root.bubbles, b, Date.now())
    }
    // Oma (or the daemon) gone: nobody is left to drop her draft.
    onSpeechChanged: if (link.speech.oma === "off") root.draft = null
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

  // After a cancel Esc stays caught a second: a habitual double Esc must not
  // reach the program below (Claude Code stops its agent on Esc).
  Timer { id: escHold; interval: 1000 }

  // Esc is a Hyprland bind made once and switched with set_enabled: hl.unbind
  // and :remove() would take Esc off in every submap, the Alt+Tab switcher's
  // too. One hyprctl at a time; when one ends, the next runs if the state it
  // set is not the wanted one any more, so the last wanted state wins.
  Process {
    id: escBind
    readonly property string luaOn: 'local b = _G.predmaxim_oma_esc or hl.bind("ESCAPE", hl.dsp.exec_cmd("omarchy-shell predmaxim.oma cancel")); _G.predmaxim_oma_esc = b; b:set_enabled(true)'
    readonly property string luaOff: 'if _G.predmaxim_oma_esc then _G.predmaxim_oma_esc:set_enabled(false) end'
    property bool grab: false   // what the running (or the last) run set
    command: ["hyprctl", "eval", grab ? luaOn : luaOff]
    stdout: StdioCollector { id: escOut }
    stderr: StdioCollector { id: escErr }
    function run(v) { grab = v; running = true }
    function apply() { if (!running && grab !== root.escWanted) run(root.escWanted) }
    onRunningChanged: apply()
    // A failed run is not retried (a Lua error would loop); the journal shows it.
    onExited: function(code) {
      if (code !== 0) console.warn("predmaxim.oma: hyprctl eval exited", code, escOut.text.trim(), escErr.text.trim())
    }
    // The shell before this one may have left the bind on.
    Component.onCompleted: run(false)
    // Unloaded or the shell quits: Esc must not stay caught until the next start.
    Component.onDestruction: Quickshell.execDetached(["hyprctl", "eval", luaOff])
  }

  // omarchy-shell predmaxim.oma toggle | talk | cancel
  IpcHandler {
    target: "predmaxim.oma"
    function toggle(): void { omaWindow.toggle() }
    function talk(): void { link.send(Model.omaCmd({ do: "talk" })) }
    function cancel(): void {
      link.send(Model.omaCmd({ do: "cancel" }))
      escHold.restart()
    }
  }

  Plugin.Orb {
    link: link
    bubbles: omaWindow.shown ? [] : Model.withDraft(root.bubbles, root.draft)
    onDismissed: function(key) { root.bubbles = Model.dismiss(root.bubbles, key) }
    onWindowToggled: omaWindow.toggle()
  }

  // Qualified: a bare `Window` is QtQuick's.
  Plugin.Window {
    id: omaWindow
    link: link
    draft: root.draft
    onShownChanged: if (shown) root.bubbles = []
  }
}
