.pragma library
// Interface text in other languages, keyed by the English text itself, so a
// string missing from a table shows in English. tr("Reading: %1", x) fills in
// %1, %2… after the lookup. The same scheme as predmaxim.todo: Quickshell
// plugins get no .qm catalogues, so Qt's qsTr isn't used.
var TABLES = {
  ru: {
    "Oma is not running": "Ома не запущена", "Oma is loading": "Ома загружается", "Oma is listening": "Ома слушает",
    "Oma is thinking": "Ома думает", "Oma is speaking": "Ома говорит", "Paused": "Пауза", "Quiet mode": "Тихий режим",
    "Oma": "Ома", "%1 waiting": "Ждут ответа: %1", "Send": "Отправить",
    "just now": "только что", "%1 min": "%1 мин", "%1 h": "%1 ч", "%1 d": "%1 д",
    "Working": "Работает", "Waiting": "Ждёт", "Done": "Готово", "Idle": "Свободен",
    "INTERMEDIATE STATUSES": "ПРОМЕЖУТОЧНЫЕ СТАТУСЫ", "Read": "Читать", "Final answer only": "Только итог",
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
