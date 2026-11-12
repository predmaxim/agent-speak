import QtQuick
import QtQuick.Effects
import Quickshell
import Quickshell.Hyprland
import Quickshell.Services.Pipewire
import Quickshell.Services.UPower
import Quickshell.Wayland
import qs.Commons
import qs.Ui
import "Model.js" as Model
import "I18n.js" as I18n

// Oma's orb: a circle anywhere on the focused screen with up to three
// bubbles beside it. It lives on the Overlay layer, over fullscreen windows
// too (the orb_over_fullscreen setting off: on Top, under them), and clicks
// pass through everything but the circle and the bubbles. The window always
// spans the free area of the screen and never changes its anchors or size: a
// drag only moves the circle inside it (a window that resized and re-anchored
// for a drag made the compositor show a frame of the old buffer — the orb
// jumped to the middle of the screen or blinked). The cost: while the circle
// animates (a conversation, short moves on events, a rare breath at rest on
// mains) the whole window repaints. The drop keeps the place, near a corner it snaps.
PanelWindow {
  id: win

  property var link
  property var bubbles: []
  property bool covered: false   // Oma's window is open
  property string openChat: ""   // the chat Oma's window shows ("agent:<id>" …), "" while it is closed
  signal dismissed(string key)
  signal windowToggled()
  signal chatOpened(string key)   // a bubble's chat: "agent:<id>", or "" for Oma's current one
  signal attached(string key, string text)   // a drop: on a satellite its chat "agent:<id>", on the orb "general"; the text for the field

  // Fixed status colours (FixedColors.qml, rules.md §9): a theme's red or yellow can be any colour.
  readonly property QtObject fixedColors: FixedColors {}

  readonly property var tr: I18n.translator(I18n.textLanguage(function(name) { return Quickshell.env(name) }))
  readonly property string mode: Model.orbMode(link.speech, micMuted)
  readonly property var tip: Model.orbTip(link.speech, micMuted)

  // The ear (ear-meter): her input — the daemon's ear_in, or the default one — its level while she listens and its
  // mute, read as the stock audio panel and microphone widget do. The mute key mutes the default input only, and
  // without a headset she records the laptop's mic past it: off is either one off. A virtual default input has no
  // PwNode.audio in Quickshell: its mute through the stock UntypedInput, and no level meter (as the stock panel).
  readonly property var defaultSource: Pipewire.defaultAudioSource
  readonly property var earNode: {
    var name = link.speech.ear_in || ""
    if (!name) return defaultSource
    var list = Pipewire.nodes.values
    for (var i = 0; i < list.length; i++) if (list[i].name === name) return list[i]
    return null
  }
  PwObjectTracker { objects: [win.defaultSource, win.earNode].filter(function(n) { return !!n }) }
  readonly property bool defaultMuted: UntypedInput.active ? UntypedInput.known && UntypedInput.muted
    : !!(defaultSource && defaultSource.audio && defaultSource.audio.muted)
  readonly property bool micMuted: defaultMuted || !!(earNode && earNode.audio && earNode.audio.muted)
  // Its own capture stream (quickshell-peak-monitor), only while she listens; not someone else's mic for Oma and
  // presence (the stock isShellLevelMeter rule).
  PwNodePeakMonitor {
    id: earPeak
    node: win.earNode && win.earNode.audio ? win.earNode : null
    enabled: win.mode === "listening" && node !== null
  }

  // The «done» ring until seen (Model.seenDone): a done agent's tab in focus or its chat open in Oma's window.
  property var seen: ({})
  function markSeen() { win.seen = Model.seenDone(win.seen, win.link.agents, win.covered ? win.openChat : "") }
  onOpenChatChanged: markSeen()
  onCoveredChanged: markSeen()
  readonly property bool moods: link.speech.orb_moods === true && link.speech.agents_voice === true

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
  readonly property var pos: Model.parsePos(dropped || saved, width, height, slotSize)
  readonly property bool atTop: pos.atTop
  readonly property bool atLeft: pos.atLeft

  property bool dragging: false
  property real dragDX: 0   // the pointer's way from the press
  property real dragDY: 0
  property bool tipShown: false

  // The hover card (orb-peek): the tooltip's lines, then the agents, the most urgent first — only while watching
  // them. Its «min» is counted at its showing (peekNow, s) and does not tick while it is open.
  property real peekNow: 0
  readonly property var peek: Model.peekRows(link.speech.agents_voice === true ? link.agents : [], peekNow)
  readonly property int peekWidth: Style.space(300)

  // Satellites (orb-satellites): a dot per agent on a quarter arc on the orb's side away from the screen's top or
  // bottom edge, within the slot's width — clear of the bubbles beside it and of the window's edge. An agent keeps its
  // dot's place (Model.satSlots); none while not watching agents, with the setting off or under Oma's window.
  readonly property bool satsShown: link.speech.agents_voice === true && link.speech.orb_satellites === true && !win.covered
  property var sats: []   // Model.satSlots: places hold from one agent line to the next
  // The chat Oma's conversation is in (her `chat` line with was "*", sent as each conversation starts): while she
  // listens there, that agent's dot is ringed — a hold on it took.
  property string talkKey: ""
  Component.onCompleted: sats = Model.satSlots(sats, link.agents, Model.SAT_MAX)
  Connections {
    target: win.link
    function onAgentsChanged() {
      win.sats = Model.satSlots(win.sats, win.link.agents, Model.SAT_MAX)
      win.markSeen()
    }
    // news she does not say now: one wave instead of words (the daemon sends it only with orb_moods on)
    function onRipple(r) { orbFace.ripple(r.tone) }
    function onChat(c) { if (c.was === "*") win.talkKey = c.key || "" }
  }
  readonly property int satDot: Style.space(8)     // what is seen
  readonly property int satZone: Style.space(14)   // what takes the pointer
  readonly property real satR: slotSize / 2 + Style.space(4) + satDot / 2   // clear of the breathing disc
  // The angle between neighbouring places, radians: the outermost, two steps from the middle, stays within the slot's width.
  readonly property real satStep: Math.asin((slotSize - satZone) / 2 / satR) / 2

  // The dots' colours: as the window's agent rows, the watch's word orange (the orb's «needs help»).
  function dotColor(kind) {
    return ({ working: Color.accent, waiting: win.fixedColors.red, health: win.fixedColors.values.orange || "#e0823d", done: Color.foreground })[kind] || Color.muted
  }

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
  // Oma's window is on Overlay too: the orb steps down under it while it is open.
  WlrLayershell.layer: win.link.speech.orb_over_fullscreen && !win.covered ? WlrLayer.Overlay : WlrLayer.Top
  anchors { top: true; bottom: true; left: true; right: true }
  margins { top: win.gap; bottom: win.gap; left: win.gap; right: win.gap }

  // The Repeater makes its bubbles after the bindings first run, and itemAt()
  // notifies nothing: without `made` the mask kept the circle alone, and
  // clicks on the bubbles went to the window below.
  property int made: 0
  property int satsMade: 0   // the same for the satellites
  mask: Region {
    Region { item: orbSlot; shape: RegionShape.Ellipse }
    Region { item: win.made && slots.itemAt(0) && slots.itemAt(0).visible ? slots.itemAt(0) : null }
    Region { item: win.made && slots.itemAt(1) && slots.itemAt(1).visible ? slots.itemAt(1) : null }
    Region { item: win.made && slots.itemAt(2) && slots.itemAt(2).visible ? slots.itemAt(2) : null }
    Region { item: win.satsMade && satRep.itemAt(0) && satRep.itemAt(0).visible ? satRep.itemAt(0) : null; shape: RegionShape.Ellipse }
    Region { item: win.satsMade && satRep.itemAt(1) && satRep.itemAt(1).visible ? satRep.itemAt(1) : null; shape: RegionShape.Ellipse }
    Region { item: win.satsMade && satRep.itemAt(2) && satRep.itemAt(2).visible ? satRep.itemAt(2) : null; shape: RegionShape.Ellipse }
    Region { item: win.satsMade && satRep.itemAt(3) && satRep.itemAt(3).visible ? satRep.itemAt(3) : null; shape: RegionShape.Ellipse }
    Region { item: win.satsMade && satRep.itemAt(4) && satRep.itemAt(4).visible ? satRep.itemAt(4) : null; shape: RegionShape.Ellipse }
  }

  Item {
    id: orbSlot
    width: win.slotSize
    height: win.slotSize
    x: Math.max(0, Math.min(parent.width - width, (win.atLeft ? win.pos.dx : parent.width - width - win.pos.dx) + (win.dragging ? win.dragDX : 0)))
    y: Math.max(0, Math.min(parent.height - height, (win.atTop ? win.pos.dy : parent.height - height - win.pos.dy) + (win.dragging ? win.dragDY : 0)))

    OrbFace {
      id: orbFace
      anchors.fill: parent
      mode: win.mode
      size: win.size
      line: win.line
      breathMax: win.breathMax
      rate: win.link.speech.rate_now || win.link.speech.rate || "medium"
      level: earPeak.enabled ? Model.level(earPeak.peak) : -1
      listenUntil: win.link.speech.listen_until
      listenSecs: win.link.speech.listen_secs || 0
      limit: (win.link.speech.limit || "") !== ""
      ring: win.moods && Model.doneUnseen(win.link.agents, win.seen)
      droop: win.moods && win.link.speech.struggling > 0
      awake: orbArea.containsMouse || (orbDrop.item !== null && orbDrop.item.containsDrag)
      idleMotion: win.link.speech.orb_idle_motion === true && !UPower.onBattery && !win.covered
      ringColor: win.fixedColors.values.green || "#57ab5a"
      limitColor: win.fixedColors.values.yellow || "#d4a72c"
      alertColor: win.fixedColors.red
    }

    // Left click talks, right click toggles the window, the middle one asks for
    // the agents in one sentence (the daemon's `summary`: built by code, no
    // model; while something sounds — stops it); a left drag past
    // Style.space(8) carries the circle and the drop keeps the place.
    MouseArea {
      id: orbArea
      anchors.fill: parent
      hoverEnabled: true
      acceptedButtons: Qt.LeftButton | Qt.RightButton | Qt.MiddleButton
      cursorShape: Qt.PointingHandCursor
      property point start    // the press, in the window: the window never moves
      property bool moved     // this press became a drag, even one cut short by a screen switch

      function inWindow(mouse) { return orbArea.mapToItem(null, mouse.x, mouse.y) }

      onContainsMouseChanged: {
        win.tipShown = false
        if (containsMouse) tipDelay.restart()
        else tipDelay.stop()
      }
      onPressed: function(mouse) {
        win.tipShown = false
        tipDelay.stop()
        start = inWindow(mouse)
        moved = false
      }
      onPositionChanged: function(mouse) {
        if (!(pressedButtons & Qt.LeftButton)) return
        var p = inWindow(mouse)
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
        else if (mouse.button === Qt.MiddleButton && win.link.speech.agents_voice === true) win.link.send(Model.cmd("summary"))
      }
      onCanceled: win.dragging = false

      // The wheel while she speaks: the tempo of this answer (the daemon's rate_now, back to the setting after the
      // answer), a step per notch; otherwise the wheel passes.
      property real wheelAcc: 0
      onWheel: function(wheel) {
        var st = win.link.speech
        if (!(st.speaking || st.oma === "speaking")) {
          wheelAcc = 0
          wheel.accepted = false
          return
        }
        var w = Model.wheelStep(wheelAcc, wheel.angleDelta.y)
        wheelAcc = w.acc
        if (w.steps) win.link.send(Model.rateStep(w.steps))
      }
    }

    // A file, a screenshot or text dropped on the orb: the general chat in Oma's window with it in the field.
    Loader {
      id: orbDrop
      anchors.fill: parent
      source: "DropTarget.qml"
      onLoaded: item.took.connect(function(t) { win.attached("general", t) })
    }
  }

  // The tooltip has its own popup window, out of the orb window's mask.
  Timer {
    id: tipDelay
    interval: 400
    onTriggered: {
      win.peekNow = Date.now() / 1000
      win.tipShown = true
    }
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
      implicitWidth: tipColumn.implicitWidth + 20
      implicitHeight: tipColumn.implicitHeight + 14
      color: Color.tooltip.background
      borderSpec: Border.surfaceSpec("tooltip", "border", Color.tooltip.border, 1)
      radius: Style.cornerRadius

      Column {
        id: tipColumn
        anchors.centerIn: parent
        spacing: Style.space(6)

        Text {
          id: tipLabel
          textFormat: Text.PlainText
          text: win.tip.map(function(l) { return win.tr(l[0], l[1]) }).join("\n")
          color: Color.tooltip.text
          font.family: Style.font.family
          font.pixelSize: Style.font.body
        }

        // The card's agents: a dot of its state, the name and its time in it, under them a line of what it is at.
        Repeater {
          model: win.peek.rows

          Item {
            id: peekRow
            required property var modelData
            width: win.peekWidth
            height: peekName.implicitHeight + (peekLine.text ? peekLine.implicitHeight : 0)

            Rectangle {
              y: (peekName.implicitHeight - height) / 2
              width: win.satDot
              height: width
              radius: width / 2
              color: win.dotColor(peekRow.modelData.dot)
            }

            Text {
              id: peekName
              x: win.satDot + Style.space(8)
              width: parent.width - x - peekAgo.implicitWidth - Style.space(8)
              textFormat: Text.PlainText
              text: peekRow.modelData.name
              color: Color.tooltip.text
              font.family: Style.font.family
              font.pixelSize: Style.font.body
              elide: Text.ElideRight
            }

            Text {
              id: peekAgo
              anchors.right: parent.right
              textFormat: Text.PlainText
              text: win.tr(peekRow.modelData.ago[0], peekRow.modelData.ago[1])
              color: Qt.darker(Color.tooltip.text, 1.4)
              font.family: Style.font.family
              font.pixelSize: Style.font.body
            }

            Text {
              id: peekLine
              visible: text !== ""
              x: peekName.x
              y: peekName.implicitHeight
              width: parent.width - x
              textFormat: Text.PlainText
              text: peekRow.modelData.fails ? win.tr("%1 failed calls this turn", peekRow.modelData.fails) : peekRow.modelData.text
              color: Qt.darker(Color.tooltip.text, 1.4)
              font.family: Style.font.family
              font.pixelSize: Style.font.body
              elide: Text.ElideRight
            }
          }
        }

        Text {
          visible: win.peek.more > 0
          textFormat: Text.PlainText
          text: win.tr("%1 more", win.peek.more)
          color: Qt.darker(Color.tooltip.text, 1.4)
          font.family: Style.font.family
          font.pixelSize: Style.font.body
        }
      }
    }
  }

  // The satellites: on the window, not in the orb's slot — a mask Region follows its item's own place, and the
  // dots' places follow the slot's. A new question flashes its dot three times (~2.4 s), then it stands red.
  Repeater {
    id: satRep
    model: Model.SAT_MAX
    onItemAdded: win.satsMade++

    Item {
      id: sat
      required property int index
      readonly property string sid: win.sats[index] || ""
      readonly property bool more: sid === "+"
      readonly property var agent: more ? null : Model.agentById(win.link.agents, sid)
      readonly property string kind: agent ? Model.dotKind(agent) : ""
      // places from the arc's middle out: one agent sits right under (or over) the orb
      readonly property real angle: [0, -1, 1, -2, 2][index] * win.satStep
      readonly property string key: agent ? agent.id + ":" + kind : ""
      property string seen: ""
      Component.onCompleted: seen = key
      onKeyChanged: {
        flash.stop()
        satDisc.opacity = 1
        if (Model.asks(seen, key)) flash.start()
        seen = key
      }

      visible: win.satsShown && (more || agent !== null)
      width: win.satZone
      height: width
      x: orbSlot.x + orbSlot.width / 2 + win.satR * Math.sin(angle) - width / 2
      y: orbSlot.y + orbSlot.height / 2 + (win.atTop ? 1 : -1) * win.satR * Math.cos(angle) - height / 2

      Rectangle {
        id: satDisc
        anchors.centerIn: parent
        width: win.satDot
        height: width
        radius: width / 2
        // «more» is hollow, without a number (the number is in its tooltip)
        color: sat.more ? "transparent" : win.dotColor(sat.kind)
        border.width: sat.more ? Math.max(1, Style.space(2)) : 0
        border.color: Color.foreground

        SequentialAnimation {
          id: flash
          loops: 3
          NumberAnimation { target: satDisc; property: "opacity"; to: 0.2; duration: Style.duration(400); easing.type: Easing.InOutSine }
          NumberAnimation { target: satDisc; property: "opacity"; to: 1; duration: Style.duration(400); easing.type: Easing.InOutSine }
        }
      }

      // Oma listens in this agent's chat (a hold on the dot took, or her answer window for it), or a drag is over the
      // dot: a green ring round it.
      Rectangle {
        anchors.centerIn: parent
        width: win.satZone
        height: width
        radius: width / 2
        color: "transparent"
        border.width: Math.max(1, Style.space(2))
        border.color: win.fixedColors.values.green || "#57ab5a"
        visible: (win.mode === "listening" && win.talkKey === "agent:" + sat.sid) || (satDrop.item !== null && satDrop.item.containsDrag)
      }

      // Left: the agent's chat in Oma's window; «more» — the window with them all. Held (0,45 s): talk to this agent
      // without naming it — Oma listens in its chat; a voiced order there still waits for «да, передай»; the release
      // after a hold is no click.
      MouseArea {
        id: satArea
        anchors.fill: parent
        hoverEnabled: true
        cursorShape: Qt.PointingHandCursor
        pressAndHoldInterval: 450
        property bool held: false
        onPressed: held = false
        onPressAndHold: if (!sat.more) {
          held = true
          win.link.send(Model.omaCmd({ do: "talk", key: "agent:" + sat.sid }))
        }
        onClicked: if (!held) sat.more ? win.windowToggled() : win.chatOpened("agent:" + sat.sid)
      }

      // A file, a screenshot or text dropped on the dot: its chat in Oma's window with that in the field.
      Loader {
        id: satDrop
        anchors.fill: parent
        active: sat.agent !== null
        source: "DropTarget.qml"
        onLoaded: item.took.connect(function(t) { win.attached("agent:" + sat.sid, t) })
      }

      // «Astral.Edo — ждёт · 15 мин», counted when the pointer comes
      PanelToolTip {
        visible: satArea.containsMouse
        text: {
          if (!satArea.containsMouse) return ""
          if (sat.more) return win.tr("%1 more", win.link.agents.length - Model.SAT_MAX + 1)
          if (!sat.agent) return ""
          var a = Model.ago(sat.agent.since, Date.now() / 1000)
          return (sat.agent.title || sat.agent.project || "") + " — "
            + (sat.agent.note ? win.tr("Needs: %1", sat.agent.note)
              : sat.kind === "health" ? sat.agent.health : win.tr(Model.stateLabel(sat.agent.state)).toLowerCase()) + " · " + win.tr(a[0], a[1])
        }
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
      onItemAdded: win.made++

      Item {
        id: slot
        required property int index
        readonly property var bubble: win.bubbles[win.atTop ? win.bubbles.length - 1 - index : index] || null
        readonly property string tone: slot.bubble ? Model.tone(slot.bubble.kind) : "agent"
        readonly property color base: tone === "me" ? Model.ME_TINT : Color.foreground
        readonly property string shown: {
          if (!slot.bubble) return ""
          if (slot.bubble.hidden) return win.tr("Permission")   // the screen is shared: no command
          var t = slot.bubble.text || ""
          if (slot.bubble.kind !== "draft" || !slot.bubble.hearing) return t
          return t ? t + " …" : "…"
        }
        // a permission card (Model.permBubbles): up while the request lives, never fading
        readonly property bool perm: !!slot.bubble && slot.bubble.kind === "permission"
        readonly property bool faded: win.bubbles.length >= Model.BUBBLES_MAX && bubble === win.bubbles[0] && !perm
        readonly property real inner: Math.min(Style.space(360) - card.contentLeftInset - card.contentRightInset,
          Math.max(who.implicitWidth, body.implicitWidth, perm && !slot.bubble.hidden ? permButtons.implicitWidth : 0))
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

          // Left: the bubble's chat in Oma's window (an agent's bubble — the
          // agent's chat, the rest — Oma's current one); right: away, and a
          // draft's phrase is cancelled, as by Esc.
          MouseArea {
            anchors.fill: parent
            acceptedButtons: Qt.LeftButton | Qt.RightButton
            cursorShape: Qt.PointingHandCursor
            onClicked: function(mouse) {
              var b = slot.bubble
              if (mouse.button === Qt.LeftButton) win.chatOpened(b.agent ? "agent:" + b.agent : "")
              else if (b.kind === "draft") win.link.send(Model.omaCmd({ do: "cancel" }))
              else win.dismissed(b.id)
            }
          }

          Column {
            id: content
            x: card.contentLeftInset
            y: card.contentTopInset
            width: slot.inner
            spacing: Style.space(4)

            // Who speaks as the chat's ТЫ/ОМА: dim bold caps, apart from the text.
            Text {
              id: who
              width: parent.width
              textFormat: Text.PlainText
              text: slot.bubble ? (slot.bubble.who || "").toUpperCase() : ""
              color: Qt.darker(Color.popups.text, 1.4)
              font.family: Style.font.family
              font.pixelSize: Style.font.body
              font.bold: true
              font.letterSpacing: 1.2
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

            // A permission card's buttons — only when its whole text is in sight: nothing unseen gets approved (a long
            // command — the click opens Oma's window, the card is there whole). The click answers at once.
            Row {
              id: permButtons
              visible: slot.perm && !slot.bubble.hidden && !body.truncated
              topPadding: Style.space(4)
              spacing: Style.space(8)

              Repeater {
                model: [true, false]

                Button {
                  required property bool modelData
                  bordered: true
                  text: modelData ? win.tr("Allow") : win.tr("Deny")
                  fontSize: Style.font.title
                  iconSize: Style.font.title
                  foreground: Color.popups.text
                  onClicked: win.link.send(Model.permit(slot.bubble.perm, modelData))
                }
              }
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
