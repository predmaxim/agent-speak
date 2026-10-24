// node test.js
const fs = require("fs")
const assert = require("assert")
const load = (file, names) =>
  new Function(fs.readFileSync(__dirname + "/" + file, "utf8").replace(".pragma library", "") + "; return { " + names + " }")()
const M = load("Model.js", "OFFLINE, SAMPLE, parseLine, orbMode, orbTip, pushBubble, expire, dismiss, ago, stateLabel, chatApply, corner, omaCmd, HOURS, cmd, set, say, sections, OLDEST_NEAR, OLDEST_FAR, maskAlpha, chatFor, SETTINGS_ROWS, settingStep, parsePos, place, withDraft, tone, TONE_ALPHA, escWanted")
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
assert.strictEqual(M.parseLine("{broken"), null)
assert.strictEqual(M.parseLine("null"), null)
assert.strictEqual(M.orbMode(M.OFFLINE), "off")
assert.strictEqual(M.orbMode(null), "off")
assert.strictEqual(M.orbMode(st), "idle")
assert.strictEqual(M.orbMode(Object.assign({}, st, { waiting: 2, quiet: true })), "waiting")
assert.strictEqual(M.orbMode(Object.assign({}, st, { quiet: true })), "quiet")
assert.strictEqual(M.orbMode(Object.assign({}, st, { oma: "listening", waiting: 1 })), "listening")
// news and manual reading are the service's speech, Oma herself is idle: still "speaking", not the waiting red
assert.strictEqual(M.orbMode(Object.assign({}, st, { speaking: true, waiting: 1 })), "speaking")
// Oma is down: the orb says so, but a waiting agent still turns it yellow
assert.strictEqual(M.orbMode(Object.assign({}, st, { oma: "off" })), "off")
assert.strictEqual(M.orbMode(Object.assign({}, st, { oma: "off", quiet: true })), "off")
assert.strictEqual(M.orbMode(Object.assign({}, st, { oma: "off", waiting: 1 })), "waiting")
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

// Every key from orbTip, ago and stateLabel has a Russian line
const keys = new Set()
for (const oma of ["off", "idle", "loading", "listening", "thinking", "speaking", "paused"])
  for (const waiting of [0, 2]) for (const quiet of [false, true])
    keys.add(M.orbTip(Object.assign({}, st, { oma, waiting, quiet }))[0])
keys.add(M.orbTip(M.OFFLINE)[0])
for (const n of [0, 100, 600, 7300, 3 * 86400]) keys.add(M.ago(0, n)[0])
for (const a of ["working", "waiting", "done", "idle", "?"]) keys.add(M.stateLabel(a))
for (const k of keys) assert.ok(Object.prototype.hasOwnProperty.call(I.TABLES.ru, k), "no ru for " + k)
assert.strictEqual(ru(...M.orbTip(Object.assign({}, st, { waiting: 2 }))), "Ждут ответа: 2")
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
assert.deepStrictEqual(M.chatFor("chat:1", { key: "chat:1", messages: m1 }), { key: "chat:1", messages: m1 })
assert.strictEqual(M.chatFor("agent:a", { key: "chat:1", was: "new", messages: m1 }), null)
assert.strictEqual(M.chatFor("chat:2", { key: "chat:1", messages: m1 }), null)
assert.deepStrictEqual(M.chatFor("new", { key: "new" }).messages, [])
assert.deepStrictEqual(M.chatFor("new", { key: "chat:x", was: "*", messages: m1 }), { key: "chat:x", messages: m1 })
assert.deepStrictEqual(M.chatFor("chat:2", { key: "agent:a", was: "*", messages: m1 }), { key: "agent:a", messages: m1 })

// Settings rows: choices go round, switches flip, quiet now overrides the schedule
const cfg = { speaker: "xenia", rate: "x-fast", quiet: false, quiet_schedule: true }
assert.strictEqual(M.SETTINGS_ROWS.length, 7)
assert.deepStrictEqual(M.settingStep("speaker", cfg, -1), { key: "speaker", value: "eugene", sample: true })
assert.deepStrictEqual(M.settingStep("rate", cfg, 1), { key: "rate", value: "x-slow", sample: true })
assert.strictEqual(M.settingStep("rate", Object.assign({}, cfg, { rate: "" }), 1).value, "x-slow")
assert.deepStrictEqual(M.settingStep("quiet_now", cfg, -1), { key: "quiet_override", value: "on", sample: false })
assert.strictEqual(M.settingStep("quiet_now", Object.assign({}, cfg, { quiet: true }), 1).value, "off")
assert.deepStrictEqual(M.settingStep("quiet_schedule", cfg, 1), { key: "quiet_schedule", value: false, sample: false })
assert.strictEqual(M.settingStep("quiet_hours", cfg, 1), null)
assert.deepStrictEqual(M.settingStep("orb_over_fullscreen", Object.assign({}, cfg, { orb_over_fullscreen: true }), 1), { key: "orb_over_fullscreen", value: false, sample: false })
assert.deepStrictEqual(M.settingStep("progress_digest", cfg, 1), { key: "progress_digest", value: true, sample: false })

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
