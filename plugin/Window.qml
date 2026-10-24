import QtQuick
import Quickshell
import Quickshell.Hyprland
import Quickshell.Wayland
import qs.Commons
import qs.Ui
import "Model.js" as Model
import "I18n.js" as I18n

// Oma's window, a modal in the middle of the screen (rules.md §9; its layer
// namespace is in Hyprland's no_anim rule). Main page: the agents on the
// left, the current chat with Oma on the right with the field under it; an
// agent's chat starts with its feed. The chat history and the settings are
// pages behind header buttons, with Back in the gear's place. The window
// keeps the current chat's key: Oma's chat lines apply only to that chat.
Item {
  id: root

  property var link
  property var draft: null           // Oma's draft bubble, shown at the chat's end
  property bool shown: false
  property string viewing: ""   // a screenshot open over the window
  function toggle() {
    if (shown) shown = false
    else open("")
  }

  // Opens on the focused screen (and stays there while open) with the chat
  // `key`, an agent's from its bubble; "" asks Oma for her current chat (a
  // keyless `open` switches nothing; her answer, `was` "*", is adopted), so
  // after a shell restart the window follows her chat again.
  function open(key) {
    openScreen = focusedScreen()
    now = Date.now() / 1000
    shown = true
    if (key) return openChat(key)
    showPage("main")
    send({ do: "open" })
    scrollDown()
  }

  readonly property var tr: I18n.translator(I18n.textLanguage(function(name) { return Quickshell.env(name) }))
  readonly property var speech: link ? link.speech : Model.OFFLINE
  readonly property var agents: link ? link.agents : []
  readonly property var tip: Model.orbTip(speech)
  readonly property var settingsSecs: Model.sections(tr)
  readonly property color fg: Color.popups.text
  readonly property color dim: Qt.darker(fg, 1.4)
  readonly property string font: Style.font.family

  property var openScreen: null
  property string page: "main"       // main | history | settings
  property string chatKey: "new"     // "new", "chat:<id>" or "agent:<agent id>"
  property string generalKey: "new"  // the last general chat: Esc from an agent's chat goes back to it
  property var messages: []
  property var chatList: []
  property real now: Date.now() / 1000

  // One cursor for keys and mouse: a row of the page (-1 none) or, with
  // onHead, the header's button headCol.
  property int cursor: -1
  property bool onHead: false
  property int headCol: 0
  property int hoursCol: 0           // the hours row's dropdown: 0 from, 1 to

  readonly property bool agentChat: chatKey.indexOf("agent:") === 0
  readonly property var agent: {
    for (var i = 0; i < agents.length; i++) if ("agent:" + agents[i].id === chatKey) return agents[i]
    return null
  }
  readonly property int rowCount: page === "main" ? agents.length : page === "history" ? chatList.length : Model.SETTINGS_ROWS.length
  // Main page: talk, new chat, history, settings, the agents-voice switch; a page: Back.
  readonly property int headCount: page === "main" ? 5 : 1

  function focusedScreen() {
    var name = Hyprland.focusedMonitor ? Hyprland.focusedMonitor.name : ""
    var list = Quickshell.screens
    for (var i = 0; i < list.length; i++) if (list[i].name === name) return list[i]
    return null
  }

  function send(action) { if (root.link) root.link.send(Model.omaCmd(action)) }

  function choose(key, value, sample) {
    if (!root.link) return
    root.link.send(Model.set(key, value))
    if (sample) root.link.send(Model.say(Model.SAMPLE))
  }

  function step(row, dx) {
    var s = Model.settingStep(row, root.speech, dx)
    if (s) root.choose(s.key, s.value, s.sample)
  }

  function setKey(key) {
    root.chatKey = key
    if (key.indexOf("agent:") !== 0) root.generalKey = key
  }

  // A chat on the right; its messages come back in Oma's `chat` line.
  function openChat(key) {
    if (key !== root.chatKey) { root.setKey(key); root.messages = [] }
    root.scrollDown()
    root.send({ do: "open", key: key })
    root.showPage("main")
  }

  function newChat() {
    root.setKey("new")
    root.messages = []
    root.scrollDown()
    root.send({ do: "new" })
    root.showPage("main")
  }

  function ask() {
    var text = field.text.trim()
    if (!text) return
    root.send({ do: "ask", text: text, key: root.chatKey })
    field.text = ""
  }

  function showPage(p) {
    if (p === "history") root.send({ do: "chats" })
    root.page = p
    root.cursor = -1
    root.onHead = false
    root.headCol = 0
    root.hoursCol = 0
    pointerGate.reset()
    Qt.callLater(root.refocus)
  }

  // The keys: the field on the main page, the catcher on the others.
  function refocus() { (root.page === "main" ? field : keyCatcher).forceActiveFocus() }

  // Esc step by step: a screenshot → a page → clear the field → the agent's chat → close.
  function back() {
    if (root.viewing) root.viewing = ""
    else if (root.page !== "main") root.showPage("main")
    else if (field.text !== "") field.text = ""
    else if (root.agentChat) root.openChat(root.generalKey)
    else root.shown = false
  }

  function press(col) {
    if (root.page !== "main") root.back()
    else if (col === 0) root.send({ do: "talk" })
    else if (col === 1) root.newChat()
    else if (col === 2) root.showPage("history")
    else if (col === 3) root.showPage("settings")
    else root.choose("agents_voice", !root.speech.agents_voice, false)
  }

  // ↑/↓ the rows, ↑ from the first one onto the header; ←/→ its buttons,
  // a setting's value or the hours row's dropdown. Returns whether the key
  // was used: the field keeps ←/→ for its text otherwise.
  function move(dx, dy) {
    pointerGate.reset()
    if (dy < 0) {
      if (root.onHead) return true
      if (root.cursor <= 0) { root.onHead = true; root.cursor = -1 }
      else root.cursor--
    } else if (dy > 0) {
      if (root.onHead) { root.onHead = false; root.cursor = root.rowCount > 0 ? 0 : -1 }
      else root.cursor = Math.min(root.rowCount - 1, root.cursor + 1)
    } else if (root.onHead) {
      root.headCol = Math.max(0, Math.min(root.headCount - 1, root.headCol + dx))
    } else if (root.page === "settings" && root.cursor >= 0) {
      var row = Model.SETTINGS_ROWS[root.cursor]
      if (row === "quiet_hours") root.hoursCol = dx < 0 ? 0 : 1
      else root.step(row, dx)
    } else return false
    var view = root.page === "main" ? agentList : root.page === "history" ? chatsView : null
    if (view && root.cursor >= 0) view.positionViewAtIndex(root.cursor, ListView.Contain)
    return true
  }

  // Enter or a click: the header button, the agent's chat, the chat from
  // the history; on a setting, open its dropdown or step its value.
  function activate() {
    if (root.onHead) return root.press(root.headCol)
    if (root.cursor < 0 || root.cursor >= root.rowCount) return
    if (root.page === "main") return root.openChat("agent:" + root.agents[root.cursor].id)
    if (root.page === "history") return root.openChat(root.chatList[root.cursor].key)
    var row = Model.SETTINGS_ROWS[root.cursor]
    var box = { speaker: speakerBox, rate: rateBox, quiet_hours: root.hoursCol ? toBox : fromBox }[row]
    if (box) box.open()
    else root.step(row, 1)
  }

  // Enter in the field: a row or button under the cursor, otherwise the question.
  function enter() {
    if (root.onHead || root.cursor >= 0) root.activate()
    else root.ask()
  }

  // To the chat's end, and kept there (chatView.follow) while the layer
  // window gets its size and long lines wrap. positionViewAtEnd needs a line:
  // an agent's chat with only its feed (the header) goes down by hand.
  function scrollDown() {
    chatView.follow = true
    Qt.callLater(function() {
      var v = chatView
      if (v.count) v.positionViewAtEnd()
      else v.contentY = v.originY + Math.max(0, v.contentHeight - v.height)
    })
  }

  // The open chat's new lines. A new array resets the view to its top: a
  // following chat goes back to its end; one the user scrolled up goes back
  // to the line at its top, as far into it, and stays scrolled up.
  function setMessages(list) {
    var v = chatView
    if (v.follow) {
      root.messages = list
      return root.scrollDown()
    }
    var i = v.indexAt(0, v.contentY)
    if (i < 0) i = v.indexAt(0, v.contentY + v.spacing)   // the gap between two lines
    var line = i >= 0 ? v.itemAtIndex(i) : null
    var off = v.contentY - (line ? line.y : v.originY)    // no line: in the agent's feed above them
    root.messages = list
    if (line) v.positionViewAtIndex(i, ListView.Beginning)
    else v.positionViewAtBeginning()
    v.contentY += off
    v.returnToBounds()
  }

  // Hidden: no dropdown stays open into the next time.
  onShownChanged: {
    viewing = ""
    if (!shown) {
      speakerBox.close()
      rateBox.close()
      fromBox.close()
      toBox.close()
    }
  }
  // The draft is the footer, not the model: it keeps the end only while the chat follows.
  onDraftChanged: if (chatView.follow) root.scrollDown()
  onRowCountChanged: if (root.cursor >= root.rowCount) root.cursor = root.rowCount - 1

  Connections {
    target: root.link
    function onChat(c) {
      var shownChat = Model.chatFor(root.chatKey, c)
      if (!shownChat) return
      root.setKey(shownChat.key)
      root.setMessages(shownChat.messages)
    }
    function onChatDelta(d) { if (d.key === root.chatKey) root.setMessages(Model.chatApply(root.messages, d)) }
    function onChats(list) { root.chatList = list }
  }

  Timer {
    interval: 30000
    repeat: true
    running: root.shown
    onTriggered: root.now = Date.now() / 1000
  }

  PanelWindow {
    id: modal
    screen: root.openScreen
    visible: root.shown
    color: Color.menu.scrim
    exclusionMode: ExclusionMode.Ignore
    anchors { top: true; bottom: true; left: true; right: true }
    WlrLayershell.namespace: "predmaxim-agent-speak"
    WlrLayershell.layer: WlrLayer.Overlay
    WlrLayershell.keyboardFocus: visible ? WlrKeyboardFocus.Exclusive : WlrKeyboardFocus.None

    MouseArea { anchors.fill: parent; onClicked: root.shown = false }

    BorderSurface {
      id: card
      anchors.centerIn: parent
      width: Math.min(Style.space(900), modal.width - Style.space(80))
      height: Math.min(Style.space(600), modal.height * 0.85)
      color: Color.popups.background
      borderSpec: Border.surfaceSpec("popups", "border", Color.popups.border, Math.max(1, Style.space(2)))
      padding: Style.spacing.panelPadding
      radius: Style.cornerRadius

      MouseArea { anchors.fill: parent }   // clicks on the card stay on it

      // The field keeps the keys on the main page (its ↑/↓/Enter move and
      // press, typing goes into it); what it leaves, Esc above all, falls
      // through to here. On the pages the catcher holds them itself.
      PanelKeyCatcher {
        id: keyCatcher
        anchors.fill: parent
        anchors.topMargin: card.contentTopInset
        anchors.rightMargin: card.contentRightInset
        anchors.bottomMargin: card.contentBottomInset
        anchors.leftMargin: card.contentLeftInset
        blocked: speakerBox.popupOpen || rateBox.popupOpen || fromBox.popupOpen || toBox.popupOpen
        onMoveRequested: function(dx, dy) { root.move(dx, dy) }
        onActivateRequested: root.activate()
        onCloseRequested: root.back()
        onTextKey: function(t) {
          if (root.page !== "main") return
          field.forceActiveFocus()
          field.insert(field.cursorPosition, t)
        }

        PointerMoveGate { id: pointerGate; referenceItem: card }

        PanelHero {
          id: hero
          anchors.left: parent.left
          anchors.right: parent.right
          anchors.top: parent.top
          title: root.page === "history" ? root.tr("Chat history") : root.page === "settings" ? root.tr("Settings") : root.tr("Oma")
          meta: root.tr(root.tip[0], root.tip[1]) + (root.speech.running ? " · " + root.tr("%1 agents", root.agents.length) : "")
          foreground: root.fg
          fontFamily: root.font
          // Idle while hidden: no animation runs for a window nobody sees.
          iconComponent: OrbFace {
            mode: root.shown ? Model.orbMode(root.speech) : "idle"
            size: Style.font.display
            line: Math.max(1, Style.space(2))
          }
          // Right to left: the agents-voice switch at the edge, the buttons, the
          // count of agents waiting for an answer. A page has only Back.
          trailingControl: Row {
            spacing: Style.space(10)

            Text {
              visible: root.page === "main" && root.speech.waiting > 0
              anchors.verticalCenter: parent.verticalCenter
              textFormat: Text.PlainText
              text: root.speech.waiting
              color: Color.urgent
              font.family: root.font
              font.pixelSize: Style.font.displayLarge
              font.bold: true
              HoverHandler { id: waitHover }
              PanelToolTip { visible: waitHover.hovered; text: root.tr("%1 waiting", root.speech.waiting) }
            }

            Repeater {
              model: root.page === "main"
                ? [{ icon: "\u{F036C}", tip: root.tr("Talk") }, { icon: "\u{F0415}", tip: root.tr("New chat") },
                   { icon: "\u{F02DA}", tip: root.tr("Chat history") }, { icon: "\u{F0493}", tip: root.tr("Settings") }]
                : [{ icon: "\u{F004D}", tip: root.tr("Back") }]

              Button {
                required property var modelData
                required property int index
                anchors.verticalCenter: parent.verticalCenter
                iconText: modelData.icon
                iconSize: Style.font.subtitle * 1.5
                horizontalPadding: Style.space(5)
                verticalPadding: Style.space(2)
                width: Math.max(implicitWidth, implicitHeight)   // square, like an icon button
                height: width
                // The mic is lit while a conversation runs; a click ends it
                // while Oma listens, and only cuts her answer short otherwise.
                selected: root.page === "main" && index === 0 && root.speech.talking === true
                tooltipText: selected && ["listening", "paused"].indexOf(root.speech.oma) >= 0
                  ? root.tr("End conversation") : modelData.tip
                foreground: root.fg
                fontFamily: root.font
                hasCursor: root.onHead && root.headCol === index
                onClicked: root.press(index)
              }
            }

            ToggleSwitch {
              id: agentsSwitch
              visible: root.page === "main"
              anchors.verticalCenter: parent.verticalCenter
              checked: root.speech.agents_voice === true
              hasCursor: root.onHead && root.headCol === 4
              foreground: root.fg
              onToggled: root.press(4)
              PanelToolTip { visible: agentsSwitch.containsMouse; text: root.tr("Read agents aloud") }
            }
          }
        }

        // The Bluetooth panel's gap: Style.space(14) above and below the line.
        PanelSeparator {
          id: rule
          anchors.top: hero.bottom
          anchors.topMargin: Style.space(14)
          foreground: root.fg
        }

        Item {
          id: pages
          anchors.top: rule.bottom
          anchors.topMargin: Style.space(14)
          anchors.left: parent.left
          anchors.right: parent.right
          anchors.bottom: parent.bottom

          // ---- Main page: the agents.
          ListView {
            id: agentList
            visible: root.page === "main"
            anchors.left: parent.left
            anchors.top: parent.top
            anchors.bottom: parent.bottom
            width: Math.round(parent.width * 0.38)
            clip: true
            spacing: Style.spacing.xs
            boundsBehavior: Flickable.StopAtBounds
            model: root.agents

            delegate: CursorSurface {
              id: agentRow
              required property var modelData
              required property int index
              width: ListView.view.width
              implicitHeight: Math.max(Style.space(50), agentText.implicitHeight + Style.spacing.rowPaddingX * 2)
              foreground: root.fg
              // The agent whose chat is open is the selected row; the cursor paints the others.
              current: root.chatKey === "agent:" + modelData.id
              hasCursor: !root.onHead && root.cursor === index && !current

              MouseArea {
                id: agentArea
                anchors.fill: parent
                hoverEnabled: true
                cursorShape: Qt.PointingHandCursor
                onPositionChanged: function(mouse) {
                  if (pointerGate.moved(agentArea, mouse)) { root.onHead = false; root.cursor = agentRow.index }
                }
                onClicked: root.openChat("agent:" + agentRow.modelData.id)
              }

              Rectangle {
                id: dot
                x: Style.spacing.rowPaddingX
                anchors.verticalCenter: parent.verticalCenter
                width: Style.space(10)
                height: width
                radius: width / 2
                color: ({ working: Color.accent, waiting: Color.urgent, done: Color.foreground })[agentRow.modelData.state] || Color.muted
                HoverHandler { id: dotHover }
                PanelToolTip { visible: dotHover.hovered; text: root.tr(Model.stateLabel(agentRow.modelData.state)) }
              }

              Column {
                id: agentText
                anchors.left: dot.right
                anchors.leftMargin: Style.space(12)
                anchors.right: agentAgo.left
                anchors.rightMargin: Style.space(10)
                anchors.verticalCenter: parent.verticalCenter
                spacing: Style.space(4)

                Text {
                  width: parent.width
                  textFormat: Text.PlainText
                  text: agentRow.modelData.title || agentRow.modelData.project || ""
                  color: root.fg
                  font.family: root.font
                  font.pixelSize: Style.font.heading
                  elide: Text.ElideRight
                }

                Text {
                  visible: text !== ""
                  width: parent.width
                  textFormat: Text.PlainText
                  text: agentRow.modelData.status || ""
                  color: root.dim
                  font.family: root.font
                  font.pixelSize: Style.font.body
                  wrapMode: Text.Wrap
                  maximumLineCount: 2
                  elide: Text.ElideRight
                }
              }

              Text {
                id: agentAgo
                anchors.right: parent.right
                anchors.rightMargin: Style.spacing.rowPaddingX
                anchors.verticalCenter: parent.verticalCenter
                textFormat: Text.PlainText
                text: {
                  var a = Model.ago(agentRow.modelData.since, root.now)
                  return root.tr(a[0], a[1])
                }
                color: root.dim
                font.family: root.font
                font.pixelSize: Style.font.body
              }
            }
          }

          Text {
            visible: root.page === "main" && root.agents.length === 0
            anchors.centerIn: agentList
            textFormat: Text.PlainText
            text: root.tr("No agents")
            color: root.dim
            font.family: root.font
            font.pixelSize: Style.font.heading
          }

          PanelSeparator {
            id: divider
            visible: root.page === "main"
            anchors.left: agentList.right
            anchors.leftMargin: Style.space(18)
            anchors.top: parent.top
            anchors.bottom: parent.bottom
            width: 1
            foreground: root.fg
          }

          // ---- Main page: the chat and its field.
          Item {
            id: chatPane
            visible: root.page === "main"
            anchors.left: divider.right
            anchors.leftMargin: Style.space(24)
            anchors.right: parent.right
            anchors.top: parent.top
            anchors.bottom: parent.bottom

            ListView {
              id: chatView
              anchors.top: parent.top
              anchors.left: parent.left
              anchors.right: parent.right
              anchors.bottom: fieldRule.top
              anchors.bottomMargin: Style.space(14)
              clip: true
              spacing: Style.space(14)
              boundsBehavior: Flickable.StopAtBounds
              model: root.messages
              header: root.agent ? feedHeader : null
              footer: root.draft ? draftFooter : null

              // Holds the end until the user scrolls (a scroll also re-estimates
              // contentHeight, which must not pull the view back); back at the
              // end, it holds again.
              property bool follow: true
              onContentHeightChanged: if (follow) root.scrollDown()
              onHeightChanged: if (follow) root.scrollDown()
              onMovementStarted: follow = false
              onMovementEnded: follow = atYEnd

              delegate: ChatLine {
                required property var modelData
                width: ListView.view.width
                role: modelData.role
                body: modelData.text
              }
            }

            // The phrase being spoken, after the last line; as in the orb's
            // bubble, "…" while Oma still hears it.
            Component {
              id: draftFooter

              Item {
                width: chatView.width
                height: draftLine.y + draftLine.height

                ChatLine {
                  id: draftLine
                  y: chatView.count > 0 ? chatView.spacing : 0
                  width: parent.width
                  role: "me"
                  body: {
                    var d = root.draft
                    if (!d) return ""
                    var t = d.text || ""
                    return !d.hearing ? t : t ? t + " …" : "…"
                  }
                  bodyColor: root.dim
                }
              }
            }

            // An agent's chat starts with the agent's feed, then the talk with Oma.
            Component {
              id: feedHeader

              Column {
                width: chatView.width
                spacing: Style.space(8)
                bottomPadding: Style.space(14)

                Repeater {
                  model: root.agent ? root.agent.feed : []

                  ChatLine {
                    required property var modelData
                    width: parent.width
                    role: modelData.kind === "prompt" ? "me" : "agent"
                    label: Qt.formatTime(new Date(modelData.at * 1000), "HH:mm")
                    body: modelData.text
                    images: modelData.images || []
                    bodyColor: modelData.kind === "question" ? Color.urgent : root.fg
                  }
                }

                PanelSeparator { foreground: root.fg }
              }
            }

            PanelSeparator {
              id: fieldRule
              anchors.bottom: field.top
              anchors.bottomMargin: Style.space(14)
              foreground: root.fg
            }

            // Borderless and caretless, like the launcher's field (rules.md §9).
            TextField {
              id: field
              anchors.left: parent.left
              anchors.right: parent.right
              anchors.bottom: parent.bottom
              height: Math.max(Style.space(34), Style.font.title + Style.spacing.controlPaddingY * 2)
              leftPadding: 0; rightPadding: 0; topPadding: 0; bottomPadding: 0
              background: null
              placeholderText: root.tr("Ask Oma…")
              placeholderTextColor: Util.alpha(root.fg, 0.58)
              foreground: root.fg
              font.family: root.font
              font.pixelSize: Style.font.heading
              cursorDelegate: Item {}
              onTextEdited: { root.cursor = -1; root.onHead = false }
              Keys.onUpPressed: root.move(0, -1)
              Keys.onDownPressed: root.move(0, 1)
              Keys.onLeftPressed: function(event) { event.accepted = root.onHead && root.move(-1, 0) }
              Keys.onRightPressed: function(event) { event.accepted = root.onHead && root.move(1, 0) }
              Keys.onReturnPressed: root.enter()
              Keys.onEnterPressed: root.enter()
              Keys.onTabPressed: {}   // Tab does nothing in a centred modal
            }
          }

          // ---- History page: the general chats, the latest first.
          ListView {
            id: chatsView
            visible: root.page === "history"
            anchors.fill: parent
            clip: true
            spacing: Style.spacing.xs
            boundsBehavior: Flickable.StopAtBounds
            model: root.chatList

            delegate: CursorSurface {
              id: chatRow
              required property var modelData
              required property int index
              width: ListView.view.width
              implicitHeight: Math.max(Style.space(50), chatTitle.implicitHeight + Style.spacing.rowPaddingX * 2)
              foreground: root.fg
              current: root.chatKey === modelData.key
              hasCursor: !root.onHead && root.cursor === index && !current

              MouseArea {
                id: chatArea
                anchors.fill: parent
                hoverEnabled: true
                cursorShape: Qt.PointingHandCursor
                onPositionChanged: function(mouse) {
                  if (pointerGate.moved(chatArea, mouse)) { root.onHead = false; root.cursor = chatRow.index }
                }
                onClicked: root.openChat(chatRow.modelData.key)
              }

              Text {
                id: chatTitle
                anchors.left: parent.left
                anchors.leftMargin: Style.spacing.rowPaddingX
                anchors.right: chatAgo.left
                anchors.rightMargin: Style.space(10)
                anchors.verticalCenter: parent.verticalCenter
                textFormat: Text.PlainText
                text: chatRow.modelData.title || root.tr("Untitled chat")
                color: root.fg
                font.family: root.font
                font.pixelSize: Style.font.heading
                elide: Text.ElideRight
              }

              Text {
                id: chatAgo
                anchors.right: parent.right
                anchors.rightMargin: Style.spacing.rowPaddingX
                anchors.verticalCenter: parent.verticalCenter
                textFormat: Text.PlainText
                text: {
                  var a = Model.ago(chatRow.modelData.updated, root.now)
                  return root.tr(a[0], a[1])
                }
                color: root.dim
                font.family: root.font
                font.pixelSize: Style.font.body
              }
            }
          }

          Text {
            visible: root.page === "history" && root.chatList.length === 0
            anchors.centerIn: chatsView
            textFormat: Text.PlainText
            text: root.tr("No chats yet")
            color: root.dim
            font.family: root.font
            font.pixelSize: Style.font.heading
          }

          // ---- Settings page: rows in Model.SETTINGS_ROWS order. The
          //      controls are the value's picture and the mouse's target;
          //      the keys stay with the catcher (rules.md §9).
          Column {
            id: settingsPage
            visible: root.page === "settings"
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.top: parent.top
            spacing: Style.spacing.xs

            SettingRow {
              row: 0
              label: root.settingsSecs[0].caption
              caps: true
              Dropdown {
                id: speakerBox
                options: root.settingsSecs[0].options
                value: root.speech.speaker
                foreground: root.fg
                fontFamily: root.font
                // A pick sets the dropdown's own value; the binding comes back for the daemon's answer.
                onChanged: function(v) { root.choose("speaker", v, true); value = Qt.binding(function() { return root.speech.speaker }) }
                onPopupOpenChanged: if (!popupOpen) root.refocus()
              }
            }

            SettingRow {
              row: 1
              label: root.settingsSecs[1].caption
              caps: true
              Dropdown {
                id: rateBox
                options: root.settingsSecs[1].options
                value: root.speech.rate
                foreground: root.fg
                fontFamily: root.font
                onChanged: function(v) { root.choose("rate", v, true); value = Qt.binding(function() { return root.speech.rate }) }
                onPopupOpenChanged: if (!popupOpen) root.refocus()
              }
            }

            PanelSectionHeader {
              leftPadding: Style.spacing.rowPaddingX
              topPadding: Style.space(14)
              bottomPadding: Style.space(4)
              text: root.tr("Quiet mode").toUpperCase()
              foreground: root.fg
              fontFamily: root.font
              fontSize: Style.font.body
              font.letterSpacing: 1.2
            }

            SettingRow {
              row: 2
              label: root.tr("Quiet now")
              ToggleSwitch {
                checked: root.speech.quiet === true
                foreground: root.fg
                onToggled: root.step("quiet_now", 1)
              }
            }

            SettingRow {
              row: 3
              label: root.tr("On schedule")
              ToggleSwitch {
                checked: root.speech.quiet_schedule === true
                foreground: root.fg
                onToggled: root.step("quiet_schedule", 1)
              }
            }

            SettingRow {
              row: 4
              label: root.tr("From / to")
              Dropdown {
                id: fromBox
                width: Style.space(100)
                options: Model.HOURS
                value: root.speech.quiet_from
                hasCursor: !root.onHead && root.cursor === 4 && root.hoursCol === 0
                foreground: root.fg
                fontFamily: root.font
                onChanged: function(v) { root.choose("quiet_from", v, false); value = Qt.binding(function() { return root.speech.quiet_from }) }
                onPopupOpenChanged: if (!popupOpen) root.refocus()
              }
              Dropdown {
                id: toBox
                width: Style.space(100)
                options: Model.HOURS
                value: root.speech.quiet_to
                hasCursor: !root.onHead && root.cursor === 4 && root.hoursCol === 1
                foreground: root.fg
                fontFamily: root.font
                onChanged: function(v) { root.choose("quiet_to", v, false); value = Qt.binding(function() { return root.speech.quiet_to }) }
                onPopupOpenChanged: if (!popupOpen) root.refocus()
              }
            }

            PanelSectionHeader {
              leftPadding: Style.spacing.rowPaddingX
              topPadding: Style.space(14)
              bottomPadding: Style.space(4)
              text: root.tr("Orb").toUpperCase()
              foreground: root.fg
              fontFamily: root.font
              fontSize: Style.font.body
              font.letterSpacing: 1.2
            }

            SettingRow {
              row: 5
              label: root.tr("Over fullscreen windows")
              ToggleSwitch {
                checked: root.speech.orb_over_fullscreen === true
                foreground: root.fg
                onToggled: root.step("orb_over_fullscreen", 1)
              }
            }
          }
        }
      }
    }

    // A screenshot over the window, fitted; a click or Esc closes it.
    Rectangle {
      anchors.fill: parent
      visible: root.viewing !== ""
      color: Qt.rgba(0, 0, 0, 0.85)

      Image {
        anchors.fill: parent
        anchors.margins: Style.space(40)
        fillMode: Image.PreserveAspectFit
        asynchronous: true
        source: root.viewing ? "file://" + root.viewing : ""
      }

      MouseArea { anchors.fill: parent; onClicked: root.viewing = "" }
    }
  }

  // A chat line: a bubble tinted like the orb's ones, as wide as its text (up
  // to 80% of the chat); mine at the right, Oma's and agents' at the left;
  // ТЫ/ОМА (an agent's feed: the time) above the text, screenshots below it
  // (such a bubble is as wide as it may be).
  component ChatLine: Item {
    id: chatLine
    property string role: "me"
    property string body: ""
    property string label: (role === "me" ? root.tr("You") : root.tr("Oma")).toUpperCase()
    property color bodyColor: root.fg
    property var images: []   // screenshots of the prompt: files in the daemon's cache
    readonly property string tone: Model.tone(role)
    readonly property color base: tone === "me" ? Model.ME_TINT : Color.foreground
    readonly property int pad: Style.space(10)
    implicitHeight: bubble.height

    Rectangle {
      id: bubble
      anchors.right: chatLine.tone === "me" ? parent.right : undefined
      width: chatLine.images.length ? parent.width * 0.8 : Math.min(parent.width * 0.8, Math.max(labelText.implicitWidth, bodyText.implicitWidth) + chatLine.pad * 2)
      height: lineText.implicitHeight + chatLine.pad * 2
      color: Qt.tint(Color.popups.background, Qt.rgba(chatLine.base.r, chatLine.base.g, chatLine.base.b, Math.max(0.05, Model.TONE_ALPHA[chatLine.tone])))
      radius: Style.cornerRadius

      Column {
        id: lineText
        x: chatLine.pad
        y: chatLine.pad
        width: parent.width - chatLine.pad * 2
        spacing: Style.space(4)

        Text {
          id: labelText
          textFormat: Text.PlainText
          text: chatLine.label
          color: root.dim
          font.family: root.font
          font.pixelSize: Style.font.body
          font.bold: true
          font.letterSpacing: 1.2
        }

        Text {
          id: bodyText
          width: parent.width
          // Agents answer in Markdown: lists, headings, code keep their shape
          // (the daemon drops ![images](…): MarkdownText would fetch them).
          textFormat: Text.MarkdownText
          onLinkActivated: function(link) { Qt.openUrlExternally(link) }
          text: chatLine.body
          color: chatLine.bodyColor
          font.family: root.font
          font.pixelSize: Style.font.title
          wrapMode: Text.Wrap
          visible: text !== ""
        }

        // Thumbnails; a click opens the picture over the window.
        Flow {
          id: pics
          width: parent.width
          spacing: Style.space(6)
          visible: chatLine.images.length > 0

          Repeater {
            model: chatLine.images

            Image {
              required property string modelData
              height: Style.space(120)
              width: Math.min(pics.width, implicitWidth * height / Math.max(1, implicitHeight))
              sourceSize.height: height * 2
              fillMode: Image.PreserveAspectFit
              asynchronous: true
              source: "file://" + modelData

              MouseArea {
                anchors.fill: parent
                cursorShape: Qt.PointingHandCursor
                onClicked: root.viewing = parent.modelData
              }
            }
          }
        }
      }
    }
  }

  // A settings row: its label at the left, its control (the children) at the right.
  component SettingRow: CursorSurface {
    id: settingRow
    property int row: 0
    property string label: ""
    property bool caps: false   // a section caption (bold caps) rather than a row's text
    default property alias control: slot.data
    width: settingsPage.width
    implicitHeight: Math.max(Style.space(50), slot.implicitHeight + Style.spacing.xs * 2)
    foreground: root.fg
    hasCursor: !root.onHead && root.cursor === row

    MouseArea {
      id: rowArea
      anchors.fill: parent
      hoverEnabled: true
      onPositionChanged: function(mouse) {
        if (pointerGate.moved(rowArea, mouse)) { root.onHead = false; root.cursor = settingRow.row }
      }
      onClicked: { root.onHead = false; root.cursor = settingRow.row; root.activate() }
    }

    Text {
      anchors.left: parent.left
      anchors.leftMargin: Style.spacing.rowPaddingX
      anchors.right: slot.left
      anchors.rightMargin: Style.space(8)
      anchors.verticalCenter: parent.verticalCenter
      textFormat: Text.PlainText
      text: settingRow.label
      color: settingRow.caps ? root.dim : root.fg
      font.family: root.font
      font.pixelSize: settingRow.caps ? Style.font.body : Style.font.title
      font.bold: settingRow.caps
      font.letterSpacing: settingRow.caps ? 1.2 : 0
      elide: Text.ElideRight
    }

    Row {
      id: slot
      anchors.right: parent.right
      anchors.rightMargin: Style.spacing.rowPaddingX
      anchors.verticalCenter: parent.verticalCenter
      spacing: Style.space(8)
    }
  }
}
