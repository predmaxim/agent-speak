import QtQuick
import QtQuick.Effects
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
// the free area of the screen; the drop keeps the place, near a corner it snaps.
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
  readonly property int textLines: 4   // a bubble's text beyond this is elided

  // The place from the daemon ("corner,dx,dy"); a drop shows its place before the daemon confirms it.
  readonly property string saved: link.speech.orb_pos || "bottom-right,0,0"
  property string dropped: ""
  onSavedChanged: dropped = ""
  // The screen is null for a moment after a focus switch.
  readonly property var pos: Model.parsePos(dropped || saved,
    (screen ? screen.width : 0) - 2 * gap, (screen ? screen.height : 0) - 2 * gap, slotSize)
  readonly property bool atTop: pos.atTop
  readonly property bool atLeft: pos.atLeft

  // A drag measures the pointer from the drag area's corner: at rest the
  // window sits pos.dx/dy off that corner, so fromCorner adds the offset back
  // and the numbers hold across the resize, wherever the bar's zone puts the area.
  property bool dragging: false
  // The offset lives in the slot only during a drag; at rest the margins carry it.
  readonly property real ox: dragging ? pos.dx : 0
  readonly property real oy: dragging ? pos.dy : 0
  property real dragDX: 0   // the pointer's way from the press
  property real dragDY: 0
  property bool tipShown: false

  // A focus switch moves the window to another screen and the drag with it is lost.
  onScreenChanged: dragging = false

  function drop() {
    win.dropped = Model.place(orbSlot.x, orbSlot.y, win.width, win.height, win.slotSize, Style.space(64))
    win.link.send(Model.set("orb_pos", win.dropped))
    win.dragging = false
  }

  screen: {
    var name = Hyprland.focusedMonitor ? Hyprland.focusedMonitor.name : ""
    var list = Quickshell.screens
    for (var i = 0; i < list.length; i++) if (list[i].name === name) return list[i]
    return null
  }
  color: "transparent"
  // Below the bar's zone, reserving none of its own (rules.md §9).
  exclusionMode: ExclusionMode.Normal
  exclusiveZone: 0
  WlrLayershell.namespace: "predmaxim-oma-orb"
  WlrLayershell.layer: WlrLayer.Top
  anchors {
    top: win.dragging || win.atTop
    bottom: win.dragging || !win.atTop
    left: win.dragging || win.atLeft
    right: win.dragging || !win.atLeft
  }
  // Out of a drag only the two anchored edges carry the offset.
  margins {
    top: win.gap + (!win.dragging && win.atTop ? win.pos.dy : 0)
    bottom: win.gap + (!win.dragging && !win.atTop ? win.pos.dy : 0)
    left: win.gap + (!win.dragging && win.atLeft ? win.pos.dx : 0)
    right: win.gap + (!win.dragging && !win.atLeft ? win.pos.dx : 0)
  }
  implicitWidth: win.slotSize + (win.bubbles.length ? win.spacing + column.width : 0)
  implicitHeight: Math.max(win.slotSize, column.height)

  mask: Region {
    Region { item: orbSlot; shape: RegionShape.Ellipse }
    Region { item: slots.itemAt(0) && slots.itemAt(0).visible ? slots.itemAt(0) : null }
    Region { item: slots.itemAt(1) && slots.itemAt(1).visible ? slots.itemAt(1) : null }
    Region { item: slots.itemAt(2) && slots.itemAt(2).visible ? slots.itemAt(2) : null }
  }

  Item {
    id: orbSlot
    width: win.slotSize
    height: win.slotSize
    x: Math.max(0, Math.min(parent.width - width, (win.atLeft ? win.ox : parent.width - width - win.ox) + (win.dragging ? win.dragDX : 0)))
    y: Math.max(0, Math.min(parent.height - height, (win.atTop ? win.oy : parent.height - height - win.oy) + (win.dragging ? win.dragDY : 0)))

    OrbFace {
      anchors.fill: parent
      mode: win.mode
      size: win.size
      line: win.line
      breathMax: win.breathMax
    }

    // Left click talks, right click toggles the window; a left drag past
    // Style.space(8) carries the circle and the drop keeps the place.
    MouseArea {
      id: orbArea
      anchors.fill: parent
      hoverEnabled: true
      acceptedButtons: Qt.LeftButton | Qt.RightButton
      cursorShape: Qt.PointingHandCursor
      property point start    // the press, from the anchored corner
      property bool moved     // this press became a drag, even one cut short by a screen switch

      function fromCorner(mouse) {
        var p = orbArea.mapToItem(null, mouse.x, mouse.y)
        var rx = win.dragging ? 0 : win.pos.dx   // the window's offset from the area corner
        var ry = win.dragging ? 0 : win.pos.dy
        return Qt.point(win.atLeft ? p.x + rx : p.x - win.width - rx, win.atTop ? p.y + ry : p.y - win.height - ry)
      }

      onContainsMouseChanged: {
        win.tipShown = false
        if (containsMouse) tipDelay.restart()
        else tipDelay.stop()
      }
      onPressed: function(mouse) {
        win.tipShown = false
        tipDelay.stop()
        start = fromCorner(mouse)
        moved = false
      }
      onPositionChanged: function(mouse) {
        if (!(pressedButtons & Qt.LeftButton)) return
        var p = fromCorner(mouse)
        if (!win.dragging && Math.hypot(p.x - start.x, p.y - start.y) <= Style.space(8)) return
        win.dragDX = p.x - start.x
        win.dragDY = p.y - start.y
        win.dragging = true
        moved = true
      }
      onReleased: function(mouse) {
        if (win.dragging) win.drop()
        else if (moved) return
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
        readonly property string tone: slot.bubble ? Model.tone(slot.bubble.kind) : "agent"
        readonly property color base: tone === "me" ? Color.accent : Color.foreground
        readonly property string shown: {
          if (!slot.bubble) return ""
          var t = slot.bubble.text || ""
          if (slot.bubble.kind !== "draft" || !slot.bubble.hearing) return t
          return t ? t + " …" : "…"
        }
        readonly property bool faded: win.bubbles.length >= Model.BUBBLES_MAX && bubble === win.bubbles[0]
        readonly property real inner: Math.min(Style.space(360) - card.contentLeftInset - card.contentRightInset,
          Math.max(who.implicitWidth, body.implicitWidth))
        visible: bubble !== null
        x: win.atLeft ? 0 : column.width - width
        width: card.width
        height: card.height

        BorderSurface {
          id: card
          width: slot.inner + contentLeftInset + contentRightInset
          height: content.implicitHeight + contentTopInset + contentBottomInset
          color: Qt.tint(Color.popups.background, Qt.rgba(slot.base.r, slot.base.g, slot.base.b, Model.TONE_ALPHA[slot.tone]))
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
            // A draft is cleared by Esc or the orb, not by a click.
            onClicked: if (slot.bubble.kind !== "draft") win.dismissed(slot.bubble.id)
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
              text: slot.shown
              color: Color.popups.text
              font.family: Style.font.family
              font.pixelSize: Style.font.title
              wrapMode: Text.Wrap
              maximumLineCount: win.textLines
              elide: Text.ElideRight
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
