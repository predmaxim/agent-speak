import QtQuick
import QtQuick.Shapes
import qs.Commons

// The orb's circle in one of Model.orbMode's modes, centred in its slot (the
// slot leaves room for the breath). Orb.qml draws it in the screen corner,
// Window.qml in its header.
Item {
  id: face

  property string mode: "off"
  property int size: Style.space(48)
  property int line: Math.max(1, Style.space(3))
  property real breathMax: 1.12

  implicitWidth: Math.ceil(size * breathMax)
  implicitHeight: implicitWidth

  // off, idle, listening, paused: a ring; speaking, waiting: filled.
  Rectangle {
    readonly property bool filled: ["speaking", "waiting"].indexOf(face.mode) >= 0
    property real breath: 1
    anchors.centerIn: parent
    width: face.size
    height: face.size
    radius: width / 2
    visible: ["off", "idle", "listening", "speaking", "paused", "waiting"].indexOf(face.mode) >= 0
    color: !filled ? "transparent" : face.mode === "waiting" ? Color.urgent : Color.accent
    border.width: filled ? 0 : face.line
    border.color: face.mode === "off" || face.mode === "idle" ? Color.muted : Color.accent
    opacity: face.mode === "off" ? 0.4 : face.mode === "paused" ? 0.5 : 1
    scale: face.mode === "listening" ? breath : 1

    SequentialAnimation on breath {
      running: face.mode === "listening"
      loops: Animation.Infinite
      NumberAnimation { from: 1; to: face.breathMax; duration: 1200; easing.type: Easing.InOutSine }
      NumberAnimation { from: face.breathMax; to: 1; duration: 1200; easing.type: Easing.InOutSine }
    }
  }

  // loading: a dashed outline; thinking: an arc. Both turn.
  Shape {
    anchors.centerIn: parent
    width: face.size
    height: face.size
    visible: face.mode === "loading" || face.mode === "thinking"
    preferredRendererType: Shape.CurveRenderer

    ShapePath {
      strokeColor: Color.accent
      strokeWidth: face.line
      strokeStyle: face.mode === "loading" ? ShapePath.DashLine : ShapePath.SolidLine
      dashPattern: [2, 2]
      capStyle: face.mode === "loading" ? ShapePath.FlatCap : ShapePath.RoundCap
      fillColor: "transparent"
      PathAngleArc {
        centerX: face.size / 2
        centerY: face.size / 2
        radiusX: (face.size - face.line) / 2
        radiusY: radiusX
        startAngle: 0
        sweepAngle: face.mode === "loading" ? 360 : 90
      }
    }

    RotationAnimation on rotation {
      from: 0
      to: 360
      duration: 1000
      loops: Animation.Infinite
      running: face.mode === "loading" || face.mode === "thinking"
    }
  }

  Text {
    anchors.centerIn: parent
    visible: face.mode === "quiet"
    textFormat: Text.PlainText
    text: "\u{F0F65}"   // 󰽥
    color: Color.foreground
    font.family: Style.font.family
    font.pixelSize: Math.round(face.size * 0.75)
  }
}
