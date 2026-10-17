import QtQuick
import QtQuick.Effects
import QtQuick.Shapes
import Quickshell
import Quickshell.Hyprland
import Quickshell.Wayland
import qs.Commons
import qs.Ui
import "Model.js" as Model
import "I18n.js" as I18n

// Oma's orb: a circle in a corner of the focused screen with up to three
// bubbles beside it. It lives on the Top layer, so a fullscreen window covers
// it, and clicks pass through everything but the circle and the bubbles. The
// window is only as big as its content, so the circle's animations repaint a
// few pixels, not the screen; while the circle is dragged the window spans
// the screen, and the drop picks the nearest corner.
PanelWindow {
  id: win

  property var link
  property var bubbles: []
  signal dismissed(string key)
  signal windowToggled()

  readonly property var tr: I18n.translator(I18n.textLanguage(function(name) { return Quickshell.env(name) }))
  readonly property string mode: Model.orbMode(link.speech)
  readonly property var tip: Model.orbTip(link.speech)

  readonly property int size: Style.space(48)
  readonly property real breathMax: 1.12
  readonly property int slotSize: Math.ceil(size * breathMax)   // room for the breath
  readonly property int line: Math.max(1, Style.space(3))
  readonly property int gap: Style.space(16)
  readonly property int spacing: Style.space(8)

  // The corner from the daemon; a drop shows its corner before the daemon confirms it.
  readonly property string saved: link.speech.orb_corner || "bottom-right"
  property string dropped: ""
  onSavedChanged: dropped = ""
  readonly property string corner: dropped || saved
  readonly property bool atTop: corner.indexOf("top") === 0
  readonly property bool atLeft: corner.indexOf("left") > 0

  // The window's top-left on the screen. Margins are `gap` on every side, so
  // this holds both for the corner window and for the screen-wide one of a drag.
  readonly property real originX: atLeft ? gap : (screen ? screen.width : 0) - gap - width
  readonly property real originY: atTop ? gap : (screen ? screen.height : 0) - gap - height

  property bool dragging: false
  property real dragX: 0   // the circle's slot on the screen while dragged
  property real dragY: 0
  property bool tipShown: false

  function drop() {
    win.dropped = Model.corner(win.dragX + win.slotSize / 2, win.dragY + win.slotSize / 2, win.screen.width, win.screen.height)
    win.link.send(Model.set("orb_corner", win.dropped))
    win.dragging = false
  }

  screen: {
    var name = Hyprland.focusedMonitor ? Hyprland.focusedMonitor.name : ""
    var list = Quickshell.screens
    for (var i = 0; i < list.length; i++) if (list[i].name === name) return list[i]
    return null
  }
  color: "transparent"
  exclusionMode: ExclusionMode.Ignore
  WlrLayershell.namespace: "predmaxim-oma-orb"
  WlrLayershell.layer: WlrLayer.Top
  anchors {
    top: win.dragging || win.atTop
    bottom: win.dragging || !win.atTop
    left: win.dragging || win.atLeft
    right: win.dragging || !win.atLeft
  }
  margins { top: win.gap; bottom: win.gap; left: win.gap; right: win.gap }
  implicitWidth: win.slotSize + (win.bubbles.length ? win.spacing + column.width : 0)
  implicitHeight: Math.max(win.slotSize, column.height)

  mask: Region {
    Region { item: orbSlot; shape: RegionShape.Ellipse }
    Region { item: slots.count > 0 && slots.itemAt(0).visible ? slots.itemAt(0) : null }
    Region { item: slots.count > 1 && slots.itemAt(1).visible ? slots.itemAt(1) : null }
    Region { item: slots.count > 2 && slots.itemAt(2).visible ? slots.itemAt(2) : null }
  }

  Item {
    id: orbSlot
    width: win.slotSize
    height: win.slotSize
    x: win.dragging ? win.dragX - win.originX : (win.atLeft ? 0 : parent.width - width)
    y: win.dragging ? win.dragY - win.originY : (win.atTop ? 0 : parent.height - height)

    // off, idle, paused: an outline; listening, speaking, waiting: filled.
    Rectangle {
      readonly property bool filled: ["listening", "speaking", "waiting"].indexOf(win.mode) >= 0
      property real breath: 1
      anchors.centerIn: parent
      width: win.size
      height: win.size
      radius: width / 2
      visible: ["off", "idle", "listening", "speaking", "paused", "waiting"].indexOf(win.mode) >= 0
      color: !filled ? "transparent" : win.mode === "waiting" ? Color.urgent : Color.accent
      border.width: filled ? 0 : win.line
      border.color: win.mode === "off" || win.mode === "idle" ? Color.muted : Color.accent
      opacity: win.mode === "off" ? 0.4 : win.mode === "paused" ? 0.5 : 1
      scale: win.mode === "listening" ? breath : 1

      SequentialAnimation on breath {
        running: win.mode === "listening"
        loops: Animation.Infinite
        NumberAnimation { from: 1; to: win.breathMax; duration: 1200; easing.type: Easing.InOutSine }
        NumberAnimation { from: win.breathMax; to: 1; duration: 1200; easing.type: Easing.InOutSine }
      }
    }

    // loading: a dashed outline; thinking: an arc. Both turn.
    Shape {
      anchors.centerIn: parent
      width: win.size
      height: win.size
      visible: win.mode === "loading" || win.mode === "thinking"
      preferredRendererType: Shape.CurveRenderer

      ShapePath {
        strokeColor: Color.accent
        strokeWidth: win.line
        strokeStyle: win.mode === "loading" ? ShapePath.DashLine : ShapePath.SolidLine
        dashPattern: [2, 2]
        capStyle: win.mode === "loading" ? ShapePath.FlatCap : ShapePath.RoundCap
        fillColor: "transparent"
        PathAngleArc {
          centerX: win.size / 2
          centerY: win.size / 2
          radiusX: (win.size - win.line) / 2
          radiusY: radiusX
          startAngle: 0
          sweepAngle: win.mode === "loading" ? 360 : 90
        }
      }

      RotationAnimation on rotation {
        from: 0
        to: 360
        duration: 1000
        loops: Animation.Infinite
        running: win.mode === "loading" || win.mode === "thinking"
      }
    }

    Text {
      anchors.centerIn: parent
      visible: win.mode === "quiet"
      textFormat: Text.PlainText
      text: "\u{F0F65}"   // 󰽥
      color: Color.foreground
      font.family: Style.font.family
      font.pixelSize: Math.round(win.size * 0.75)
    }

    // Left click talks, right click toggles the window; a left drag past
    // Style.space(8) carries the circle and the drop saves the nearest corner.
    MouseArea {
      id: orbArea
      anchors.fill: parent
      hoverEnabled: true
      acceptedButtons: Qt.LeftButton | Qt.RightButton
      cursorShape: Qt.PointingHandCursor
      property point grab     // the press inside the slot
      property point start    // the press on the screen

      function onScreen(mouse) {
        var p = orbArea.mapToItem(null, mouse.x, mouse.y)
        return Qt.point(win.originX + p.x, win.originY + p.y)
      }

      onContainsMouseChanged: {
        win.tipShown = false
        if (containsMouse) tipDelay.restart()
        else tipDelay.stop()
      }
      onPressed: function(mouse) {
        win.tipShown = false
        tipDelay.stop()
        grab = Qt.point(mouse.x, mouse.y)
        start = onScreen(mouse)
      }
      onPositionChanged: function(mouse) {
        if (!(pressedButtons & Qt.LeftButton)) return
        var p = onScreen(mouse)
        if (!win.dragging && Math.hypot(p.x - start.x, p.y - start.y) <= Style.space(8)) return
        win.dragX = Math.max(win.gap, Math.min(win.screen.width - win.gap - win.slotSize, p.x - grab.x))
        win.dragY = Math.max(win.gap, Math.min(win.screen.height - win.gap - win.slotSize, p.y - grab.y))
        win.dragging = true
      }
      onReleased: function(mouse) {
        if (win.dragging) win.drop()
        else if (mouse.button === Qt.LeftButton) win.link.send(Model.omaCmd({ do: "talk" }))
        else if (mouse.button === Qt.RightButton) win.windowToggled()
      }
      onCanceled: win.dragging = false
    }
  }

  // The tooltip has its own popup window: the orb's window is too small for it.
  Timer {
    id: tipDelay
    interval: 400
    onTriggered: win.tipShown = true
  }

  PopupWindow {
    visible: win.tipShown && orbArea.containsMouse && !orbArea.pressed
    color: "transparent"
    implicitWidth: Math.ceil(tipCard.implicitWidth)
    implicitHeight: Math.ceil(tipCard.implicitHeight)
    anchor.item: orbSlot
    anchor.edges: (win.atTop ? Edges.Bottom : Edges.Top) | (win.atLeft ? Edges.Left : Edges.Right)
    anchor.gravity: (win.atTop ? Edges.Bottom : Edges.Top) | (win.atLeft ? Edges.Right : Edges.Left)
    anchor.adjustment: PopupAdjustment.Slide

    BorderSurface {
      id: tipCard
      implicitWidth: tipLabel.implicitWidth + 20
      implicitHeight: tipLabel.implicitHeight + 14
      color: Color.tooltip.background
      borderSpec: Border.surfaceSpec("tooltip", "border", Color.tooltip.border, 1)
      radius: Style.cornerRadius

      Text {
        id: tipLabel
        anchors.centerIn: parent
        textFormat: Text.PlainText
        text: win.tr(win.tip[0], win.tip[1])
        color: Color.tooltip.text
        font.family: Style.font.family
        font.pixelSize: Style.font.body
      }
    }
  }

  // Bubbles on the orb's side towards the middle of the screen, the newest
  // next to it: at the column's bottom in a bottom corner, at its top in a top one.
  Column {
    id: column
    spacing: win.spacing
    x: win.atLeft ? orbSlot.x + orbSlot.width + win.spacing : orbSlot.x - win.spacing - width
    y: win.atTop ? orbSlot.y : orbSlot.y + orbSlot.height - height

    Repeater {
      id: slots
      model: Model.BUBBLES_MAX

      Item {
        id: slot
        required property int index
        readonly property var bubble: win.bubbles[win.atTop ? win.bubbles.length - 1 - index : index] || null
        readonly property bool draft: bubble !== null && bubble.kind === "draft"
        readonly property bool faded: win.bubbles.length >= Model.BUBBLES_MAX && bubble === win.bubbles[0]
        readonly property real inner: Math.min(Style.space(360) - card.contentLeftInset - card.contentRightInset,
          Math.max(who.implicitWidth, body.implicitWidth, draft ? send.implicitWidth : 0))
        visible: bubble !== null
        x: win.atLeft ? 0 : column.width - width
        width: card.width
        height: card.height

        BorderSurface {
          id: card
          width: slot.inner + contentLeftInset + contentRightInset
          height: content.implicitHeight + contentTopInset + contentBottomInset
          color: Color.popups.background
          borderSpec: Border.surfaceSpec("popups", "border", Color.popups.border, Math.max(1, Style.space(2)))
          padding: Style.spacing.xxl
          radius: Style.cornerRadius
          layer.enabled: slot.faded
          layer.effect: MultiEffect {
            maskEnabled: true
            maskSource: fadeMask
            maskThresholdMin: 0.5
            maskSpreadAtMin: 1.0
          }

          MouseArea {
            anchors.fill: parent
            cursorShape: Qt.PointingHandCursor
            onClicked: win.dismissed(slot.bubble.id)
          }

          Column {
            id: content
            x: card.contentLeftInset
            y: card.contentTopInset
            width: slot.inner
            spacing: Style.space(4)

            Text {
              id: who
              width: parent.width
              textFormat: Text.PlainText
              text: slot.bubble ? slot.bubble.who || "" : ""
              color: Color.popups.text
              font.family: Style.font.family
              font.pixelSize: Style.font.body
              font.bold: true
              elide: Text.ElideRight
            }

            Text {
              id: body
              width: parent.width
              textFormat: Text.PlainText
              text: slot.bubble ? slot.bubble.text || "" : ""
              color: Color.popups.text
              font.family: Style.font.family
              font.pixelSize: Style.font.title
              wrapMode: Text.Wrap
            }

            Button {
              id: send
              visible: slot.draft
              text: win.tr("Send")
              fontSize: Style.font.title
              foreground: Color.popups.text
              bordered: true
              onClicked: win.link.send(Model.omaCmd({ do: "send" }))
            }
          }
        }

        // The oldest bubble's alpha, read by its MultiEffect: OLDEST_NEAR at
        // the edge towards its neighbour, OLDEST_FAR at the far one.
        Rectangle {
          id: fadeMask
          anchors.fill: card
          visible: false
          layer.enabled: slot.faded
          gradient: Gradient {
            GradientStop { position: 0; color: Qt.rgba(0, 0, 0, Model.maskAlpha(win.atTop ? Model.OLDEST_NEAR : Model.OLDEST_FAR)) }
            GradientStop { position: 1; color: Qt.rgba(0, 0, 0, Model.maskAlpha(win.atTop ? Model.OLDEST_FAR : Model.OLDEST_NEAR)) }
          }
        }
      }
    }
  }
}
