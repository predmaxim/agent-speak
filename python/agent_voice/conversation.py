"""Машина состояний разговора Омы: события слуха, агента и звука → действия.

Режимы: «по клику» (клик включает слух, фразы копятся в черновик, второй клик
или send отправляет) и диалог (слух всегда включён, каждая фраза уходит сразу,
перебить можно голосом). Тихий режим показывает ответ (show), но не озвучивает (say).
"""
from .chunker import Chunker


class Conversation:
    def __init__(self):
        self.state = "idle"    # idle | listening | thinking | speaking | paused
        self.dialog = False
        self.draft = ""
        self.turn = False      # агент отвечает на нашу реплику
        self.audio_busy = False
        self.quiet = False
        self.mic = False       # микрофон занят другим (voxtype и т. п.)
        self.chunker = Chunker()

    def _to(self, state):
        if state == self.state:
            return []
        self.state = state
        return [("state", state)]

    def _rest(self):
        """Состояние покоя: когда ничего не говорим и не ждём ответ."""
        if self.dialog or (self.state in ("listening", "paused") and not self.turn):
            return "paused" if self.mic else "listening"
        return "idle"

    def _after_sound(self):
        return self._to("thinking" if self.turn else self._rest())

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
        if self.dialog:
            return []
        if self.state in ("listening", "paused"):
            return self.send()
        acts = self._stop_turn() + [("ear", True)]
        return acts + self._to("paused" if self.mic else "listening")

    def send(self):
        """Отправить черновик (или отменить слух, если он пуст)."""
        if self.dialog or self.state not in ("listening", "paused"):
            return []
        text, self.draft = self.draft.strip(), ""
        # статусы агентов, что звучат сейчас, обрываем
        acts = ([("stop",)] if self.audio_busy else []) + [("drop_draft",), ("ear", False)]
        return acts + (self._send(text) if text else self._to("idle"))

    def cancel(self):
        acts = self._stop_turn()
        if self.draft or self.state in ("listening", "paused"):
            acts.append(("drop_draft",))
        self.draft = ""
        if not self.dialog and self.state in ("listening", "paused"):
            acts.append(("ear", False))
        return acts + self._to(self._rest() if self.dialog else "idle")

    def set_dialog(self, on):
        if on == self.dialog:
            return []
        self.dialog = on
        if on:
            acts = [("drop_draft",)] if self.draft else []
            self.draft = ""
            acts.append(("ear", True))
            if self.state in ("idle", "listening", "paused"):
                acts += self._to("paused" if self.mic else "listening")
            return acts
        acts = [("ear", False)]
        if self.state in ("listening", "paused"):
            acts += self._to("idle")
        return acts

    def ask(self, text):
        t = text.strip()
        if len(t) < 2:
            return []
        acts = self._stop_turn()
        if not self.dialog and self.state in ("listening", "paused"):
            acts.append(("ear", False))  # слух по клику выключаем, черновик остаётся
        return acts + self._send(t)

    def speech_start(self):
        if not self.dialog or self.state in ("idle", "paused"):
            return []
        acts = self._stop_turn()
        return acts + self._to("listening") if acts else []

    def phrase(self, text):
        t = text.strip()
        if len(t) < 2 or self.state != "listening":
            return []
        if self.dialog:
            return self._send(t)
        self.draft = f"{self.draft} {t}".strip()
        return [("draft", self.draft)]

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
        if on and self.state == "listening":
            return self._to("paused")
        if not on and self.state == "paused":
            return self._to("listening")
        return []

    def set_quiet(self, on):
        self.quiet = on
        if on and self.state == "speaking":
            return [("stop",)] + self._after_sound()
        return []
