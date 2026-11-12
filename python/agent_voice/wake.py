"""Слово-побудка «Ома» (дизайн 2026-11-12, «wake-word», и итоги пробы волны 0). Две ступени в том же слухе (Ear):
1) Vosk small-ru 0.22 (Apache-2.0) с грамматикой ["ома", "[unk]"] — потоком, по кадру 32 мс, только пока VAD слышит речь;
2) whisper с hotwords="Ома" по звуку от начала «ома» − 0,5 с до конца речи (не больше 3 с): первое слово — «Ома».
Промах первой или второй ступени — звук забыт: в журнал и на диск не попадает."""
import json
import os
import re
from collections import deque

from .segment import Segmenter

RATE, FRAME = 16000, 512  # как у слуха: кадр 32 мс
MODEL = os.path.expanduser("~/.local/share/agent-speak/vosk-model-small-ru-0.22")
GRAMMAR = '["ома", "[unk]"]'
HOTWORDS = "Ома"
PRE = 16    # кадров до начала «ома» во второй ступени: 0,5 с — слово целиком, даже если Vosk поставил начало позже
CLIP = 94   # кадров звука второй ступени, не больше 3 с: под телевизором речь не кончается никогда
RING = 313  # кадров (10 с) последнего звука: разговор после побудки начинается с её клипа, его кадры ещё здесь
# Обращение в начале реплики: «Ома», «Ома,», «Oma!» — whisper пишет имя и латиницей; «Омар», «Омский» — не оно
NAME = re.compile(r"^[\W_]*(?:ома|oma)(?![a-zа-яё])[\W_]*", re.I)


def named(text):
    """Вторая ступень: whisper услышал имя первым словом."""
    return NAME.match(text.strip()) is not None


def strip_wake(text):
    """Реплика без обращения «Ома» в начале: «Ома, что у Jira?» → «что у Jira?»."""
    return NAME.sub("", text.strip(), count=1)


def available(path=MODEL):
    """Есть модель и колесо vosk (install.sh): иначе побудки нет — «нет модели»."""
    import importlib.util
    return os.path.isdir(path) and importlib.util.find_spec("vosk") is not None


class VoskWake:
    """Первая ступень: потоковый KaldiRecognizer на одну речь. feed/end — время начала «ома» от start() в секундах
    или None. Порога по conf нет: в грамматике Vosk даёт 1.0 и «дома», «кома» — отсекает вторая ступень."""

    def __init__(self, path=MODEL):
        self.path, self._model, self._rec = path, None, None

    def load(self):
        """Модель — один раз (0,4 с CPU): в потоке, не в цикле событий."""
        if self._model is None:
            import vosk
            vosk.SetLogLevel(-1)
            self._model = vosk.Model(self.path)

    def start(self):
        # новый распознаватель на каждую речь (0,1 мс): время слов Vosk считает от создания, Reset его не обнуляет
        import vosk
        self._rec = vosk.KaldiRecognizer(self._model, RATE, GRAMMAR)
        self._rec.SetWords(True)

    def feed(self, frame):
        # Result() — на каждой границе фразы Vosk: без него CPU растёт с 3,7 % до 23 % ядра (проба волны 0)
        return self._hit(self._rec.Result()) if self._rec.AcceptWaveform(frame) else None

    def end(self):
        rec, self._rec = self._rec, None
        return self._hit(rec.FinalResult()) if rec else None

    @staticmethod
    def _hit(result):
        for w in json.loads(result).get("result", []):
            if w.get("word") == "ома":
                return float(w["start"])
        return None


class Waker:
    """Первая ступень в слухе: кадры с вероятностью VAD → речь (Segmenter) → детектор. frame() отдаёт клипы для второй
    ступени: [(кадр начала, звук)]. Пока вторая ступень думает (busy), новых клипов нет."""

    def __init__(self, det):
        self.det = det
        self.seg = Segmenter(keep=RING)  # речь под телевизором не кончается: её буфер — не больше кольца
        self.ring = deque(maxlen=RING)   # (кадр, вероятность VAD, звук) — для клипа и для разговора после побудки
        self.k0 = 0          # номер кадра, с которого идёт речь в детектор
        self.clip = None     # начало клипа, который ещё дописывается
        self.busy = False

    def reset(self):
        """Кадры не слушаются (mute, чужой захват, её голос в динамиках): речь и клип — заново."""
        if self.ring or self.seg.buf is not None:
            self.seg, self.clip = Segmenter(keep=RING), None
            self.ring.clear()

    def frame(self, k, prob, frame):
        self.ring.append((k, prob, frame))
        evs = self.seg.feed(prob, frame)
        if evs and evs[0][0] == "start":
            self.k0 = k - len(self.seg.buf) + 1  # предзахват и начало речи — кадры подряд до этого
            self.det.start()
            hits = [self.det.feed(f) for f in self.seg.buf]
        elif evs:
            hits = [self.det.end()]  # речь кончилась
        else:
            hits = [self.det.feed(frame)] if self.seg.buf is not None else []
        for s in hits:
            if s is not None and self.clip is None and not self.busy:
                self.clip = max(self.k0 + int(s * RATE / FRAME) - PRE, self.ring[0][0])
        if self.clip is None or (k + 1 - self.clip < CLIP and self.seg.buf is not None):
            return []
        at, self.clip, self.busy = self.clip, None, True
        return [(at, b"".join(f for n, _, f in self.ring if at <= n < at + CLIP))]

    def since(self, at):
        """Кадры с at: разговор после побудки слышит фразу с именем целиком, а не с середины."""
        return [x for x in self.ring if x[0] >= at]
