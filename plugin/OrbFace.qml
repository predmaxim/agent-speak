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

  // Own colours (rules.md, the agent-speak section): a theme has no roles for
  // a gradient. Cool — at rest, listening and speaking; warm — thinking.
  readonly property var cool: ["#4f7cff", "#9b5cff", "#ff5ca8"]
  readonly property var warm: ["#ffb547", "#ff7a3d", "#e83d6b"]

  implicitWidth: Math.ceil(size * breathMax)
  implicitHeight: implicitWidth

  // off: a dim ring; waiting (an agent waits for an answer): the theme's alarm colour.
  Rectangle {
    anchors.centerIn: parent
    width: face.size
    height: face.size
    radius: width / 2
    visible: face.mode === "off" || face.mode === "waiting"
    color: face.mode === "waiting" ? Color.urgent : "transparent"
    border.width: face.mode === "off" ? face.line : 0
    border.color: Color.muted
    opacity: face.mode === "off" ? 0.4 : 1
  }

  // idle, paused: a still gradient disc; listening: it flows and breathes;
  // speaking: it flows faster; thinking: the warm colours flow. The middle
  // colour fills the disc, the other two are soft spots circling inside it
  // against each other, so the colours drift.
  Item {
    id: disc
    readonly property bool flowing: ["listening", "speaking", "thinking"].indexOf(face.mode) >= 0
    readonly property var colors: face.mode === "thinking" ? face.warm : face.cool
    property real spin: 0
    property real breath: 1
    anchors.centerIn: parent
    width: face.size
    height: face.size
    visible: ["idle", "paused", "listening", "speaking", "thinking"].indexOf(face.mode) >= 0
    opacity: face.mode === "paused" ? 0.5 : 1
    scale: face.mode === "listening" ? breath : 1

    NumberAnimation on spin {
      from: 0
      to: 360
      duration: face.mode === "listening" ? 6000 : face.mode === "speaking" ? 2500 : 3000
      loops: Animation.Infinite
      running: disc.flowing
    }

    SequentialAnimation on breath {
      running: face.mode === "listening"
      loops: Animation.Infinite
      NumberAnimation { from: 1; to: face.breathMax; duration: 1200; easing.type: Easing.InOutSine }
      NumberAnimation { from: face.breathMax; to: 1; duration: 1200; easing.type: Easing.InOutSine }
    }

    // [colour, way round]: the fill, then the spots.
    Repeater {
      model: [[1, 0], [0, 1], [2, -1]]

      Shape {
        id: sheet
        required property var modelData
        readonly property color tint: disc.colors[modelData[0]]
        // A spot starts top-left (the other bottom-right) and circles a third of the radius off the middle.
        readonly property real a: (225 + disc.spin * modelData[1] + (modelData[1] < 0 ? 180 : 0)) * Math.PI / 180
        readonly property real cx: face.size * (0.5 + 0.22 * Math.cos(a) * Math.abs(modelData[1]))
        readonly property real cy: face.size * (0.5 + 0.22 * Math.sin(a) * Math.abs(modelData[1]))
        anchors.fill: parent
        preferredRendererType: Shape.CurveRenderer

        ShapePath {
          strokeColor: "transparent"
          fillColor: sheet.modelData[1] ? "transparent" : sheet.tint
          fillGradient: sheet.modelData[1] ? spot : null
          PathAngleArc {
            centerX: face.size / 2
            centerY: face.size / 2
            radiusX: face.size / 2
            radiusY: radiusX
            startAngle: 0
            sweepAngle: 360
          }
        }

        RadialGradient {
          id: spot
          centerX: sheet.cx
          centerY: sheet.cy
          centerRadius: face.size * 0.6
          focalX: sheet.cx
          focalY: sheet.cy
          GradientStop { position: 0; color: sheet.tint }
          GradientStop { position: 1; color: Qt.rgba(sheet.tint.r, sheet.tint.g, sheet.tint.b, 0) }
        }
      }
    }
  }

  // loading (whisper warms up): a dashed ring that turns.
  Shape {
    anchors.centerIn: parent
    width: face.size
    height: face.size
    visible: face.mode === "loading"
    preferredRendererType: Shape.CurveRenderer

    ShapePath {
      strokeColor: Color.accent
      strokeWidth: face.line
      strokeStyle: ShapePath.DashLine
      dashPattern: [2, 2]
      capStyle: ShapePath.FlatCap
      fillColor: "transparent"
      PathAngleArc {
        centerX: face.size / 2
        centerY: face.size / 2
        radiusX: (face.size - face.line) / 2
        radiusY: radiusX
        startAngle: 0
        sweepAngle: 360
      }
    }

    RotationAnimation on rotation {
      from: 0
      to: 360
      duration: 1000
      loops: Animation.Infinite
      running: face.mode === "loading"
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
