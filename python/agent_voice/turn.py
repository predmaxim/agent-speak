"""Конец реплики по звуку: Smart Turn v3.2 (pipecat-ai/smart-turn, BSD-2), ONNX 8 МБ на CPU, русский среди 23 языков.
Адаптер: complete(pcm) — вероятность, что пользователь договорил; модели нет или сбой — None, Ома ждёт паузу, как раньше."""
import os

import numpy as np

MODEL = os.path.expanduser("~/.local/share/agent-speak/smart-turn-v3.2-cpu.onnx")
RATE, SECONDS = 16000, 8


def features(audio, extractor):
    """Как в inference.py Smart Turn: последние 8 с, нули — в начало, нормировка всего куска
    (WhisperFeatureExtractor(chunk_length=8), do_normalize), log-mel Whisper 80×800."""
    n = RATE * SECONDS
    a = audio[-n:] if len(audio) > n else np.pad(audio, (n - len(audio), 0))
    a = (a - a.mean()) / np.sqrt(a.var() + 1e-7)
    return extractor(a.astype(np.float32), padding=0)[None].astype(np.float32)


class SmartTurn:
    def __init__(self, path=MODEL):
        self.path, self._session, self._extractor = path, None, None

    def complete(self, pcm):
        """pcm — s16 16 кГц моно, вся реплика. Вероятность конца реплики 0..1 или None."""
        if self._session is None:
            if not os.path.exists(self.path):
                return None
            import onnxruntime as ort
            from faster_whisper.feature_extractor import FeatureExtractor
            so = ort.SessionOptions()
            so.inter_op_num_threads = 1
            self._session = ort.InferenceSession(self.path, sess_options=so, providers=["CPUExecutionProvider"])
            self._extractor = FeatureExtractor(feature_size=80, chunk_length=SECONDS)
        audio = np.frombuffer(pcm, np.int16).astype(np.float32) / 32768
        return float(self._session.run(None, {"input_features": features(audio, self._extractor)})[0].reshape(-1)[0])
