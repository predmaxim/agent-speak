import QtQuick
import Quickshell
import Quickshell.Io
import qs.Commons as Commons

// Fixed status colours shared by my plugins (red, orange, yellow, green,
// blue), named once in ~/.config/omarchy/predmaxim/palette.json: a theme's
// red or yellow can be any colour. Without that file the theme's red, and
// `values` is empty, so callers keep their own fallback for the rest
// (values.orange || "#e0823d"). The same file in every plugin that uses it.
QtObject {
  id: root

  readonly property var values: {
    try {
      var parsed = JSON.parse(paletteFile.text() || "{}")
      return parsed && typeof parsed === "object" && !Array.isArray(parsed) ? parsed : ({})
    } catch (e) {
      return ({})
    }
  }
  readonly property color red: values.red || Commons.Color.urgent

  property FileView paletteFile: FileView {
    path: Quickshell.env("HOME") + "/.config/omarchy/predmaxim/palette.json"
    watchChanges: true
    printErrors: false
    blockLoading: true
    onFileChanged: reload()
  }
}
