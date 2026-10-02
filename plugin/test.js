// node test.js
const fs = require("fs")
const assert = require("assert")
const load = (file, names) =>
  new Function(fs.readFileSync(__dirname + "/" + file, "utf8").replace(".pragma library", "") + "; return { " + names + " }")()
const M = load("Model.js", "OFFLINE, ICONS, SAMPLE, parse, view, busy, cmd, set, say, sections")
const I = load("I18n.js", "TABLES, translator")
const ru = I.translator("ru")

// The daemon's state line
const st = M.parse('{"running":true,"speaking":true,"paused":false,"mode":"auto","read_intermediate":true,"speaker":"xenia","rate":"medium","project":"agent-speak"}')
assert.strictEqual(st.speaker, "xenia")
// Anything else from the socket never replaces the state
assert.strictEqual(M.parse("{broken"), null)
assert.strictEqual(M.parse('{"speaking":true}'), null)
assert.strictEqual(M.parse(""), null)
assert.strictEqual(M.parse("null"), null)

// Icon, lit, tooltip per state (spec table)
const at = patch => Object.assign({}, st, patch)
const v = s => { const x = M.view(s); return [x.icon, x.lit, ru(x.tip, x.arg)] }
assert.deepStrictEqual(v(st), [M.ICONS.speaking, true, "Читаю: agent-speak"])
assert.deepStrictEqual(v(at({ project: "" })), [M.ICONS.speaking, true, "Читаю…"])
assert.deepStrictEqual(v(at({ speaking: false, paused: true })), [M.ICONS.paused, true, "Пауза — правый клик продолжит"])
assert.deepStrictEqual(v(at({ speaking: false })), [M.ICONS.idle, true, "Авто: агент в фокусе читается сам"])
assert.deepStrictEqual(v(at({ speaking: false, mode: "manual" })), [M.ICONS.idle, false, "Вручную: Super+Alt+R"])
assert.deepStrictEqual(v(M.OFFLINE), [M.ICONS.off, false, "Сервис озвучки не запущен"])
assert.deepStrictEqual(v(null), [M.ICONS.off, false, "Сервис озвучки не запущен"])
assert.strictEqual(new Set(Object.values(M.ICONS)).size, 4)
assert.strictEqual(M.view(st).tip, "Reading: %1") // English without a table
assert.strictEqual(I.translator("en")(M.view(st).tip, "x"), "Reading: x")

// Right click and Pause/Stop: only while speaking or paused
assert.strictEqual(M.busy(st), true)
assert.strictEqual(M.busy(at({ speaking: false, paused: true })), true)
assert.strictEqual(M.busy(at({ speaking: false })), false)
assert.strictEqual(M.busy(at({ running: false })), false)
assert.strictEqual(M.busy(M.OFFLINE), false)
assert.strictEqual(M.busy(null), false)

// Command lines
assert.strictEqual(M.cmd("pause"), '{"cmd":"pause"}\n')
assert.strictEqual(M.cmd("subscribe"), '{"cmd":"subscribe"}\n')
assert.strictEqual(M.set("read_intermediate", false), '{"cmd":"set","key":"read_intermediate","value":false}\n')
assert.deepStrictEqual(JSON.parse(M.set("speaker", "baya")), { cmd: "set", key: "speaker", value: "baya" })
assert.strictEqual(M.say(M.SAMPLE), '{"cmd":"say","text":"Так звучит этот голос."}\n')

// Window sections: keys, values the daemon accepts, value types as in the state line
const secs = M.sections(ru)
assert.deepStrictEqual(secs.map(s => s.key), ["mode", "read_intermediate", "speaker", "rate"])
assert.deepStrictEqual(secs.map(s => s.sample), [false, false, true, true])
assert.deepStrictEqual(secs.map(s => s.caption), ["РЕЖИМ", "ПРОМЕЖУТОЧНЫЕ СТАТУСЫ", "ГОЛОС", "СКОРОСТЬ"])
assert.deepStrictEqual(secs[0].options, [{ value: "auto", label: "Авто" }, { value: "manual", label: "Вручную" }])
assert.deepStrictEqual(secs[1].options, [{ value: true, label: "Читать" }, { value: false, label: "Только итог" }])
assert.deepStrictEqual(secs[2].options.map(o => o.value), ["xenia", "baya", "kseniya", "aidar", "eugene"])
assert.deepStrictEqual(secs[2].options.map(o => o.label), ["Xenia", "Baya", "Kseniya", "Aidar", "Eugene"])
assert.deepStrictEqual(secs[3].options.map(o => o.value), ["x-slow", "slow", "medium", "fast", "x-fast"])
assert.deepStrictEqual(secs[3].options.map(o => o.label), ["Очень медленно", "Медленно", "Обычно", "Быстро", "Очень быстро"])
for (const s of secs) for (const o of s.options) assert.strictEqual(typeof o.value, typeof st[s.key], s.key)

// Every tr("…") has a Russian line
for (const f of ["Panel.qml", "Indicator.qml", "Link.qml", "Model.js"]) {
  if (!fs.existsSync(__dirname + "/" + f)) continue
  for (const m of fs.readFileSync(__dirname + "/" + f, "utf8").matchAll(/\btr\("((?:[^"\\]|\\.)*)"/g))
    assert.ok(Object.prototype.hasOwnProperty.call(I.TABLES.ru, JSON.parse(`"${m[1]}"`)), f + ": no ru for " + m[1])
}

// QML: no own property or id may reuse a built-in name (rules.md §9, test of predmaxim.vpn)
const builtins = ["state", "status", "data", "children", "visible", "enabled", "opacity", "parent", "left", "right", "top", "bottom", "x", "y", "width", "height", "states", "focus", "settings", "opened", "bar"]
for (const f of ["Panel.qml", "Indicator.qml", "Link.qml"]) {
  if (!fs.existsSync(__dirname + "/" + f)) continue
  const src = fs.readFileSync(__dirname + "/" + f, "utf8")
  for (const m of src.matchAll(/^\s*(?:readonly\s+|required\s+)?property\s+\S+\s+(\w+)/gm))
    assert.ok(!builtins.includes(m[1]), f + ": property '" + m[1] + "' shadows a built-in")
  for (const m of src.matchAll(/\bid:\s*(\w+)/g))
    assert.ok(!builtins.includes(m[1]), f + ": id '" + m[1] + "' shadows a built-in")
}

console.log("ok")
