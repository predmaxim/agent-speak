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
import time
import traceback

from . import link
from .context import foreign_capture, locked, mic_outputs, on_ac
from .conversation import Conversation
from .orders import Orders


# Хвост звука после конца речи демона — speaker::TAIL: pw-cat ещё доигрывает свой буфер.
TAIL = 1.0

def cuda_env():
    """CUDA-библиотеки из pip (nvidia-*) видны ctranslate2 только через LD_LIBRARY_PATH при старте."""
    libs = glob.glob(os.path.join(sys.prefix, "lib/python3*/site-packages/nvidia/*/lib"))
    have = os.environ.get("LD_LIBRARY_PATH", "")
    if libs and libs[0] not in have:
        os.environ["LD_LIBRARY_PATH"] = ":".join(libs + ([have] if have else []))
        os.execv(sys.executable, [sys.executable, "-m", "agent_voice", *sys.argv[1:]])


def notify(text):
    """Уведомление без ожидания: notify-send не держит цикл Омы (зависший dbus и т. п.)."""
    with contextlib.suppress(OSError):
        subprocess.Popen(["notify-send", "-t", "4000", "-a", "Озвучка", "Ома", text],
                         stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def kill_children():
    """SIGTERM ещё живым детям (claude, pw-record): os._exit минует atexit-сборщик Agent SDK."""
    pids = []
    for f in glob.glob("/proc/self/task/*/children"):  # дети всех потоков процесса
        with contextlib.suppress(OSError), open(f) as fh:
            pids += fh.read().split()
    for pid in pids:
        with contextlib.suppress(OSError):
            os.kill(int(pid), signal.SIGTERM)


class Oma:
    """Связывает разговор, слух, мозг и демона: команды и события → действия Conversation → исполнение."""

    def __init__(self, conv, ear, brain, whisper, send=link.send, notify=notify, clock=time.monotonic, orders=None):
        self.conv, self.ear, self.brain, self.whisper, self.send = conv, ear, brain, whisper, send
        self.orders = orders or Orders()
        self.utter, self.sending = [], []  # звук фраз черновика; при выбросе черновика — к отправке
        self.notify, self.clock = notify, clock
        self.history = []  # текущий чат, как его видит окно: история из open() + реплики этого процесса
        self.n = 0         # номер ответа: пузырь oma-<pid>-<n>
        self.tasks = set()
        self.sound = 0     # номер события audio: хвост pw-cat отсчитывается от последнего
        self.notified = (None, 0.0)  # последнее уведомление: текст, время

    def emit(self, line):
        self.send({"cmd": "emit", "line": line})

    def bg(self, coro):
        """Фоновая задача: ссылку держим (иначе её может собрать GC), сбой — в журнал."""
        t = asyncio.create_task(coro)
        self.tasks.add(t)
        t.add_done_callback(self._bg_done)

    def _bg_done(self, t):
        self.tasks.discard(t)
        if not t.cancelled() and t.exception():
            print(f"agent_voice: фон: {t.exception()!r}", file=sys.stderr, flush=True)

    def warm(self):
        """whisper — в фоне (заодно отметка «используется»); загрузился — орб уходит из «loading»."""
        self.bg(self._warm())

    async def _warm(self):
        was = self.whisper.ready
        await self.whisper.get()
        if not was:
            await self.act([("state", self.conv.state)])

    def _warn(self, text):
        """В журнал — каждая ошибка; уведомление — новый текст или тот же, но не чаще раза в 5 мин."""
        print(f"agent_voice: {text}", file=sys.stderr, flush=True)
        now = self.clock()
        if text != self.notified[0] or now - self.notified[1] >= 300:
            self.notified = (text, now)
            self.notify(text[:200])

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
            self.ear.speakers = None  # разговор по клику — вход по тому, куда она говорит
            if c.state == "idle" and self.brain.key.startswith("agent:"):  # с Омой-помощником — в общем чате
                await self.switch(await self.general(), was="*")
            await self.act(c.talk())
        elif do == "cancel":
            await self.act(c.cancel())
        elif do == "warm":  # демон прогревает слух до окна ответа: whisper не грузится при открытом микрофоне
            self.warm()
        elif do == "listen":
            # экран заблокирован или микрофон держит чужой (conv.mic при закрытом слухе не знает: pid нет) — не открываем
            if not await asyncio.to_thread(locked) and not foreign_capture(await asyncio.to_thread(mic_outputs), set()):
                self.ear.speakers = True if cmd.get("speakers") else None  # «Ты тут?» в динамики — и слушать ноутбуком
                key = cmd.get("key")  # окно после вопроса — ответ в чат спросившего агента
                if key and key != self.brain.key and c.state == "idle":
                    await self.switch(key)
                await self.act(c.listen(float(cmd.get("secs", 5)), cmd.get("kind", "news"), cmd.get("said", "")))
        elif do == "chats":
            self.emit({"chats": await asyncio.to_thread(self.brain.chats)})
        elif do == "open" and not cmd.get("key"):  # окно спрашивает текущий чат: без переключения, was "*" — принять его ключ
            self.emit({"chat": {"key": self.brain.key, "was": "*", "messages": self.history}})
        elif do in ("ask", "open", "new"):
            want = cmd.get("key")
            key = "new" if do == "new" else await self.general() if want == "general" else want or self.brain.key
            was = "general" if want == "general" else None  # окно ждёт ответа на «general» и примет настоящий ключ
            if key != self.brain.key:
                await self.switch(key, was)
            elif do != "ask":
                self.emit({"chat": {"key": key, **({"was": was} if was else {}), "messages": self.history}})
            if do == "ask":
                await self.act(c.ask(cmd.get("text", "")))

    async def switch(self, key, was=None):
        """Перейти в чат key; was — ключ, на который окно ждёт ответа ("*" — любой)."""
        if self.conv.turn:  # ход прошлого чата обрываем, пока его клиент жив
            await self.act(self.conv.cancel())
        self.history = []  # open упал — чужая история не останется под новым ключом
        self.history = await self.brain.open(key)
        self.emit({"chat": {"key": key, **({"was": was} if was else {}), "messages": self.history}})

    async def general(self):
        """Общий чат: текущий, если он не чат агента; иначе последний общий, если он моложе STALE_HOURS, иначе новый."""
        from .brain import STALE_HOURS  # мозг (Claude SDK) грузится лениво, как в main
        if not self.brain.key.startswith("agent:"):
            return self.brain.key
        chats = await asyncio.to_thread(self.brain.chats)
        return chats[0]["key"] if chats and time.time() - chats[0]["updated"] < STALE_HOURS * 3600 else "new"

    async def _tail_over(self, sound):
        await asyncio.sleep(TAIL)
        if self.sound == sound:  # хвост pw-cat доиграл, новой речи не было — дальше сырой микрофон
            self.ear.speaking = False

    async def event(self, ev):
        kind, c = ev[0], self.conv
        if kind == "start":
            await self.act(c.speech_start(ev[1]))
        elif kind == "phrase":
            pcm, before = getattr(self.ear, "audio", {}).pop(ev[2], b""), c.draft
            acts = c.phrase(ev[1], ev[2])
            if pcm and c.draft != before:  # фраза вошла в черновик; начало срезано как эхо — её звук целиком не распознавать
                self.utter.append(pcm if c.draft.endswith(ev[1].strip()) else None)
            if ev[1].strip():  # без текста: в нём бывает чужой разговор в комнате
                verdict = "принята" if c.draft != before else ("эхо" if c._echoing() or ev[2] == c.echo_n else "не принята")
                print(f"agent_voice: слух: фраза {len(ev[1].split())} сл., {verdict}", file=sys.stderr, flush=True)
            await self.act(acts)
        elif kind == "tick":
            await self.act(c.tick())
        elif kind == "locked":
            await self.act(c.end())
        elif kind == "delta":
            await self.act(c.delta(ev[1]))
        elif kind == "done":
            await self.act(c.done())
            self.emit({"chat": {"key": self.brain.key, "messages": self.history}})
            self.emit({"chats": await asyncio.to_thread(self.brain.chats)})
        elif kind == "audio":
            # её голос звучит: в динамиках слух — на входе с эхоподавлением; «смолкла» демон шлёт, записав
            # последний кусок, а pw-cat доигрывает ещё ~0,7 с — сырой микрофон слышал бы их без эхоподавления
            self.sound += 1
            if ev[1]:
                self.ear.speaking = True
            else:
                self.bg(self._tail_over(self.sound))
            await self.act(c.audio(ev[1]))
        elif kind == "quiet":
            await self.act(c.set_quiet(ev[1]))
        elif kind == "mic":
            self.ear.paused = ev[1]  # микрофон пишет другой (voxtype и т. п.) — кадры не слушаем
            await self.act(c.mic_busy(ev[1]))
        elif kind == "opened":
            self.emit({"chat": {"key": ev[1], "was": ev[2], "messages": self.history}})
        elif kind == "error":
            self._warn(ev[1])
            if not self.ear.open and c.active:  # слух умер сам
                await self.act(c.end())
            elif c.turn and ev[1].startswith("Claude"):  # ход мозга оборвался — в покой
                await self.act(c.done())

    async def hear(self, pcm):
        """Реплика целиком: whisper по кускам между паузами теряет смысл на краях («этот спик» вместо «AgentSpeak»)."""
        from .ear import transcribe
        return await asyncio.to_thread(transcribe, await self.whisper.get(), pcm)

    async def _whole(self, text):
        """Текст реплики, сказанной голосом: несколько фраз — распознать звук целиком; не вышло — как в черновике."""
        parts, self.sending = self.sending, []
        if len(parts) < 2 or None in parts:  # None — фраза с её голосом в начале: целиком эхо вернулось бы
            return text
        try:
            whole = (await self.hear(b"".join(parts))).strip()
        except Exception as e:
            print(f"agent_voice: реплика целиком: {e!r}", file=sys.stderr, flush=True)
            return text
        if whole and whole != text:  # сам текст в журнал не пишем: в нём бывает чужой разговор в комнате
            print(f"agent_voice: реплика целиком распознана иначе, чем по кускам ({len(whole)} и {len(text)} знаков)", file=sys.stderr, flush=True)
        return whole or text

    async def act(self, acts):
        failed = said = None
        for a in acts:
            k = a[0]
            if k == "ear":
                if a[1]:
                    await self.ear.start()
                    self.conv.echo = getattr(self.ear, "echo", False)  # в динамиках её голос просачивается в слух
                    if not self.whisper.ready:  # от батареи: грузится по первому включению слуха
                        self.warm()
                else:
                    await self.ear.stop()
            elif k == "send":
                key = self.brain.key
                a = (k, await self._whole(a[1]) if self.conv.voiced else a[1], a[2])
                self.orders.turn(self.conv.voiced, a[1])
                if self.conv.voiced:
                    said = await self.orders.confirm(a[1])  # «подтверждаю …» сверяет код, мозг не зовём
                if said is None:
                    note, self.orders.note = self.orders.note, ""
                    try:
                        await self.brain.send(note + a[2] + a[1])
                    except Exception as e:
                        failed = e
                if key.startswith("chat:") and self.brain.key == "new":  # устаревший чат сменился новым: реплика — первая в нём
                    self.history = []
                self.n += 1
                self._chat("me", a[1])
                self.emit({"bubble": {"id": f"me-{os.getpid()}-{self.n}", "who": "Ты", "text": a[1], "kind": "me", "append": False}})
            elif k == "presence":
                self.send({"cmd": "presence", "state": a[1]})
            elif k == "interrupt":
                await self.brain.interrupt()
            elif k == "stop":  # перебивание: финалы агентов, удержанные за Омой, демон не выбрасывает
                self.send({"cmd": "hush"})
            elif k == "halt":  # «отбой»: замолчать совсем — и её ответ, и чтение, и новости
                self.send({"cmd": "stop"})
            elif k == "say":
                self.send({"cmd": "voice", "text": a[1]})
            elif k == "show":
                self.emit({"bubble": {"id": f"oma-{os.getpid()}-{self.n}", "who": "Ома", "text": a[1], "kind": "oma", "append": True}})
                self._chat("oma", a[1])
            elif k == "draft":
                self.emit({"bubble": {"id": "draft", "who": "Ты", "text": a[1], "kind": "draft", "hearing": a[2], "append": False}})
            elif k == "drop_draft":
                self.utter, self.sending = [], self.utter  # отправка следом заберёт звук; без неё — забыт новым черновиком
                self.emit({"bubble": {"id": "draft", "who": "Ты", "text": "", "kind": "draft", "append": False, "drop": True}})
            elif k == "state":
                s = "loading" if a[1] == "listening" and not self.whisper.ready else a[1]
                self.send({"cmd": "voice_state", "state": s, "talking": self.conv.active})
        if failed:  # после всех действий: ход уже в «thinking», done() вернёт в покой
            await self.event(("error", f"Claude: {failed}"))
        if said is not None:  # фраза кода вместо ответа мозга — как его ход: после «thinking» из этого же списка
            await self.event(("delta", said))
            await self.event(("done", None))


async def serve(oma, events, reader):
    """Команды из reader и события из очереди → Ома по одной. EOF обрывает диспетчер сразу, даже посреди await
    (повисший connect не держит старую Ому с открытым pw-record). Упавший читатель или диспетчер — наружу."""
    async def read():
        async for line in reader:
            try:
                events.put_nowait(("cmd", json.loads(line)))
            except ValueError:
                print(f"agent_voice: не JSON: {line[:100]!r}", file=sys.stderr, flush=True)

    async def dispatch():
        while True:
            ev = await events.get()
            try:
                await (oma.command(ev[1]) if ev[0] == "cmd" else oma.event(ev))
            except Exception as e:  # сбой одной команды не роняет Ому
                await oma.event(("error", f"{ev[0]}: {e!r}"))

    tasks = [asyncio.create_task(read()), asyncio.create_task(dispatch())]
    try:
        done, _ = await asyncio.wait(tasks, return_when=asyncio.FIRST_COMPLETED)
    finally:
        for t in tasks:
            t.cancel()
        await asyncio.wait(tasks, timeout=1)
    for t in done:
        t.result()


async def main():
    events, agents, loops, oma = asyncio.Queue(), [], [], None

    async def keep(name, fn):
        """Фоновый цикл под присмотром: упал — событие error; через 2 с снова (подписка — и после конца потока)."""
        while True:
            try:
                await fn()
            except Exception as e:
                events.put_nowait(("error", f"{name}: {e!r}"))
            await asyncio.sleep(2)

    try:
        from .brain import Brain
        from .ear import Ear, Whisper

        loop = asyncio.get_running_loop()
        whisper = Whisper()
        orders = Orders()  # один на мозг и Ому: ждущее подтверждения опасное видят оба
        oma = Oma(Conversation(), Ear(events, whisper), Brain(events, lambda: agents, send=link.send, orders=orders), whisper,
                  orders=orders)

        def on_agents(lst):
            agents[:] = lst  # снимок для инструментов мозга

        last = {}

        def on_status(d):  # только смены: повтор «не говорит» до начала звука не должен закончить ответ
            for field, kind in (("speaking", "audio"), ("quiet", "quiet")):
                v = bool(d.get(field))
                if last.get(field) != v:
                    last[field] = v
                    events.put_nowait((kind, v))

        async def daemon():
            with contextlib.suppress(OSError):  # демона ещё нет или он закрыл сокет — keep переподключит
                await link.watch(on_status, on_agents)

        async def power():
            while True:
                ac = on_ac()
                oma.bg(oma.ear.warm())  # VAD тёплый всегда: CPU, дёшево; первый клик не ждёт torch
                if ac or oma.ear.open:  # от сети — тёплый; слух открыт — модель «используется»
                    oma.warm()
                if not oma.ear.open:  # «300 с без слуха» считаются от закрытия слуха
                    whisper.maybe_unload(ac)
                await asyncio.sleep(30)

        async def ticker():
            while True:
                await asyncio.sleep(0.2 if oma.conv.active else 1)
                if oma.conv.active:
                    events.put_nowait(("tick",))

        async def lock():
            while True:
                await asyncio.sleep(2)
                if oma.conv.active and await asyncio.to_thread(locked):
                    events.put_nowait(("locked",))

        async def mic():
            busy = False
            while True:
                pid = oma.ear.pid  # до await: слух мог переоткрыться, а снимок pactl — со старым pw-record
                now = pid is not None and foreign_capture(await asyncio.to_thread(mic_outputs), {pid})
                if now != busy:
                    busy = now
                    events.put_nowait(("mic", busy))
                await asyncio.sleep(1)

        reader = asyncio.StreamReader(limit=1 << 22)  # ask со вставленным логом бывает длинным
        await loop.connect_read_pipe(lambda: asyncio.StreamReaderProtocol(reader), sys.stdin)
        loop.add_signal_handler(signal.SIGTERM, reader.feed_eof)  # voice_stop: как EOF
        loops = [asyncio.create_task(keep(n, f)) for n, f in (("daemon", daemon), ("power", power), ("mic", mic), ("ticker", ticker), ("lock", lock))]
        await oma.act([("state", oma.conv.state)])  # демон считает Ому «off», пока она не скажет о себе
        await serve(oma, events, reader)
    finally:  # pw-record и claude не должны остаться сиротами
        exc = sys.exc_info()[1]
        for t in loops + list(oma.tasks if oma else ()):
            t.cancel()
        await asyncio.gather(*loops, return_exceptions=True)
        if oma:
            with contextlib.suppress(Exception):
                await asyncio.wait_for(oma.ear.stop(), 3)
            with contextlib.suppress(Exception):  # SDK ждёт до 5 с, пока claude допишет сессию
                await asyncio.wait_for(oma.brain.close(), 10)
        kill_children()
        failed = isinstance(exc, Exception)
        if failed:
            traceback.print_exception(exc)
            notify(f"Ома упала: {exc!r}"[:200])
        sys.stdout.flush()
        sys.stderr.flush()
        # сразу: asyncio.run ждал бы потоки (загрузка whisper/VAD, pactl), а дети уже закрыты
        os._exit(1 if failed else 0)


if __name__ == "__main__":
    cuda_env()
    asyncio.run(main())
