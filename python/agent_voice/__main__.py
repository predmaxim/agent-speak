"""agent_voice: процесс Омы. Запускает демон agent-speak: python -m agent_voice, stdin — трубка.
Команды демона — строками JSON в stdin ({"do": …}); EOF — демон ушёл, выходим. События Омы — в сокет демона."""
import asyncio
import contextlib
import glob
import json
import os
import signal
import subprocess
import sys

from . import link
from .context import foreign_capture, mic_outputs, on_ac
from .conversation import Conversation


def cuda_env():
    """CUDA-библиотеки из pip (nvidia-*) видны ctranslate2 только через LD_LIBRARY_PATH при старте."""
    libs = glob.glob(os.path.join(sys.prefix, "lib/python3*/site-packages/nvidia/*/lib"))
    have = os.environ.get("LD_LIBRARY_PATH", "")
    if libs and libs[0] not in have:
        os.environ["LD_LIBRARY_PATH"] = ":".join(libs + ([have] if have else []))
        os.execv(sys.executable, [sys.executable, "-m", "agent_voice", *sys.argv[1:]])


def notify(text):
    with contextlib.suppress(OSError, subprocess.SubprocessError):  # зависший dbus не держит цикл Омы
        subprocess.run(["notify-send", "-t", "4000", "-a", "Озвучка", "Ома", text], timeout=5)


class Oma:
    """Связывает разговор, слух, мозг и демона: команды и события → действия Conversation → исполнение."""

    def __init__(self, conv, ear, brain, whisper, send=link.send):
        self.conv, self.ear, self.brain, self.whisper, self.send = conv, ear, brain, whisper, send
        self.history = []  # текущий чат, как его видит окно: история из open() + реплики этого процесса
        self.n = 0         # номер ответа: пузырь oma-<n>
        self.tasks = set()

    def emit(self, line):
        self.send({"cmd": "emit", "line": line})

    def warm(self):
        """whisper — в фоне (заодно отметка «используется»); загрузился — орб уходит из «loading»."""
        t = asyncio.create_task(self._warm())
        self.tasks.add(t)  # держим ссылку: иначе задачу может собрать GC
        t.add_done_callback(self.tasks.discard)

    async def _warm(self):
        was = self.whisper.ready
        try:
            await self.whisper.get()
        except Exception as e:
            print(f"agent_voice: whisper: {e!r}", file=sys.stderr, flush=True)
            return
        if not was:
            await self.act([("state", self.conv.state)])

    def _dialog(self, on):
        acts = self.conv.set_dialog(on)
        if not any(a[0] == "state" for a in acts):  # демон узнаёт dialog только из voice_state
            acts.append(("state", self.conv.state))
        return acts

    def _chat(self, role, text):
        """Реплика — в историю текущего чата и в окно; ответ Омы — одним сообщением, как в окне."""
        h = self.history
        if role == "oma" and h and h[-1]["role"] == "oma":
            h[-1]["text"] += " " + text
        else:
            h.append({"role": role, "text": text})
        self.emit({"chat_delta": {"key": self.brain.key, "role": role, "text": text}})

    async def command(self, cmd):
        do, c = cmd.get("do"), self.conv
        if do == "talk":
            await self.act(c.talk())
        elif do == "send":
            await self.act(c.send())
        elif do == "cancel":
            await self.act(c.cancel())
        elif do == "dialog":
            await self.act(self._dialog(not c.dialog))
        elif do == "chats":
            self.emit({"chats": await asyncio.to_thread(self.brain.chats)})
        elif do in ("ask", "open", "new"):
            key = "new" if do == "new" else cmd.get("key") or self.brain.key
            if key != self.brain.key:
                if c.turn:  # ход прошлого чата обрываем, пока его клиент жив
                    await self.act(c.cancel())
                self.history = []  # open упал — чужая история не останется под новым ключом
                self.history = await self.brain.open(key)
                self.emit({"chat": {"key": key, "messages": self.history}})
            elif do != "ask":
                self.emit({"chat": {"key": key, "messages": self.history}})
            if do == "ask":
                await self.act(c.ask(cmd.get("text", "")))

    async def event(self, ev):
        kind, c = ev[0], self.conv
        if kind == "start":
            await self.act(c.speech_start())
        elif kind == "phrase":
            await self.act(c.phrase(ev[1]))
        elif kind == "delta":
            await self.act(c.delta(ev[1]))
        elif kind == "done":
            await self.act(c.done())
            self.emit({"chat": {"key": self.brain.key, "messages": self.history}})
            self.emit({"chats": await asyncio.to_thread(self.brain.chats)})
        elif kind == "audio":
            await self.act(c.audio(ev[1]))
        elif kind == "quiet":
            await self.act(c.set_quiet(ev[1]))
        elif kind == "mic":
            self.ear.paused = ev[1]  # микрофон пишет другой (voxtype и т. п.) — кадры не слушаем
            await self.act(c.mic_busy(ev[1]))
        elif kind == "opened":
            self.emit({"chat": {"key": ev[1], "was": ev[2], "messages": self.history}})
        elif kind == "error":
            print(f"agent_voice: {ev[1]}", file=sys.stderr, flush=True)
            notify(ev[1][:200])
            if not self.ear.open and (c.dialog or c.state in ("listening", "paused")):  # слух умер сам
                await self.act(self._dialog(False) if c.dialog else c.cancel())
            elif c.turn and ev[1].startswith("Claude"):  # ход мозга оборвался — в покой
                await self.act(c.done())

    async def act(self, acts):
        failed = None
        for a in acts:
            k = a[0]
            if k == "ear":
                if a[1]:
                    await self.ear.start()
                    if not self.whisper.ready:  # от батареи: грузится по первому включению слуха
                        self.warm()
                else:
                    await self.ear.stop()
            elif k == "send":
                key = self.brain.key
                try:
                    await self.brain.send(a[1])
                except Exception as e:
                    failed = e
                if self.brain.key != key:  # устаревший чат сменился новым: реплика — первая в нём
                    self.history = []
                self.n += 1
                self._chat("me", a[1])
            elif k == "interrupt":
                await self.brain.interrupt()
            elif k == "stop":
                self.send({"cmd": "stop"})
            elif k == "say":
                self.send({"cmd": "voice", "text": a[1]})
            elif k == "show":
                self.emit({"bubble": {"id": f"oma-{self.n}", "who": "Ома", "text": a[1], "kind": "oma", "append": True}})
                self._chat("oma", a[1])
            elif k == "draft":
                self.emit({"bubble": {"id": "draft", "who": "Ты", "text": a[1], "kind": "draft", "append": False}})
            elif k == "drop_draft":
                self.emit({"bubble": {"id": "draft", "who": "Ты", "text": "", "kind": "draft", "append": False, "drop": True}})
            elif k == "state":
                s = "loading" if a[1] == "listening" and not self.whisper.ready else a[1]
                self.send({"cmd": "voice_state", "state": s, "dialog": self.conv.dialog})
        if failed:  # после всех действий: ход уже в «thinking», done() вернёт в покой
            await self.event(("error", f"Claude: {failed}"))


async def main():
    from .brain import Brain
    from .ear import Ear, Whisper

    loop = asyncio.get_running_loop()
    events, agents = asyncio.Queue(), []
    whisper = Whisper()
    oma = Oma(Conversation(), Ear(events, whisper), Brain(events, lambda: agents), whisper)

    def on_agents(lst):
        agents[:] = lst  # снимок для инструментов мозга

    last = {}

    def on_status(d):  # только смены: повтор «не говорит» до начала звука не должен закончить ответ
        for field, kind in (("speaking", "audio"), ("quiet", "quiet")):
            v = bool(d.get(field))
            if last.get(field) != v:
                last[field] = v
                events.put_nowait((kind, v))

    async def stdin():
        r = asyncio.StreamReader(limit=1 << 22)  # ask со вставленным логом бывает длинным
        try:
            await loop.connect_read_pipe(lambda: asyncio.StreamReaderProtocol(r), sys.stdin)
            async for line in r:
                try:
                    events.put_nowait(("cmd", json.loads(line)))
                except ValueError:
                    print(f"agent_voice: не JSON: {line[:100]!r}", file=sys.stderr, flush=True)
        finally:
            events.put_nowait(("eof",))

    async def daemon():
        while True:  # демон закрыл подписку или ещё не слушает — переподключиться
            with contextlib.suppress(OSError):
                await link.watch(on_status, on_agents)
            await asyncio.sleep(2)

    async def power():
        while True:
            ac = on_ac()
            if ac or oma.ear.open:  # от сети — тёплый; слух открыт — модель «используется»
                oma.warm()
            if not oma.ear.open:  # «300 с без слуха» считаются от закрытия слуха
                whisper.maybe_unload(ac)
            await asyncio.sleep(30)

    async def mic():
        busy = False
        while True:
            pid = oma.ear.pid  # до await: слух мог переоткрыться, а снимок pactl — со старым pw-record
            now = pid is not None and foreign_capture(await asyncio.to_thread(mic_outputs), {pid})
            if now != busy:
                busy = now
                events.put_nowait(("mic", busy))
            await asyncio.sleep(1)

    loop.add_signal_handler(signal.SIGTERM, events.put_nowait, ("eof",))
    tasks = [asyncio.create_task(c) for c in (stdin(), daemon(), power(), mic())]
    try:
        await oma.act([("state", oma.conv.state)])  # демон считает Ому «off», пока она не скажет о себе
        while (ev := await events.get())[0] != "eof":
            try:
                await (oma.command(ev[1]) if ev[0] == "cmd" else oma.event(ev))
            except Exception as e:  # сбой одной команды не роняет Ому
                await oma.event(("error", f"{ev[0]}: {e!r}"))
    finally:  # pw-record и claude не должны остаться сиротами
        for t in tasks + list(oma.tasks):
            t.cancel()
        await asyncio.gather(*tasks, return_exceptions=True)
        with contextlib.suppress(Exception):
            await asyncio.wait_for(oma.ear.stop(), 3)
        with contextlib.suppress(Exception):
            await asyncio.wait_for(oma.brain.close(), 3)
        if whisper.loading:  # поток загрузки не прервать, а asyncio.run ждал бы его; дети уже закрыты
            sys.stdout.flush()
            sys.stderr.flush()
            os._exit(0)


if __name__ == "__main__":
    cuda_env()
    try:
        asyncio.run(main())
    except Exception as e:
        print(f"agent_voice: {e!r}", file=sys.stderr, flush=True)
        notify(str(e)[:200])
        sys.exit(1)
