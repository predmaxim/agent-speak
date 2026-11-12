import QtQuick
import Quickshell.Services.Mpris
import "Model.js" as Model

// Music pause: while Oma has the sound (`hold`: the daemon's media_hold and the music_pause setting), the MPRIS
// players that were playing pause; after, only they play again. Acts on the edges only: music the user starts
// meanwhile keeps playing. The list lives here: a shell restart mid-pause leaves the music paused (README).
QtObject {
  id: root

  property bool hold: false
  property var held: []   // dbusName of the players paused for Oma
  // Bound from the start, like the stock media service: Mpris finds the players only once first asked.
  readonly property var players: Mpris.players ? Mpris.players.values : []

  onHoldChanged: {
    if (hold) {
      var now = Model.toPause(root.players)
      root.held = now.map(function(p) { return p.dbusName })
      now.forEach(function(p) { p.pause() })
    } else {
      Model.toResume(root.players, root.held, MprisPlaybackState.Paused).forEach(function(p) { p.play() })
      root.held = []
    }
  }
}
