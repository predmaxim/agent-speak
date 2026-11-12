import QtQuick
import QtQuick.Shapes
import qs.Commons
import qs.Commons as Commons
import "Model.js" as Model

// The orb's circle in one of Model.orbMode's modes, centred in its slot (the
// slot leaves room for the breath). Orb.qml draws it in the screen corner,
// Window.qml in its header.
Item {
  id: face

  property string mode: "off"
  property int size: Style.space(48)
  property int line: Math.max(1, Style.space(3))
  property real breathMax: 1.12

  // What the orb knows besides its mode (Orb.qml feeds them all; the window gives some): a plain face without them.
  property string rate: "medium"      // the tempo she speaks at (the wheel's rate_now or the setting): the pulse follows it
  property real level: -1             // the ear meter while listening (Model.level of her input), 0..1; −1 — no node
  property var listenUntil: null      // she still listens until (epoch, s) for listenSecs — the arc on the rim
  property real listenSecs: 0
  property bool ring: false           // a done agent not seen yet: a still green ring on the rim
  property bool droop: false          // an agent failed 3+ tool calls this turn: smaller and paler (standing modes only)
  property bool limit: false          // a limit window near its end: a still yellow arc on the rim
  property bool awake: false          // the pointer is over it: it wakes up a little
  property bool idleMotion: false     // a rare breath at rest (orb_idle_motion, on mains, not under Oma's window)
  property color ringColor: "#57ab5a"  // palette.json green
  property color limitColor: "#d4a72c" // palette.json yellow
  property color alertColor: "#e5534b" // palette.json red

  // Own colours (rules.md, the agent-speak section): a theme has no roles for
  // a gradient. At rest and paused: the theme's accent, flat; listening green
  // to yellow, thinking yellow, speaking blue, waiting red, the watch needing
  // the user orange, each with a thin outline icon in the middle. At rest,
  // paused, waiting, trouble and with the mic off the disc stands
  // still: they may last for hours, and a flow repaints every frame (at rest
  // only a rare breath, on mains, and short moves on events).
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
    waiting: { colors: ["#ff9a8f", "#e5484d", "#a8262c"], icon: "question", speed: 0, swing: 0, period: 3000 },
    trouble: { colors: ["#ffd08a", "#f5a524", "#c26a00"], icon: "alert", speed: 0, swing: 0, period: 3000 },
    // the mic is off while she hears: grey and still, the crossed mic
    muted: { colors: [Qt.lighter(Commons.Color.muted, 1.2), Commons.Color.muted, Qt.darker(Commons.Color.muted, 1.35)], flat: true,
      icon: "mic-off", speed: 0, swing: 0, period: 3000 }
  })
  readonly property var look: looks[face.mode] || looks.idle

  // Long modes stand: the ring, the limit arc and the droop belong to them; the rim's listening arc and the loading
  // dashes to the others.
  readonly property bool standing: ["idle", "paused", "waiting", "trouble", "quiet"].indexOf(face.mode) >= 0
  readonly property bool drooping: face.droop && ["idle", "waiting", "trouble"].indexOf(face.mode) >= 0
  function pale(c) { return drooping ? Qt.tint(c, Qt.rgba(Commons.Color.muted.r, Commons.Color.muted.g, Commons.Color.muted.b, 0.4)) : c }

  // A change of mode flows over: colours, speed, breath and shine slide to the
  // new look rather than jump.
  property color c0: pale(look.colors[0])
  property color c1: pale(look.colors[1])
  property color c2: pale(look.colors[2])
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

  // The speech pulse by her tempo: slower speech, a slower beat.
  readonly property real period: look.period * (face.mode === "speaking" ? ({ "x-slow": 1.35, slow: 1.15, fast: 0.87, "x-fast": 0.75 })[face.rate] || 1 : 1)

  // The flow's angle and the breath's phases step on with the frames, so a
  // new speed or period goes on from where they are. Listening, the frames also
  // ease the ear meter down (up at once, down in ~150 ms) and count the rim's arc down.
  property real spin: 0
  property real phase: 0
  property real phase2: 0   // the echo's own phase, 1.618 times as fast: never in step, no metronome
  property real heard: -1   // the ear meter as shown
  property real remain: -1  // the listening arc, 0..1
  FrameAnimation {
    running: face.speed > 0.5 || face.swing > 0.001
    onTriggered: {
      face.spin = (face.spin + face.speed * frameTime) % 360
      var step = 2 * Math.PI * frameTime * 1000 / face.period
      face.phase = (face.phase + step) % (2 * Math.PI)
      face.phase2 = (face.phase2 + 1.618 * step) % (2 * Math.PI)
      face.heard = face.level < 0 ? -1 : face.level >= face.heard ? face.level
        : face.heard + (face.level - face.heard) * (1 - Math.exp(-frameTime / 0.15))
      face.remain = Model.listenLeft(face.listenUntil, face.listenSecs, Date.now() / 1000)
    }
  }
  // 0..1: a beat with an echo, livelier than a plain sine.
  readonly property real beat: 0.65 * (1 - Math.cos(phase)) / 2 + 0.35 * (1 - Math.cos(phase2)) / 2

  // Short moves on events, each a factor of the disc's scale (their product stays within breathMax): a push as the
  // mic opens, waking under the pointer, a rare breath at rest. Durations through Style.duration: «less motion» is none.
  property real pop: 0
  property real inhale: 0
  property real wake: face.awake ? 1 : 0
  Behavior on wake { NumberAnimation { duration: Style.duration(180); easing.type: Easing.OutCubic } }

  property string was: "off"
  onModeChanged: {
    var hear = ["listening", "loading"]
    if (hear.indexOf(face.mode) >= 0 && hear.indexOf(face.was) < 0) popMove.restart()
    face.was = face.mode
  }
  SequentialAnimation {
    id: popMove
    NumberAnimation { target: face; property: "pop"; to: 1; duration: Style.duration(120); easing.type: Easing.OutCubic }
    NumberAnimation { target: face; property: "pop"; to: 0; duration: Style.duration(180); easing.type: Easing.InOutSine }
  }

  // At rest, on mains: one slow breath in 45 s (rules.md §9 — the user asked to bring the orb to life).
  Timer {
    interval: 45000
    repeat: true
    running: face.idleMotion && face.mode === "idle" && !Style.reduceMotion
    onTriggered: breath.restart()
  }
  SequentialAnimation {
    id: breath
    NumberAnimation { target: face; property: "inhale"; to: 1; duration: Style.duration(800); easing.type: Easing.InOutSine }
    NumberAnimation { target: face; property: "inhale"; to: 0; duration: Style.duration(800); easing.type: Easing.InOutSine }
  }

  // Listening with her input found, its level is the breath; otherwise the mode's breath.
  readonly property real grow: face.mode === "listening" && face.heard >= 0 ? 1 + 0.10 * face.heard : 1 + face.swing * face.beat
  readonly property real zoom: Math.min(face.breathMax, face.grow * (1 + 0.08 * face.pop) * (1 + 0.04 * face.wake)
    * (1 + 0.03 * face.inhale) * (face.drooping ? 0.94 : 1))

  // News that she does not say now: one wave from the centre to the rim in the event's colour.
  property real wave: 0
  property color waveColor: Commons.Color.accent
  function ripple(tone) {
    face.waveColor = ({ final: face.ringColor, question: face.alertColor })[tone] || Commons.Color.accent
    waveMove.restart()
  }
  NumberAnimation { id: waveMove; target: face; property: "wave"; from: 0; to: 1; duration: Style.duration(600); easing.type: Easing.OutCubic }

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
    scale: face.zoom
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
          GradientStop { position: 0; color: Qt.rgba(1, 1, 1, 0.35 * face.shine + 0.35 * face.wake) }
          GradientStop { position: 1; color: Qt.rgba(1, 1, 1, 0) }
        }
        PathAngleArc { centerX: disc.r; centerY: disc.r; radiusX: disc.r; radiusY: disc.r; sweepAngle: 360 }
      }
    }

    // Every icon is there, the mode's one shows: a change fades one into the other.
    Repeater {
      model: ["mic", "brain", "speaker", "pause", "question", "alert", "mic-off"]

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

  // The rim: over the disc and apart from it (in quiet the disc is gone, the rim stays), breathing with it. The done
  // ring and the limit arc over it stand in the long modes; the arc of how long she still listens runs down from the
  // top while she listens and nothing is heard. All still: no frames of their own.
  Item {
    anchors.centerIn: parent
    width: face.size
    height: face.size
    scale: face.zoom

    RimArc { side: face.size; stroke: face.line; shown: face.ring && face.standing; tint: face.ringColor }
    RimArc { side: face.size; stroke: face.line; shown: face.limit && face.standing; tint: face.limitColor; start: 0; sweep: 90 }
    RimArc { side: face.size; stroke: face.line; shown: face.mode === "listening" && face.remain >= 0; tint: face.c0; sweep: 360 * Math.max(0, face.remain) }
  }

  // The wave of unspoken news, out of the disc: it shows in quiet too.
  Rectangle {
    anchors.centerIn: parent
    visible: waveMove.running
    width: face.size * face.wave
    height: width
    radius: width / 2
    color: "transparent"
    border.width: face.line
    border.color: face.waveColor
    opacity: 1 - face.wave
  }

  // An inline component sees no ids of the file: its size and line come in.
  component RimArc: Shape {
    id: arc
    property real side: 0
    property real stroke: 1
    property bool shown: false
    property color tint: "transparent"
    property real start: -90
    property real sweep: 360
    anchors.fill: parent
    opacity: arc.shown ? 1 : 0
    visible: opacity > 0
    preferredRendererType: Shape.CurveRenderer
    Behavior on opacity { NumberAnimation { duration: Style.duration(400) } }

    ShapePath {
      strokeColor: arc.tint
      strokeWidth: arc.stroke
      capStyle: ShapePath.FlatCap
      fillColor: "transparent"
      PathAngleArc {
        centerX: arc.side / 2
        centerY: arc.side / 2
        radiusX: (arc.side - arc.stroke) / 2
        radiusY: radiusX
        startAngle: arc.start
        sweepAngle: arc.sweep
      }
    }
  }
}
