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
    "Hearing": "Слух", "Pause music while Oma speaks and listens": "Пауза музыки, пока Ома говорит и слушает",
    "Answer to «Oma»": "Отзываться на «Ома»", "Hears its name": "Слышит имя",
    "Not hearing its name: on battery": "Не слышит имя: от батареи", "Not hearing its name: mic is off": "Не слышит имя: микрофон выключен",
    "Not hearing its name: mic is busy": "Не слышит имя: микрофон занят", "Not hearing its name: no model": "Не слышит имя: нет модели",
    "Not hearing its name: no built-in mic": "Не слышит имя: нет встроенного микрофона",
    "Orb": "Орб", "Over fullscreen windows": "Над полноэкранными окнами", "Agent satellites": "Спутники агентов", "Orb mood": "Настроение орба", "Mic is off": "Микрофон выключен", "%1 failed calls this turn": "ошибок за ход: %1",
    "Tempo: very slow": "Темп: очень медленно", "Tempo: slow": "Темп: медленно", "Tempo: normal": "Темп: обычно",
    "Tempo: fast": "Темп: быстро", "Tempo: very fast": "Темп: очень быстро", "Breathes at rest": "Дышит в покое", "%1 more": "ещё %1",
    "Agents": "Агенты", "General chat": "Общий чат", "Copy": "Копировать", "Copied": "Скопировано", "Retell progress": "Пересказывать промежуточное",
    "Warn about the limit": "Предупреждать о лимите",
    "Remember past days": "Помнить прошлые дни", "Memory": "Память", "I remember": "Помню", "Days": "Дни",
    "Forget everything": "Забыть всё", "Edit": "Изменить", "Delete": "Удалить", "Cancel": "Отмена", "Forget": "Забыть",
    "Forget everything? The day notes and «I remember» go for good.": "Забыть всё? Заметки дней и «Помню» удалятся насовсем.",
    "Delete the note of %1?": "Удалить заметку за %1?", "Delete this line from memory?": "Удалить эту строку из памяти?",
    "Changes apply from a new chat.": "Правки действуют с нового чата.",
    "Memory is off: nothing new is kept, what is here stays until «Forget everything».":
      "Память выключена: новое не собирается, прежнее хранится до «Забыть всё».",
    "Hidden while the screen is shared": "Скрыто: идёт показ экрана", "Nothing remembered yet": "Пока ничего не помню",
    "Watch agents": "Следить за агентами", "Oma is off": "Ома выключена", "Turn Oma on": "Включить Ому", "Turn Oma off": "Выключить Ому",
    "Turn sound off": "Выключить звук", "Turn sound on": "Включить звук", "Oma: sound off": "Ома: звук выключен",
    "Quiet: screen sharing": "Тихо: показ экрана", "Quiet: do not disturb": "Тихо: не беспокоить",
    "Quiet and brief while sharing the screen": "При показе экрана — тихо и коротко", "Summary after quiet": "Сводка после тишины",
    "Intonation by meaning": "Интонация по смыслу", "Sound cues": "Звуковые знаки",
    "Results aloud at most every": "Итоги вслух не чаще", "No limit": "Без ограничения"
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
