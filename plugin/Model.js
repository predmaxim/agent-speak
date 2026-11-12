.pragma library

// agent-speakd's lines -> what the orb and the window show, and the
// command lines the plugin writes to the daemon's socket.

var SAMPLE = "Так звучит этот голос. Слышно, что я спрашиваю?"

// No connection to the daemon: the status-line fields, empty, in the line's order (struct Status, test.js checks).
var OFFLINE = { running: false, speaking: false, paused: false, speaker: "", rate: "", project: "",
  oma: "off", talking: false, agents_voice: false, progress_digest: false, quiet: false, quiet_schedule: false, quiet_from: "", quiet_to: "",
  orb_pos: "", orb_over_fullscreen: true, waiting: 0, trouble: 0, played_at: 0, out: "", voiced: 0, user_paused: false,
  // the Jarvis wave's fields, declared ahead (README, «Устройство»)
  quiet_why: "", dnd_quiet: false, hush_summary: false, screencast_privacy: false, orb_satellites: false,
  media_hold: false, music_pause: false, limit: "", limit_warn: false, ear_in: "", listen_until: null, listen_secs: 0,
  struggling: 0, orb_moods: false, orb_idle_motion: false, rate_now: "", news_every: 0, earcons: false, intonation: false,
  done_facts: false, permission_buttons: false, agent_inbox: false, wake: "off", wake_word: false, memory: false }

// One socket line -> {type, value}, or null for anything else (a broken line must not blank the state).
function parseLine(line) {
  var s = null
  try { s = JSON.parse(line) } catch (e) { return null }
  if (!s || typeof s !== "object") return null
  if (typeof s.running === "boolean") return { type: "state", value: s }
  var types = ["agents", "bubble", "chat", "chat_delta", "chats", "ripple", "permissions", "later", "memory", "shown"]
  var lists = ["agents", "chats", "permissions", "later"]
  for (var i = 0; i < types.length; i++)
    if (s[types[i]] !== undefined) {
      var v = s[types[i]]
      if (lists.indexOf(types[i]) >= 0 && !Array.isArray(v)) v = []
      return { type: types[i], value: v }
    }
  return null
}

// What the orb shows, by priority. Oma down is "off", yet a waiting agent still shows. `muted`: the mic is off (the
// default input or Oma's own, read by the orb from PipeWire) — while she hears, the grey disc with the crossed mic.
function orbMode(st, muted) {
  if (!st || !st.running) return "off"
  if (muted && ["loading", "listening", "paused"].indexOf(st.oma) >= 0) return "muted"
  if (["loading", "listening", "thinking", "speaking", "paused"].indexOf(st.oma) >= 0) return st.oma
  if (st.speaking) return "speaking"   // news or manual reading: the service speaks, Oma is idle
  if (st.trouble > 0 && st.agents_voice !== false) return "trouble"   // the watch needs the user, above waiting
  if (st.waiting > 0 && st.agents_voice !== false) return "waiting"   // not watching: no red
  if (st.oma === "off") return "off"
  return st.quiet || QUIET_WHY[st.quiet_why] ? "quiet" : "idle"
}

// Quiet by reason (the daemon's quiet_why) -> its tooltip line; "dnd" waits for Omarchy's signal (README).
var QUIET_WHY = { dnd: "Quiet: do not disturb", screencast: "Quiet: screen sharing" }

// The window header's items, left to right; the power switch is at the edge.
// Oma off: nothing but the switch works; a page has only Back.
function headItems(page, running) {
  if (page !== "main") return ["back"]
  return running ? ["talk", "new", "history", "sound", "settings", "power"] : ["power"]
}

// Tooltip: its lines, each a key for tr(key, arg); the orb's tooltip and the window header draw them all.
function orbTip(st, muted) {
  var mode = orbMode(st, muted)
  var tips = { off: "Oma is not running", loading: "Oma is loading", listening: "Oma is listening", thinking: "Oma is thinking",
    speaking: "Oma is speaking", paused: "Paused", quiet: "Quiet mode", idle: "Oma", muted: "Mic is off" }
  var why = st && QUIET_WHY[st.quiet_why]
  var lines = mode === "trouble" ? [["%1 need help", st.trouble]] : mode === "waiting" ? [["%1 waiting", st.waiting]]
    : mode === "quiet" && !st.quiet && why ? [] : [[tips[mode], ""]]   // quiet by reason only: the reason is the line
  if (why) lines.push([why, ""])
  // the wheel's tempo of this answer (the daemon's rate_now): until the answer ends
  if (st && TEMPO[st.rate_now]) lines.push([TEMPO[st.rate_now], ""])
  // a limit window near its end: the daemon's phrase, already Russian, so no tr line — it passes tr() as is
  if (st && st.limit) lines.push([st.limit, ""])
  return lines
}

// The ear meter: PwNodePeakMonitor's peak (linear 0..1) in decibels, −50 dB → 0 … −10 dB → 1 (a calibration for the
// laptop's mic and a headset: speech at arm's length is about −30); no node — −1.
var LEVEL_FLOOR = -50
var LEVEL_TOP = -10
function level(peak) {
  if (peak < 0) return -1
  var db = peak > 0 ? 20 * Math.log(peak) / Math.LN10 : -Infinity
  return Math.max(0, Math.min(1, (db - LEVEL_FLOOR) / (LEVEL_TOP - LEVEL_FLOOR)))
}

// How long she still listens, a share of the window: the daemon's listen_until (epoch, s) and listen_secs at `now`;
// −1 — no arc.
function listenLeft(until, secs, now) {
  if (until === null || until === undefined || !(secs > 0)) return -1
  return Math.max(0, Math.min(1, (until - now) / secs))
}

// The «done» ring, until seen: `seen` maps an agent id to the since of its done the user has seen — its tab in focus
// or its chat open in Oma's window (`open`, a chat key). Gone agents are forgotten. Returns the new map.
function seenDone(seen, agents, open) {
  var out = {}
  Array.prototype.slice.call(agents || []).forEach(function(a) {
    if (a.state === "done" && (a.focused || open === "agent:" + a.id)) out[a.id] = a.since
    else if (seen[a.id] !== undefined) out[a.id] = seen[a.id]
  })
  return out
}

// A done agent the user has not seen yet: the ring burns.
function doneUnseen(agents, seen) {
  return Array.prototype.slice.call(agents || []).some(function(a) { return a.state === "done" && seen[a.id] !== a.since })
}

// The tempo line by the daemon's rate (RATES in src/config.rs).
var TEMPO = { "x-slow": "Tempo: very slow", slow: "Tempo: slow", medium: "Tempo: normal", fast: "Tempo: fast", "x-fast": "Tempo: very fast" }

// The wheel over the orb: angleDelta.y piles up in `acc`, a step per ±120 (a notch; a touchpad sends many small ones —
// stepping on each, as the stock PanelSlider does, would jump), the rest is kept for the next turn.
function wheelStep(acc, dy) {
  var a = acc + dy, n = Math.trunc(a / 120) || 0   // || 0: no -0
  return { steps: n, acc: a - n * 120 }
}

function rateStep(step) {
  return JSON.stringify({ cmd: "rate_step", step: step }) + "\n"
}

// An agent's dot (the window's, the orb's card and satellites): its state; the watch's word (orange) is above any but
// waiting — the question's red stays first. An agent's note to Oma («needs», `agent-speak note`) waits for the user too.
function dotKind(a) {
  if (a.state === "waiting" || a.note) return "waiting"
  if (a.health) return "health"
  return a.state === "working" || a.state === "done" ? a.state : "idle"
}

var URGENCY = ["waiting", "health", "working", "done", "idle"]

// Agents, the most urgent first: waiting (the longest waiting on top), the watch's word, working, done, idle (the
// freshest on top). The daemon's agent line keeps its own order (stable for the window), so the orb sorts here.
function byUrgency(agents) {
  // through a QML signal or property the array may be an array-like sequence: Array.isArray is false for it
  return Array.prototype.slice.call(agents || []).map(function(a, i) { return { a: a, i: i, g: URGENCY.indexOf(dotKind(a)) } })
    .sort(function(x, y) { return x.g - y.g || (x.g === 0 ? x.a.since - y.a.since : y.a.since - x.a.since) || x.i - y.i })
    .map(function(x) { return x.a })
}

function clip(s, n) {
  s = String(s || "").trim()
  return s.length > n ? s.slice(0, n - 1) + "…" : s
}

var PEEK_MAX = 8
var FAILS_DROOP = 3   // as agents::FAILS_DROOP

// The orb's hover card: {rows, more} — up to PEEK_MAX agents, the most urgent first, and how many more. A row: its
// dot, name, time in its state (a tr key, at `now`) and a second line up to 60 chars — the question's first line for
// a waiting one (or its note), the watch's word, else the status (or the count of failed calls, `fails`).
function peekRows(agents, now) {
  var all = byUrgency(agents)
  return { more: Math.max(0, all.length - PEEK_MAX), rows: all.slice(0, PEEK_MAX).map(function(a) {
    var dot = dotKind(a)
    var ask = String(a.ask || "").split("\n").filter(function(l) { return l.trim() !== "" })[0]
    var text = dot === "waiting" ? ask || a.note || a.status : dot === "health" ? a.health : a.status
    // fails: tool calls that could not run this turn (the daemon's FAILS_DROOP and up) — a working row counts them instead
    return { id: a.id, dot: dot, name: a.title || a.project || "", ago: ago(a.since, now), text: clip(text, 60),
      fails: dot === "working" && a.fails >= FAILS_DROOP ? a.fails : 0 }
  }) }
}

var SAT_MAX = 5

// The satellites' places, `max` of them, an agent id or "" each: a living agent keeps its place (prev, the last
// result), a new one takes the first free place, the most urgent first, a gone one frees its own. Over `max` agents:
// the most urgent max - 1 and "+" — the hollow «more» — at the last place.
function satSlots(prev, agents, max) {
  var all = byUrgency(agents), n = all.length > max ? max - 1 : max
  var ids = all.slice(0, n).map(function(a) { return a.id })
  var slots = []
  for (var i = 0; i < n; i++) slots.push(prev && ids.indexOf(prev[i]) >= 0 ? prev[i] : "")
  ids.forEach(function(id) { if (slots.indexOf(id) < 0) slots[slots.indexOf("")] = id })
  return n < max ? slots.concat(["+"]) : slots
}

function agentById(agents, id) {
  return Array.prototype.slice.call(agents || []).filter(function(a) { return a.id === id })[0] || null
}

// A satellite's flash: `key` ("<id>:<dotKind>") says its agent waits now, and `was` is the same agent before it did.
function asks(was, key) {
  var id = key.slice(0, key.lastIndexOf(":") + 1)
  return /:waiting$/.test(key) && was !== key && id !== "" && was.indexOf(id) === 0
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

// Quiet mode keeps the bubbles until dismissed; while the screen is shared they fade as usual: they must not hang on someone else's screen.
function bubblesKept(st) {
  return !!st && st.quiet === true && st.quiet_why !== "screencast"
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

var TONE_ALPHA = { me: 0.22, oma: 0.07, agent: 0 }

// My lines' own tint, the orb's listening green: the theme's accent may be
// grey, the foreground's shade (rules.md, the agent-speak section).
var ME_TINT = "#6fd38c"

// Esc is grabbed only while it means something: a draft Oma is still hearing
// (listening or loading; paused means the mic is someone else's, Esc too), or
// the hold after a cancel.
function escWanted(draft, st, holding) {
  return (draft !== null && !!st && st.running && ["listening", "loading"].indexOf(st.oma) >= 0) || holding
}

function dismiss(stack, id) {
  return stack.filter(function(b) { return b.id !== id })
}

// The window's main page: row 0 is Oma's general chat, then the agents. The
// key cursor keeps a row's chat key, not its number: news and new agents move
// rows, the cursor stays on its agent; a row that is gone is -1.
function rowKey(agents, i) {
  return i === 0 ? "general" : i > 0 && i <= agents.length ? "agent:" + agents[i - 1].id : ""
}

function rowIndex(agents, key) {
  if (key === "general") return 0
  for (var i = 0; i < agents.length; i++) if ("agent:" + agents[i].id === key) return i + 1
  return -1
}

// Oma (the Python half) takes commands: the daemon is up and she has said
// her state; "off" — she is down or still starting, the daemon drops them.
function omaUp(st) {
  return !!st && st.running === true && st.oma !== "off"
}

// A typed question goes out once, while Oma takes it; the field keeps it
// until it is `taken`: her chat echoes my line back with the question's id.
function askable(text, asked, st) {
  return text !== "" && text !== asked && omaUp(st)
}

function taken(id, d) {
  return id !== "" && d.role === "me" && d.id === id
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

// Text for the clipboard: without the stress marks (U+0301) Oma puts in for the voice.
function plainCopy(text) {
  return String(text || "").replace(/\u0301/g, "")
}

// A `chat` line against the window's chat `cur`: the chat to show (the key
// switches when the chat it showed got its id, `was`, and to Oma's current
// chat on her answer to a keyless `open`, `was` "*"), or null when the line
// is about another chat.
function chatFor(cur, c) {
  var key = c.was === "*" || c.was === cur ? c.key : cur
  // through a QML signal the array arrives as an array-like sequence: Array.isArray is false for it
  var list = c.messages && typeof c.messages.length === "number" ? Array.prototype.slice.call(c.messages) : []
  return c.key === key ? { key: key, messages: list } : null
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

// Which agent's chat the window shows (or none): Oma keeps that agent's news quiet.
function view(shown, key) {
  return JSON.stringify({ cmd: "view", agent: shown && key.indexOf("agent:") === 0 ? key.slice(6) : "" }) + "\n"
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
  var voices = ["kseniya", "xenia", "baya", "aidar", "eugene"] // как SPEAKERS в src/config.rs: умолчание первым
  return [
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

// The settings page, top to bottom: one fixed order of sections — the voice, quiet mode, hearing, the orb,
// agents (the Jarvis wave's design). The voice has no header: its first rows are captions themselves.
// A new setting goes into its section's rows and into Window.qml at the same place (test.js checks);
// a section's header is drawn once it has rows.
var SETTINGS_SECTIONS = [
  { title: "", rows: ["speaker", "rate", "intonation", "earcons"] },
  { title: "Quiet mode", rows: ["quiet_now", "quiet_schedule", "quiet_hours", "screencast_privacy", "hush_summary"] },
  { title: "Hearing", rows: ["music_pause"] },
  { title: "Orb", rows: ["orb_over_fullscreen", "orb_satellites", "orb_moods", "orb_idle_motion"] },
  { title: "Agents", rows: ["agents_voice", "progress_digest", "news_every", "memory", "memory_page", "agent_inbox", "limit_warn"] }
]

// «Results aloud at most every»: minutes between batches of finished turns (news_every); 0 — each at once, no batch.
var NEWS_EVERY = [0, 2, 5, 10, 15, 30]

// Its dropdown's options: the stock Dropdown keeps values as strings.
function newsEvery(tr) {
  return NEWS_EVERY.map(function(v) { return { value: String(v), label: v ? tr("%1 min", v) : tr("No limit") } })
}

// The rows in that order: a row's place on the page (the key cursor) is its index here.
var SETTINGS_ROWS = [].concat.apply([], SETTINGS_SECTIONS.map(function(s) { return s.rows }))

// ←/→ (dx) or Enter (dx 1) on a settings row: the `set` to send ({key,
// value, sample}) or null. Choices go round, switches flip; the hours row
// has two dropdowns, the window walks them itself.
function settingStep(row, st, dx) {
  if (row === "quiet_now") return { key: "quiet_override", value: st.quiet ? "off" : "on", sample: false }
  if (row === "quiet_schedule") return { key: "quiet_schedule", value: !st.quiet_schedule, sample: false }
  if (row === "orb_over_fullscreen") return { key: row, value: !st.orb_over_fullscreen, sample: false }
  if (row === "orb_satellites" || row === "orb_moods" || row === "orb_idle_motion") return { key: row, value: !st[row], sample: false }
  if (row === "progress_digest") return { key: row, value: !st.progress_digest, sample: false }
  if (row === "agents_voice") return { key: row, value: !st.agents_voice, sample: false }
  if (row === "screencast_privacy" || row === "hush_summary") return { key: row, value: !st[row], sample: false }
  if (row === "music_pause") return { key: row, value: !st.music_pause, sample: false }
  if (row === "limit_warn" || row === "agent_inbox") return { key: row, value: !st[row], sample: false }
  if (row === "memory") return { key: row, value: !st.memory, sample: false }
  if (row === "intonation" || row === "earcons") return { key: row, value: !st[row], sample: false }
  if (row === "news_every") {
    var k = NEWS_EVERY.length, j = NEWS_EVERY.indexOf(st.news_every)
    return { key: row, value: NEWS_EVERY[j < 0 ? 0 : ((j + dx) % k + k) % k], sample: false }
  }
  var sec = sections(function(t) { return t }).filter(function(x) { return x.key === row })[0]
  if (!sec) return null
  var values = sec.options.map(function(o) { return o.value })
  var n = values.length, i = values.indexOf(st[row])
  return { key: row, value: values[i < 0 ? 0 : ((i + dx) % n + n) % n], sample: sec.sample }
}

// The memory page's rows, top to bottom: the «I remember» lines (as in MEMORY.md, a line is its own id), the day
// notes (the freshest first, id — the date), «Forget everything»; none before Oma's answer.
function memoryRows(mem) {
  if (!mem) return []
  // through a QML signal the arrays arrive as array-like sequences
  var list = function(x) { return Array.prototype.slice.call(x || []) }
  return list(mem.prefs).map(function(p) { return { kind: "pref", id: p, text: p } })
    .concat(list(mem.days).map(function(d) { return { kind: "day", id: d.id, text: d.text } }))
    .concat([{ kind: "all", id: "", text: "" }])
}

// Music pause: Oma took the sound (the daemon's media_hold) — the MPRIS players to pause: playing ones that can pause.
function toPause(players) {
  return players.filter(function(p) { return p.isPlaying && p.canPause })
}

// Oma let the sound go — the players to play again: the ones paused for her (their dbusName in `held`), still paused
// (pausedState is MprisPlaybackState.Paused) and able to play. Anything playing by then (the user started it) — none.
function toResume(players, held, pausedState) {
  if (players.some(function(p) { return p.isPlaying })) return []
  return players.filter(function(p) { return held.indexOf(p.dbusName) >= 0 && p.playbackState === pausedState && p.canPlay })
}
