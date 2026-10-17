import QtQuick

// Oma's window in the middle of the screen; only its open state so far.
Item {
  property var link
  property bool shown: false
  function toggle() { shown = !shown }
}
