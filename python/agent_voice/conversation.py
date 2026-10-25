"""Машина состояний разговора Омы: события слуха, агента и звука → действия.

Один режим. Клик включает слух; распознанные фразы копятся в черновик; после
паузы (SEND_PAUSE) черновик уходит агенту; после ответа слух снова ждёт фразу.
Esc (cancel) отменяет фразу, включая ещё распознаваемые куски; SILENCE_END секунд
тишины или end() заканчивают разговор. Речь нумеруется: speech_start(n) и
phrase(text, n) — по номерам отбрасываются куски, распознанные после отмены.
Тихий режим показывает ответ (show), но не озвучивает (say). Часы подставляются
снаружи (clock), дедлайны проверяет tick().
"""
import re
import time

from .chunker import Chunker

SEND_PAUSE = 1.3   # с от прихода фразы до отправки (VAD уже ждал свою паузу)
SILENCE_END = 30   # с тишины до конца разговора
WINDOW_END = 5     # с тишины после ответа Омы в разговоре из окна ответа
ACKS = {"ок", "окей", "ok", "okay", "понял", "поняла", "ясно", "спасибо", "хорошо", "угу", "ага", "принято"}
YES = {"да", "тут", "здесь", "я", "ага", "угу", "слушаю"}
CANCEL = {"отбой", "отмена", "отмени", "стоп"}


def words(text):
    return re.findall(r"[a-zа-я]+", text.lower().replace("ё", "е"))


def is_ack(text):
    w = words(text)
    return 1 <= len(w) <= 3 and all(x in ACKS for x in w)


def is_yes(text):
    w = words(text)
    return 1 <= len(w) <= 3 and all(x in YES for x in w)


def is_cancel(text):
    w = words(text)
    return 1 <= len(w) <= 3 and all(x in CANCEL for x in w)


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
        self.send_at = None    # когда отправить черновик
        self.quiet_at = None   # когда закончить разговор от тишины
        self.window = None     # разговор открыт окном ответа: {kind, said, first, close}

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

    def _send(self, text, ctx=""):
        self.turn = True
        self.chunker.reset()
        return [("send", text, ctx)] + self._to("thinking")

    def _stop_turn(self):
        """Оборвать звук и ход агента (если они есть)."""
        acts = []
        if self.audio_busy or self.state == "speaking":
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
        first, w["first"] = w["first"], False
        acts, ctx = ([("presence", "here")] if first else []), ""
        if first and w["kind"] == "here":
            if is_yes(text):
                return acts + self.end()
            ctx = "Ома спросила: «Ты тут? Есть новости». "
        elif first:
            if w["kind"] == "news" and is_ack(text):
                return acts + self.end()
            ctx = f"Ома только что сказала: «{w['said']}». "
        w["close"] = is_ack(text)
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
        return self._stop_turn() + self._drop() + self._send(t)

    def speech_start(self, n):
        if not self.active or self.state in ("idle", "paused"):
            return []
        self.started = n
        self.send_at = self.quiet_at = None
        if self.window:
            self.window["close"] = False
        acts = self._stop_turn()
        if self.state != "listening":
            acts += self._to("listening")
            self.quiet_at = None
        return acts + [("draft", self.draft, True)]

    def phrase(self, text, n):
        self.heard = max(self.heard, n)
        if n <= self.cut or self.state != "listening":
            return []
        t = text.strip()
        if len(t) < 2:
            return [("draft", self.draft, self.hearing)] if self.draft or self.hearing else [("drop_draft",)]
        self.draft = f"{self.draft} {t}".strip()
        self.send_at = self.clock() + SEND_PAUSE
        self.quiet_at = self.clock() + self.silence
        return [("draft", self.draft, self.hearing)]

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
            if is_cancel(text):  # «отбой» — не реплика: оборвать всё, что звучит (и чтение), и слушать дальше
                return [("halt",)] + self.cancel()
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
        if acts and not self.quiet:
            acts += self._to("speaking")
        return acts

    def delta(self, text):
        if not self.turn:
            return []
        return self._speak(self.chunker.feed(text))

    def done(self):
        if not self.turn:
            return []
        self.turn = False
        acts = self._speak(self.chunker.flush())
        if self.state == "speaking":
            return acts  # дозвучит — audio(False) вернёт в покой
        if self._closing():
            return acts + self.end()
        return acts + self._to(self._rest())

    def audio(self, busy):
        self.audio_busy = busy
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
            return [("stop",)] + self._after_sound()
        return []
