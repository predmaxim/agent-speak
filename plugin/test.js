// node test.js
const fs = require("fs")
const assert = require("assert")
const load = (file, names) =>
  new Function(fs.readFileSync(__dirname + "/" + file, "utf8").replace(".pragma library", "") + "; return { " + names + " }")()
const M = load("Model.js", "OFFLINE, SAMPLE, parseLine, orbMode, headItems, orbTip, pushBubble, expire, dismiss, ago, stateLabel, chatApply, corner, omaCmd, HOURS, cmd, set, say, sections, OLDEST_NEAR, OLDEST_FAR, maskAlpha, chatFor, SETTINGS_SECTIONS, SETTINGS_ROWS, settingStep, parsePos, place, withDraft, tone, TONE_ALPHA, escWanted, view, plainCopy, rowKey, rowIndex, omaUp, askable, taken, dotKind, byUrgency, peekRows, PEEK_MAX, satSlots, SAT_MAX, asks, agentById")
const I = load("I18n.js", "TABLES, translator")
const ru = I.translator("ru")
const st = { running: true, oma: "idle", waiting: 0, quiet: false }
assert.strictEqual(M.parseLine('{"running":true,"oma":"idle"}').type, "state")
assert.strictEqual(M.parseLine('{"agents":[]}').type, "agents")
assert.strictEqual(M.parseLine('{"bubble":{"id":"x"}}').value.id, "x")
assert.strictEqual(M.parseLine('{"chat":{"key":"k"}}').type, "chat")
assert.strictEqual(M.parseLine('{"chat_delta":{"key":"k"}}').type, "chat_delta")
assert.strictEqual(M.parseLine('{"chats":[]}').type, "chats")
assert.strictEqual(M.parseLine('{"nope":1}'), null)
assert.deepStrictEqual(M.parseLine('{"agents":null}').value, [])
assert.deepStrictEqual(M.parseLine('{"chats":{}}').value, [])
// The Jarvis wave's lines: a ripple on the orb, the permission cards, the deferred orders (lists, like agents)
assert.deepStrictEqual(M.parseLine('{"ripple":{"tone":"final"}}'), { type: "ripple", value: { tone: "final" } })
assert.deepStrictEqual(M.parseLine('{"permissions":[{"id":"perm-7"}]}'), { type: "permissions", value: [{ id: "perm-7" }] })
assert.deepStrictEqual(M.parseLine('{"later":[]}'), { type: "later", value: [] })
assert.deepStrictEqual(M.parseLine('{"permissions":null}').value, [])
assert.deepStrictEqual(M.parseLine('{"later":"x"}').value, [])
assert.strictEqual(M.parseLine("{broken"), null)
assert.strictEqual(M.parseLine("null"), null)
assert.strictEqual(M.orbMode(M.OFFLINE), "off")
assert.strictEqual(M.orbMode(null), "off")
assert.strictEqual(M.orbMode(st), "idle")
assert.strictEqual(M.orbMode(Object.assign({}, st, { waiting: 2, quiet: true })), "waiting")
assert.strictEqual(M.orbMode(Object.assign({}, st, { quiet: true })), "quiet")
assert.strictEqual(M.orbMode(Object.assign({}, st, { oma: "listening", waiting: 1 })), "listening")
// Watch agents off: a waiting agent does not turn the orb red
assert.strictEqual(M.orbMode(Object.assign({}, st, { waiting: 2, agents_voice: false })), "idle")
// The header: off — only the power switch; a page — only Back
assert.deepStrictEqual(M.headItems("main", true), ["talk", "new", "history", "sound", "settings", "power"])
assert.deepStrictEqual(M.headItems("main", false), ["power"])
assert.deepStrictEqual(M.headItems("settings", true), ["back"])
// news and manual reading are the service's speech, Oma herself is idle: still "speaking", not the waiting red
assert.strictEqual(M.orbMode(Object.assign({}, st, { speaking: true, waiting: 1 })), "speaking")
// Oma is down: the orb says so, but a waiting agent still turns it yellow
assert.strictEqual(M.orbMode(Object.assign({}, st, { oma: "off" })), "off")
assert.strictEqual(M.orbMode(Object.assign({}, st, { oma: "off", quiet: true })), "off")
assert.strictEqual(M.orbMode(Object.assign({}, st, { oma: "off", waiting: 1 })), "waiting")
// The watch needs the user: its own orange, above waiting; not while watching is off, not over speech
assert.strictEqual(M.orbMode(Object.assign({}, st, { trouble: 1 })), "trouble")
assert.strictEqual(M.orbMode(Object.assign({}, st, { trouble: 1, waiting: 2 })), "trouble")
assert.strictEqual(M.orbMode(Object.assign({}, st, { trouble: 1, agents_voice: false })), "idle")
assert.strictEqual(M.orbMode(Object.assign({}, st, { trouble: 1, speaking: true })), "speaking")
assert.strictEqual(M.OFFLINE.trouble, 0)
// The tooltip is a list of lines [key, arg]: the orb's tooltip and the window header draw them all (one line today)
assert.deepStrictEqual(M.orbTip(Object.assign({}, st, { trouble: 1 })), [["%1 need help", 1]])
assert.deepStrictEqual(M.orbTip(st), [["Oma", ""]])
let s = []
for (const id of ["a", "b", "c", "d"]) s = M.pushBubble(s, { id, who: "T", text: id, kind: "status" }, 1000)
assert.deepStrictEqual(s.map(b => b.id), ["b", "c", "d"])
s = M.pushBubble(s, { id: "b", text: "ещё", append: true }, 2000)
assert.deepStrictEqual(s.map(b => [b.id, b.text]), [["c", "c"], ["d", "d"], ["b", "b ещё"]])
s = M.pushBubble(s, { id: "draft", text: "раз", kind: "draft" }, 2000)
s = M.pushBubble(s, { id: "draft", text: "раз два", kind: "draft", append: false }, 2100)
assert.strictEqual(s[s.length - 1].text, "раз два")
assert.deepStrictEqual(M.pushBubble(s, { id: "draft", drop: true }, 2200).map(b => b.id), ["d", "b"])
const kept = M.pushBubble(s, { id: "b", drop: false }, 2150)
assert.strictEqual(kept[kept.length - 1].text, "b ещё")
assert.strictEqual(M.pushBubble(kept, { id: "b", append: true }, 2150).pop().text, "b ещё")
s = M.pushBubble(s, { id: "draft", text: "раз два", kind: "draft", append: false }, 2100)
assert.strictEqual(M.expire(s, 2100 + 9999, false).length, 3)
// The draft is not in the stack any more: it holds nothing back
assert.strictEqual(M.expire(s, 2100 + 10000, false).length, 0)
assert.strictEqual(M.expire(s, 2100 + 999999, true).length, 3)
assert.deepStrictEqual(M.dismiss(s, "d").map(b => b.id), ["b", "draft"])
assert.deepStrictEqual(M.ago(1000, 1030), ["just now", 0])
assert.deepStrictEqual(M.ago(1000, 1000 + 180), ["%1 min", 3])
assert.deepStrictEqual(M.ago(1000, 1000 + 7300), ["%1 h", 2])
assert.deepStrictEqual(M.ago(1000, 1000 + 3 * 86400), ["%1 d", 3])
let msgs = M.chatApply([], { role: "me", text: "как дела" })
msgs = M.chatApply(msgs, { role: "oma", text: "Почта ждёт." })
msgs = M.chatApply(msgs, { role: "oma", text: "Остальные работают." })
assert.deepStrictEqual(msgs, [{ role: "me", text: "как дела" }, { role: "oma", text: "Почта ждёт. Остальные работают." }])
assert.strictEqual(M.corner(10, 10, 1000, 800), "top-left")
assert.strictEqual(M.corner(990, 10, 1000, 800), "top-right")
assert.strictEqual(M.corner(10, 790, 1000, 800), "bottom-left")
assert.strictEqual(M.corner(990, 790, 1000, 800), "bottom-right")
assert.strictEqual(M.omaCmd({ do: "talk" }), '{"cmd":"oma","action":{"do":"talk"}}\n')
// the window shows an agent's chat: its feed is in front of the user, Oma keeps its news quiet
assert.strictEqual(M.view(true, "agent:s1"), '{"cmd":"view","agent":"s1"}\n')
assert.strictEqual(M.view(true, "chat:x"), '{"cmd":"view","agent":""}\n')
assert.strictEqual(M.view(false, "agent:s1"), '{"cmd":"view","agent":""}\n')
assert.strictEqual(M.HOURS.length, 24)
assert.strictEqual(M.HOURS[0], "00:00")
assert.strictEqual(M.HOURS[23], "23:00")
// The oldest bubble's mask: smoothstep(maskAlpha(a)) is a again
const smooth = a => 3 * a * a - 2 * a * a * a
for (const a of [M.OLDEST_NEAR, M.OLDEST_FAR, 0, 0.5, 1]) assert.ok(Math.abs(smooth(M.maskAlpha(a)) - a) < 1e-9, "maskAlpha " + a)

// Orb position: corner + offsets, snapped to the corner near it
assert.strictEqual(M.place(0, 0, 1000, 800, 50, 64), "top-left,0,0")
assert.strictEqual(M.place(940, 740, 1000, 800, 50, 64), "bottom-right,0,0")
assert.strictEqual(M.place(30, 30, 1000, 800, 50, 64), "top-left,0,0")
assert.strictEqual(M.place(100, 0, 1000, 800, 50, 64), "top-left,100,0")
assert.strictEqual(M.place(400, 100, 1000, 800, 50, 64), "top-left,400,100")
assert.strictEqual(M.place(700, 600, 1000, 800, 50, 64), "bottom-right,250,150")
assert.deepStrictEqual(M.parsePos("top-right,12,34", 1000, 800, 50), { atTop: true, atLeft: false, dx: 12, dy: 34 })
assert.deepStrictEqual(M.parsePos("bottom-left,5000,5000", 1000, 800, 50), { atTop: false, atLeft: true, dx: 950, dy: 750 })
for (const bad of ["", "junk", "top-left,1", "middle,1,1"])
  assert.deepStrictEqual(M.parsePos(bad, 1000, 800, 50), { atTop: false, atLeft: false, dx: 0, dy: 0 })
const d = { id: "draft", kind: "draft", text: "" }
assert.deepStrictEqual(M.withDraft([{ id: "a" }, { id: "b" }, { id: "c" }], d).map(b => b.id), ["b", "c", "draft"])
assert.strictEqual(M.withDraft([{ id: "a" }], null).length, 1)
assert.deepStrictEqual(["me", "draft", "oma", "status", "final", "question"].map(M.tone), ["me", "me", "oma", "agent", "agent", "agent"])
assert.deepStrictEqual(M.TONE_ALPHA, { me: 0.22, oma: 0.07, agent: 0 })
for (const oma of ["listening", "loading"])
  assert.strictEqual(M.escWanted(d, Object.assign({}, st, { oma }), false), true)
for (const oma of ["off", "idle", "paused", "thinking", "speaking"])
  assert.strictEqual(M.escWanted(d, Object.assign({}, st, { oma }), false), false)
assert.strictEqual(M.escWanted(d, M.OFFLINE, false), false)
assert.strictEqual(M.escWanted(null, st, true), true)
assert.strictEqual(M.escWanted(null, st, false), false)
assert.strictEqual(M.OFFLINE.talking, false)
assert.ok(!("dialog" in M.OFFLINE) && !("orb_corner" in M.OFFLINE))
// OFFLINE is the status line with nothing in it: the same fields in the same order as struct Status in the daemon
const status = /struct Status<'a> \{([\s\S]*?)\n\}/.exec(fs.readFileSync(__dirname + "/../src/daemon.rs", "utf8"))[1]
assert.deepStrictEqual(Object.keys(M.OFFLINE), [...status.matchAll(/^\s*(\w+):/gm)].map(m => m[1]))
assert.strictEqual(M.OFFLINE.listen_until, null)
assert.strictEqual(M.OFFLINE.wake, "off")

// Every key from orbTip, ago and stateLabel has a Russian line
const keys = new Set()
for (const oma of ["off", "idle", "loading", "listening", "thinking", "speaking", "paused"])
  for (const waiting of [0, 2]) for (const quiet of [false, true])
    for (const l of M.orbTip(Object.assign({}, st, { oma, waiting, quiet }))) keys.add(l[0])
for (const l of M.orbTip(M.OFFLINE)) keys.add(l[0])
for (const n of [0, 100, 600, 7300, 3 * 86400]) keys.add(M.ago(0, n)[0])
for (const a of ["working", "waiting", "done", "idle", "?"]) keys.add(M.stateLabel(a))
for (const k of keys) assert.ok(Object.prototype.hasOwnProperty.call(I.TABLES.ru, k), "no ru for " + k)
assert.strictEqual(ru(...M.orbTip(Object.assign({}, st, { waiting: 2 }))[0]), "Ждут ответа: 2")
for (const l of M.orbTip(Object.assign({}, st, { trouble: 1 }))) keys.add(l[0])
assert.strictEqual(ru(...M.orbTip(Object.assign({}, st, { trouble: 2 }))[0]), "Нужна помощь: 2")
// No dead Russian lines: each is a tr() literal or a key above
// Command lines
assert.strictEqual(M.cmd("pause"), '{"cmd":"pause"}\n')
assert.strictEqual(M.cmd("subscribe"), '{"cmd":"subscribe"}\n')
assert.strictEqual(M.set("agents_voice", false), '{"cmd":"set","key":"agents_voice","value":false}\n')
assert.deepStrictEqual(JSON.parse(M.set("speaker", "baya")), { cmd: "set", key: "speaker", value: "baya" })
assert.strictEqual(M.say(M.SAMPLE), '{"cmd":"say","text":"Так звучит этот голос."}\n')

// Window sections: keys, values the daemon accepts, value types as in the state line
const secs = M.sections(ru)
const sample = { speaker: "xenia", rate: "medium" }
assert.deepStrictEqual(secs.map(s => s.key), ["speaker", "rate"])
assert.deepStrictEqual(secs.map(s => s.sample), [true, true])
assert.deepStrictEqual(secs.map(s => s.caption), ["ГОЛОС", "СКОРОСТЬ"])
assert.deepStrictEqual(secs[0].options.map(o => o.value), ["xenia", "baya", "kseniya", "aidar", "eugene"])
assert.deepStrictEqual(secs[0].options.map(o => o.label), ["Xenia", "Baya", "Kseniya", "Aidar", "Eugene"])
assert.deepStrictEqual(secs[1].options.map(o => o.value), ["x-slow", "slow", "medium", "fast", "x-fast"])
assert.deepStrictEqual(secs[1].options.map(o => o.label), ["Очень медленно", "Медленно", "Обычно", "Быстро", "Очень быстро"])
for (const s of secs) for (const o of s.options) assert.strictEqual(typeof o.value, typeof sample[s.key], s.key)

// The window's chat: a chat that got its id replaces the one shown, other chats are skipped
const m1 = [{ role: "me", text: "привет" }]
assert.deepStrictEqual(M.chatFor("new", { key: "chat:1", was: "new", messages: m1 }), { key: "chat:1", messages: m1 })
// the general chat: the window asks for "general", Oma answers with its real key
assert.deepStrictEqual(M.chatFor("general", { key: "chat:g", was: "general", messages: m1 }), { key: "chat:g", messages: m1 })
assert.strictEqual(M.chatFor("general", { key: "agent:a", messages: m1 }), null)
// a QML signal turns the messages array into an array-like sequence (Array.isArray is false): still the history
assert.deepStrictEqual(M.chatFor("new", { key: "new", messages: { 0: m1[0], length: 1 } }).messages, [m1[0]])
assert.strictEqual(M.plainCopy("Астра\u0301л и до\u0301ки"), "Астрал и доки")
assert.deepStrictEqual(M.chatFor("chat:1", { key: "chat:1", messages: m1 }), { key: "chat:1", messages: m1 })
assert.strictEqual(M.chatFor("agent:a", { key: "chat:1", was: "new", messages: m1 }), null)
assert.strictEqual(M.chatFor("chat:2", { key: "chat:1", messages: m1 }), null)
assert.deepStrictEqual(M.chatFor("new", { key: "new" }).messages, [])
assert.deepStrictEqual(M.chatFor("new", { key: "chat:x", was: "*", messages: m1 }), { key: "chat:x", messages: m1 })
assert.deepStrictEqual(M.chatFor("chat:2", { key: "agent:a", was: "*", messages: m1 }), { key: "agent:a", messages: m1 })

// The main page's key cursor holds its row by chat key: a reordered list keeps it on the same agent
const ags = [{ id: "a" }, { id: "b" }, { id: "c" }]
assert.deepStrictEqual([-1, 0, 1, 3, 4].map(i => M.rowKey(ags, i)), ["", "general", "agent:a", "agent:c", ""])
assert.strictEqual(M.rowIndex(ags, "general"), 0)
assert.strictEqual(M.rowIndex(ags, "agent:b"), 2)
assert.strictEqual(M.rowIndex([{ id: "b" }, { id: "c" }, { id: "a" }], "agent:b"), 1)
assert.strictEqual(M.rowIndex(ags, "agent:gone"), -1)
assert.strictEqual(M.rowIndex(ags, ""), -1)

// A typed question: sent once, only while Oma takes commands; the field keeps it until her chat echoes it back
assert.strictEqual(M.omaUp(st), true)
assert.strictEqual(M.omaUp(Object.assign({}, st, { oma: "loading" })), true)
assert.strictEqual(M.omaUp(Object.assign({}, st, { oma: "off" })), false)   // the daemon runs, Oma is down
assert.strictEqual(M.omaUp(M.OFFLINE), false)                                // no daemon
assert.strictEqual(M.askable("как дела", "", st), true)
assert.strictEqual(M.askable("", "", st), false)
assert.strictEqual(M.askable("как дела", "как дела", st), false)              // already sent, not echoed yet
assert.strictEqual(M.askable("как дела", "", Object.assign({}, st, { oma: "off" })), false)
assert.strictEqual(M.askable("как дела", "", M.OFFLINE), false)
// taken: by the id the window gave the question, not by the text — a voiced line or another typed one with the same words is not it
assert.strictEqual(M.taken("17", { key: "chat:1", role: "me", text: "как дела", id: "17" }), true)
assert.strictEqual(M.taken("17", { key: "chat:1", role: "me", text: "как дела" }), false)              // said by voice: no id
assert.strictEqual(M.taken("17", { key: "chat:1", role: "me", text: "как дела", id: "16" }), false)
assert.strictEqual(M.taken("17", { key: "chat:1", role: "oma", text: "как дела", id: "17" }), false)
assert.strictEqual(M.taken("", { key: "chat:1", role: "me", text: "" }), false)

// Settings rows: choices go round, switches flip, quiet now overrides the schedule
const cfg = { speaker: "xenia", rate: "x-fast", quiet: false, quiet_schedule: true }
assert.strictEqual(M.SETTINGS_ROWS.length, 9)
assert.deepStrictEqual(M.settingStep("speaker", cfg, -1), { key: "speaker", value: "eugene", sample: true })
assert.deepStrictEqual(M.settingStep("rate", cfg, 1), { key: "rate", value: "x-slow", sample: true })
assert.strictEqual(M.settingStep("rate", Object.assign({}, cfg, { rate: "" }), 1).value, "x-slow")
assert.deepStrictEqual(M.settingStep("quiet_now", cfg, -1), { key: "quiet_override", value: "on", sample: false })
assert.strictEqual(M.settingStep("quiet_now", Object.assign({}, cfg, { quiet: true }), 1).value, "off")
assert.deepStrictEqual(M.settingStep("quiet_schedule", cfg, 1), { key: "quiet_schedule", value: false, sample: false })
assert.strictEqual(M.settingStep("quiet_hours", cfg, 1), null)
assert.deepStrictEqual(M.settingStep("orb_over_fullscreen", Object.assign({}, cfg, { orb_over_fullscreen: true }), 1), { key: "orb_over_fullscreen", value: false, sample: false })
assert.deepStrictEqual(M.settingStep("orb_satellites", Object.assign({}, cfg, { orb_satellites: true }), 1), { key: "orb_satellites", value: false, sample: false })
assert.deepStrictEqual(M.settingStep("progress_digest", cfg, 1), { key: "progress_digest", value: true, sample: false })
assert.deepStrictEqual(M.settingStep("agents_voice", Object.assign({}, cfg, { agents_voice: true }), 1), { key: "agents_voice", value: false, sample: false })
// every row steps but the hours (two dropdowns, the window walks them)
for (const r of M.SETTINGS_ROWS) assert.strictEqual(M.settingStep(r, cfg, 1) === null, r === "quiet_hours", r)

// The settings page: one fixed order of sections (the Jarvis wave's design); SETTINGS_ROWS are their rows in it
assert.deepStrictEqual(M.SETTINGS_SECTIONS.map(s => s.title), ["", "Quiet mode", "Hearing", "Orb", "Agents"])
assert.deepStrictEqual(M.SETTINGS_ROWS, [].concat(...M.SETTINGS_SECTIONS.map(s => s.rows)))
// Window.qml draws them in that order: a header per section with rows (the first has none: its rows are captions),
// a SettingRow per key, and no row numbers by hand — a row is SETTINGS_ROWS.indexOf(key)
const winQml = fs.readFileSync(__dirname + "/Window.qml", "utf8")
const page = winQml.slice(winQml.indexOf("id: settingsPage"), winQml.indexOf("component ChatLine"))
const drawn = [...page.matchAll(/text: root\.tr\("([^"]+)"\)\.toUpperCase\(\)|\bkey: "(\w+)"/g)].map(m => m[1] ? "#" + m[1] : m[2])
assert.deepStrictEqual(drawn, [].concat(...M.SETTINGS_SECTIONS.filter(s => s.rows.length).map(s => (s.title ? ["#" + s.title] : []).concat(s.rows))))
assert.ok(!/\brow:\s*\d|cursor === \d/.test(page), "a settings row by its number")

// The orb's hover card: an agent per row, the most urgent first — waiting (the longest waiting on top), the watch's
// word, working, done, idle (in 3–5 the freshest on top); its second line: the question's first line, the word, the status
const ag = (id, state, since, more) => Object.assign({ id, state, since, title: "", project: "p-" + id, status: "s-" + id, health: "", ask: "" }, more)
const crowd = [ag("i", "idle", 10), ag("d1", "done", 20), ag("w1", "working", 30), ag("q2", "waiting", 50, { ask: "\nКакой вид: плитка или список?\n\nЖдёт ответа на AskUserQuestion" }),
  ag("h", "working", 5, { health: "завис" }), ag("w2", "working", 40, { title: "Astral.Edo" }), ag("q1", "waiting", 45)]
assert.deepStrictEqual(M.byUrgency(crowd).map(a => a.id), ["q1", "q2", "h", "w2", "w1", "d1", "i"])
assert.deepStrictEqual(crowd.map(M.dotKind), ["idle", "done", "working", "waiting", "health", "working", "waiting"])
assert.strictEqual(M.dotKind(ag("x", "waiting", 0, { health: "завис" })), "waiting")   // the question's red stays first
const peek = M.peekRows(crowd, 45 + 15 * 60)
assert.strictEqual(peek.more, 0)
assert.deepStrictEqual(peek.rows.map(r => [r.id, r.dot, r.name, r.text]), [
  ["q1", "waiting", "p-q1", "s-q1"], ["q2", "waiting", "p-q2", "Какой вид: плитка или список?"], ["h", "health", "p-h", "завис"],
  ["w2", "working", "Astral.Edo", "s-w2"], ["w1", "working", "p-w1", "s-w1"], ["d1", "done", "p-d1", "s-d1"], ["i", "idle", "p-i", "s-i"]])
assert.deepStrictEqual(peek.rows[0].ago, ["%1 min", 15])
assert.strictEqual(M.peekRows([ag("x", "working", 0, { status: "я".repeat(80) })], 0).rows[0].text.length, 60)
const many = Array.from({ length: 11 }, (_, i) => ag("a" + i, "working", i))
assert.deepStrictEqual([M.peekRows(many, 0).rows.length, M.peekRows(many, 0).more], [M.PEEK_MAX, 3])
assert.deepStrictEqual(M.peekRows([], 0), { rows: [], more: 0 })
// a QML signal's sequence (Array.isArray false) is still the list
assert.deepStrictEqual(M.byUrgency({ 0: crowd[0], length: 1 }).map(a => a.id), ["i"])

// Satellites: an agent keeps its place while it lives, a new one takes the first free place (the most urgent first),
// a gone one frees its own; over SAT_MAX — the most urgent SAT_MAX - 1 and the hollow «more» ("+") last
assert.strictEqual(M.SAT_MAX, 5)
let sl = M.satSlots([], [ag("a", "working", 1), ag("b", "waiting", 2)], 5)
assert.deepStrictEqual(sl, ["b", "a", "", "", ""])
sl = M.satSlots(sl, [ag("a", "working", 1), ag("c", "working", 3)], 5)
assert.deepStrictEqual(sl, ["c", "a", "", "", ""])
sl = M.satSlots(sl, [ag("a", "waiting", 1), ag("c", "working", 3), ag("d", "done", 4)], 5)
assert.deepStrictEqual(sl, ["c", "a", "d", "", ""], "a question does not move a dot")
const six = ["a", "c", "d", "e", "f", "g"].map((id, i) => ag(id, i === 5 ? "waiting" : "working", i))
sl = M.satSlots(sl, six, 5)
assert.deepStrictEqual(sl, ["g", "f", "d", "e", "+"], "«more»: the four most urgent (g waits, then the freshest), d holds its place")
assert.deepStrictEqual(M.satSlots(sl, six.slice(0, 3), 5), ["c", "a", "d", "", ""])
assert.deepStrictEqual(M.satSlots(null, [], 5), ["", "", "", "", ""])
assert.strictEqual(M.agentById(crowd, "h").health, "завис")
assert.strictEqual(M.agentById(crowd, ""), null)
// A satellite flashes when its own agent turns to waiting — not a new agent, not one that waited already
assert.strictEqual(M.asks("a:working", "a:waiting"), true)
assert.strictEqual(M.asks("a:health", "a:waiting"), true)
for (const [was, key] of [["", "a:waiting"], ["b:working", "a:waiting"], ["a:waiting", "a:waiting"], ["a:waiting", "a:working"], ["a:working", ""]])
  assert.strictEqual(M.asks(was, key), false, was + " -> " + key)

// Russian plural forms
assert.deepStrictEqual([1, 2, 5, 11, 12, 21, 22, 25, 111, 0].map(n => ru("%1 agents", n)),
  ["1 агент", "2 агента", "5 агентов", "11 агентов", "12 агентов", "21 агент", "22 агента", "25 агентов", "111 агентов", "0 агентов"])

// Every tr("…") in the QML files and Model.js has a Russian line, and no ru line is dead
const files = fs.readdirSync(__dirname).filter(f => f.endsWith(".qml"))
const used = new Set(keys)
for (const f of files.concat("Model.js"))
  for (const m of fs.readFileSync(__dirname + "/" + f, "utf8").matchAll(/\btr\("((?:[^"\\]|\\.)*)"/g)) {
    const k = JSON.parse(`"${m[1]}"`)
    assert.ok(Object.prototype.hasOwnProperty.call(I.TABLES.ru, k), f + ": no ru for " + k)
    used.add(k)
  }
for (const k of Object.keys(I.TABLES.ru)) assert.ok(used.has(k), "unused ru line: " + k)

// QML: no own property or id may reuse a built-in name (rules.md §9, test of predmaxim.vpn)
const builtins = ["state", "status", "data", "children", "visible", "enabled", "opacity", "parent", "left", "right", "top", "bottom", "x", "y", "width", "height", "states", "focus", "settings", "opened", "bar"]
for (const f of files) {
  const src = fs.readFileSync(__dirname + "/" + f, "utf8")
  for (const m of src.matchAll(/^\s*(?:readonly\s+|required\s+)?property\s+\S+\s+(\w+)/gm))
    assert.ok(!builtins.includes(m[1]), f + ": property '" + m[1] + "' shadows a built-in")
  for (const m of src.matchAll(/\bid:\s*(\w+)/g))
    assert.ok(!builtins.includes(m[1]), f + ": id '" + m[1] + "' shadows a built-in")
}

console.log("ok")
