.pragma library

// agent-speakd's lines -> what the orb and the window show, and the
// command lines the plugin writes to the daemon's socket.

var SAMPLE = "Так звучит этот голос."

// No connection to the daemon: the status-line fields, empty.
var OFFLINE = { running: false, speaking: false, paused: false, read_intermediate: false, speaker: "", rate: "", project: "",
  oma: "off", talking: false, agents_voice: false, quiet: false, quiet_schedule: false, quiet_from: "", quiet_to: "", orb_pos: "", orb_over_fullscreen: true, waiting: 0 }

// One socket line -> {type, value}, or null for anything else (a broken line must not blank the state).
function parseLine(line) {
  var s = null
  try { s = JSON.parse(line) } catch (e) { return null }
  if (!s || typeof s !== "object") return null
  if (typeof s.running === "boolean") return { type: "state", value: s }
  var types = ["agents", "bubble", "chat", "chat_delta", "chats"]
  for (var i = 0; i < types.length; i++)
    if (s[types[i]] !== undefined) {
      var v = s[types[i]]
      if ((types[i] === "agents" || types[i] === "chats") && !Array.isArray(v)) v = []
      return { type: types[i], value: v }
    }
  return null
}

// What the orb shows, by priority. Oma down is "off", yet a waiting agent still shows.
function orbMode(st) {
  if (!st || !st.running) return "off"
  if (["loading", "listening", "thinking", "speaking", "paused"].indexOf(st.oma) >= 0) return st.oma
  if (st.waiting > 0) return "waiting"
  if (st.oma === "off") return "off"
  return st.quiet ? "quiet" : "idle"
}

// Tooltip: a key for tr(key, arg).
function orbTip(st) {
  var mode = orbMode(st)
  var tips = { off: "Oma is not running", loading: "Oma is loading", listening: "Oma is listening", thinking: "Oma is thinking",
    speaking: "Oma is speaking", paused: "Paused", quiet: "Quiet mode", idle: "Oma" }
  return mode === "waiting" ? ["%1 waiting", st.waiting] : [tips[mode], ""]
}

var BUBBLES_MAX = 3
var BUBBLE_TTL = 10000

// The oldest of three bubbles fades away from its neighbour: alpha at the
// edge towards it and at the far edge.
var OLDEST_NEAR = 0.8
var OLDEST_FAR = 0.1

// MultiEffect's softest mask (maskThresholdMin 0.5, maskSpreadAtMin 1) draws
// smoothstep(mask alpha), measured on Qt 6.11; this is the mask alpha that
// comes out as `alpha`.
function maskAlpha(alpha) {
  return 0.5 - Math.sin(Math.asin(1 - 2 * alpha) / 3)
}

// Add, update (append or replace) or drop a bubble; the touched one goes last.
function pushBubble(stack, b, now) {
  var rest = stack.filter(function(x) { return x.id !== b.id })
  if (b.drop) return rest
  var old = stack.filter(function(x) { return x.id === b.id })[0]
  var text = old ? old.text : ""
  if (b.text !== undefined) text = old && b.append ? old.text + " " + b.text : b.text
  var nb = Object.assign({}, old || {}, b, { text: text, at: now })
  return rest.concat([nb]).slice(-BUBBLES_MAX)
}

// Everything goes once the newest bubble is old enough; quiet mode keeps them.
function expire(stack, now, quiet) {
  if (quiet || !stack.length) return stack
  var last = Math.max.apply(null, stack.map(function(b) { return b.at }))
  return now - last >= BUBBLE_TTL ? [] : stack
}

// The draft is shown apart from the TTL'd stack: last, and the stack still holds BUBBLES_MAX.
function withDraft(stack, draft) {
  return draft ? stack.concat([draft]).slice(-BUBBLES_MAX) : stack
}

// Bubble kind (or chat role) -> tint: mine and the draft, Oma's, any agent's.
function tone(kind) {
  return kind === "me" || kind === "draft" ? "me" : kind === "oma" ? "oma" : "agent"
}

var TONE_ALPHA = { me: 0.18, oma: 0.07, agent: 0 }

// Esc is grabbed only while it means something: a draft Oma is still hearing
// (listening or loading; paused means the mic is someone else's, Esc too), or
// the hold after a cancel.
function escWanted(draft, st, holding) {
  return (draft !== null && !!st && st.running && ["listening", "loading"].indexOf(st.oma) >= 0) || holding
}

function dismiss(stack, id) {
  return stack.filter(function(b) { return b.id !== id })
}

// Time since `since` (seconds): a key for tr(key, n).
function ago(since, now) {
  var d = Math.max(0, now - since)
  if (d < 60) return ["just now", 0]
  if (d < 3600) return ["%1 min", Math.floor(d / 60)]
  if (d < 86400) return ["%1 h", Math.floor(d / 3600)]
  return ["%1 d", Math.floor(d / 86400)]
}

function stateLabel(state) {
  return { working: "Working", waiting: "Waiting", done: "Done" }[state] || "Idle"
}

// A streamed message: Oma's consecutive parts join into one, mine start a new one.
function chatApply(messages, delta) {
  var last = messages[messages.length - 1]
  if (delta.role === "oma" && last && last.role === "oma")
    return messages.slice(0, -1).concat([{ role: "oma", text: last.text + " " + delta.text }])
  return messages.concat([{ role: delta.role, text: delta.text }])
}

// A `chat` line against the window's chat `cur`: the chat to show (the key
// switches when the chat it showed got its id, `was`, and to Oma's current
// chat on her answer to a keyless `open`, `was` "*"), or null when the line
// is about another chat.
function chatFor(cur, c) {
  var key = c.was === "*" || c.was === cur ? c.key : cur
  return c.key === key ? { key: key, messages: Array.isArray(c.messages) ? c.messages : [] } : null
}

// The screen corner nearest to the orb's centre (x, y) on a w x h screen.
function corner(x, y, w, h) {
  return (y < h / 2 ? "top" : "bottom") + "-" + (x < w / 2 ? "left" : "right")
}

// "<corner>,<dx>,<dy>" -> corner flags and offsets from it, clamped to the
// area (w x h) the slot (side `slot`) can move in; junk is the default corner.
function parsePos(s, w, h, slot) {
  var m = /^(top|bottom)-(left|right),(\d+),(\d+)$/.exec(s || "")
  if (!m) return { atTop: false, atLeft: false, dx: 0, dy: 0 }
  return { atTop: m[1] === "top", atLeft: m[2] === "left",
    dx: Math.min(+m[3], Math.max(0, w - slot)), dy: Math.min(+m[4], Math.max(0, h - slot)) }
}

// Where a drag ended: (x, y) is the slot's top-left in the w x h area. The
// corner is the one nearest the slot's centre; within `snap` of it, sticks.
function place(x, y, w, h, slot, snap) {
  var c = corner(x + slot / 2, y + slot / 2, w, h)
  var dx = Math.max(0, Math.round(c.indexOf("left") >= 0 ? x : w - slot - x))
  var dy = Math.max(0, Math.round(c.indexOf("top") >= 0 ? y : h - slot - y))
  return Math.hypot(dx, dy) <= snap ? c + ",0,0" : c + "," + dx + "," + dy
}

var HOURS = []
for (var i = 0; i < 24; i++) HOURS.push((i < 10 ? "0" : "") + i + ":00")

function omaCmd(action) {
  return JSON.stringify({ cmd: "oma", action: action }) + "\n"
}

function cmd(name) {
  return JSON.stringify({ cmd: name }) + "\n"
}

function set(key, value) {
  return JSON.stringify({ cmd: "set", key: key, value: value }) + "\n"
}

function say(text) {
  return JSON.stringify({ cmd: "say", text: text }) + "\n"
}

// The window's selectors. Values are exactly what the daemon's `set` accepts;
// sample: play SAMPLE after the choice, to compare by ear.
function sections(tr) {
  var voices = ["xenia", "baya", "kseniya", "aidar", "eugene"]
  return [
    { key: "read_intermediate", caption: tr("INTERMEDIATE STATUSES"), sample: false,
      options: [{ value: true, label: tr("Read") }, { value: false, label: tr("Final answer only") }] },
    { key: "speaker", caption: tr("VOICE"), sample: true,
      options: voices.map(function(v) { return { value: v, label: v.charAt(0).toUpperCase() + v.slice(1) } }) },
    { key: "rate", caption: tr("SPEED"), sample: true,
      options: [
        { value: "x-slow", label: tr("Very slow") }, { value: "slow", label: tr("Slow") },
        { value: "medium", label: tr("Normal") }, { value: "fast", label: tr("Fast") },
        { value: "x-fast", label: tr("Very fast") }
      ] }
  ]
}

// The settings page's rows, top to bottom: the three sections, quiet mode, the orb.
var SETTINGS_ROWS = ["read_intermediate", "speaker", "rate", "quiet_now", "quiet_schedule", "quiet_hours", "orb_over_fullscreen"]

// ←/→ (dx) or Enter (dx 1) on a settings row: the `set` to send ({key,
// value, sample}) or null. Choices go round, switches flip; the hours row
// has two dropdowns, the window walks them itself.
function settingStep(row, st, dx) {
  if (row === "quiet_now") return { key: "quiet_override", value: st.quiet ? "off" : "on", sample: false }
  if (row === "quiet_schedule") return { key: "quiet_schedule", value: !st.quiet_schedule, sample: false }
  if (row === "orb_over_fullscreen") return { key: row, value: !st.orb_over_fullscreen, sample: false }
  var sec = sections(function(t) { return t }).filter(function(x) { return x.key === row })[0]
  if (!sec) return null
  var values = sec.options.map(function(o) { return o.value })
  var n = values.length, i = values.indexOf(st[row])
  return { key: row, value: values[i < 0 ? 0 : ((i + dx) % n + n) % n], sample: sec.sample }
}
