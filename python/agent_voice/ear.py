"""Слух: pw-record → Silero VAD (кадры 32 мс) → Segmenter → faster-whisper.
В очередь out: ("start",) — начало речи, ("phrase", текст) — распознанная фраза."""
import asyncio
import subprocess

import numpy as np

from .segment import Segmenter

RATE, FRAME = 16000, 512  # 512 отсчётов = 32 мс


def load_whisper():
    from faster_whisper import WhisperModel
    try:
        return WhisperModel("large-v3-turbo", device="cuda", compute_type="float16")
    except Exception as e:
        print(f"agent_voice: whisper на GPU не загрузился ({e}), CPU small", flush=True)
        return WhisperModel("small", device="cpu", compute_type="int8")


def transcribe(model, pcm):
    audio = np.frombuffer(pcm, np.int16).astype(np.float32) / 32768
    segs, _ = model.transcribe(audio, language="ru", beam_size=1)
    return " ".join(s.text.strip() for s in segs).strip()


def source():
    """Источник эхоподавления, если модуль загружен; иначе — микрофон по умолчанию."""
    names = subprocess.run(["pactl", "list", "short", "sources"], capture_output=True, text=True).stdout
    return "echo-cancel-source" if "echo-cancel-source" in names else None


async def listen(out, model):
    import torch
    from silero_vad import load_silero_vad
    vad = load_silero_vad()
    src = source()
    if not src:
        await out.put(("warn", "нет эхоподавления: без наушников перебивание сработает на свой голос"))
    cmd = ["pw-record", "--rate", str(RATE), "--channels", "1", "--format", "s16"]
    cmd += ["--target", src] if src else []
    proc = await asyncio.create_subprocess_exec(*cmd, "-", stdout=asyncio.subprocess.PIPE)
    phrases = asyncio.Queue()

    async def recognize():  # отдельно: распознавание не задерживает чтение микрофона (перебивание)
        while True:
            pcm = await phrases.get()
            await out.put(("phrase", await asyncio.to_thread(transcribe, model, pcm)))

    task = asyncio.create_task(recognize())
    seg = Segmenter()
    try:
        while True:
            b = await proc.stdout.readexactly(FRAME * 2)
            x = torch.from_numpy(np.frombuffer(b, np.int16).astype(np.float32) / 32768)
            for ev in seg.feed(vad(x, RATE).item(), b):
                if ev[0] == "start":
                    await out.put(ev)
                else:
                    phrases.put_nowait(ev[1])
    finally:
        task.cancel()
        proc.kill()
