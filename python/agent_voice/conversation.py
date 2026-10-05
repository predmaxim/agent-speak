"""Машина состояний разговора: события слуха, агента и звука → действия."""
from .chunker import Chunker


class Conversation:
    def __init__(self):
        self.state = "listening"
        self.turn = False      # агент отвечает на нашу реплику
        self.audio_busy = False
        self.chunker = Chunker()

    def _to(self, state):
        if state == self.state:
            return []
        self.state = state
        return [("state", state)]

    def speech_start(self):
        acts = []
        if self.audio_busy or self.state == "speaking":
            acts.append(("stop",))
        if self.turn:
            acts.append(("interrupt",))
        if not acts:
            return []
        self.turn = False
        self.chunker.reset()
        return acts + self._to("listening")

    def phrase(self, text):
        t = text.strip()
        if len(t) < 2:
            return []
        self.turn = True
        self.chunker.reset()
        return [("send", t)] + self._to("thinking")

    def delta(self, text):
        if not self.turn:
            return []
        said = [("say", s) for s in self.chunker.feed(text)]
        return said + (self._to("speaking") if said else [])

    def done(self):
        if not self.turn:
            return []
        self.turn = False
        said = [("say", s) for s in self.chunker.flush()]
        if said:
            return said + self._to("speaking")
        return [] if self.audio_busy else self._to("listening")

    def audio(self, busy):
        self.audio_busy = busy
        if not busy and not self.turn:
            return self._to("listening")
        return []
