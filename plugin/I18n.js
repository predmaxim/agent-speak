.pragma library
// Interface text in other languages, keyed by the English text itself, so a
// string missing from a table shows in English. tr("Reading: %1", x) fills in
// %1, %2… after the lookup; a Russian line given as [one, few, many] picks its
// form by the first argument (1 агент, 2 агента, 5 агентов). The same scheme
// as predmaxim.todo: Quickshell plugins get no .qm catalogues, so Qt's qsTr
// isn't used.
var TABLES = {
  ru: {
    "Oma is not running": "Ома не запущена", "Oma is loading": "Ома загружается", "Oma is listening": "Ома слушает",
    "Oma is thinking": "Ома думает", "Oma is speaking": "Ома говорит", "Paused": "Пауза", "Quiet mode": "Тихий режим",
    "Oma": "Ома", "%1 waiting": "Ждут ответа: %1", "%1 need help": "Нужна помощь: %1", "End conversation": "Закончить разговор",
    "just now": "только что", "%1 min": "%1 мин", "%1 h": "%1 ч", "%1 d": "%1 д",
    "Working": "Работает", "Waiting": "Ждёт", "Done": "Готово", "Idle": "Свободен",
    "VOICE": "ГОЛОС", "SPEED": "СКОРОСТЬ",
    "Very slow": "Очень медленно", "Slow": "Медленно", "Normal": "Обычно", "Fast": "Быстро", "Very fast": "Очень быстро",
    "%1 agents": ["%1 агент", "%1 агента", "%1 агентов"], "Talk": "Говорить", "New chat": "Новый чат",
    "Chat history": "История чатов", "Settings": "Настройки", "Back": "Назад",
    "No agents": "Агентов нет", "You": "Ты", "Ask Oma…": "Спросить Ому…", "Untitled chat": "Без названия",
    "No chats yet": "Чатов пока нет", "Quiet now": "Сейчас тихо", "On schedule": "По расписанию", "From / to": "С / до",
    "Orb": "Орб", "Over fullscreen windows": "Над полноэкранными окнами",
    "Agents": "Агенты", "Retell progress": "Пересказывать промежуточное",
    "Watch agents": "Следить за агентами", "Oma is off": "Ома выключена", "Turn Oma on": "Включить Ому", "Turn Oma off": "Выключить Ому",
    "Turn sound off": "Выключить звук", "Turn sound on": "Включить звук", "Oma: sound off": "Ома: звук выключен"
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
    if (Array.isArray(out)) {
      var n = Math.abs(Number(arguments[1])) % 100, d = n % 10
      out = out[d === 1 && n !== 11 ? 0 : d >= 2 && d <= 4 && (n < 12 || n > 14) ? 1 : 2]
    }
    for (var i = 1; i < arguments.length; i++) out = out.split("%" + i).join(String(arguments[i]))
    return out
  }
}
