"""Машина состояний разговора Омы: события слуха, агента и звука → действия.

Один режим. Клик включает слух; распознанные фразы копятся в черновик; после
паузы (SEND_PAUSE) черновик уходит агенту; после ответа слух снова ждёт фразу.
Esc (cancel) отменяет фразу, включая ещё распознаваемые куски; SILENCE_END секунд
тишины или end() заканчивают разговор. Речь нумеруется: speech_start(n) и
phrase(text, n) — по номерам отбрасываются куски, распознанные после отмены.
Тихий режим показывает ответ (show), но не озвучивает (say). Часы подставляются
снаружи (clock), дедлайны проверяет tick().
"""
import time

from .chunker import Chunker

SEND_PAUSE = 1.3   # с от прихода фразы до отправки (VAD уже ждал свою паузу)
SILENCE_END = 30   # с тишины до конца разговора


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

    @property
    def hearing(self):
        """Слух слышит речь, которая ещё не распознана."""
        return self.started > max(self.heard, self.cut)

    def _to(self, state):
        if state == self.state:
            return []
        self.state = state
        if self.active and state in ("listening", "paused"):
            self.quiet_at = self.clock() + SILENCE_END
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

    def _send(self, text):
        self.turn = True
        self.chunker.reset()
        return [("send", text)] + self._to("thinking")

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

    def end(self):
        """Конец разговора: оборвать ход, выбросить черновик, закрыть слух."""
        acts = self._stop_turn() + self._drop()
        self.quiet_at = None
        if self.active:
            self.active = False
            acts.append(("ear", False))
        return acts + self._to("idle")

    def cancel(self):
        """Esc: выбросить фразу; идущий ответ оборвать."""
        acts = self._stop_turn() + self._drop()
        return acts + (self._to(self._rest()) if self.active and self.state in ("thinking", "speaking") else [])

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
            return [("draft", self.draft, self.hearing)] if self.draft else [("drop_draft",)]
        self.draft = f"{self.draft} {t}".strip()
        self.send_at = self.clock() + SEND_PAUSE
        self.quiet_at = self.clock() + SILENCE_END
        return [("draft", self.draft, self.hearing)]

    def tick(self):
        now = self.clock()
        if self.state == "listening" and not self.hearing and self.draft and self.send_at is not None and now >= self.send_at:
            text = self.draft.strip()
            acts = ([("stop",)] if self.audio_busy else []) + self._drop()  # статус агента, что звучит, обрываем
            return acts + self._send(text)
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
        return acts + self._to(self._rest())

    def audio(self, busy):
        self.audio_busy = busy
        if not busy and self.state == "speaking":
            return self._after_sound()
        return []

    def mic_busy(self, on):
        self.mic = on
        if on:
            acts = self._to("paused") if self.state == "listening" else []
            return acts + self._drop()
        return self._to("listening") if self.state == "paused" else []

    def set_quiet(self, on):
        self.quiet = on
        if on and self.state == "speaking":
            return [("stop",)] + self._after_sound()
        return []
