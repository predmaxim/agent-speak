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
  property int size: Style.space(48)
  property int line: Math.max(1, Style.space(3))
  property real breathMax: 1.12

  // Own colours (rules.md, the agent-speak section): a theme has no roles for
  // a gradient. At rest and paused: the theme's accent, flat; listening green
  // to yellow, thinking yellow, speaking blue, waiting red, each with a thin
  // outline icon in the middle. At rest, paused and waiting the disc stands
  // still: they may last for hours, and a flow repaints every frame.
  // colors: [first, middle, last] across the disc; speed: the flow, °/s;
  // swing: the breath's scale (up to breathMax - 1) and its period, ms.
  readonly property var rest: [Qt.lighter(Commons.Color.accent, 1.2), Commons.Color.accent, Qt.darker(Commons.Color.accent, 1.35)]
  readonly property var looks: ({
    idle: { colors: rest, flat: true, icon: "", speed: 0, swing: 0, period: 3000 },
    paused: { colors: rest, flat: true, icon: "pause", speed: 0, swing: 0, period: 3000 },
    loading: { colors: ["#e3f27a", "#6fd38c", "#2fae9a"], icon: "mic", speed: 30, swing: 0, period: 3000 },
    listening: { colors: ["#e3f27a", "#6fd38c", "#2fae9a"], icon: "mic", speed: 70, swing: 0.06, period: 1800 },
    thinking: { colors: ["#fff09a", "#ffc93d", "#f0971e"], icon: "brain", speed: 120, swing: 0.02, period: 1800 },
    speaking: { colors: ["#8fd3ff", "#3d8bff", "#4b56e8"], icon: "speaker", speed: 80, swing: 0.06, period: 480 },
    waiting: { colors: ["#ff9a8f", "#e5484d", "#a8262c"], icon: "question", speed: 0, swing: 0, period: 3000 }
  })
  readonly property var look: looks[face.mode] || looks.idle

  // A change of mode flows over: colours, speed, breath and shine slide to the
  // new look rather than jump.
  property color c0: look.colors[0]
  property color c1: look.colors[1]
  property color c2: look.colors[2]
  property real speed: look.speed
  property real swing: look.swing
  property real shine: look.flat ? 0 : 1
  Behavior on c0 { ColorAnimation { duration: 700; easing.type: Easing.InOutSine } }
  Behavior on c1 { ColorAnimation { duration: 700; easing.type: Easing.InOutSine } }
  Behavior on c2 { ColorAnimation { duration: 700; easing.type: Easing.InOutSine } }
  Behavior on speed { NumberAnimation { duration: 900; easing.type: Easing.InOutSine } }
  Behavior on swing { NumberAnimation { duration: 500; easing.type: Easing.InOutSine } }
  Behavior on shine { NumberAnimation { duration: 700; easing.type: Easing.InOutSine } }

  implicitWidth: Math.ceil(size * breathMax)
  implicitHeight: implicitWidth

  // The flow's angle and the breath's phase step on with the frames, so a
  // new speed or period goes on from where they are.
  property real spin: 0
  property real phase: 0
  FrameAnimation {
    running: face.speed > 0.5 || face.swing > 0.001
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
  // gives it depth (not at rest: flat), the mode's icon sits in the middle.
  Item {
    id: disc
    anchors.centerIn: parent
    width: face.size
    height: face.size
    opacity: ({ off: 0, quiet: 0, loading: 0.45 })[face.mode] ?? 1
    scale: 1 + face.swing * face.beat
    Behavior on opacity { NumberAnimation { duration: 400 } }

    readonly property real r: face.size / 2
    readonly property real a: (225 + face.spin) * Math.PI / 180
    readonly property real b: (45 - face.spin * 1.6) * Math.PI / 180

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
          GradientStop { position: 0; color: Qt.rgba(face.c2.r, face.c2.g, face.c2.b, 0.85 * face.shine) }
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
          GradientStop { position: 0; color: Qt.rgba(1, 1, 1, 0.35 * face.shine) }
          GradientStop { position: 1; color: Qt.rgba(1, 1, 1, 0) }
        }
        PathAngleArc { centerX: disc.r; centerY: disc.r; radiusX: disc.r; radiusY: disc.r; sweepAngle: 360 }
      }
    }

    // Every icon is there, the mode's one shows: a change fades one into the other.
    Repeater {
      model: ["mic", "brain", "speaker", "pause", "question"]

      Image {
        required property string modelData
        anchors.centerIn: parent
        width: Math.round(face.size * 0.5)
        height: width
        sourceSize: Qt.size(width, height)
        source: Qt.resolvedUrl("icons/" + modelData + ".svg")
        opacity: face.look.icon === modelData ? 0.95 : 0
        visible: opacity > 0
        Behavior on opacity { NumberAnimation { duration: 400 } }
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
