"""Слух: pw-record → Silero VAD (кадры 32 мс) → Segmenter → faster-whisper.
В очередь out: ("start", n) — начало речи, ("phrase", текст, n) — распознанная фраза;
n — Ear.seq, номер речи (растёт всегда, при перезапуске слуха не сбрасывается)."""
import asyncio
import contextlib
import re
import time

import numpy as np

from .context import OWN_NODE
from .segment import Segmenter

RATE, FRAME = 16000, 512  # 512 отсчётов = 32 мс
# Вход по умолчанию: эхоподавление живёт в цепочке микрофона (omarchy-mic-denoise).
REC_CMD = ["pw-record", "-P", "{ node.name = " + OWN_NODE + " }", "--rate", str(RATE), "--channels", "1", "--format", "s16", "-"]


def load_whisper():
    from faster_whisper import WhisperModel
    try:
        return WhisperModel("large-v3-turbo", device="cuda", compute_type="float16")
    except Exception as e:
        print(f"agent_voice: whisper на GPU не загрузился ({e}), CPU small", flush=True)
        return WhisperModel("small", device="cpu", compute_type="int8")


# Титры YouTube из обучающих данных: whisper уверенно «слышит» их в тишине и шуме, порог no_speech их не ловит.
HALLUCINATION = re.compile(r"субтитр|dimatorzok|синецкая|продолжение следует|спасибо за просмотр|подпис\w* на( наш)? канал", re.I)


def transcribe(model, pcm):
    audio = np.frombuffer(pcm, np.int16).astype(np.float32) / 32768
    segs, _ = model.transcribe(audio, language="ru", beam_size=1)
    text = " ".join(s.text.strip() for s in segs).strip()
    return "" if HALLUCINATION.search(text) else text  # вся фраза — шум, «Спасибо.» рядом с титрами тоже


class Whisper:
    """Модель whisper: грузится один раз и лениво; на батарее выгружается после простоя."""

    def __init__(self, load=load_whisper, clock=time.monotonic, idle=300):
        self._load, self._clock, self._idle = load, clock, idle
        self._model, self._task, self._used = None, None, 0.0

    @property
    def ready(self):
        return self._model is not None

    @property
    def loading(self):
        return self._task is not None and not self._task.done()

    async def get(self):
        self._used = self._clock()
        if self._model is None:
            if self._task is None:  # параллельные вызовы ждут одну и ту же загрузку
                self._task = asyncio.ensure_future(asyncio.to_thread(self._load))
            task = self._task
            try:
                self._model = await asyncio.shield(task)
            finally:
                if task.done() and self._task is task:
                    self._task = None
        self._used = self._clock()
        return self._model

    def maybe_unload(self, on_ac):
        if on_ac or self._model is None or self._clock() - self._used <= self._idle:
            return
        self._model = None
        try:
            import torch
            if torch.cuda.is_available():
                torch.cuda.empty_cache()
        except ImportError:
            pass


def load_vad():
    from silero_vad import load_silero_vad  # импорт torch — здесь, в потоке, а не в цикле событий
    return load_silero_vad()


class Ear:
    """pw-record → VAD → Segmenter → whisper; микрофон включается по start()/stop()."""

    def __init__(self, out, whisper, cmd=REC_CMD, vad_load=load_vad):
        self.out, self.whisper, self.cmd = out, whisper, list(cmd)
        self._vad_load, self._lock = vad_load, asyncio.Lock()  # lock: start/stop идут по очереди, побеждает последний вызов
        self.paused = False
        self.seq = 0
        self._proc = self._vad = self._tasks = None

    @property
    def open(self):
        return self._proc is not None

    @property
    def pid(self):
        return self._proc.pid if self._proc else None

    async def warm(self):
        """VAD — заранее (от сети): первый start не ждёт torch и silero."""
        async with self._lock:
            await self._load_vad()

    async def _load_vad(self):
        if self._vad is None:
            self._vad = await asyncio.to_thread(self._vad_load)

    async def start(self):
        async with self._lock:
            if self.open:
                return
            await self._load_vad()
            proc = self._proc = await asyncio.create_subprocess_exec(*self.cmd, stdout=asyncio.subprocess.PIPE)
            phrases = asyncio.Queue()
            self._tasks = [asyncio.create_task(self._recognize(phrases)),
                           asyncio.create_task(self._read(proc, phrases))]

    async def stop(self):
        async with self._lock:
            await self._close()

    async def _close(self):
        proc, tasks, self._proc, self._tasks = self._proc, self._tasks, None, None
        if proc is None:
            return
        for t in tasks:
            t.cancel()
        with contextlib.suppress(ProcessLookupError):
            proc.kill()
        await asyncio.gather(*tasks, proc.wait(), return_exceptions=True)

    async def _recognize(self, phrases):  # отдельно: распознавание не задерживает чтение микрофона (перебивание)
        while True:
            n, pcm = await phrases.get()
            try:
                model = await self.whisper.get()
                await self.out.put(("phrase", await asyncio.to_thread(transcribe, model, pcm), n))
            except Exception as e:
                await self.out.put(("error", f"recognize: {e!r}"))  # слух продолжает работать
                await self.out.put(("phrase", "", n))  # речь закрыта: иначе Conversation.hearing застрянет

    async def _read(self, proc, phrases):
        import torch
        seg = Segmenter()
        try:
            while True:
                b = await proc.stdout.readexactly(FRAME * 2)
                if self.paused:
                    seg = Segmenter()
                    continue
                x = torch.from_numpy(np.frombuffer(b, np.int16).astype(np.float32) / 32768)
                for ev in seg.feed(self._vad(x, RATE).item(), b):
                    if ev[0] == "start":
                        self.seq += 1
                        await self.out.put(("start", self.seq))
                    else:
                        phrases.put_nowait((self.seq, ev[1]))  # фраза закрывает последнюю начатую речь
        except asyncio.IncompleteReadError:
            reason = "pw-record завершился"
        except Exception as e:
            reason = f"слух: {e!r}"
        # читатель кончил сам (не stop): сообщить и привести Ear в закрытое состояние
        await self.out.put(("error", reason))
        if self._proc is proc:
            self._tasks[0].cancel()
            self._proc = self._tasks = None
            with contextlib.suppress(ProcessLookupError):
                proc.kill()
            await proc.wait()
