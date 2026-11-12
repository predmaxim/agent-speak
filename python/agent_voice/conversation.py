"""Машина состояний разговора Омы: события слуха, агента и звука → действия.

Один режим. Клик включает слух; распознанные фразы копятся в черновик; после
паузы (SEND_PAUSE) черновик уходит агенту; после ответа слух снова ждёт фразу.
Esc (cancel) отменяет фразу, включая ещё распознаваемые куски; SILENCE_END секунд
тишины или end() заканчивают разговор. Речь нумеруется: speech_start(n) и
phrase(text, n) — по номерам отбрасываются куски, распознанные после отмены.
Тихий режим показывает ответ (show), но не озвучивает (say). Часы подставляются
снаружи (clock), дедлайны проверяет tick().
"""
import difflib
import re
import time

from .chunker import Chunker
from .wake import strip_wake

SEND_PAUSE = 1.3   # с от прихода фразы до отправки (VAD уже ждал свою паузу)
# Smart Turn (turn.py) — договорил ли: уверен — сразу; оборвал на полуслове — ждать продолжения дольше; не знает — как было
DONE, UNDONE, WAIT_MORE = 0.9, 0.5, 2.5
SILENCE_END = 30   # с тишины до конца разговора
WINDOW_END = 5     # с тишины после ответа Омы в разговоре из окна ответа
ACKS = {"ок", "окей", "ok", "okay", "понял", "поняла", "ясно", "спасибо", "хорошо", "угу", "ага", "принято"}
YES = {"да", "тут", "здесь", "я", "ага", "угу", "слушаю"}
CANCEL = ("отбо", "отмен")  # конец разговора; основы: whisper пишет любую форму — «отмену», «отменить», «отбою»
HOLD = ("стоп", "хватит", "подожд")  # замолчи и слушай дальше
# Похоже на ответ на «Эй, ты тут?»: остальная речь в комнате (ребёнок, гости) — не пользователь Оме
ANSWER_WORDS = {"да", "ага", "угу", "ок", "окей", "тут", "я", "что", "как", "нет", "не", "ну", "щас", "через"}
ANSWER_MAX = 8  # слов: ответ Оме короткий; длинное — разговор в комнате, в нём «да» и «нет» есть всегда
ANSWER_STEMS = ("здес", "слуша", "говор", "рассказ", "давай", "нов", "потом", "позж", "минут", "секунд", "сейчас",
                "подожд", "занят", "отбо", "отмен", "стоп", "ома", "хорош", "ладн")
ECHO_SHARE = 0.6    # доля слов фразы из её же речи, с которой фраза — эхо
ECHO_RUN = 3        # слов её речи по порядку в начале фразы, с которых это начало — эхо
# Что она делает, пока мозг занят инструментом или раздумьем: тишина хуже повтора; presence, show и later — мгновенные, без слова.
# Варианты звучат по кругу, по порядку (дизайн 2026-11-12, «result-first»); после запуска — первый.
FILES = ("Смотрю файлы.", "Гляну в файлы.", "Ищу в файлах.")
FILLERS = {"mcp__oma__agents": ("Смотрю агентов.", "Гляну, что у агентов.", "Сейчас посмотрю агентов."),
           "mcp__oma__agent_feed": ("Смотрю ленту агента.", "Гляну его ленту.", "Открываю ленту агента."),
           "mcp__oma__agent_tail": ("Читаю переписку агента.", "Гляну переписку.", "Смотрю, что он писал."),
           "mcp__oma__selection": ("Смотрю выделенное.", "Гляну выделенное.", "Читаю выделенное."),
           "mcp__oma__tell": ("Готовлю поручение.", "Формулирую поручение.", "Собираю поручение."),
           "mcp__oma__projects": ("Смотрю проекты.", "Гляну список проектов.", "Ищу проект."),
           "mcp__oma__launch": ("Готовлю запуск.", "Собираю задание.", "Готовлю агента."),
           "Read": FILES, "Glob": FILES, "Grep": FILES,
           "WebSearch": ("Ищу в сети.", "Гляну в интернете.", "Поищу в сети."),
           "thinking": ("Секунду, подумаю.", "Дай подумать.", "Сейчас подумаю."), "mcp__oma__presence": (), "mcp__oma__show": (),
           "mcp__oma__later": ()}
OTHER = ("Секунду.", "Минутку.", "Сейчас.")
# Род Омы — по голосу (gender строки состояния службы): шаблон «{м|ж}» — «Понял{|а}», «пош{ёл|ла}»; тот же синтаксис —
# phrases::gendered в службе. Подстановки без «|» («{path}») не трогаются: шаблон разрешают до подстановки чужого текста.
GENDERED = re.compile(r"\{([^{}|]*)\|([^{}|]*)\}")


def gendered(text, g):
    """Шаблон рода → форма её рода: g "m" — мужская, иначе женская."""
    return GENDERED.sub(lambda m: m[1] if g == "m" else m[2], text)


# Голосом — коротко, текстом — сколько нужно: после этой метки ответ мозга только в окне (формат задаёт мозг, как [молчу])
MORE = "‹подробно›"
# Междометие, кашель, вздох — как их пишет whisper («Кхм.», «Эм...», «Э-э-э», «Кхе-кхе»): не реплика
NOISE = re.compile(r"(?:э+м*|м+|х+м+|к+х+[еам]*|а+|о+х*|ой|ай|эх|ух|уф|ах|фу|ну+)$")


def words(text):
    return re.findall(r"[a-zа-я]+", text.lower().replace("ё", "е"))


def is_ack(text):
    w = words(text)
    return 1 <= len(w) <= 3 and all(x in ACKS for x in w)


def is_yes(text):
    w = words(text)
    return 1 <= len(w) <= 3 and all(x in YES for x in w)


def is_answer(text):
    w = words(text)
    return 0 < len(w) <= ANSWER_MAX and (w[0] in ANSWER_WORDS or w[0].startswith(ANSWER_STEMS))


def is_echo(text, said):
    """Фраза — её же голос из динамиков: почти все слова (по основам) — из того, что она сейчас сказала.
    whisper слышит эхо неточно («Не совсем понимаю» вместо «Не совсем поняла»), поэтому по основам, а не дословно."""
    w, s = words(text), {x[:4] for x in words(said)}
    return bool(w) and bool(s) and sum(x[:4] in s for x in w) / len(w) >= ECHO_SHARE


def strip_echo(text, said, tail=False):
    """Начало фразы — её последние слова из динамиков: пользователь заговорил, пока она договаривала, и её хвост
    с его словами — один кусок. Срезается начало, совпавшее с её речью по порядку (по основам, от ECHO_RUN слов;
    whisper искажает отдельные слова — пропуск до двух); дальше — слова пользователя. tail — фраза начата под её
    голос: тогда хватает и одного слова, если это конец её речи («Простаивает. Ок.»)."""
    low = text.lower().replace("ё", "е")  # та же длина: позиции слов годятся и для text
    toks = list(re.finditer(r"[a-zа-я]+", low))
    w, s = [m.group()[:4] for m in toks], [x[:4] for x in words(said)]
    end = got = last = 0
    for i, j, n in difflib.SequenceMatcher(None, w, s, autojunk=False).get_matching_blocks():
        if not n or i - end > 2:
            break
        end, got, last = i + n, got + n, j + n
    echo = got >= ECHO_RUN or (tail and got and last == len(s))
    return text[toks[end - 1].end():].lstrip(" ,.;:!?…-—") if echo else text


def is_noise(text):
    """Не слова: междометия и пометки whisper в скобках («[кашель]», «(вздыхает)», «*кашляет*»)."""
    return all(NOISE.match(x) for x in words(re.sub(r"\[[^]]*\]|\([^)]*\)|\*[^*]*\*", " ", text)))


def is_barge(text):
    """Перебив — слова; поддакивание («угу», «ок») и кашель её не обрывают."""
    return len(text.strip()) >= 2 and not is_noise(text) and not is_ack(text)


def is_hold(text):
    w = words(text)
    return 1 <= len(w) <= 3 and all(x.startswith(HOLD) for x in w)


def is_cancel(text):
    w = words(text)
    return 1 <= len(w) <= 3 and all(x.startswith(CANCEL) for x in w)


class Conversation:
    def __init__(self, clock=time.monotonic):
        self.clock = clock
        self.state = "idle"    # idle | listening | thinking | speaking | paused
        self.active = False    # разговор идёт: слух включён
        self.draft = ""
        self.turn = False      # агент отвечает на нашу реплику
        self.turn_no = 0       # номер хода: итоги очереди к мозгу (доставка, вопрос подтверждения, сбой) — только своему ходу
        self.audio_busy = False
        self.quiet = False
        self.mic = False       # микрофон занят другим (voxtype и т. п.)
        self.chunker = Chunker()
        self.started = self.heard = self.cut = 0  # номера речи: начата / пришла / отменена до
        self.echo_n = 0        # номер речи, начатой, пока звучал её голос: эхо судится по началу фразы, а не по распознаванию
        self.held = 0          # номер речи, начатой, пока она думала или говорила: обрывать ли — решает распознанный текст
        self.suspended = False # её речь на паузе по перебиву: перебил не ей («[молчу]», кашель) — продолжит, иначе выбросить
        self.head = ""         # начало хода до первого знака; ход с «[» — метка «[молчу]»/«[конец]», решится в done; None — решено
        self.send_at = None    # когда отправить черновик
        self.heard_at = 0.0    # когда в черновик вошла последняя фраза
        self.quiet_at = None   # когда закончить разговор от тишины
        self.window = None     # разговор открыт окном ответа: {kind, said, first, close}
        self.echo = False      # слух — через эхоподавление (Ома в динамиках): её голос просачивается в микрофон
        self.said = ""         # что она сказала в этом ответе — по нему узнаётся эхо
        self.played_at = 0.0   # когда кончается (кончился) звук службы — её played_at, часы time.monotonic()
        self.voiced = False    # последняя реплика сказана голосом, не набрана: опасное по ней — только с подтверждением
        self.hold = ""         # конец текста, похожий на начало метки MORE: ждёт следующей дельты
        self.more = None       # подробности после метки MORE (None — метки ещё не было)
        self.rotor = {}        # сколько раз звучал круг вариантов FILLERS: следующий — по порядку

    @property
    def silence(self):
        return WINDOW_END if self.window else SILENCE_END

    @property
    def hearing(self):
        """Слух слышит речь, которая ещё не распознана."""
        return self.started > max(self.heard, self.cut)

    def _to(self, state):
        if state == self.state:
            return []
        self.state = state
        if self.active and state in ("listening", "paused"):
            self.quiet_at = self._silence_until()
        elif state in ("thinking", "speaking"):
            self.quiet_at = None
        return [("state", state)]

    def _silence_until(self):
        """Когда закончить разговор от тишины; окно ответа, на которое ещё не ответили, — не раньше своего конца (secs
        службы от его начала): эхо её вопроса и речь в комнате его не сокращают."""
        t = self.clock() + self.silence
        return max(t, self.window["until"]) if self.window and self.window["first"] else t

    def _rest(self):
        """Состояние покоя: когда ничего не говорим и не ждём ответ."""
        if not self.active:
            return "idle"
        return "paused" if self.mic else "listening"

    def _after_sound(self):
        return self._to("thinking" if self.turn else self._rest())

    def _drop(self):
        """Выбросить черновик и недораспознанную речь."""
        acts = [("drop_draft",)] if self.draft or self.hearing else []
        self.draft = ""
        self.cut = self.started
        self.send_at = None
        return acts

    def _send(self, text, ctx="", voiced=True):
        self.turn, self.voiced, self.said = True, voiced, ""
        self.turn_no += 1
        self.hold, self.more = "", None
        self.head = ""
        self.chunker.reset()
        return [("send", text, ctx)] + self._to("thinking")

    def _pause_her(self):
        acts = [] if self.suspended else [("pause_voice", True)]
        self.suspended = True
        return acts

    def _resume(self):
        """Перебив оказался не ей: она продолжает прерванный ответ (звучавшая фраза — с начала)."""
        self.suspended = False
        return [("pause_voice", False)] + self._to("speaking")

    def _discard(self):
        """Прерванный ответ больше не нужен: выбросить и снять паузу — новости и чтение идут дальше."""
        if not self.suspended:
            return []
        self.suspended = False
        return [("stop",), ("pause_voice", False)]

    def _barge(self):
        """Перебили словами: её речь — на паузу (мозг ответит «[молчу]» — продолжит), ход мозга — оборвать."""
        acts = self._pause_her() + ([("interrupt",)] if self.turn else [])
        self.turn = False
        self.chunker.reset()
        return acts + self._to("listening")

    def _stop_turn(self, sound=True):
        """Оборвать звук и ход агента (если они есть); sound=False — звук уже остановила служба: только ход."""
        acts = self._discard()
        if not acts and (self.audio_busy or self.state == "speaking"):
            acts.append(("stop",))
        if not sound:
            acts = []
        if self.turn:
            acts.append(("interrupt",))
        self.turn = False
        self.chunker.reset()
        return acts

    def talk(self):
        if self.state in ("listening", "paused"):
            return self.end()  # клик во время слушания закрывает разговор
        acts = self._stop_turn()
        if not self.active:
            self.active = True
            acts.append(("ear", True))
        return acts + self._to(self._rest())

    def converse(self):
        """Удержание спутника, пока она слушает: разговор не кончается, а окно ответа (новость, «Ты тут?», вопрос агента)
        становится обычным разговором — реплика без контекста той новости, после ответа слух открыт, конец — по тишине."""
        self.window = None
        if self.active and self.state in ("listening", "paused"):
            self.quiet_at = self._silence_until()
        return []

    def _window_send(self, text):
        """Реплика в разговоре из окна ответа: «ок»/«да» без отправки, пометка для мозга."""
        w = self.window
        if w.get("asked"):  # ответ на вопрос подтверждения: его сверит код поручения, после — окно закроется, как было
            w.update(asked=False, close=True)
            return self._send(text)
        if w["first"] and w["kind"] == "here" and not is_answer(text):
            return []  # речь в комнате не Оме — «Эй, ты тут?» ждёт ответа дальше
        first, w["first"] = w["first"], False
        acts, ctx = ([("presence", "here")] if first else []), ""
        if first and w["kind"] == "here":
            if is_yes(text):
                return acts + self.end()
            ctx = f"Твой вопрос вслух: «{w['said']}» "  # пометки мозгу о ней — без рода: голос меняет её род
        elif first:
            if w["kind"] == "news" and is_ack(text):
                return acts + self.end()
            ctx = f"Твои слова вслух: «{w['said']}». "
        # окно открыла Ома сама: одна реплика и её ответ, дальше микрофон закрыт — разговор в комнате не уходит
        # в Claude; продолжить разговор — кликом
        w["close"] = True
        return acts + self._send(text, ctx)

    def _closing(self):
        return self.window and self.window["close"] and not self.turn

    def listen(self, secs, kind="news", said=""):
        """Окно ответа после новостей или вопроса «Ты тут?» (служба шлёт его, когда пошёл звук вопроса: он ещё звучит):
        слух на secs секунд; said — что она сказала, по нему узнаётся эхо; заговорил пользователь — обычный разговор."""
        if self.state != "idle" or self.quiet or self.mic:  # не мешаем ходу, звуку и чужому микрофону
            return []
        self.active = True
        self.window = {"kind": kind, "said": said, "first": True, "close": False, "until": self.clock() + secs}
        self.said = said  # эхо — по ней, а не по её прошлому ответу
        acts = [("ear", True)] + self._to("listening")
        self.quiet_at = self.window["until"]  # после _to: окно — ровно secs, а не WINDOW_END
        return acts

    def wake(self, rest):
        """Слово-побудка в покое; rest — что после «Ома» во второй ступени. «Тут», «да» — «я на месте» без разговора;
        иначе — разговор, как по клику, но слух уже открыт (его вход не меняется). Только имя — щелчок «слушаю»;
        с продолжением — без щелчка: фразу целиком распознает слух, и она уйдёт, как после клика."""
        if self.active or self.quiet or self.mic:
            return []
        if is_yes(rest):
            return [("presence", "here"), ("chime",)]
        acts = self._stop_turn()  # «Ома, стоп» под новости в наушниках — как клик: звук обрывается
        self.active = True
        return acts + ([] if rest.strip() else [("chime",)]) + self._to("listening")

    def end(self, stop=True):
        """Конец разговора: оборвать ход (stop), выбросить черновик, закрыть слух."""
        acts = (self._stop_turn() if stop else []) + self._drop()
        self.quiet_at = None
        self.window = None
        self.mic = False  # занятость микрофона — для разговора: следующий узнает её заново (Oma.mic_now)
        if self.active:
            self.active = False
            acts.append(("ear", False))
        return acts + self._to("idle")

    def answered(self):
        """«Ты тут?» решён без слов (кнопка наушников, служба уже знает «я тут»): закрыть окно вопроса, пока в нём не
        ответили словами. Звук не трогать — его решает служба (сводка по кнопке не обрывается); мозгу — ни слова."""
        w = self.window
        return self.end(stop=False) if w and w["kind"] == "here" and w["first"] else []

    def cancel(self, sound=True):
        """Esc: выбросить фразу; идущий ответ оборвать. sound=False — кнопка наушников: звук остановила служба и, может,
        поставила сводку следом — hush срезал бы и её."""
        acts = self._stop_turn(sound) + self._drop()
        return acts + self._to(self._rest())

    def ask(self, text):
        """Набранное в окне. Порог в два знака — только у слуха (шум whisper), набранный знак — намеренный: окно ждёт
        его строку в чате по id, без неё вопрос остался бы в поле бледным навсегда."""
        t = text.strip()
        if not t:
            return []
        return self._stop_turn() + self._drop() + self._send(t, voiced=False)

    def _echoing(self, at):
        """В момент at её голос звучал из динамиков, и слух его слышал: она говорит или канал pw-cat после «смолкла»
        ещё не доигран (played_at службы). Речь, начатая после конца её звука, — не эхо, даже если это её же слова
        («Скажи: да, передай» → «Да, передай»)."""
        playing = self.audio_busy or self.state == "speaking" or at < self.played_at
        return self.echo and bool(self.said) and playing

    def speech_start(self, n, at=None):
        """Начало речи n; at — когда она началась (слух меряет по звуку), без него — сейчас."""
        if not self.active or self.state in ("idle", "paused"):
            return []
        at = self.clock() if at is None else at
        if self._echoing(at):
            self.echo_n = n
        if self._echoing(at) and (self.audio_busy or self.state == "speaking"):
            self.started = n  # её голос или перебив — решит распознанный текст: обрывать её по эху нельзя
            return []
        if self.window and not self.window["first"]:
            return []  # на окно уже ответили: дальше — не Оме, её ответ не обрывать
        if self.turn and self.state == "thinking" and not self.audio_busy:
            self.started = self.held = n  # кашель, пока она думает, не обрывает ответ: перебив решит текст (phrase)
            return [("draft", self.draft, True)]
        if self.state == "speaking" and not self.echo:  # наушники: её — на паузу, обрывать ли — решит текст (phrase)
            self.started = self.held = n
            return self._pause_her() + [("draft", self.draft, True)]
        self.started = n
        self.send_at = self.quiet_at = None
        acts = self._stop_turn()
        if self.state != "listening":
            acts += self._to("listening")
            self.quiet_at = None
        return acts + [("draft", self.draft, True)]

    def phrase(self, text, n):
        self.heard = max(self.heard, n)
        if self.window and not self.window["first"] and not self.window.get("asked"):
            return []  # на окно уже ответили: речь в комнате — не Оме; ответ на её вопрос подтверждения — Оме
        pre = []
        if n == self.echo_n:  # началась под её голос — начало может быть её, когда бы фразу ни распознали
            if is_echo(text, self.said):
                return [("draft", self.draft, self.hearing)] if self.draft or self.hearing else []
            text = strip_echo(text, self.said, tail=True)
            if self.state == "speaking" and is_barge(text):
                pre = self._barge()  # не её слова — перебивает пользователь
        if n == self.held and self.state in ("thinking", "speaking"):  # и под эхом: пока она думает, слова её обрывают
            if not is_barge(text):  # кашель, «угу» — она продолжает
                acts = self._resume() if self.suspended else []
                return acts + ([("draft", self.draft, self.hearing)] if self.draft or self.hearing else [("drop_draft",)])
            pre = self._barge() if self.state == "speaking" else self._stop_turn() + self._to("listening")
        if n <= self.cut or self.state != "listening":
            return pre
        t = strip_wake(text)  # «Ома, …» — обращение, не реплика: в окне «Ты тут?» «Ома, тут» — это «тут»
        if len(t) < 2 or is_noise(t):
            return [("draft", self.draft, self.hearing)] if self.draft or self.hearing else [("drop_draft",)]
        self.draft = f"{self.draft} {t}".strip()
        self.heard_at = self.clock()
        self.send_at = self.heard_at + SEND_PAUSE
        self.quiet_at = self._silence_until()
        return pre + [("draft", self.draft, self.hearing)]

    def turn_end(self, done):
        """Smart Turn по фразе, вошедшей в черновик (done — вероятность, что договорил; None — не знает): уверен —
        отправка сразу, оборвал на полуслове — ждать продолжения дольше. Эхо и непринятое он не судит: зря CPU."""
        if done is not None and self.send_at is not None:
            self.send_at = self.heard_at + (0.0 if done >= DONE else SEND_PAUSE if done >= UNDONE else WAIT_MORE)

    def tick(self):
        now = self.clock()
        if self.active and not self.hearing and self.state in ("listening", "paused"):
            # дедлайны снимает speech_start; если фраза их не вернула (шум, отмена) — взводим здесь
            if self.quiet_at is None:
                self.quiet_at = self._silence_until()
            if self.draft and self.send_at is None:
                self.send_at = now + SEND_PAUSE
        if self.state == "listening" and not self.hearing and self.draft and self.send_at is not None and now >= self.send_at:
            text = self.draft.strip()
            if is_cancel(text):  # «отбой» — не реплика: оборвать всё, что звучит (и чтение), и закончить разговор
                return [("halt",)] + self.end()
            if is_hold(text):  # «стоп» — оборвать всё, что звучит, и слушать дальше
                return [("halt",)] + self._stop_turn() + self._drop() + self._to(self._rest())
            if is_ack(text) and not self.window and not self.said.rstrip().endswith("?"):
                return self.end()  # «ок» после её ответа — разговор окончен; на её вопрос «хорошо» — ответ, уходит мозгу
            acts = ([("stop",)] if self.audio_busy else []) + self._drop()  # статус агента, что звучит, обрываем
            return acts + (self._window_send(text) if self.window else self._send(text))
        if self.quiet_at is not None and now >= self.quiet_at and not self.hearing and self.state in ("listening", "paused"):
            return self.end()
        return []

    def _speak(self, sentences):
        acts = []
        for s in sentences:
            acts.append(("show", s))
            if not self.quiet:
                acts.append(("say", s))
                self.said = f"{self.said} {s}"
        if acts and not self.quiet:
            acts += self._to("speaking")
        return acts

    def delta(self, text):
        if not self.turn:
            return []
        if self.head is not None:  # метка молчания — первым знаком хода: до решения ничего не звучит
            self.head += text
            if not self.head.strip() or self.head.lstrip().startswith("["):
                return []
            text, self.head = self.head, None
            return self._discard() + self._voice(text)  # ответ по делу — прерванный не нужен
        return self._voice(text)

    def _voice(self, text):
        """Текст хода: до метки MORE — голосом и в окне, после — только в окне (одним куском в конце хода)."""
        if self.more is not None:
            self.more += text
            return []
        buf = self.hold + text
        i = buf.find(MORE)
        if i >= 0:
            self.hold, self.more = "", buf[i + len(MORE):]
            return self._speak(self.chunker.feed(buf[:i]) + self.chunker.flush())
        keep = next((k for k in range(min(len(MORE) - 1, len(buf)), 0, -1) if MORE.startswith(buf[-k:])), 0)
        self.hold = buf[len(buf) - keep:]
        return self._speak(self.chunker.feed(buf[:len(buf) - keep]))

    def _finish(self):
        """Конец хода: недосказанное — голосом, подробности после метки — в окно."""
        rest, more, self.hold, self.more = self.hold, (self.more or "").strip(), "", None
        return self._speak(self.chunker.feed(rest) + self.chunker.flush()) + ([("show", more)] if more else [])

    def done(self):
        if not self.turn:
            return []
        self.turn = False
        head, self.head = (self.head or "").strip(), None
        if head.lower().startswith("[конец"):  # «ок, спасибо» после её ответа — разговор окончен
            return self.end()
        if head.lower().startswith("[молч"):
            if self.suspended:
                return self._resume()  # перебили не её — продолжает прерванный ответ
            acts = []
        else:
            acts = self._discard() + self._voice(self._unmark(head)) + self._finish()
        if self.state == "speaking":
            return acts  # дозвучит — audio(False) вернёт в покой
        if self._closing():
            return acts + self.end()
        return acts + self._to(self._rest())

    @staticmethod
    def _unmark(text):
        """Ход начался с «[», но это не метка: скобки не читаются."""
        return text.replace("[", "").replace("]", "")

    def question(self, text):
        """Вопрос подтверждения поручения говорит код, сразу при вызове инструмента; через неё, а не мимо: её слова
        знает защита от эха. Ход мозга на нём кончается (хук PostToolUse в brain.py) — вопрос звучит последним."""
        if not self.turn:
            return []
        if self.window:  # окно, открытое ею, закрылось бы после этого ответа — а вопрос ждёт ответа: ещё одна реплика
            self.window.update(close=False, asked=True)
        return self._discard() + self._speak(self.chunker.flush() + [text])

    def tool(self, name):
        """Мозг взялся за инструмент (или задумался) — сказать, что она делает, если сейчас не звучит."""
        if not self.turn or self.audio_busy or self.state == "speaking" or (name == "thinking" and self.said):
            return []
        variants = FILLERS.get(name, OTHER)
        if not variants:
            return []
        n = self.rotor.get(variants, 0)
        self.rotor[variants] = n + 1
        return self._discard() + self._speak([variants[n % len(variants)]])

    def audio(self, busy, until=None, done=True):
        """Звук службы: busy — звучит; until — played_at, когда он кончается (без него — сейчас); done — все её реплики
        служба отработала (voiced: записаны, оборваны или звука не будет): «не звучит», пока её реплика ещё в очереди
        службы, речь не кончает, а реплика без звука — кончает."""
        self.audio_busy = busy
        if self.suspended:
            return []  # её пауза — не конец речи
        if not busy:
            self.played_at = self.clock() if until is None else until
        if not busy and done and self.state == "speaking":
            return self.end() if self._closing() else self._after_sound()
        return []

    def mic_busy(self, on):
        self.mic = on
        if on:
            acts = self._to("paused") if self.state == "listening" else []
            had = self.draft or self.hearing
            self.cut = self.started
            if not had:
                return acts
            # черновик остаётся, гасится только «…»
            return acts + ([("draft", self.draft, False)] if self.draft else [("drop_draft",)])
        return self._to("listening") if self.state == "paused" else []

    def set_quiet(self, on):
        self.quiet = on
        if on and self.state == "speaking":
            return (self._discard() or [("stop",)]) + self._after_sound()
        return []
