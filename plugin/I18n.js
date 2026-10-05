.pragma library
// Interface text in other languages, keyed by the English text itself, so a
// string missing from a table shows in English. tr("Reading: %1", x) fills in
// %1, %2… after the lookup. The same scheme as predmaxim.todo: Quickshell
// plugins get no .qm catalogues, so Qt's qsTr isn't used.
var TABLES = {
  ru: {
    "Speech": "Озвучка",
    "Reading: %1": "Читаю: %1", "Reading…": "Читаю…",
    "Paused — right click resumes": "Пауза — правый клик продолжит",
    "Auto: the focused agent is read aloud": "Авто: агент в фокусе читается сам",
    "Manual: Super+Shift+Alt+S": "Вручную: Super+Shift+Alt+S",
    "Speech service is not running": "Сервис озвучки не запущен",
    "Pause": "Пауза", "Resume": "Продолжить", "Stop": "Стоп",
    "MODE": "РЕЖИМ", "Auto": "Авто", "Manual": "Вручную",
    "INTERMEDIATE STATUSES": "ПРОМЕЖУТОЧНЫЕ СТАТУСЫ", "Read": "Читать", "Final answer only": "Только итог",
    "VOICE CHAT": "РАЗГОВОР", "Finish talking to %1": "Закончить разговор с %1",
    "Talking to %1: starting": "Разговор с %1: запуск", "Talking to %1: listening": "Разговор с %1: слушаю",
    "Talking to %1: thinking": "Разговор с %1: думает", "Talking to %1: speaking": "Разговор с %1: говорит",
    "VOICE": "ГОЛОС", "SPEED": "СКОРОСТЬ",
    "Very slow": "Очень медленно", "Slow": "Медленно", "Normal": "Обычно", "Fast": "Быстро", "Very fast": "Очень быстро"
  }
}

// Text follows LC_MESSAGES, overridden by LC_ALL and defaulting to LANG, as
// the system splits it. env is name -> value (Quickshell.env in QML).
function textLanguage(env) {
  var names = ["LC_ALL", "LC_MESSAGES", "LANG"]
  for (var i = 0; i < names.length; i++) {
    var v = String(env(names[i]) || "").split(".")[0].split("@")[0]
    if (v && v !== "C" && v !== "POSIX") {
      var l = v.slice(0, 2).toLowerCase()
      return TABLES[l] ? l : "en"
    }
  }
  return "en"
}

function translator(lang) {
  var table = TABLES[lang] || {}
  return function(text) {
    var out = Object.prototype.hasOwnProperty.call(table, text) ? table[text] : text
    for (var i = 1; i < arguments.length; i++) out = out.split("%" + i).join(String(arguments[i]))
    return out
  }
}
