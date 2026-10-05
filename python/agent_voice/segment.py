"""Фразы из потока вероятностей Silero VAD: старт после start_frames речи подряд,
конец после end_frames тишины. Кадр — 32 мс."""
from collections import deque


class Segmenter:
    def __init__(self, start_frames=8, end_frames=22, preroll=8, threshold=0.5):
        self.start_frames, self.end_frames, self.threshold = start_frames, end_frames, threshold
        self.pre = deque(maxlen=preroll)   # тишина до речи: начало слова не обрезается
        self.run = []                      # кадры кандидата в речь
        self.buf = None                    # идущая фраза
        self.silence = 0

    def feed(self, prob, frame):
        speech = prob >= self.threshold
        if self.buf is None:
            if not speech:
                self.pre.extend(self.run + [frame]) if self.run else self.pre.append(frame)
                self.run = []
                return []
            self.run.append(frame)
            if len(self.run) < self.start_frames:
                return []
            self.buf = list(self.pre) + self.run
            self.pre.clear()
            self.run, self.silence = [], 0
            return [("start",)]
        self.buf.append(frame)
        self.silence = 0 if speech else self.silence + 1
        if self.silence < self.end_frames:
            return []
        audio, self.buf = b"".join(self.buf), None
        return [("phrase", audio)]
