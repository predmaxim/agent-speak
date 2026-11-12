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
// pages behind header buttons, with Back in the gear's place; Oma's memory is
// a page behind the settings' «Memory ›» row. The window
// keeps the current chat's key: Oma's chat lines apply only to that chat.
Item {
  id: root
  // Fixed status colours (FixedColors.qml): a theme's red can be any colour.
  readonly property QtObject fixedColors: FixedColors {}

  property var link
  property var draft: null           // Oma's draft bubble, shown at the chat's end
  property bool micMuted: false      // the mic is off (read by the orb from PipeWire)
  property bool shown: false
  property string viewing: ""   // a screenshot open over the window
  property string jumpErr: ""   // why `agent-speak show` from this window did not show the agent (its bubble's text)
  function toggle() {
    if (shown) shown = false
    else open("")
  }

  // Opens on the focused screen (and stays there while open) with the chat
  // `key`, an agent's from its bubble; "" asks Oma for her current chat (her
  // answer, `was` "*", is adopted), so after a shell restart the window
  // follows her chat again.
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
  readonly property var tip: Model.orbTip(speech, micMuted)
  readonly property var settingsSecs: Model.sections(tr)
  readonly property color fg: Color.popups.text
  readonly property color dim: Qt.darker(fg, 1.4)
  readonly property string font: Style.font.family

  property var openScreen: null
  property string page: "main"       // main | history | settings | memory
  property string chatKey: "new"     // "new", "chat:<id>", "agent:<agent id>"; "general" until Oma names it
  property var memView: null         // Oma's answer for the memory page, {prefs, days}; null until it comes
  // While the screen is shared (quiet for that reason) the memory page shows none of its text.
  readonly property bool memHidden: speech.quiet_why === "screencast"
  // Oma down: what she sent may be stale and nothing can change it — no rows until she is back and answers again.
  readonly property var memRows: memHidden || !omaUp ? [] : Model.memoryRows(memView)
  property string editId: ""         // the «I remember» line being edited, as it was (its id): rows come and go, the id stays
  // The line went (Oma's new view, the screen shared, Oma down): its edit ends with it.
  readonly property bool editing: editId !== "" && memRows.some(function(r) { return r.kind === "pref" && r.id === editId })
  onEditingChanged: if (!editing && editId !== "") stopEdit()
  property string openDay: ""        // the day note shown whole
  property var pendingDel: null      // the memory row the confirm dialog asks about
  property var messages: []
  property var chatList: []
  property real now: Date.now() / 1000
  property string asked: ""          // the question sent, until Oma's chat has it
  property string askedId: ""        // its id: her chat line for it comes back with it
  readonly property bool omaUp: Model.omaUp(speech)
  // Oma went down: what she had not taken is lost with her, Enter sends it again.
  // Back up: the memory page asks again for what she remembers.
  onOmaUpChanged: {
    if (!omaUp) { asked = ""; askedId = "" }
    else if (page === "memory") send({ do: "memory" })
  }

  // One cursor for keys and mouse: a row of the page (-1 none) or, with
  // onHead, the header's button headCol. On the main page it holds the row's
  // chat key (Model.rowKey), so it stays on its agent when the list changes.
  readonly property int cursor: page === "main" ? Model.rowIndex(agents, cursorKey, topKeys) : cursorRow
  property string cursorKey: ""
  property int cursorRow: -1
  function setCursor(i) {
    if (page === "main") cursorKey = Model.rowKey(agents, i, topKeys)
    else cursorRow = i
  }
  // Main page, between Oma and the agents: the permission cards and the deferred orders («Later»), the daemon's lists.
  readonly property var perms: link ? link.permissions : []
  readonly property var laters: link ? link.later : []
  readonly property var topKeys: Model.topRows(perms, laters)
  // While the screen is shared a card shows only «Permission»: no command, no buttons — nothing unseen is approved.
  readonly property bool sharing: speech.quiet_why === "screencast"
  // A card's button under the keys (←/→): 0 Allow, 1 Deny; -1 none — Enter on a card does not answer by chance.
  property int permCol: -1
  onCursorKeyChanged: permCol = -1
  onSharingChanged: permCol = -1   // the buttons are gone: a choice made before must not answer the hidden card
  property bool onHead: false
  property int headCol: 0
  property int hoursCol: 0           // the hours row's dropdown: 0 from, 1 to

  readonly property bool agentChat: chatKey.indexOf("agent:") === 0
  readonly property var agent: {
    for (var i = 0; i < agents.length; i++) if ("agent:" + agents[i].id === chatKey) return agents[i]
    return null
  }
  // Main page: row 0 — the general chat with Oma, the cards and deferred orders, then the agents.
  readonly property int rowCount: page === "main" ? agents.length + 1 + topKeys.length : page === "history" ? chatList.length
    : page === "memory" ? memRows.length : Model.SETTINGS_ROWS.length
  // Main page: talk, new chat, history, sound, settings, the power switch; Oma off: the switch; a page: Back.
  readonly property var headItems: Model.headItems(page, speech.running === true)
  readonly property int headCount: headItems.length

  // A header button's look: the sound button shows what a click will do.
  function headButton(id) {
    var quiet = root.speech.quiet === true
    return { id: id, icon: { talk: "\u{F036C}", "new": "\u{F0415}", history: "\u{F02DA}", sound: quiet ? "\u{F0581}" : "\u{F057E}",
                             settings: "\u{F0493}", back: "\u{F004D}" }[id],
             tip: { talk: root.tr("Talk"), "new": root.tr("New chat"), history: root.tr("Chat history"),
                    sound: quiet ? root.tr("Turn sound on") : root.tr("Turn sound off"), settings: root.tr("Settings"), back: root.tr("Back") }[id] }
  }

  // Oma as a whole (the service), through the link: on, it reconnects at once.
  function power(on) { if (root.link) root.link.power(on) }

  function focusedScreen() {
    var name = Hyprland.focusedMonitor ? Hyprland.focusedMonitor.name : ""
    var list = Quickshell.screens
    for (var i = 0; i < list.length; i++) if (list[i].name === name) return list[i]
    return null
  }

  function send(action) { if (root.link) root.link.send(Model.omaCmd(action)) }

  // To the clipboard as plain text, like the jira plugin (wl-copy).
  function copy(text) { if (text) Quickshell.execDetached(["wl-copy", "--", Model.plainCopy(text)]) }

  // A typed character goes to the field, wherever the keys were.
  function typeIn(t) {
    field.forceActiveFocus()
    field.insert(field.cursorPosition, t)
  }

  // A drop on the orb or a satellite: at the cursor, apart from the word before it (the field keeps an unfinished message).
  function attach(t) { typeIn(Model.attachText(field.text.slice(0, field.cursorPosition), t)) }

  function choose(key, value, sample) {
    if (!root.link) return
    root.link.send(Model.set(key, value))
    if (sample) root.link.send(Model.say(Model.SAMPLE))
  }

  function step(row, dx) {
    var s = Model.settingStep(row, root.speech, dx)
    if (s) root.choose(s.key, s.value, s.sample)
  }

  // Tell the service which agent's feed is in front of the user.
  function tellView(): void { if (root.link) root.link.send(Model.view(root.shown, root.chatKey)) }
  onChatKeyChanged: tellView()

  // A chat on the right; its messages come back in Oma's `chat` line. `open`
  // only shows it (an agent's row or bubble, a history row, Esc to «Ома»):
  // her brain moves there with the first question typed in it; talk goes to
  // her current chat (the general one instead of an agent's) and the window
  // follows it (`chat` with `was` "*").
  function openChat(key) {
    if (key !== root.chatKey) { root.chatKey = key; root.messages = [] }
    root.scrollDown()
    root.send({ do: "open", key: key })
    root.showPage("main")
  }

  // Ctrl+Enter or a double click on an agent: its Orca tab or herdr pane and
  // window come to the front (`agent-speak show`). Shown — the service's
  // `shown` line closes this window (Oma.qml); not shown — its error stays by
  // the field. Not an agent: false.
  function jump(key) {
    if (key.indexOf("agent:") !== 0) return false
    root.jumpErr = ""
    Quickshell.execDetached([Quickshell.env("HOME") + "/.local/bin/agent-speak", "show", key.slice(6)])
    return true
  }

  // Enter in the field; Ctrl+Enter shows the agent under the cursor, else the one whose chat is open.
  function enterWith(modifiers) {
    if (!(modifiers & Qt.ControlModifier) || !root.jump(root.onHead || root.cursor < 0 ? root.chatKey : root.cursorKey)) root.enter()
  }

  function newChat() {
    root.chatKey = "new"
    root.messages = []
    root.scrollDown()
    root.send({ do: "new" })
    root.showPage("main")
  }

  // The question stays in the field until Oma echoes it into the chat
  // (onChatDelta, by its id): with Oma down it is not lost, the note says why.
  // The id is the time: unique across shell restarts, one question per Enter.
  function ask() {
    var text = field.text.trim()
    if (!Model.askable(text, root.asked, root.speech)) return
    root.asked = text
    root.askedId = String(Date.now())
    root.send({ do: "ask", text: text, key: root.chatKey, id: root.askedId })
  }

  function showPage(p) {
    if (p === "history") root.send({ do: "chats" })
    if (p === "memory") { root.memView = null; root.send({ do: "memory" }) }   // nothing stale to act on before her answer
    settingsView.contentY = 0
    memoryView.contentY = 0
    root.editId = ""
    root.openDay = ""
    confirm.opened = false
    root.page = p
    root.cursorKey = ""
    root.cursorRow = -1
    root.onHead = false
    root.headCol = 0
    root.hoursCol = 0
    pointerGate.reset()
    Qt.callLater(root.refocus)
  }

  // The keys: the field on the main page, the catcher on the others.
  function refocus() { (root.page === "main" ? field : keyCatcher).forceActiveFocus() }

  // Esc step by step: a screenshot → an edit → a page (memory: back to the settings, on its row) → clear the field →
  // the agent's chat → close.
  function back() {
    if (root.viewing) root.viewing = ""
    else if (root.editing) root.stopEdit()
    else if (root.page === "memory") {
      root.showPage("settings")
      root.setCursor(Model.SETTINGS_ROWS.indexOf("memory_page"))
      Qt.callLater(root.showRow, settingsView, settingsPage, root.cursor)
    }
    else if (root.page !== "main") root.showPage("main")
    else if (field.text !== "") field.text = ""
    else if (root.agentChat) root.openChat("general")
    else root.shown = false
  }

  function press(col) {
    var id = root.headItems[col]
    if (id === "back") root.back()
    else if (id === "talk") root.send({ do: "talk" })
    else if (id === "new") root.newChat()
    else if (id === "history") root.showPage("history")
    else if (id === "sound") root.step("quiet_now", 1)
    else if (id === "settings") root.showPage("settings")
    else if (id === "power") root.power(root.speech.running !== true)
  }

  // ↑/↓ the rows, ↑ from the first one onto the header; ←/→ its buttons,
  // a setting's value or the hours row's dropdown. Returns whether the key
  // was used: the field keeps ←/→ for its text otherwise.
  function move(dx, dy) {
    pointerGate.reset()
    if (dy < 0) {
      if (root.onHead) return true
      if (root.cursor <= 0) { root.onHead = true; root.setCursor(-1) }
      else root.setCursor(root.cursor - 1)
    } else if (dy > 0) {
      if (root.onHead) { root.onHead = false; root.setCursor(root.rowCount > 0 ? 0 : -1) }
      else root.setCursor(Math.min(root.rowCount - 1, root.cursor + 1))
    } else if (root.onHead) {
      root.headCol = Math.max(0, Math.min(root.headCount - 1, root.headCol + dx))
    } else if (root.page === "settings" && root.cursor >= 0) {
      var row = Model.SETTINGS_ROWS[root.cursor]
      if (row === "quiet_hours") root.hoursCol = dx < 0 ? 0 : 1
      else root.step(row, dx)
    } else if (root.page === "main" && root.cursorKey.indexOf("perm:") === 0 && !root.sharing) {
      root.permCol = dx < 0 ? 0 : 1
    } else return false
    var view = root.page === "main" ? agentList : root.page === "history" ? chatsView : null
    var head = 1 + root.topKeys.length   // rows in the list's header: Oma, the cards, the deferred orders
    if (view === agentList && root.cursor < head) agentList.positionViewAtBeginning()
    else if (view && root.cursor >= 0) view.positionViewAtIndex(root.cursor - (view === agentList ? head : 0), ListView.Contain)
    if (root.page === "settings" && root.cursor >= 0) root.showRow(settingsView, settingsPage, root.cursor)
    if (root.page === "memory" && root.cursor >= 0) root.showRow(memoryView, memoryPage, root.cursor)
    return true
  }

  // The row `row` of a scrolled page (the flickable `view`, its column `col`) fully in view (its header too for the first one).
  function showRow(view, col, row) {
    var kids = col.children
    for (var i = 0; i < kids.length; i++) {
      if (kids[i].row !== row) continue
      var top = row === 0 ? 0 : kids[i].y, bottom = kids[i].y + kids[i].height
      var maxY = Math.max(0, view.contentHeight - view.height)
      if (top < view.contentY) view.contentY = top
      else if (bottom > view.contentY + view.height) view.contentY = Math.min(maxY, bottom - view.height)
      return
    }
  }

  // The memory page: Enter or a click edits an «I remember» line, shows a day note whole (or folds it), asks to forget all.
  function memActivate(r) {
    if (r.kind === "pref") root.editId = r.id
    else if (r.kind === "day") root.openDay = root.openDay === r.id ? "" : r.id
    else root.askDel(r)
  }

  function stopEdit() {
    root.editId = ""
    root.refocus()
  }

  // Oma answers with the new view; an emptied line is a delete — the dialog asks first, as for ×.
  function saveEdit(text) {
    if (text.trim() === "") return root.askDel({ kind: "pref", id: root.editId })
    root.send({ do: "memory_set", kind: "pref", id: root.editId, text: text })
    root.stopEdit()
  }

  // Deleting is for good: the dialog asks first, Cancel preselected.
  function askDel(r) {
    if (root.editId !== "") root.stopEdit()
    root.pendingDel = r
    confirm.selectedIndex = 0
    confirm.opened = true
  }

  // Enter or a click: the header button, the agent's chat, the chat from
  // the history; on a setting, open its dropdown or step its value.
  function activate() {
    if (root.onHead) return root.press(root.headCol)
    if (root.cursor < 0 || root.cursor >= root.rowCount) return
    if (root.page === "main") return root.activateMain(root.cursorKey)
    if (root.page === "history") return root.openChat(root.chatList[root.cursor].key)
    if (root.page === "memory") return root.memActivate(root.memRows[root.cursor])
    var row = Model.SETTINGS_ROWS[root.cursor]
    if (row === "memory_page") return root.showPage("memory")
    var box = { speaker: speakerBox, rate: rateBox, news_every: newsBox, quiet_hours: root.hoursCol ? toBox : fromBox }[row]
    if (box) box.open()
    else root.step(row, 1)
  }

  // A main-page row: a card with a chosen button answers, a card without one and a deferred order open their agent's
  // chat (checks of a PR have none), the rest — their chat.
  function activateMain(key) {
    var item = function(list, id) { return Array.prototype.slice.call(list).filter(function(x) { return x.id === id })[0] }
    if (key.indexOf("perm:") === 0) {
      if (root.permCol >= 0 && !root.sharing) return root.permit(key.slice(5), root.permCol === 0)
      var p = item(root.perms, key.slice(5))
      return p ? root.openChat("agent:" + p.agent) : null
    }
    if (key.indexOf("later:") === 0) {
      var o = item(root.laters, key.slice(6))
      return o && o.agent ? root.openChat("agent:" + o.agent) : null
    }
    root.openChat(key)
  }

  // A card's button: the click is the user's own, as typed — the daemon answers the agent's hook at once.
  function permit(id, allow) { if (root.link) root.link.send(Model.permit(id, allow)) }

  // A deferred order's × or Delete: cancelled at once — it sends nothing.
  function cancelLater(id) { if (root.link) root.link.send(Model.laterCancel(id)) }

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

  // The open chat's new lines. The view's model (ScriptModel) only adds and
  // changes lines, the view is not reset: one the user scrolled up stays
  // there, a selection in it too; a following chat goes on to its end.
  function setMessages(list) {
    root.messages = list
    if (chatView.follow) root.scrollDown()
  }

  // Hidden: no dropdown stays open into the next time.
  onShownChanged: {
    viewing = ""
    jumpErr = ""
    tellView()
    if (!shown) {
      speakerBox.close()
      rateBox.close()
      fromBox.close()
      toBox.close()
    }
  }
  // The draft is the footer, not the model: it keeps the end only while the chat follows.
  onDraftChanged: if (chatView.follow) root.scrollDown()
  onRowCountChanged: if (root.cursor >= root.rowCount) root.setCursor(root.rowCount - 1)
  // The service went off: only its switch works (Model.headItems), no page is left to send into nothing.
  onSpeechChanged: if (speech.running !== true && page !== "main") showPage("main")

  Connections {
    target: root.link
    function onChat(c) {
      var shownChat = Model.chatFor(root.chatKey, c)
      if (!shownChat) return
      root.chatKey = shownChat.key
      root.setMessages(shownChat.messages)
    }
    function onChatDelta(d) {
      if (Model.taken(root.askedId, d)) {
        if (field.text.trim() === root.asked) field.text = ""   // not what the user typed since
        root.asked = ""
        root.askedId = ""
      }
      if (d.key === root.chatKey) root.setMessages(Model.chatApply(root.messages, d))
    }
    function onChats(list) { root.chatList = list }
    // The orb's bubbles wait while the window is open: the show error is said by the field.
    function onBubble(b) { if (b.id === "show" && root.shown) root.jumpErr = b.text }
    function onMemory(m) { root.memView = m }
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

      // What the blocked catcher lets through: the confirm dialog's keys.
      Keys.onPressed: function(event) { if (confirm.handleKey(event)) event.accepted = true }

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
        blocked: speakerBox.popupOpen || rateBox.popupOpen || fromBox.popupOpen || toBox.popupOpen || confirm.opened || root.editing
        onMoveRequested: function(dx, dy) { root.move(dx, dy) }
        onActivateRequested: root.activate()
        onCloseRequested: root.back()
        onDeleteRequested: if (root.page === "memory" && root.cursor >= 0) root.askDel(root.memRows[root.cursor])
        onTextKey: function(t) { if (root.page === "main") root.typeIn(t) }

        // The memory page's deletes: a day, a line, everything — for good.
        ConfirmDialog {
          id: confirm
          anchors.fill: parent
          z: 10
          message: !root.pendingDel ? ""
            : root.pendingDel.kind === "all" ? root.tr("Forget everything? The day notes and «I remember» go for good.")
            : root.pendingDel.kind === "day" ? root.tr("Delete the note of %1?", root.pendingDel.id)
            : root.tr("Delete this line from memory?")
          cancelText: root.tr("Cancel")
          confirmText: root.pendingDel && root.pendingDel.kind === "all" ? root.tr("Forget") : root.tr("Delete")
          fontFamily: root.font
          onConfirmed: { opened = false; root.send({ do: "memory_del", kind: root.pendingDel.kind, id: root.pendingDel.id }) }
          onCanceled: opened = false
        }

        PointerMoveGate { id: pointerGate; referenceItem: card }

        PanelHero {
          id: hero
          anchors.left: parent.left
          anchors.right: parent.right
          anchors.top: parent.top
          title: root.page === "history" ? root.tr("Chat history") : root.page === "settings" ? root.tr("Settings")
            : root.page === "memory" ? root.tr("Memory") : root.tr("Oma")
          meta: root.tip.map(function(l) { return root.tr(l[0], l[1]) }).join(" · ") + (root.speech.running ? " · " + root.tr("%1 agents", root.agents.length) : "")
          foreground: root.fg
          fontFamily: root.font
          // Idle while hidden: no animation runs for a window nobody sees.
          iconComponent: OrbFace {
            mode: root.shown ? Model.orbMode(root.speech, root.micMuted) : "idle"
            size: Style.font.display
            line: Math.max(1, Style.space(2))
            rate: root.speech.rate_now || root.speech.rate || "medium"
            limit: (root.speech.limit || "") !== ""
            limitColor: fixedColors.values.yellow || "#d4a72c"
          }
          // Right to left: the power switch at the edge, the buttons, the
          // count of agents waiting for an answer. A page has only Back.
          trailingControl: Row {
            spacing: Style.space(10)

            Text {
              visible: root.page === "main" && root.speech.waiting > 0 && root.speech.agents_voice !== false
              anchors.verticalCenter: parent.verticalCenter
              textFormat: Text.PlainText
              text: root.speech.waiting
              color: fixedColors.red
              font.family: root.font
              font.pixelSize: Style.font.displayLarge
              font.bold: true
              HoverHandler { id: waitHover }
              PanelToolTip { visible: waitHover.hovered; text: root.tr("%1 waiting", root.speech.waiting) }
            }

            Repeater {
              model: root.headItems.filter(function(id) { return id !== "power" }).map(root.headButton)

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
                selected: modelData.id === "talk" && root.speech.talking === true
                tooltipText: selected && ["listening", "paused"].indexOf(root.speech.oma) >= 0
                  ? root.tr("End conversation") : modelData.tip
                foreground: root.fg
                fontFamily: root.font
                hasCursor: root.onHead && root.headCol === index
                onClicked: root.press(index)
              }
            }

            ToggleSwitch {
              id: powerSwitch
              visible: root.page === "main"
              anchors.verticalCenter: parent.verticalCenter
              checked: root.speech.running === true
              hasCursor: root.onHead && root.headCol === root.headCount - 1
              foreground: root.fg
              onToggled: root.press(root.headCount - 1)
              PanelToolTip { visible: powerSwitch.containsMouse; text: root.speech.running ? root.tr("Turn Oma off") : root.tr("Turn Oma on") }
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

          Text {
            visible: root.page === "main" && root.speech.running !== true
            anchors.centerIn: parent
            textFormat: Text.PlainText
            text: root.tr("Oma is off")
            color: root.dim
            font.family: root.font
            font.pixelSize: Style.font.heading
          }

          // ---- Main page: the agents.
          ListView {
            id: agentList
            visible: root.page === "main" && root.speech.running === true
            anchors.left: parent.left
            anchors.top: parent.top
            anchors.bottom: parent.bottom
            width: Math.round(parent.width * 0.38)
            clip: true
            spacing: Style.spacing.xs
            boundsBehavior: Flickable.StopAtBounds
            // Keyed by id, like the bar's module list: each `agents` line only
            // moves, updates, adds and removes rows, so the scroll stays.
            model: ScriptModel {
              objectProp: "id"
              values: root.agents
            }

            // Row 0: the general chat — Oma as an assistant, not about an agent.
            // Tinted even when not open (the user's choice over an accent bar):
            // it stays easy to find among the agents.
            header: Column {
              width: agentList.width
              spacing: Style.spacing.xs
              bottomPadding: Style.spacing.xs

              CursorSurface {
                id: omaRow
                width: parent.width
                implicitHeight: Math.max(Style.space(50), omaText.implicitHeight + Style.spacing.rowPaddingX * 2)
                foreground: root.fg
                current: !root.agentChat
                hasCursor: !root.onHead && root.cursor === 0 && !current
                color: hasCursor ? fill : current ? currentFill : Util.alpha(Color.accent, 0.14)

                MouseArea {
                  id: omaArea
                  anchors.fill: parent
                  hoverEnabled: true
                  cursorShape: Qt.PointingHandCursor
                  onPositionChanged: function(mouse) {
                    if (pointerGate.moved(omaArea, mouse)) { root.onHead = false; root.setCursor(0) }
                  }
                  onClicked: root.openChat("general")
                }

                // In the agents' dot column, a little larger than a dot.
                Item {
                  id: omaSlot
                  x: Style.spacing.rowPaddingX
                  anchors.verticalCenter: parent.verticalCenter
                  width: Style.space(10)
                  height: width

                  OrbFace {
                    anchors.centerIn: parent
                    mode: "idle"
                    size: Style.space(16)
                    line: 1
                  }
                }

                Column {
                  id: omaText
                  anchors.left: omaSlot.right
                  anchors.leftMargin: Style.space(12)
                  anchors.right: parent.right
                  anchors.rightMargin: Style.spacing.rowPaddingX
                  anchors.verticalCenter: parent.verticalCenter
                  spacing: Style.space(4)

                  Text {
                    textFormat: Text.PlainText
                    text: root.tr("Oma")
                    color: root.fg
                    font.family: root.font
                    font.pixelSize: Style.font.heading
                  }

                  Text {
                    textFormat: Text.PlainText
                    text: root.tr("General chat")
                    color: root.dim
                    font.family: root.font
                    font.pixelSize: Style.font.body
                  }
                }
              }

              // «Permissions»: a card per request an agent waits on (its hook holds the request) — what is approved,
              // whole, its description dim, Allow / Deny; a click answers at once (the user's own, as typed). The card
              // goes when the request is decided anywhere. The screen shared — only «Permission»: no command, no buttons.
              RowsCaption {
                visible: root.perms.length > 0
                text: root.tr("Permissions").toUpperCase()
              }

              Repeater {
                model: root.perms

                CursorSurface {
                  id: permRow
                  required property var modelData
                  readonly property string key: "perm:" + modelData.id
                  width: parent.width
                  implicitHeight: Math.max(Style.space(50), permText.implicitHeight + Style.spacing.rowPaddingX * 2)
                  foreground: root.fg
                  hasCursor: !root.onHead && root.cursorKey === key

                  MouseArea {
                    id: permArea
                    anchors.fill: parent
                    hoverEnabled: true
                    cursorShape: Qt.PointingHandCursor
                    onPositionChanged: function(mouse) {
                      if (pointerGate.moved(permArea, mouse)) { root.onHead = false; root.cursorKey = permRow.key }
                    }
                    onClicked: root.openChat("agent:" + permRow.modelData.agent)
                  }

                  Rectangle {
                    id: permDot
                    x: Style.spacing.rowPaddingX
                    y: Style.spacing.rowPaddingX + (permWho.implicitHeight - height) / 2
                    width: Style.space(10)
                    height: width
                    radius: width / 2
                    color: fixedColors.red
                  }

                  Column {
                    id: permText
                    anchors.left: permDot.right
                    anchors.leftMargin: Style.space(12)
                    anchors.right: parent.right
                    anchors.rightMargin: Style.spacing.rowPaddingX
                    anchors.verticalCenter: parent.verticalCenter
                    spacing: Style.space(4)

                    Text {
                      id: permWho
                      width: parent.width
                      textFormat: Text.PlainText
                      text: [permRow.modelData.who, root.sharing ? "" : permRow.modelData.tool, permRow.modelData.sub ? root.tr("subagent") : ""]
                        .filter(function(s) { return s }).join(" · ")
                      color: root.fg
                      font.family: root.font
                      font.pixelSize: Style.font.heading
                      elide: Text.ElideRight
                    }

                    Text {
                      width: parent.width
                      textFormat: Text.PlainText
                      text: root.sharing ? root.tr("Permission") : permRow.modelData.text
                      color: root.sharing ? root.dim : root.fg
                      font.family: root.font
                      font.pixelSize: Style.font.body
                      wrapMode: Text.WrapAtWordBoundaryOrAnywhere
                    }

                    Text {
                      visible: !root.sharing && text !== ""
                      width: parent.width
                      textFormat: Text.PlainText
                      text: permRow.modelData.note || ""
                      color: root.dim
                      font.family: root.font
                      font.pixelSize: Style.font.body
                      wrapMode: Text.Wrap
                    }

                    Row {
                      visible: !root.sharing
                      topPadding: Style.space(4)
                      spacing: Style.space(8)

                      Repeater {
                        model: [true, false]

                        Button {
                          required property bool modelData
                          required property int index
                          bordered: true
                          text: modelData ? root.tr("Allow") : root.tr("Deny")
                          fontSize: Style.font.title
                          iconSize: Style.font.title
                          foreground: root.fg
                          fontFamily: root.font
                          hasCursor: root.cursorKey === permRow.key && root.permCol === index
                          onClicked: root.permit(permRow.modelData.id, modelData)
                        }
                      }
                    }
                  }
                }
              }

              // «Later»: deferred orders — to tell an agent when it ends its turn, to say how a PR's checks went; × or
              // Delete cancels at once.
              RowsCaption {
                visible: root.laters.length > 0
                text: root.tr("Later").toUpperCase()
              }

              Repeater {
                model: root.laters

                CursorSurface {
                  id: laterRow
                  required property var modelData
                  readonly property string key: "later:" + modelData.id
                  readonly property bool tell: modelData.kind === "tell"
                  width: parent.width
                  implicitHeight: Math.max(Style.space(50), laterText.implicitHeight + Style.spacing.rowPaddingX * 2)
                  foreground: root.fg
                  hasCursor: !root.onHead && root.cursorKey === key

                  MouseArea {
                    id: laterArea
                    anchors.fill: parent
                    hoverEnabled: true
                    cursorShape: laterRow.tell ? Qt.PointingHandCursor : Qt.ArrowCursor
                    onPositionChanged: function(mouse) {
                      if (pointerGate.moved(laterArea, mouse)) { root.onHead = false; root.cursorKey = laterRow.key }
                    }
                    onClicked: root.activateMain(laterRow.key)
                  }

                  Text {
                    id: laterIcon
                    x: Style.spacing.rowPaddingX - Style.space(2)
                    anchors.verticalCenter: parent.verticalCenter
                    width: Style.space(10)
                    textFormat: Text.PlainText
                    text: "\u{F0150}"
                    color: root.dim
                    font.family: root.font
                    font.pixelSize: Style.font.body
                  }

                  Column {
                    id: laterText
                    anchors.left: laterIcon.right
                    anchors.leftMargin: Style.space(12)
                    anchors.right: laterCancel.left
                    anchors.rightMargin: Style.space(10)
                    anchors.verticalCenter: parent.verticalCenter
                    spacing: Style.space(4)

                    Text {
                      width: parent.width
                      textFormat: Text.PlainText
                      text: laterRow.tell ? root.tr("%1, when done", laterRow.modelData.who) : root.tr("%1: checks of PR %2", laterRow.modelData.who, laterRow.modelData.pr)
                      color: root.fg
                      font.family: root.font
                      font.pixelSize: Style.font.heading
                      elide: Text.ElideRight
                    }

                    Text {
                      visible: laterRow.tell && !root.sharing
                      width: parent.width
                      textFormat: Text.PlainText
                      text: "«" + (laterRow.modelData.text || "") + "»"
                      color: root.dim
                      font.family: root.font
                      font.pixelSize: Style.font.body
                      wrapMode: Text.Wrap
                      maximumLineCount: 3
                      elide: Text.ElideRight
                    }
                  }

                  Text {
                    id: laterCancel
                    anchors.right: parent.right
                    anchors.rightMargin: Style.spacing.rowPaddingX
                    anchors.verticalCenter: parent.verticalCenter
                    textFormat: Text.PlainText
                    text: "\u{F0156}"
                    color: cancelArea.containsMouse ? root.fg : root.dim
                    font.family: root.font
                    font.pixelSize: Style.font.title

                    MouseArea {
                      id: cancelArea
                      anchors.fill: parent
                      anchors.margins: -Style.space(6)
                      hoverEnabled: true
                      cursorShape: Qt.PointingHandCursor
                      onClicked: root.cancelLater(laterRow.modelData.id)
                    }
                    PanelToolTip { visible: cancelArea.containsMouse; text: root.tr("Cancel") }
                  }
                }
              }

              RowsCaption {
                visible: root.agents.length > 0
                text: root.tr("Agents").toUpperCase()
              }
            }

            delegate: CursorSurface {
              id: agentRow
              required property var modelData
              required property int index
              width: ListView.view.width
              implicitHeight: Math.max(Style.space(50), agentText.implicitHeight + Style.spacing.rowPaddingX * 2)
              foreground: root.fg
              // The agent whose chat is open is the selected row; the cursor paints the others.
              current: root.chatKey === "agent:" + modelData.id
              hasCursor: !root.onHead && root.cursorKey === "agent:" + modelData.id && !current

              MouseArea {
                id: agentArea
                anchors.fill: parent
                hoverEnabled: true
                cursorShape: Qt.PointingHandCursor
                onPositionChanged: function(mouse) {
                  if (pointerGate.moved(agentArea, mouse)) { root.onHead = false; root.cursorKey = "agent:" + agentRow.modelData.id }
                }
                onClicked: root.openChat("agent:" + agentRow.modelData.id)
                onDoubleClicked: root.jump("agent:" + agentRow.modelData.id)
              }

              Rectangle {
                id: dot
                x: Style.spacing.rowPaddingX
                anchors.verticalCenter: parent.verticalCenter
                width: Style.space(10)
                height: width
                radius: width / 2
                color: ({ working: Color.accent, waiting: fixedColors.red, done: Color.foreground })[agentRow.modelData.state] || Color.muted
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

                // the watch's word on the agent: hung, limit until 15:20, connection lost…
                Text {
                  visible: text !== ""
                  width: parent.width
                  textFormat: Text.PlainText
                  text: agentRow.modelData.health || ""
                  color: fixedColors.red
                  font.family: root.font
                  font.pixelSize: Style.font.body
                  elide: Text.ElideRight
                }

                // the agent's own note to Oma (`agent-speak note`): what it needs from the user, until the user answers it
                Text {
                  visible: text !== ""
                  width: parent.width
                  textFormat: Text.PlainText
                  text: agentRow.modelData.note ? root.tr("Needs: %1", agentRow.modelData.note) : ""
                  color: fixedColors.red
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
            visible: root.page === "main" && root.speech.running === true && root.agents.length === 0
            anchors.centerIn: agentList
            textFormat: Text.PlainText
            text: root.tr("No agents")
            color: root.dim
            font.family: root.font
            font.pixelSize: Style.font.heading
          }

          PanelSeparator {
            id: divider
            visible: root.page === "main" && root.speech.running === true
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
            visible: root.page === "main" && root.speech.running === true
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
              // Lines have no id: compared by content, a new line is added, the rest stay.
              model: ScriptModel {
                values: root.messages
                comparisonMode: ObjectComparison.Structure
              }
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
                  // Every `agents` line brings the feed anew: by content, only its new lines are made.
                  model: ScriptModel {
                    values: root.agent ? root.agent.feed : []
                    comparisonMode: ObjectComparison.Structure
                  }

                  ChatLine {
                    required property var modelData
                    width: parent.width
                    role: modelData.kind === "prompt" ? "me" : "agent"
                    label: Qt.formatTime(new Date(modelData.at * 1000), "HH:mm")
                    body: modelData.text
                    images: modelData.images || []
                    bodyColor: modelData.kind === "question" ? fixedColors.red : root.fg
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
              leftPadding: 0; topPadding: 0; bottomPadding: 0
              rightPadding: omaNote.visible ? omaNote.width + Style.space(10) : 0
              background: null
              placeholderText: root.tr("Ask Oma…")
              placeholderTextColor: Util.alpha(root.fg, 0.58)
              foreground: root.fg
              color: root.asked !== "" && text.trim() === root.asked ? root.dim : root.fg   // sent, Oma has not taken it yet
              font.family: root.font
              font.pixelSize: Style.font.heading
              cursorDelegate: Item {}
              onTextEdited: { root.setCursor(-1); root.onHead = false; root.jumpErr = "" }
              Keys.onUpPressed: root.move(0, -1)
              Keys.onDownPressed: root.move(0, 1)
              // ←/→ — the header's buttons or a permission card's; otherwise the text's own
              Keys.onLeftPressed: function(event) { event.accepted = (root.onHead || root.cursorKey.indexOf("perm:") === 0) && root.move(-1, 0) }
              Keys.onRightPressed: function(event) { event.accepted = (root.onHead || root.cursorKey.indexOf("perm:") === 0) && root.move(1, 0) }
              // Delete on a deferred order cancels it; with text in the field Delete is the text's
              Keys.onDeletePressed: function(event) {
                event.accepted = root.cursorKey.indexOf("later:") === 0 && field.text === ""
                if (event.accepted) root.cancelLater(root.cursorKey.slice(6))
              }
              Keys.onReturnPressed: function(event) { root.enterWith(event.modifiers) }
              Keys.onEnterPressed: function(event) { root.enterWith(event.modifiers) }
              Keys.onTabPressed: {}   // Tab does nothing in a centred modal
            }

            // Oma is down (the service runs): a question waits in the field, Enter sends it when she is back.
            // A show from here failed: why, until the next typing or show.
            Text {
              id: omaNote
              visible: !root.omaUp || root.jumpErr !== ""
              anchors.right: parent.right
              anchors.verticalCenter: field.verticalCenter
              textFormat: Text.PlainText
              text: root.jumpErr !== "" ? root.jumpErr : root.tr("Oma is not running")
              color: root.dim
              font.family: root.font
              font.pixelSize: Style.font.body
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
                  if (pointerGate.moved(chatArea, mouse)) { root.onHead = false; root.setCursor(chatRow.index) }
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

          // ---- Memory page: «I remember» (Enter edits a line), the day notes (Enter shows one whole), «Forget
          //      everything»; Delete deletes a row — each after the dialog. While the screen is shared — no text.
          Flickable {
            id: memoryView
            visible: root.page === "memory"
            anchors.fill: parent
            clip: true
            contentHeight: memoryPage.implicitHeight
            boundsBehavior: Flickable.StopAtBounds

            Column {
              id: memoryPage
              width: memoryView.width
              spacing: Style.spacing.xs

              Repeater {
                model: root.memRows

                // A row with its section's header above the first of its kind.
                Column {
                  id: memRow
                  required property var modelData
                  required property int index
                  readonly property int row: index
                  readonly property bool editing: modelData.kind === "pref" && root.editId === modelData.id
                  readonly property bool whole: modelData.kind === "day" && root.openDay === modelData.id
                  width: memoryPage.width
                  spacing: Style.spacing.xs

                  PanelSectionHeader {
                    visible: memRow.modelData.kind !== "all" && (memRow.index === 0 || root.memRows[memRow.index - 1].kind !== memRow.modelData.kind)
                    leftPadding: Style.spacing.rowPaddingX
                    topPadding: memRow.index === 0 ? 0 : Style.space(14)
                    bottomPadding: Style.space(4)
                    text: (memRow.modelData.kind === "pref" ? root.tr("I remember") : root.tr("Days")).toUpperCase()
                    foreground: root.fg
                    fontFamily: root.font
                    fontSize: Style.font.body
                    font.letterSpacing: 1.2
                  }

                  CursorSurface {
                    width: parent.width
                    implicitHeight: Math.max(Style.space(50), memText.implicitHeight + Style.spacing.xs * 2)
                    foreground: root.fg
                    current: memRow.editing || memRow.whole
                    hasCursor: !root.onHead && root.cursor === memRow.index && !current

                    MouseArea {
                      id: memArea
                      anchors.fill: parent
                      hoverEnabled: true
                      cursorShape: Qt.PointingHandCursor
                      onPositionChanged: function(mouse) {
                        if (pointerGate.moved(memArea, mouse)) { root.onHead = false; root.setCursor(memRow.index) }
                      }
                      onClicked: { root.onHead = false; root.setCursor(memRow.index); root.activate() }
                    }

                    Column {
                      id: memText
                      anchors.left: parent.left
                      anchors.leftMargin: Style.spacing.rowPaddingX
                      anchors.right: memIcons.left
                      anchors.rightMargin: Style.space(10)
                      anchors.verticalCenter: parent.verticalCenter
                      spacing: Style.space(4)

                      Text {
                        visible: memRow.modelData.kind === "day"
                        textFormat: Text.PlainText
                        text: memRow.modelData.id
                        color: root.dim
                        font.family: root.font
                        font.pixelSize: Style.font.body
                      }

                      Text {
                        visible: !memRow.editing
                        width: parent.width
                        textFormat: Text.PlainText
                        text: memRow.modelData.kind === "all" ? root.tr("Forget everything") : memRow.modelData.text
                        color: memRow.modelData.kind === "all" ? root.fixedColors.red : root.fg
                        font.family: root.font
                        font.pixelSize: Style.font.title
                        wrapMode: Text.Wrap
                        maximumLineCount: memRow.modelData.kind === "day" && !memRow.whole ? 2 : undefined
                        elide: Text.ElideRight
                      }

                      // Enter saves (empty — asks to delete the line), Esc leaves it as it was. A new view of
                      // Oma's re-creates the row already editing: the text and the focus are set on creation too.
                      TextField {
                        visible: memRow.editing
                        width: parent.width
                        height: Math.max(Style.space(34), Style.font.title + Style.spacing.controlPaddingY * 2)
                        leftPadding: 0; rightPadding: 0; topPadding: 0; bottomPadding: 0
                        background: null
                        foreground: root.fg
                        font.family: root.font
                        font.pixelSize: Style.font.title
                        function begin() { text = memRow.modelData.text; forceActiveFocus() }
                        onVisibleChanged: if (visible) begin()
                        Component.onCompleted: if (memRow.editing) begin()
                        onAccepted: root.saveEdit(text)
                        Keys.onEscapePressed: root.stopEdit()
                        Keys.onTabPressed: {}
                      }
                    }

                    // The mouse's targets; the keys are Enter and Delete.
                    Row {
                      id: memIcons
                      anchors.right: parent.right
                      anchors.rightMargin: Style.spacing.rowPaddingX
                      anchors.verticalCenter: parent.verticalCenter
                      spacing: Style.space(14)

                      Repeater {
                        model: memRow.modelData.kind === "pref" ? ["edit", "delete"] : memRow.modelData.kind === "day" ? ["delete"] : []

                        Text {
                          id: memIcon
                          required property string modelData
                          textFormat: Text.PlainText
                          text: modelData === "edit" ? "\u{F03EB}" : "\u{F0156}"
                          color: memIconArea.containsMouse ? root.fg : root.dim
                          font.family: root.font
                          font.pixelSize: Style.font.title

                          MouseArea {
                            id: memIconArea
                            anchors.fill: parent
                            anchors.margins: -Style.space(6)
                            hoverEnabled: true
                            cursorShape: Qt.PointingHandCursor
                            onClicked: {
                              root.onHead = false
                              root.setCursor(memRow.index)
                              if (memIcon.modelData === "edit") root.memActivate(memRow.modelData)
                              else root.askDel(memRow.modelData)
                            }
                          }
                          PanelToolTip { visible: memIconArea.containsMouse; text: memIcon.modelData === "edit" ? root.tr("Edit") : root.tr("Delete") }
                        }
                      }
                    }
                  }
                }
              }

              // Off, the files stay until «Forget everything»; on, the brain takes them with a new chat.
              Text {
                visible: root.memRows.length > 0
                width: parent.width
                leftPadding: Style.spacing.rowPaddingX
                rightPadding: Style.spacing.rowPaddingX
                topPadding: Style.space(8)
                textFormat: Text.PlainText
                wrapMode: Text.Wrap
                text: root.speech.memory === true ? root.tr("Changes apply from a new chat.")
                  : root.tr("Memory is off: nothing new is kept, what is here stays until «Forget everything».")
                color: root.dim
                font.family: root.font
                font.pixelSize: Style.font.body
              }
            }
          }

          Text {
            visible: root.page === "memory" && text !== ""
            anchors.centerIn: parent
            textFormat: Text.PlainText
            text: root.memHidden ? root.tr("Hidden while the screen is shared") : !root.omaUp ? root.tr("Oma is not running")
              : root.memRows.length === 1 ? root.tr("Nothing remembered yet") : ""
            color: root.dim
            font.family: root.font
            font.pixelSize: Style.font.heading
          }

          // ---- Settings page: sections and rows in Model.SETTINGS_SECTIONS order. The
          //      controls are the value's picture and the mouse's target;
          //      the keys stay with the catcher (rules.md §9).
          // The rows outgrow the card: they scroll, and the key cursor keeps its row in view.
          Flickable {
            id: settingsView
            visible: root.page === "settings"
            anchors.fill: parent
            clip: true
            contentHeight: settingsPage.implicitHeight
            boundsBehavior: Flickable.StopAtBounds

            Column {
              id: settingsPage
              width: settingsView.width
              spacing: Style.spacing.xs

              SettingRow {
                key: "speaker"
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
                key: "rate"
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

              SettingRow {
                key: "intonation"
                label: root.tr("Intonation by meaning")
                ToggleSwitch {
                  checked: root.speech.intonation === true
                  foreground: root.fg
                  onToggled: root.step("intonation", 1)
                }
              }

              SettingRow {
                key: "earcons"
                label: root.tr("Sound cues")
                ToggleSwitch {
                  checked: root.speech.earcons === true
                  foreground: root.fg
                  onToggled: root.step("earcons", 1)
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
                key: "quiet_now"
                label: root.tr("Quiet now")
                // quiet by reason (screen sharing): the switch is only the user's own quiet, the note says why it is quiet anyway
                note: Model.QUIET_WHY[root.speech.quiet_why] ? root.tr(Model.QUIET_WHY[root.speech.quiet_why]) : ""
                ToggleSwitch {
                  checked: root.speech.quiet === true
                  foreground: root.fg
                  onToggled: root.step("quiet_now", 1)
                }
              }

              SettingRow {
                key: "quiet_schedule"
                label: root.tr("On schedule")
                ToggleSwitch {
                  checked: root.speech.quiet_schedule === true
                  foreground: root.fg
                  onToggled: root.step("quiet_schedule", 1)
                }
              }

              SettingRow {
                key: "quiet_hours"
                label: root.tr("From / to")
                Dropdown {
                  id: fromBox
                  width: Style.space(100)
                  options: Model.HOURS
                  value: root.speech.quiet_from
                  hasCursor: !root.onHead && root.cursor === Model.SETTINGS_ROWS.indexOf("quiet_hours") && root.hoursCol === 0
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
                  hasCursor: !root.onHead && root.cursor === Model.SETTINGS_ROWS.indexOf("quiet_hours") && root.hoursCol === 1
                  foreground: root.fg
                  fontFamily: root.font
                  onChanged: function(v) { root.choose("quiet_to", v, false); value = Qt.binding(function() { return root.speech.quiet_to }) }
                  onPopupOpenChanged: if (!popupOpen) root.refocus()
                }
              }

              SettingRow {
                key: "screencast_privacy"
                label: root.tr("Quiet and brief while sharing the screen")
                ToggleSwitch {
                  checked: root.speech.screencast_privacy === true
                  foreground: root.fg
                  onToggled: root.step("screencast_privacy", 1)
                }
              }

              SettingRow {
                key: "hush_summary"
                label: root.tr("Summary after quiet")
                ToggleSwitch {
                  checked: root.speech.hush_summary === true
                  foreground: root.fg
                  onToggled: root.step("hush_summary", 1)
                }
              }

              PanelSectionHeader {
                leftPadding: Style.spacing.rowPaddingX
                topPadding: Style.space(14)
                bottomPadding: Style.space(4)
                text: root.tr("Hearing").toUpperCase()
                foreground: root.fg
                fontFamily: root.font
                fontSize: Style.font.body
                font.letterSpacing: 1.2
              }

              SettingRow {
                key: "music_pause"
                label: root.tr("Pause music while Oma speaks and listens")
                ToggleSwitch {
                  checked: root.speech.music_pause === true
                  foreground: root.fg
                  onToggled: root.step("music_pause", 1)
                }
              }

              SettingRow {
                key: "headset_button"
                label: root.tr("Headset button")
                note: root.tr("Play — «I'm here» or «hush», next — the summary")
                ToggleSwitch {
                  checked: root.speech.headset_button === true
                  foreground: root.fg
                  onToggled: root.step("headset_button", 1)
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
                key: "orb_over_fullscreen"
                label: root.tr("Over fullscreen windows")
                ToggleSwitch {
                  checked: root.speech.orb_over_fullscreen === true
                  foreground: root.fg
                  onToggled: root.step("orb_over_fullscreen", 1)
                }
              }

              SettingRow {
                key: "orb_satellites"
                label: root.tr("Agent satellites")
                ToggleSwitch {
                  checked: root.speech.orb_satellites === true
                  foreground: root.fg
                  onToggled: root.step("orb_satellites", 1)
                }
              }

              SettingRow {
                key: "orb_moods"
                label: root.tr("Orb mood")
                ToggleSwitch {
                  checked: root.speech.orb_moods === true
                  foreground: root.fg
                  onToggled: root.step("orb_moods", 1)
                }
              }

              SettingRow {
                key: "orb_idle_motion"
                label: root.tr("Breathes at rest")
                ToggleSwitch {
                  checked: root.speech.orb_idle_motion === true
                  foreground: root.fg
                  onToggled: root.step("orb_idle_motion", 1)
                }
              }

              PanelSectionHeader {
                leftPadding: Style.spacing.rowPaddingX
                topPadding: Style.space(14)
                bottomPadding: Style.space(4)
                text: root.tr("Agents").toUpperCase()
                foreground: root.fg
                fontFamily: root.font
                fontSize: Style.font.body
                font.letterSpacing: 1.2
              }

              SettingRow {
                key: "agents_voice"
                label: root.tr("Watch agents")
                ToggleSwitch {
                  checked: root.speech.agents_voice === true
                  foreground: root.fg
                  onToggled: root.step("agents_voice", 1)
                }
              }

              SettingRow {
                key: "progress_digest"
                label: root.tr("Retell progress")
                ToggleSwitch {
                  checked: root.speech.progress_digest === true
                  foreground: root.fg
                  onToggled: root.step("progress_digest", 1)
                }
              }

              SettingRow {
                key: "news_every"
                label: root.tr("Results aloud at most every")
                Dropdown {
                  id: newsBox
                  options: Model.newsEvery(root.tr)
                  value: String(root.speech.news_every)
                  foreground: root.fg
                  fontFamily: root.font
                  onChanged: function(v) { root.choose("news_every", Number(v), false); value = Qt.binding(function() { return String(root.speech.news_every) }) }
                  onPopupOpenChanged: if (!popupOpen) root.refocus()
                }
              }

              SettingRow {
                key: "done_facts"
                label: root.tr("Git facts in results")
                ToggleSwitch {
                  checked: root.speech.done_facts === true
                  foreground: root.fg
                  onToggled: root.step("done_facts", 1)
                }
              }

              SettingRow {
                key: "permission_buttons"
                label: root.tr("Permission buttons")
                ToggleSwitch {
                  checked: root.speech.permission_buttons === true
                  foreground: root.fg
                  onToggled: root.step("permission_buttons", 1)
                }
              }

              SettingRow {
                key: "memory"
                label: root.tr("Remember past days")
                ToggleSwitch {
                  checked: root.speech.memory === true
                  foreground: root.fg
                  onToggled: root.step("memory", 1)
                }
              }

              // A page, not a value: Enter or a click opens what Oma remembers.
              SettingRow {
                key: "memory_page"
                label: root.tr("Memory")
                Text {
                  textFormat: Text.PlainText
                  text: "\u{F0142}"
                  color: root.dim
                  font.family: root.font
                  font.pixelSize: Style.font.title
                }
              }

              SettingRow {
                key: "agent_inbox"
                label: root.tr("Agents write to Oma")
                ToggleSwitch {
                  checked: root.speech.agent_inbox === true
                  foreground: root.fg
                  onToggled: root.step("agent_inbox", 1)
                }
              }

              SettingRow {
                key: "limit_warn"
                label: root.tr("Warn about the limit")
                ToggleSwitch {
                  checked: root.speech.limit_warn === true
                  foreground: root.fg
                  onToggled: root.step("limit_warn", 1)
                }
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
    property bool copied: false   // the copy button shows a check for a moment
    readonly property bool copyable: role === "oma" && body !== ""
    readonly property string tone: Model.tone(role)
    readonly property color base: tone === "me" ? Model.ME_TINT : Color.foreground
    readonly property int pad: Style.space(10)
    implicitHeight: bubble.height

    Rectangle {
      id: bubble
      anchors.right: chatLine.tone === "me" ? parent.right : undefined
      width: chatLine.images.length ? parent.width * 0.8 : Math.min(parent.width * 0.8, Math.max(labelText.implicitWidth + (chatLine.copyable ? copyIcon.implicitWidth + Style.space(16) : 0), bodyText.implicitWidth) + chatLine.pad * 2)
      height: lineText.implicitHeight + chatLine.pad * 2
      color: Qt.tint(Color.popups.background, Qt.rgba(chatLine.base.r, chatLine.base.g, chatLine.base.b, Math.max(0.05, Model.TONE_ALPHA[chatLine.tone])))
      radius: Style.cornerRadius

      Column {
        id: lineText
        x: chatLine.pad
        y: chatLine.pad
        width: parent.width - chatLine.pad * 2
        spacing: Style.space(4)

        // ТЫ/ОМА at the left; Oma's lines have a copy button at the right.
        Item {
          width: parent.width
          height: labelText.implicitHeight

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
            id: copyIcon
            visible: chatLine.copyable
            anchors.right: parent.right
            anchors.verticalCenter: parent.verticalCenter
            textFormat: Text.PlainText
            text: chatLine.copied ? "\u{F012C}" : "\u{F018F}"
            color: copyArea.containsMouse ? root.fg : root.dim
            font.family: root.font
            font.pixelSize: Style.font.title

            MouseArea {
              id: copyArea
              anchors.fill: parent
              anchors.margins: -Style.space(6)
              hoverEnabled: true
              cursorShape: Qt.PointingHandCursor
              onClicked: { root.copy(chatLine.body); chatLine.copied = true; copiedTimer.restart() }
            }
            PanelToolTip { visible: copyArea.containsMouse; text: chatLine.copied ? root.tr("Copied") : root.tr("Copy") }
            Timer { id: copiedTimer; interval: 1500; onTriggered: chatLine.copied = false }
          }
        }

        // Selectable: a drag selects, Ctrl+C copies; other keys go back to the field.
        TextEdit {
          id: bodyText
          width: parent.width
          readOnly: true
          selectByMouse: true
          // Agents answer in Markdown: lists, headings, code keep their shape
          // (the daemon drops ![images](…): MarkdownText would fetch them).
          textFormat: TextEdit.MarkdownText
          onLinkActivated: function(link) { Qt.openUrlExternally(link) }
          text: chatLine.body
          color: chatLine.bodyColor
          selectionColor: Color.accent
          selectedTextColor: Color.popups.background
          font.family: root.font
          font.pixelSize: Style.font.title
          wrapMode: TextEdit.Wrap
          visible: chatLine.body !== ""
          Keys.onPressed: function(e) {
            if (e.matches(StandardKey.Copy)) root.copy(bodyText.selectedText)
            else if (e.key === Qt.Key_Escape) { bodyText.deselect(); root.refocus() }
            else if (e.text !== "" && !(e.modifiers & Qt.ControlModifier)) { bodyText.deselect(); root.typeIn(e.text) }
            else return
            e.accepted = true
          }
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

  // A settings row: its label at the left (a note, why it is so now, under it), its control (the children) at the right.
  // Its place for the key cursor is its key's in Model.SETTINGS_ROWS.
  component SettingRow: CursorSurface {
    id: settingRow
    property string key: ""
    readonly property int row: Model.SETTINGS_ROWS.indexOf(key)
    property string label: ""
    property string note: ""    // a second line, dim; "" — none
    property bool caps: false   // a section caption (bold caps) rather than a row's text
    default property alias control: slot.data
    width: settingsPage.width
    implicitHeight: Math.max(Style.space(50), Math.max(slot.implicitHeight, texts.implicitHeight) + Style.spacing.xs * 2)
    foreground: root.fg
    hasCursor: !root.onHead && root.cursor === row

    MouseArea {
      id: rowArea
      anchors.fill: parent
      hoverEnabled: true
      onPositionChanged: function(mouse) {
        if (pointerGate.moved(rowArea, mouse)) { root.onHead = false; root.setCursor(settingRow.row) }
      }
      onClicked: { root.onHead = false; root.setCursor(settingRow.row); root.activate() }
    }

    Column {
      id: texts
      anchors.left: parent.left
      anchors.leftMargin: Style.spacing.rowPaddingX
      anchors.right: slot.left
      anchors.rightMargin: Style.space(8)
      anchors.verticalCenter: parent.verticalCenter

      Text {
        width: parent.width
        textFormat: Text.PlainText
        text: settingRow.label
        color: settingRow.caps ? root.dim : root.fg
        font.family: root.font
        font.pixelSize: settingRow.caps ? Style.font.body : Style.font.title
        font.bold: settingRow.caps
        font.letterSpacing: settingRow.caps ? 1.2 : 0
        elide: Text.ElideRight
      }

      Text {
        visible: settingRow.note !== ""
        width: parent.width
        textFormat: Text.PlainText
        text: settingRow.note
        color: root.dim
        font.family: root.font
        font.pixelSize: Style.font.body
        elide: Text.ElideRight
      }
    }

    Row {
      id: slot
      anchors.right: parent.right
      anchors.rightMargin: Style.spacing.rowPaddingX
      anchors.verticalCenter: parent.verticalCenter
      spacing: Style.space(8)
    }
  }

  // A caption over the main page's rows («Permissions», «Later», «Agents»): dim bold caps.
  component RowsCaption: Text {
    leftPadding: Style.spacing.rowPaddingX
    topPadding: Style.space(6)
    textFormat: Text.PlainText
    color: root.dim
    font.family: root.font
    font.pixelSize: Style.font.body
    font.bold: true
    font.letterSpacing: 1.2
  }
}
