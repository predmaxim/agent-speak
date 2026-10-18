import QtQuick
import QtQuick.Shapes
import qs.Commons
import qs.Commons as Commons

// The orb's circle in one of Model.orbMode's modes, centred in its slot (the
// slot leaves room for the breath). Orb.qml draws it in the screen corner,
// Window.qml in its header.
Item {
  id: face

  property string mode: "off"
  property bool hearing: false   // listening, and a voice is heard (Oma's draft waits for it)
  property int size: Style.space(48)
  property int line: Math.max(1, Style.space(3))
  property real breathMax: 1.12

  // Own colours (rules.md, the agent-speak section): a theme has no roles for
  // a gradient. Each mode has its hue: at rest a still indigo, listening
  // azure with a lit rim (brighter and quicker while a voice is heard),
  // thinking violet, speaking coral, paused grey, waiting the theme's alarm
  // (still: it may wait for hours, and a flow repaints every frame).
  // colors: [first, middle, last] across the disc; speed: the flow, °/s;
  // swing: the breath's scale (up to breathMax - 1) and its period, ms; rim: 0..1.
  readonly property var looks: ({
    idle: { colors: ["#5d7bff", "#3b33a8", "#7d55e6"], speed: 0, swing: 0, period: 3000, rim: 0 },
    loading: { colors: ["#4fd8ff", "#2b6fe8", "#6a5cff"], speed: 30, swing: 0, period: 3000, rim: 0 },
    listening: { colors: ["#4fd8ff", "#2b6fe8", "#6a5cff"], speed: 70, swing: 0.05, period: 1800, rim: 0.7 },
    hearing: { colors: ["#9ef3ff", "#3d97ff", "#8f6dff"], speed: 200, swing: 0.1, period: 600, rim: 1 },
    thinking: { colors: ["#d47bff", "#7a3cf0", "#ff5bc8"], speed: 120, swing: 0.02, period: 1800, rim: 0 },
    speaking: { colors: ["#ffc965", "#ff7a59", "#ff4f8b"], speed: 80, swing: 0.06, period: 480, rim: 0 },
    paused: { colors: ["#8c96bd", "#545b7a", "#787a99"], speed: 0, swing: 0, period: 3000, rim: 0 },
    waiting: { colors: [Qt.lighter(Commons.Color.urgent, 1.4), Commons.Color.urgent, Qt.darker(Commons.Color.urgent, 1.35)], speed: 0, swing: 0, period: 3000, rim: 0 }
  })
  readonly property var look: face.mode === "listening" && face.hearing ? looks.hearing : looks[face.mode] || looks.idle

  // A change of mode flows over: colours, speed, breath and rim slide to the
  // new look rather than jump.
  property color c0: look.colors[0]
  property color c1: look.colors[1]
  property color c2: look.colors[2]
  property real speed: look.speed
  property real swing: look.swing
  property real rim: look.rim
  Behavior on c0 { ColorAnimation { duration: 700; easing.type: Easing.InOutSine } }
  Behavior on c1 { ColorAnimation { duration: 700; easing.type: Easing.InOutSine } }
  Behavior on c2 { ColorAnimation { duration: 700; easing.type: Easing.InOutSine } }
  Behavior on speed { NumberAnimation { duration: 900; easing.type: Easing.InOutSine } }
  Behavior on swing { NumberAnimation { duration: 500; easing.type: Easing.InOutSine } }
  Behavior on rim { NumberAnimation { duration: 400; easing.type: Easing.InOutSine } }

  implicitWidth: Math.ceil(size * breathMax)
  implicitHeight: implicitWidth

  // The flow's angle and the breath's phase step on with the frames, so a
  // new speed or period goes on from where they are.
  property real spin: 0
  property real phase: 0
  FrameAnimation {
    running: face.speed > 0.5 || face.swing > 0.001 || face.rim > 0.01
    onTriggered: {
      face.spin = (face.spin + face.speed * frameTime) % 360
      face.phase = (face.phase + 2 * Math.PI * frameTime * 1000 / face.look.period) % (2 * Math.PI)
    }
  }
  // 0..1: a beat with an echo, livelier than a plain sine.
  readonly property real beat: 0.65 * (1 - Math.cos(phase)) / 2 + 0.35 * (1 - Math.cos(2 * phase)) / 2

  // off: a dim ring.
  Rectangle {
    anchors.centerIn: parent
    width: face.size
    height: face.size
    radius: width / 2
    color: "transparent"
    border.width: face.line
    border.color: Commons.Color.muted
    opacity: face.mode === "off" ? 0.4 : 0
    Behavior on opacity { NumberAnimation { duration: 400 } }
  }

  // The gradient disc: the colours run across it and turn with the flow, a
  // soft spot of the last colour circles the other way, a still shine on top
  // gives it depth; the rim lights up while Oma listens.
  Item {
    id: disc
    anchors.centerIn: parent
    width: face.size
    height: face.size
    opacity: ({ off: 0, quiet: 0, paused: 0.6, loading: 0.45 })[face.mode] ?? 1
    scale: 1 + face.swing * face.beat
    Behavior on opacity { NumberAnimation { duration: 400 } }

    readonly property real r: face.size / 2
    readonly property real a: (225 + face.spin) * Math.PI / 180
    readonly property real b: (45 - face.spin * 1.6) * Math.PI / 180
    readonly property color glow: Qt.lighter(face.c0, 1.25)

    Shape {
      anchors.fill: parent
      preferredRendererType: Shape.CurveRenderer

      ShapePath {
        strokeColor: "transparent"
        fillGradient: LinearGradient {
          x1: disc.r + disc.r * Math.cos(disc.a)
          y1: disc.r + disc.r * Math.sin(disc.a)
          x2: disc.r - disc.r * Math.cos(disc.a)
          y2: disc.r - disc.r * Math.sin(disc.a)
          GradientStop { position: 0; color: face.c0 }
          GradientStop { position: 0.5; color: face.c1 }
          GradientStop { position: 1; color: face.c2 }
        }
        PathAngleArc { centerX: disc.r; centerY: disc.r; radiusX: disc.r; radiusY: disc.r; sweepAngle: 360 }
      }

      ShapePath {
        strokeColor: "transparent"
        fillGradient: RadialGradient {
          centerX: disc.r * (1 + 0.45 * Math.cos(disc.b))
          centerY: disc.r * (1 + 0.45 * Math.sin(disc.b))
          centerRadius: disc.r * 0.9
          focalX: centerX
          focalY: centerY
          GradientStop { position: 0; color: Qt.rgba(face.c2.r, face.c2.g, face.c2.b, 0.85) }
          GradientStop { position: 1; color: Qt.rgba(face.c2.r, face.c2.g, face.c2.b, 0) }
        }
        PathAngleArc { centerX: disc.r; centerY: disc.r; radiusX: disc.r; radiusY: disc.r; sweepAngle: 360 }
      }

      ShapePath {
        strokeColor: "transparent"
        fillGradient: RadialGradient {
          centerX: disc.r * 0.7
          centerY: disc.r * 0.6
          centerRadius: disc.r * 0.8
          focalX: centerX
          focalY: centerY
          GradientStop { position: 0; color: Qt.rgba(1, 1, 1, 0.35) }
          GradientStop { position: 1; color: Qt.rgba(1, 1, 1, 0) }
        }
        PathAngleArc { centerX: disc.r; centerY: disc.r; radiusX: disc.r; radiusY: disc.r; sweepAngle: 360 }
      }

      ShapePath {
        strokeColor: Qt.rgba(disc.glow.r, disc.glow.g, disc.glow.b, face.rim * (0.5 + 0.5 * face.beat))
        strokeWidth: face.line
        fillColor: "transparent"
        PathAngleArc {
          centerX: disc.r
          centerY: disc.r
          radiusX: disc.r - face.line / 2
          radiusY: radiusX
          sweepAngle: 360
        }
      }
    }
  }

  // loading (whisper warms up): a dashed ring turns over the dimmed disc.
  Shape {
    anchors.centerIn: parent
    width: face.size
    height: face.size
    opacity: face.mode === "loading" ? 1 : 0
    visible: opacity > 0
    preferredRendererType: Shape.CurveRenderer
    Behavior on opacity { NumberAnimation { duration: 400 } }

    ShapePath {
      strokeColor: face.c0
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
    opacity: face.mode === "quiet" ? 1 : 0
    textFormat: Text.PlainText
    text: "\u{F0F65}"   // 󰽥
    color: Commons.Color.foreground
    font.family: Style.font.family
    font.pixelSize: Math.round(face.size * 0.75)
    Behavior on opacity { NumberAnimation { duration: 400 } }
  }
}
