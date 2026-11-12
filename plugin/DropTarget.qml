import QtQuick
import "Model.js" as Model

// A drop on the orb or a satellite (orb-address B): a file, a screenshot, a link or text — `took` with Model.dropText
// of it. Always a copy: a file manager offering a move must not lose its file. Orb.qml loads it apart, with a Loader:
// whether a Wayland drag reaches a layer-shell surface (Quickshell on Hyprland) is not verified yet, and a failure
// here must not take the orb with it.
DropArea {
  signal took(string text)
  keys: ["text/uri-list", "text/plain"]
  onEntered: function(drag) { drag.accept(Qt.CopyAction) }
  onDropped: function(drop) {
    var t = Model.dropText(drop.hasUrls ? drop.urls : [], drop.hasText ? drop.text : "")
    if (!t) return
    drop.accept(Qt.CopyAction)
    took(t)
  }
}
