"""Слух: pw-record → Silero VAD (кадры 32 мс) → Segmenter → faster-whisper.
В очередь out: ("start", n) — начало речи, ("phrase", текст, n) — распознанная фраза;
n — Ear.seq, номер речи (растёт всегда, при перезапуске слуха не сбрасывается)."""
import asyncio
import contextlib
import os
import re
import sys
import time

import numpy as np

from .context import MIC_EVENT, OWN_NODE, mic_target, muted
from .segment import Segmenter
from .wake import HOTWORDS, Waker, named

RATE, FRAME = 16000, 512  # 512 отсчётов = 32 мс
# Вход выбирает context.mic_target: шумодав цепочки omarchy-mic-denoise портит распознавание, эхоподавление — только когда Ома в динамиках.
REC_CMD = ["pw-record", "-P", "{ node.name = " + OWN_NODE + " }", "--rate", str(RATE), "--channels", "1", "--format", "s16", "-"]


def rec_cmd(cmd: list, target: str) -> list:
    """pw-record с --target перед "-" (stdout); пустой target — вход по умолчанию."""
    return cmd[:-1] + ["--target", target, cmd[-1]] if target else list(cmd)


async def reap(*procs):
    """Убить pw-record и дождаться. Process.wait() ждёт и закрытия труб, а непрочитанный stdout (поток второго
    входа отстал — буфер полон) стоит на паузе и EOF не видит: wait() висел бы вечно — дочитываем stdout."""
    procs = [p for p in procs if p is not None]
    for p in procs:
        with contextlib.suppress(ProcessLookupError):
            p.kill()
    await asyncio.gather(*(p.stdout.read() for p in procs), *(p.wait() for p in procs), return_exceptions=True)


def load_whisper():
    from faster_whisper import WhisperModel
    try:
        return WhisperModel("large-v3-turbo", device="cuda", compute_type="float16")
    except Exception as e:
        print(f"agent_voice: whisper на GPU не загрузился ({e}), CPU small", flush=True)
        return WhisperModel("small", device="cpu", compute_type="int8")


# Титры YouTube из обучающих данных: whisper уверенно «слышит» их в тишине и шуме, порог no_speech их не ловит.
# «Играет музыка», «[музыка]» — так whisper подписывает звук, в котором не слышит речи
HALLUCINATION = re.compile(r"субтитр|dimatorzok|синецкая|продолжение следует|спасибо за просмотр|подпис\w* на( наш)? канал"
                           r"|^\W*играет музыка\W*$|^\W*\[?музыка\]?\W*$", re.I)


def transcribe(model, pcm, hotwords=None):
    """hotwords — подсказка декодеру (вторая ступень побудки: «Ома»; без неё полнота 0,51, с ней 0,88 — проба волны 0)."""
    audio = np.frombuffer(pcm, np.int16).astype(np.float32) / 32768
    segs, _ = model.transcribe(audio, language="ru", beam_size=1, hotwords=hotwords)
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
    """pw-record → VAD → Segmenter → whisper; микрофон включается по start()/stop(). Режим побудки (waking): тот же
    поток, но речь идёт не в whisper, а детектору имени (wake.Waker); подтверждённое имя — событие ("wake", текст, кадр)."""

    def __init__(self, out, whisper, cmd=REC_CMD, vad_load=load_vad, mic=mic_target, muted=muted, wake=None, watch=None):
        self.out, self.whisper, self.cmd, self._mic, self._muted = out, whisper, list(cmd), mic, muted
        self._vad_load, self._lock = vad_load, asyncio.Lock()  # lock: start/stop идут по очереди, побеждает последний вызов
        self._wake, self._watch_cmd = wake, watch  # первая ступень побудки (wake.VoskWake) и MicWatch (context.MIC_WATCH)
        self.paused = False
        self.target = ""  # вход, который она пишет (mic_target): "" — вход по умолчанию; орб берёт с него уровень
        self.seq = 0
        self.audio = {}  # номер речи → её звук, пока Ома его не забрала
        self.echo = False  # слух на входе с эхоподавлением: Ома говорит в динамики
        self.speakers = None  # True — Ома говорит в динамики, хоть наушники и «в ушах»; None — по сервису присутствия
        self.speaking = False  # звучит её голос: в динамиках слух берёт кадры входа с эхоподавлением
        self.waking = False    # слух открыт на «Ома» (Ома в покое): фразы не распознаются
        self.mic_pending = False  # проверка микрофона уже в очереди: пачка событий pactl — одна проверка
        self._aec = None       # второй pw-record — mic_aec, только когда она говорит в динамики
        self._aec_name = ""    # вход с эхоподавлением, если разговор начнётся с побудки (take)
        self._replay = None    # кадр, с которого разговор после побудки слышит звук (фраза с именем — целиком)
        self._proc = self._vad = self._tasks = self._sub = self._confirming = None

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

    async def _rec(self, target):
        return await asyncio.create_subprocess_exec(*rec_cmd(self.cmd, target), stdout=asyncio.subprocess.PIPE)

    async def start(self, wake=False):
        """Слух на разговор или (wake) на «Ома» — только встроенный микрофон, без эхоподавления. Уже открыт на том же
        входе — меняется только режим: начало фразы не съедается. False — входа для побудки нет."""
        async with self._lock:
            await self._load_vad()
            if wake:
                await asyncio.to_thread(self._wake.load)
            kw = {"wake": True} if wake else {}
            if self.speakers is not None:
                kw["out"] = self.speakers
            # shortcut: микрофон выбирается при включении слуха; наушники вынули посреди разговора — слух на прежнем до следующего включения
            got = await asyncio.to_thread(lambda: self._mic(**kw))
            target, echo, aec = (tuple(got) + (False, ""))[:3] if isinstance(got, tuple) else (got, False, "")
            if wake and not target:  # встроенного нет (крышка, настольный): Bluetooth держал бы наушники в режиме гарнитуры
                await self._close()
                return False
            if self.open and target == self.target and not (wake and self._aec):  # поток mic_aec ради побудки не держим
                if not wake and aec and self._aec is None:
                    self._aec = await self._rec(aec)
                self.echo, self.waking, self._aec_name, self._replay = echo, wake, aec, None
                return True
            await self._close()
            print(f"agent_voice: слух{' на «Ома»' if wake else ''}: вход {target or 'по умолчанию'}"
                  f"{', пока говорит — ' + aec if aec and not wake else ''}", file=sys.stderr, flush=True)
            self.target, self.echo, self.waking, self._aec_name, self._replay = target, echo, wake, aec, None
            proc = self._proc = await self._rec(target)
            self._aec = await self._rec(aec) if aec and not wake else None
            self._sub = await asyncio.create_subprocess_exec(*self._watch_cmd, stdout=asyncio.subprocess.PIPE,
                                                             env={**os.environ, "LC_ALL": "C"}) if self._watch_cmd else None
            # микрофон выключен — кадры выбрасываются до VAD с первого, как при чужом микрофоне: ничего не распознаётся.
            # Спрашиваем, пока pw-record уже пишет в трубу (чтения ещё нет): начало фразы не ждёт pactl
            if await asyncio.to_thread(self._muted, target):
                self.paused = True
            phrases = asyncio.Queue()
            self._tasks = [asyncio.create_task(self._recognize(phrases)), asyncio.create_task(self._read(proc, phrases))]
            if self._sub:
                self._tasks.append(asyncio.create_task(self._watch(self._sub)))
            return True

    async def take(self, at):
        """Имя подтвердилось — разговор на том же входе (перезапуск pw-record срезал бы продолжение фразы); фразу с
        именем слух слышит заново с кадра at. Её голос в динамиках — добавить вход с эхоподавлением."""
        async with self._lock:
            if not (self.open and self.waking):
                return False
            if self.echo and self._aec_name and self._aec is None:
                self._aec = await self._rec(self._aec_name)
            self._replay, self.waking = at, False
            return True

    async def stop(self):
        async with self._lock:
            await self._close()

    async def _close(self):
        proc, aec, sub, tasks = self._proc, self._aec, self._sub, self._tasks
        self._proc = self._aec = self._sub = self._tasks = None
        self.waking = False
        if proc is None:
            return
        tasks = tasks + ([self._confirming] if self._confirming else [])
        for t in tasks:
            t.cancel()
        await asyncio.gather(*tasks, return_exceptions=True)  # до reap: дочитывать stdout, пока его ждёт _read, нельзя
        await reap(proc, aec, sub)
        await self._closed()

    async def _closed(self):
        """Слух закрыт — для Омы микрофон свободен (как прежний опрос без pw-record): последняя проверка, MicWatch нет."""
        if self._watch_cmd:
            await self._mic_check()

    async def _recognize(self, phrases):  # отдельно: распознавание не задерживает чтение микрофона (перебивание)
        while True:
            n, pcm = await phrases.get()
            if not pcm:  # после обрыва одна тишина: речь закрыта, распознавать нечего
                await self.out.put(("phrase", "", n))
                continue
            try:
                model = await self.whisper.get()
                text = await asyncio.to_thread(transcribe, model, pcm)
                self.audio[n] = pcm  # реплику из нескольких фраз Ома при отправке распознаёт целиком
                await self.out.put(("phrase", text, n))
            except Exception as e:
                await self.out.put(("error", f"recognize: {e!r}"))  # слух продолжает работать
                await self.out.put(("phrase", "", n))  # речь закрыта: иначе Conversation.hearing застрянет

    async def _confirm(self, waker, at, pcm):
        """Вторая ступень побудки: whisper с подсказкой «Ома» — имя первым словом. Иначе звук забыт, текст — никуда."""
        try:
            text = await asyncio.to_thread(transcribe, await self.whisper.get(), pcm, HOTWORDS)
            if named(text):
                await self.out.put(("wake", text, at))
        except Exception as e:
            await self.out.put(("error", f"wake: {e!r}"))
        finally:
            waker.busy = False

    async def _watch(self, sub):
        """MicWatch: чужой захват, mute и вход по умолчанию — по событиям pactl subscribe, а не опросом; первая — сразу."""
        await self._mic_check()
        while line := await sub.stdout.readline():
            if MIC_EVENT.search(line.decode(errors="replace")):
                await self._mic_check()

    async def _mic_check(self):
        if not self.mic_pending:
            self.mic_pending = True
            await self.out.put(("mic_check",))

    async def _heard(self, seg, prob, b, k, base, phrases):
        """Кадр разговора: начало речи — событием (время по звуку), конец — фраза на распознавание."""
        for ev in seg.feed(prob, b):
            if ev[0] == "start":
                self.seq += 1  # начало речи — первый из start_frames кадров речи подряд
                await self.out.put(("start", self.seq, base + (k - seg.start_frames) * FRAME / RATE))
            else:
                phrases.put_nowait((self.seq, ev[1]))  # фраза закрывает последнюю начатую речь

    async def _read(self, proc, phrases):
        import torch
        seg, mode, waker = Segmenter(), False, None
        k, base = 0, None  # время по звуку: кадр k записан не позже base + k·32 мс; задержка чтения его не сдвигает
        try:
            while True:
                b = await proc.stdout.readexactly(FRAME * 2)
                k += 1
                t = time.monotonic() - k * FRAME / RATE
                base = t if base is None else min(base, t)
                aec = self._aec
                if aec is not None:  # читаем оба всё время — иначе отстающий поток копит задержку
                    a = await aec.stdout.readexactly(FRAME * 2)
                    if self.speaking:
                        b = a
                if self.waking != mode:  # побудка ↔ разговор: начатая в другом режиме речь не тянется
                    seg, mode = Segmenter(), self.waking
                    at, self._replay = self._replay, None
                    if not mode and waker and at is not None:  # разговор с побудки: фраза с именем — целиком
                        for n, p, f in waker.since(at):
                            await self._heard(seg, p, f, n, base, phrases)
                    waker = Waker(self._wake) if mode else None
                if self.paused or (mode and self.echo and self.speaking):  # в динамиках её голос побудкой не слушается
                    seg = Segmenter()
                    if waker:
                        waker.reset()
                    continue
                x = torch.from_numpy(np.frombuffer(b, np.int16).astype(np.float32) / 32768)
                p = self._vad(x, RATE).item()
                if not mode:
                    await self._heard(seg, p, b, k, base, phrases)
                    continue
                for at, pcm in waker.frame(k, p, b):  # первая ступень — в цикле: ~1 мс на кадр, вторая — в потоке
                    self._confirming = asyncio.create_task(self._confirm(waker, at, pcm))
        except asyncio.IncompleteReadError:
            reason = "pw-record завершился"
        except Exception as e:
            reason = f"слух: {e!r}"
        # читатель кончил сам (не stop): сообщить и привести Ear в закрытое состояние
        await self.out.put(("error", reason))
        if self._proc is proc:
            me = asyncio.current_task()
            tasks = [t for t in self._tasks + [self._confirming] if t and t is not me]
            aec, sub = self._aec, self._sub
            self._proc = self._tasks = self._aec = self._sub = None
            self.waking = False
            for t in tasks:
                t.cancel()
            await reap(proc, aec, sub)
            await self._closed()
