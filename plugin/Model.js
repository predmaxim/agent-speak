.pragma library

// agent-speakd's state line -> what the bar icon and the window show, and the
// command lines the plugin writes to the daemon's socket.

var SAMPLE = "Так звучит этот голос."

// Nerd Font glyphs: volume-high, pause, volume-low, volume-off.
var ICONS = {
  speaking: String.fromCodePoint(0xF057E),
  paused: String.fromCodePoint(0xF03E4),
  idle: String.fromCodePoint(0xF057F),
  off: String.fromCodePoint(0xF0581)
}

// No connection to the daemon.
var OFFLINE = { running: false, speaking: false, paused: false, mode: "", read_intermediate: false, speaker: "", rate: "", project: "" }

// A state line, or null for anything else (a broken line must not blank the icon).
function parse(line) {
  var s = null
  try { s = JSON.parse(line) } catch (e) { return null }
  return s && typeof s.running === "boolean" ? s : null
}

// Icon, whether it is lit in the indicator group, and the tooltip key for tr(tip, arg).
function view(st) {
  if (!st || !st.running) return { icon: ICONS.off, lit: false, tip: "Speech service is not running", arg: "" }
  if (st.paused) return { icon: ICONS.paused, lit: true, tip: "Paused — right click resumes", arg: "" }
  if (st.speaking) return st.project
    ? { icon: ICONS.speaking, lit: true, tip: "Reading: %1", arg: st.project }
    : { icon: ICONS.speaking, lit: true, tip: "Reading…", arg: "" }
  if (st.mode === "auto") return { icon: ICONS.idle, lit: true, tip: "Auto: the focused agent is read aloud", arg: "" }
  return { icon: ICONS.idle, lit: false, tip: "Manual: Super+Shift+Alt+S", arg: "" }
}

// Something to pause, resume or stop.
function busy(st) {
  return !!st && st.running && (st.speaking || st.paused)
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
