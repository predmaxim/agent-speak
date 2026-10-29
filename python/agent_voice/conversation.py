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
ECHO_WINDOW = 3     # с после её речи: хвост её голоса из динамиков ещё в микрофоне
ECHO_SHARE = 0.6    # доля слов фразы из её же речи, с которой фраза — эхо
ECHO_RUN = 3        # слов её речи по порядку в начале фразы, с которых это начало — эхо
# Что она делает, пока мозг занят инструментом или раздумьем: тишина хуже повтора; presence — мгновенный, без слова
FILLERS = {"mcp__oma__agents": "Смотрю агентов.", "mcp__oma__agent_feed": "Смотрю ленту агента.",
           "mcp__oma__agent_tail": "Читаю переписку агента.", "mcp__oma__selection": "Смотрю выделенное.",
           "mcp__oma__tell": "Передаю агенту.", "mcp__oma__projects": "Смотрю проекты.", "mcp__oma__launch": "Запускаю агента.",
           "Read": "Смотрю файлы.", "Glob": "Смотрю файлы.", "Grep": "Смотрю файлы.", "WebSearch": "Ищу в сети.",
           "thinking": "Секунду, подумаю.", "mcp__oma__presence": ""}
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
        self.audio_busy = False
        self.quiet = False
        self.mic = False       # микрофон занят другим (voxtype и т. п.)
        self.chunker = Chunker()
        self.started = self.heard = self.cut = 0  # номера речи: начата / пришла / отменена до
        self.echo_n = 0        # номер речи, начатой под её голос: эхо судится по началу фразы, а не по её распознаванию
        self.held = 0          # номер речи, начатой, пока она думала или говорила: обрывать ли — решает распознанный текст
        self.suspended = False # её речь на паузе по перебиву: перебил не ей («[молчу]», кашель) — продолжит, иначе выбросить
        self.head = ""         # начало хода до первого знака; ход с «[» — метка «[молчу]»/«[конец]», решится в done; None — решено
        self.send_at = None    # когда отправить черновик
        self.quiet_at = None   # когда закончить разговор от тишины
        self.window = None     # разговор открыт окном ответа: {kind, said, first, close}
        self.echo = False      # слух — через эхоподавление (Ома в динамиках): её голос просачивается в микрофон
        self.said = ""         # что она сказала в этом ответе — по нему узнаётся эхо
        self.spoke_at = None   # когда смолк её звук
        self.voiced = False    # последняя реплика сказана голосом, не набрана: опасное по ней — только с подтверждением

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
            self.quiet_at = self.clock() + self.silence
        elif state in ("thinking", "speaking"):
            self.quiet_at = None
        return [("state", state)]

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

    def _stop_turn(self):
        """Оборвать звук и ход агента (если они есть)."""
        acts = self._discard()
        if not acts and (self.audio_busy or self.state == "speaking"):
            acts.append(("stop",))
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

    def _window_send(self, text):
        """Реплика в разговоре из окна ответа: «ок»/«да» без отправки, пометка для мозга."""
        w = self.window
        if w["first"] and w["kind"] == "here" and not is_answer(text):
            return []  # речь в комнате не Оме — «Эй, ты тут?» ждёт ответа дальше
        first, w["first"] = w["first"], False
        acts, ctx = ([("presence", "here")] if first else []), ""
        if first and w["kind"] == "here":
            if is_yes(text):
                return acts + self.end()
            ctx = "Ома спросила: «Эй, ты тут? Есть новости». "
        elif first:
            if w["kind"] == "news" and is_ack(text):
                return acts + self.end()
            ctx = f"Ома только что сказала: «{w['said']}». "
        # окно открыла Ома сама: одна реплика и её ответ, дальше микрофон закрыт — разговор в комнате не уходит
        # в Claude; продолжить разговор — кликом
        w["close"] = True
        return acts + self._send(text, ctx)

    def _closing(self):
        return self.window and self.window["close"] and not self.turn

    def listen(self, secs, kind="news", said=""):
        """Окно ответа после новостей: слух на secs секунд; заговорил пользователь — обычный разговор."""
        if self.state != "idle" or self.quiet or self.mic:  # не мешаем ходу, звуку и чужому микрофону
            return []
        self.active = True
        self.window = {"kind": kind, "said": said, "first": True, "close": False}
        acts = [("ear", True)] + self._to("listening")
        self.quiet_at = self.clock() + secs  # после _to: он ставит SILENCE_END
        return acts

    def end(self):
        """Конец разговора: оборвать ход, выбросить черновик, закрыть слух."""
        acts = self._stop_turn() + self._drop()
        self.quiet_at = None
        self.window = None
        if self.active:
            self.active = False
            acts.append(("ear", False))
        return acts + self._to("idle")

    def cancel(self):
        """Esc: выбросить фразу; идущий ответ оборвать."""
        acts = self._stop_turn() + self._drop()
        return acts + self._to(self._rest())

    def ask(self, text):
        t = text.strip()
        if len(t) < 2:
            return []
        return self._stop_turn() + self._drop() + self._send(t, voiced=False)

    def _echoing(self):
        """Её голос сейчас (или только что) звучит из динамиков, и слух его слышит."""
        recent = self.spoke_at is not None and self.clock() - self.spoke_at < ECHO_WINDOW
        return self.echo and bool(self.said) and (self.audio_busy or self.state == "speaking" or recent)

    def speech_start(self, n):
        if not self.active or self.state in ("idle", "paused"):
            return []
        if self._echoing():
            self.echo_n = n
        if self._echoing() and (self.audio_busy or self.state == "speaking"):
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

    def phrase(self, text, n, done=None):
        self.heard = max(self.heard, n)
        if self.window and not self.window["first"]:
            return []  # на окно уже ответили: речь в комнате — не Оме
        pre = []
        if self._echoing() or n == self.echo_n:  # пользователь договаривал дольше ECHO_WINDOW — начало всё равно её
            if is_echo(text, self.said):
                return [("draft", self.draft, self.hearing)] if self.draft or self.hearing else []
            text = strip_echo(text, self.said, tail=n == self.echo_n)
            if self.state == "speaking" and is_barge(text):
                pre = self._barge()  # не её слова — перебивает пользователь
        elif n == self.held and self.state in ("thinking", "speaking"):
            if not is_barge(text):  # кашель, «угу» — она продолжает
                acts = self._resume() if self.suspended else []
                return acts + ([("draft", self.draft, self.hearing)] if self.draft or self.hearing else [("drop_draft",)])
            pre = self._barge() if self.state == "speaking" else self._stop_turn() + self._to("listening")
        if n <= self.cut or self.state != "listening":
            return pre
        t = text.strip()
        if len(t) < 2 or is_noise(t):
            return [("draft", self.draft, self.hearing)] if self.draft or self.hearing else [("drop_draft",)]
        self.draft = f"{self.draft} {t}".strip()
        wait = SEND_PAUSE if done is None else 0.0 if done >= DONE else SEND_PAUSE if done >= UNDONE else WAIT_MORE
        self.send_at = self.clock() + wait
        self.quiet_at = self.clock() + self.silence
        return pre + [("draft", self.draft, self.hearing)]

    def tick(self):
        now = self.clock()
        if self.active and not self.hearing and self.state in ("listening", "paused"):
            # дедлайны снимает speech_start; если фраза их не вернула (шум, отмена) — взводим здесь
            if self.quiet_at is None:
                self.quiet_at = now + self.silence
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
            return self._discard() + self._speak(self.chunker.feed(text))  # ответ по делу — прерванный не нужен
        return self._speak(self.chunker.feed(text))

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
            acts = self._discard() + self._speak(self.chunker.feed(self._unmark(head)) + self.chunker.flush())
        if self.state == "speaking":
            return acts  # дозвучит — audio(False) вернёт в покой
        if self._closing():
            return acts + self.end()
        return acts + self._to(self._rest())

    @staticmethod
    def _unmark(text):
        """Ход начался с «[», но это не метка: скобки не читаются."""
        return text.replace("[", "").replace("]", "")

    def tool(self, name):
        """Мозг взялся за инструмент (или задумался) — сказать, что она делает, если сейчас не звучит."""
        if not self.turn or self.audio_busy or self.state == "speaking" or (name == "thinking" and self.said):
            return []
        phrase = FILLERS.get(name, "Секунду.")
        return self._discard() + self._speak([phrase]) if phrase else []

    def audio(self, busy):
        self.audio_busy = busy
        if self.suspended:
            return []  # её пауза — не конец речи
        if not busy:
            self.spoke_at = self.clock()
        if not busy and self.state == "speaking":
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
