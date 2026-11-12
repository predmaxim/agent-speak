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

from . import earlog, link, memory, wake
from .context import MIC_WATCH, foreign_capture, locked, mic_outputs, muted, on_ac
from .conversation import Conversation, gendered
from .orders import Orders
from .turn import SmartTurn
from .wake import strip_wake


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


def status_events(d, last):
    """Строка состояния службы → события Омы, только смены. Её звук — audio(speaking, played_at, voiced, user_paused):
    смена speaking или, пока служба молчит, рост voiced (реплика без звука — только код или эмодзи: звука не будет,
    played_at прежний) и смена паузы пользователя. Конец её речи решает Ома по voiced, а не по «не говорит»: повтор
    «не говорит» до начала звука — не конец; на паузе пользователя (кнопка «пауза») её фраза ждёт в очереди службы —
    тоже конец. Её собственная пауза при перебиве (paused без user_paused) — не конец: ответ по делу её снимает."""
    speaking, played, voiced, user_paused = bool(d.get("speaking")), d.get("played_at"), d.get("voiced"), bool(d.get("user_paused"))
    evs = [("audio", speaking, played, voiced, user_paused)] if last.get("speaking") != speaking or (
        not speaking and (last.get("voiced"), last.get("user_paused")) != (voiced, user_paused)) else []
    why = d.get("quiet_why") or ""  # тишина по причине (показ экрана): для разговора — тоже тишина
    for field, v in (("quiet", bool(d.get("quiet")) or bool(why)), ("quiet_why", why), ("out", d.get("out"))):
        if last.get(field) != v:
            evs.append((field, v))
        last[field] = v
    limit = d.get("limit") or ""  # фраза «лимит почти кончился»; нет поля (старая служба) — как пустая
    if last.get("limit", "") != limit:
        evs.append(("limit", limit))
    gender = "m" if d.get("gender") == "m" else "f"  # род Омы по голосу; нет поля (старая служба) — женский, как раньше
    if last.get("gender", "f") != gender:
        evs.append(("gender", gender))
    last["gender"] = gender
    remember = d.get("memory") is not False
    if last.get("memory") != remember:  # и первая строка: включили, пока Омы не было, — узнаётся по метке на диске
        evs.append(("memory", remember))
    last["memory"] = remember
    last.update(speaking=speaking, voiced=voiced, user_paused=user_paused, limit=limit)
    return evs


def kill_children():
    """SIGTERM ещё живым детям (claude, pw-record): os._exit минует atexit-сборщик Agent SDK."""
    pids = []
    for f in glob.glob("/proc/self/task/*/children"):  # дети всех потоков процесса
        with contextlib.suppress(OSError), open(f) as fh:
            pids += fh.read().split()
    for pid in pids:
        with contextlib.suppress(OSError):
            os.kill(int(pid), signal.SIGTERM)


async def mic_poll(oma, events, busy):
    """Одна проверка микрофона (по событию MicWatch): чужой захват или mute — слух не слушает, разговор на паузе. Событие —
    при смене и когда флаг слуха разошёлся с фактом: Ear.start сам ставит paused при mute, и mute, снятый раньше проверки,
    иначе оставил бы её глухой. Причина — в oma.mic_why: побудка называет её в строке состояния."""
    pid = oma.ear.pid  # до await: слух мог переоткрыться, а снимок pactl — со старым pw-record
    foreign = pid is not None and foreign_capture(await asyncio.to_thread(mic_outputs), {pid})
    mute = pid is not None and not foreign and await asyncio.to_thread(muted, oma.ear.target)
    oma.mic_why, now = "busy" if foreign else "muted" if mute else "", foreign or mute
    if now != busy or oma.ear.paused != now:
        events.put_nowait(("mic", now))
    return now

class Oma:
    """Связывает разговор, слух, мозг и демона: команды и события → действия Conversation → исполнение."""

    def __init__(self, conv, ear, brain, whisper, send=link.send, notify=notify, clock=time.monotonic, orders=None, turn_end=None,
                 events=None):
        self.conv, self.ear, self.brain, self.whisper, self.send = conv, ear, brain, whisper, send
        self.events = asyncio.Queue() if events is None else events  # очередь диспетчера: итоги фоновых задач — через неё
        self.outbox = None  # последняя задача очереди к мозгу (to_brain)
        self.orders = orders or Orders()
        self.orders.ask = self._ask    # вопрос подтверждения — голосом через Conversation
        self.utter, self.sending = [], []  # звук фраз черновика; при выбросе черновика — к отправке
        self.pre = self.pre_sending = None  # (число фраз, задача) — реплика целиком, распознаваемая заранее
        self.turn_end = turn_end or SmartTurn()
        self.notify, self.clock = notify, clock
        self.history = []  # текущий чат, как его видит окно: история из open() + реплики этого процесса
        self.n = 0         # номер ответа: пузырь oma-<pid>-<n>
        self.tasks = set()
        self.sound = 0     # номер события audio: хвост pw-cat отсчитывается от последнего
        self.says = 0      # её реплик (voice), принятых сокетом службы: «говорю» кончается, когда служба отработала все (voiced)
        # shortcut: voice, легший в буфер сокета, который служба закрыла непрочитанным, навсегда держит says впереди voiced —
        # до перезапуска Омы каждый ответ держит её в «говорю», пока его не оборвут (Esc, клик, пауза, перебив); случится — номер последней реплики в voice
        # и в строке состояния вместо счёта (потеря одной самоисправится на следующей)
        self.out = None    # куда идёт её голос (out службы): speakers | headphones
        self.why = ""      # причина тишины (quiet_why службы): "" | dnd | screencast
        self.limit = ""    # фраза службы «лимит почти кончился» (limit); "" — окна у порога нет
        self.gender = "f"  # её род по голосу (gender службы): "f" | "m"; его же знают мозг и поручения
        self.notified = (None, 0.0)  # последнее уведомление: текст, время
        self.ask_id = None  # id набранной реплики из окна, пока команда ask идёт: её строка в чате вернётся с ним
        self.ear_sent = ("", None, "off")  # (вход, quiet_at, побудка) последней строки ear службе: как у неё до первой строки
        self.wake_on = False  # служба разрешила слово-побудку ({"do": "wake"}): настройка, не тихо, экран не заблокирован
        self.ac = None        # от сети (power, раз в 30 с); побудка — только от сети
        self.has_model = None  # модель Vosk и колесо стоят (wake.available) — узнаётся при первой побудке
        self.no_mic = False   # для побудки нет встроенного микрофона
        self.mic_why = ""     # микрофон: "" | busy (пишет другой) | muted — по последней проверке MicWatch
        self.mic_now = False  # последняя проверка: слух не слушает

    def _show(self, text):
        self.emit({"bubble": {"id": f"oma-{os.getpid()}-{self.n}", "who": "Ома", "text": text, "kind": "oma", "append": True}})
        self._chat("oma", text)

    def _ask(self, text, order):
        """Вопрос подтверждения из инструмента мозга (задача SDK) — в очередь диспетчера, по порядку с ходом мозга;
        order — поручение, которое будет ждать фразы, если вопрос прозвучит."""
        self.events.put_nowait(("confirm", text, order))

    def emit(self, line):
        self.send({"cmd": "emit", "line": line})

    def bg(self, coro):
        """Фоновая задача: ссылку держим (иначе её может собрать GC), сбой — в журнал."""
        t = asyncio.create_task(coro)
        self.tasks.add(t)
        t.add_done_callback(self._bg_done)
        return t

    def to_brain(self, fn, *args, turn=0):
        """В очередь к мозгу: переход в чат, прогрев, реплика с доставкой поручения и перебив идут по порядку в своей
        задаче — диспетчер их не ждёт: Esc, клик и «стоп» работают, пока claude поднимается или поручение уходит.
        Сбой — событием error хода turn (0 — не ход): разговор не остаётся в «думаю», ошибка — в уведомление."""
        prev = self.outbox

        async def run():
            if prev and not prev.done():
                await asyncio.wait([prev])  # упавшая предыдущая очередь не держит
            try:
                await fn(*args)
            except Exception as e:
                self.events.put_nowait(("error", f"Claude: {e!r}", turn))
        self.outbox = self.bg(run())

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

    def _chat(self, role, text, id=None):
        """Реплика — в историю текущего чата и в окно; ответ Омы — одним сообщением, как в окне. id — набранной
        реплики: окно узнаёт, что Ома её приняла, по нему, а не по совпадению текста."""
        h = self.history
        if role == "oma" and h and h[-1]["role"] == "oma":
            h[-1]["text"] += " " + text
        else:
            h.append({"role": role, "text": text})
        self.emit({"chat_delta": {"key": self.brain.key, "role": role, "text": text, **({"id": id} if id else {})}})

    async def command(self, cmd):
        do, c = cmd.get("do"), self.conv
        if do == "talk":
            self.ear.speakers = None  # разговор по клику — вход по тому, куда она говорит
            key = cmd.get("key")  # удержание спутника орба — разговор с этим агентом, в его чате
            leave = not key and c.state == "idle" and self.brain.key.startswith("agent:")  # с Омой-помощником — в общем чате
            # слух — сразу: смена чата и выход прежнего claude начало фразы не задерживают; удержание, пока она слушает,
            # разговор не кончает (клик кончил бы), а переводит в чат агента — и окно ответа делает обычным разговором;
            # пока она думает или говорит ответ окна, удержание обрывает его, как клик, и окно снимает тоже
            await self.act(c.converse() if key and c.state in ("listening", "paused") else c.talk() + (c.converse() if key else []))
            await self.follow(await self.general() if leave else (key if key != self.brain.key else None))
            if c.active:
                self.to_brain(self.brain.warm)  # claude поднимается, пока пользователь говорит
        elif do == "cancel":  # sound false — от кнопки наушников: звук служба уже остановила
            await self.act(c.cancel(cmd.get("sound", True) is not False))
        elif do == "here":  # «Ты тут?» решён кнопкой наушников — окно вопроса не нужно
            await self.act(c.answered())
        elif do == "warm":  # демон прогревает слух до окна ответа: whisper не грузится при открытом микрофоне
            self.warm()
        elif do == "wake":  # служба: побудку можно (настройка, не тихо, экран не заблокирован) или нельзя
            self.wake_on = bool(cmd.get("on"))
            await self._wake_sync()
        elif do == "rewire":  # наушники вставили или вынули: в динамиках её голос побудкой не слушается — пересчитать
            if getattr(self.ear, "waking", False):
                await self._wake_open()
        elif do == "listen":
            answered = False
            try:
                # экран заблокирован или микрофон держит чужой (conv.mic при закрытом слухе не знает: pid нет) — не открываем
                if not await asyncio.to_thread(locked) and not foreign_capture(await asyncio.to_thread(mic_outputs), set()):
                    self.ear.speakers = True if cmd.get("speakers") else None  # «Ты тут?» в динамики — и слушать ноутбуком
                    key = cmd.get("key")  # окно после вопроса — ответ в чат спросившего агента
                    idle = c.state == "idle"
                    acts = c.listen(float(cmd.get("secs", 5)), cmd.get("kind", "news"), cmd.get("said", ""))
                    await self.act(acts)
                    answered = any(a[0] == "state" for a in acts)
                    if idle and c.active:  # окно ответа открылось — разговор начался
                        await self.follow(key if key != self.brain.key else None)
                    if key and c.active:
                        self.to_brain(self.brain.warm)  # ответ агенту: claude поднимается, пока окно слушает
            finally:
                # ответ на каждый listen — и на отказ и сбой: служба держит музыку на паузе, пока его ждёт (media_hold)
                if not answered:
                    await self.act([("state", c.state)])
        elif do == "chats":
            self.emit({"chats": await asyncio.to_thread(self.brain.chats)})
        elif do in ("memory", "memory_set", "memory_del"):  # страница «Память» окна: вид; правка «Помню» и удаление — и новый вид
            if do == "memory_set" and cmd.get("kind") == "pref":
                await asyncio.to_thread(memory.set_pref, str(cmd.get("id", "")), str(cmd.get("text", "")))
            elif do == "memory_del":
                await asyncio.to_thread(memory.drop, cmd.get("kind"), str(cmd.get("id", "")))
            self.emit({"memory": await asyncio.to_thread(memory.view)})
        elif do == "open" and not cmd.get("key"):  # окно спрашивает текущий чат: без переключения, was "*" — принять его ключ
            self.emit({"chat": {"key": self.brain.key, "was": "*", "messages": self.history}})
        elif do == "open":  # окно смотрит чат (клик по агенту): мозг не переключается — её ответ не обрывается, claude живёт
            want = cmd.get("key")
            key = await self.general() if want == "general" else want
            was = "general" if want == "general" else None  # окно ждёт ответа на «general» и примет настоящий ключ
            msgs = self.history if key == self.brain.key else await asyncio.to_thread(self.brain.messages, key)
            self.emit({"chat": {"key": key, **({"was": was} if was else {}), "messages": msgs}})
        elif do in ("ask", "new"):  # реплика в чате или новый чат — мозг переходит туда
            want = cmd.get("key")
            key = "new" if do == "new" else await self.general() if want == "general" else want or self.brain.key
            was = "general" if want == "general" else None  # окно ждёт ответа на «general» и примет настоящий ключ
            if key != self.brain.key:
                await self.switch(key, was)
            elif do == "new":
                self.emit({"chat": {"key": key, "messages": self.history}})
            if do == "ask":
                self.ask_id = cmd.get("id")
                await self.act(c.ask(cmd.get("text", "")))
                self.ask_id = None

    async def follow(self, key=None):
        """Разговор начался — окно идёт за ним (was "*"): в чат key, куда переходит мозг, иначе в текущий чат мозга —
        окно могло смотреть другой (open только показывает), и chat_delta разговора оно отсеивало бы по ключу."""
        if key:
            await self.switch(key, "*")
        elif self.conv.active:
            self.emit({"chat": {"key": self.brain.key, "was": "*", "messages": self.history}})

    async def switch(self, key, was=None):
        """Перейти в чат key; was — ключ, на который окно ждёт ответа ("*" — любой). Идёт разговор — всегда "*": голос
        теперь уходит туда, и орб по такой строке переносит обводку спутника (окно её тоже примет)."""
        if self.conv.turn:  # ход прошлого чата обрываем, пока его клиент жив
            await self.act(self.conv.cancel())
        self.to_brain(self._open, key, "*" if self.conv.active else was)

    async def _open(self, key, was):
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

    async def _tail_over(self, sound, until):
        await asyncio.sleep(max(0.0, until - time.monotonic()))  # played_at — те же часы CLOCK_MONOTONIC
        if self.sound == sound:  # канал pw-cat доиграл, новой речи не было — дальше сырой микрофон
            self.ear.speaking = False

    async def event(self, ev):
        kind, c = ev[0], self.conv
        if kind in ("delta", "done") and len(ev) > 2 and ev[2] != c.turn_no:
            return  # итог реплики из очереди к мозгу, а ход уже другой: чужой ход он не закрывает
        if kind == "start":
            await self.act(c.speech_start(*ev[1:]))
        elif kind == "phrase":
            pcm, before = getattr(self.ear, "audio", {}).pop(ev[2], b""), c.draft
            acts = c.phrase(ev[1], ev[2])
            if pcm and c.draft != before:  # фраза вошла в черновик; начало срезано как эхо — её звук целиком не распознавать
                c.turn_end(await self._turn_done(pcm))  # Smart Turn — только по принятой: эхо и непринятое не судит
                # сверка — с тем, что вошло в черновик: обращение «Ома» Conversation.phrase срезает
                self.utter.append(pcm if c.draft.endswith(strip_wake(ev[1]).strip()) else None)
                self._prehear()
            verdict = "принята" if c.draft != before else ("эхо" if ev[2] == c.echo_n else "не принята")
            if ev[1].strip():  # без текста: в нём бывает чужой разговор в комнате
                print(f"agent_voice: слух: фраза {len(ev[1].split())} сл., {verdict}", file=sys.stderr, flush=True)
            earlog.verdict(ev[2], verdict=verdict, draft=c.draft, state=c.state, her_words=c.said[-300:],
                           window=(c.window or {}).get("kind"), started_under_her_voice=ev[2] == c.echo_n)
            await self.act(acts + c.tick())  # срок отправки уже настал (Smart Turn уверен) — не ждать тика (до 0,2 с)
        elif kind == "tick":
            await self.act(c.tick())
        elif kind == "locked":
            await self.act(c.end())
        elif kind == "delta":
            await self.act(c.delta(ev[1]))
        elif kind == "tool":
            await self.act(c.tool(ev[1]))
        elif kind == "ask_again":  # переспрос подтверждения кодом: вопрос того же хода, поручение уже ждёт
            await self.act(c.question(ev[1]) if ev[2] == c.turn_no else [])
        elif kind == "confirm":  # вопрос — только в ходе, который вызвал инструмент; не прозвучал — поручение не ждёт
            acts = c.question(ev[1]) if ev[2]["n"] == c.turn_no else []
            if acts:
                self.orders.pending = ev[2]
                self._show(ev[2]["shown"])  # полный текст, который уйдёт, — в окно рядом с вопросом
            await self.act(acts)
        elif kind == "done":
            await self.act(c.done())
            self.emit({"chat": {"key": self.brain.key, "messages": self.history}})
            self.emit({"chats": await asyncio.to_thread(self.brain.chats)})
        elif kind == "audio":
            # её голос звучит: в динамиках слух — на входе с эхоподавлением; «смолкла» служба шлёт, записав
            # последний кусок, а pw-cat доигрывает канал до played_at — сырой микрофон слышал бы его без эхоподавления
            until = ev[2] if len(ev) > 2 else None
            voiced = ev[3] if len(ev) > 3 else None  # нет счётчика — конец её речи по «не говорит», как раньше
            user_paused = len(ev) > 4 and ev[4]  # пауза пользователя (не её): фраза ждёт в очереди службы, а Ома слушает с таймером тишины
            self.sound += 1
            if ev[1]:
                self.ear.speaking = True
            else:
                self.bg(self._tail_over(self.sound, until or 0.0))
            # >=, а не ==: лишняя засчитанная реплика оставила бы её в «говорю» навсегда, а не на один ответ раньше
            await self.act(c.audio(ev[1], until, voiced is None or voiced >= self.says or user_paused))
        elif kind == "quiet":
            await self.act(c.set_quiet(ev[1]))
        elif kind == "out":
            self.out = ev[1]
        elif kind == "quiet_why":
            self.why = ev[1]
        elif kind == "limit":
            self.limit = ev[1]
        elif kind == "gender":  # голос сменили: её слова о себе — в его роде, мозгу — со следующей реплики
            self.gender = self.brain.gender = self.orders.gender = ev[1]
        elif kind == "memory":  # сказанное при выключенной памяти в заметки дней не попадёт
            self.bg(asyncio.to_thread(memory.switched, ev[1]))
        elif kind == "mic":
            self.ear.paused = ev[1]  # микрофон пишет другой (voxtype и т. п.) — кадры не слушаем
            # в покое (слух на имя) — только слух: окно ответа открывается, как раньше, и уже на паузе
            await self.act(c.mic_busy(ev[1]) if c.active else [])
        elif kind == "mic_check":  # MicWatch: событие pactl — проверить захват и mute
            self.ear.mic_pending = False
            self.mic_now = await mic_poll(self, self.events, self.mic_now)
            self._ear_line()
        elif kind == "ac":  # power, раз в 30 с: от сети ли — побудка открывает или закрывает слух
            self.ac = ev[1]
            await self._wake_sync()
        elif kind == "wake":
            await self._woke(ev[1], ev[2])
        elif kind == "opened":
            self.emit({"chat": {"key": ev[1], "was": ev[2], "messages": self.history}})
        elif kind == "wake_died":  # слух на имя умер сам (перезапуск звука): тик ac откроет снова, орб — «сбой слуха»
            print(f"agent_voice: слово-побудка: {ev[1]}", file=sys.stderr, flush=True)
            self._ear_line()
        elif kind == "error":
            self._warn(ev[1])
            if not self.ear.open and c.active:  # слух умер сам
                await self.act(c.end())
            elif c.turn and ev[1].startswith("Claude") and (len(ev) < 3 or ev[2] == c.turn_no):  # ход мозга оборвался — в покой
                await self.act(c.done())

    async def _turn_done(self, pcm):
        """Договорил ли: Smart Turn по всей реплике (фразы черновика и эта); сбой — None, пауза как раньше."""
        try:
            p = await asyncio.to_thread(self.turn_end.complete, b"".join([p for p in self.utter if p] + [pcm]))
            if p is not None:  # для подстройки порогов DONE/UNDONE; текста нет
                print(f"agent_voice: конец реплики {p:.2f}", file=sys.stderr, flush=True)
            return p
        except Exception as e:
            print(f"agent_voice: конец реплики: {e!r}", file=sys.stderr, flush=True)
            return None

    def _prehear(self):
        """Реплика из нескольких фраз распознаётся целиком сразу, пока идёт пауза: отправка не ждёт whisper (~0,3 с)."""
        if self.pre:
            self.pre[1].cancel()
        parts, self.pre = self.utter, None
        if len(parts) >= 2 and None not in parts:
            t = asyncio.ensure_future(self.hear(b"".join(parts)))
            t.add_done_callback(lambda t: t.cancelled() or t.exception())  # сбой заберёт _whole; брошенная — без шума
            self.pre = (len(parts), t)

    async def hear(self, pcm):
        """Реплика целиком: whisper по кускам между паузами теряет смысл на краях («этот спик» вместо «AgentSpeak»)."""
        from .ear import transcribe
        return await asyncio.to_thread(transcribe, await self.whisper.get(), pcm)

    async def _whole(self, text, parts, pre):
        """Текст реплики, сказанной голосом: несколько фраз — распознать звук целиком; не вышло — как в черновике."""
        if len(parts) < 2 or None in parts:  # None — фраза с её голосом в начале: целиком эхо вернулось бы
            return text
        try:
            # целиком — снова с обращением: разговор с побудки начинается с «Ома, …»
            whole = strip_wake(await (pre[1] if pre and pre[0] == len(parts) else self.hear(b"".join(parts))))
        except Exception as e:
            print(f"agent_voice: реплика целиком: {e!r}", file=sys.stderr, flush=True)
            return text
        earlog.whole(text, whole)
        if whole and whole != text:  # сам текст в журнал не пишем: в нём бывает чужой разговор в комнате
            print(f"agent_voice: реплика целиком распознана иначе, чем по кускам ({len(whole)} и {len(text)} знаков)", file=sys.stderr, flush=True)
        return whole or text

    def _sound(self):
        """Её звук одной строкой для мозга: тихий режим, его причина и куда идёт голос — из строки состояния службы
        (quiet, quiet_why, out): мозг не выдумывает причину молчания; в конце строки — её род по голосу (gender): смена
        голоса посреди чата действует со следующего ответа; окно лимита у порога — следующей строкой фраза службы (limit)."""
        if self.conv.quiet:
            why = {"screencast": "показ экрана", "dnd": "не беспокоить"}.get(self.why, "тихий режим")
            sound = f"Твой голос: выключен ({why}), ответы только текстом в окне."
        else:
            where = {"speakers": "в динамиках", "headphones": "в наушниках"}.get(self.out)
            sound = f"Твой голос: включён, звучит {where}." if where else "Твой голос: включён."
        sound += gendered(" Ты говоришь о себе в {мужском|женском} роде.", self.gender)
        return f"{sound}\n{self.limit}" if self.limit else sound

    async def _deliver(self, text, ctx, voiced, parts, pre, sound, n, id=None):
        """Реплика мозгу или подтверждение поручения, которое передаёт код, — в очереди к мозгу; n — номер её хода,
        id — набранной реплики из окна."""
        key, failed = self.brain.key, None
        text = await self._whole(text, parts, pre) if voiced else text
        self.orders.turn(voiced, text, n)
        said = await self.orders.confirm(text)  # подтверждение сверяет код, мозг не зовём; другая реплика снимает ожидание
        if said is None:
            note, self.orders.note = self.orders.note, ""
            try:
                await self.brain.send(note + ctx + text, sound=sound)
            except Exception as e:
                failed = e
        if key.startswith("chat:") and self.brain.key == "new":  # устаревший чат сменился новым: реплика — первая в нём
            self.history = []
        self.n += 1
        self._chat("me", text, id)
        self.emit({"bubble": {"id": f"me-{os.getpid()}-{self.n}", "who": "Ты", "text": text, "kind": "me", "append": False}})
        if failed:  # ход уже в «thinking», done() вернёт в покой
            self.events.put_nowait(("error", f"Claude: {failed}", n))
        if said is not None:  # фраза кода вместо ответа мозга — как его ход
            # «Не расслышала. Скажи: да, передай» — снова вопрос подтверждения: окно ответа его тоже ждёт (Conversation.question)
            self.events.put_nowait(("ask_again" if self.orders.pending else "delta", said, n))
            self.events.put_nowait(("done", None, n))

    def _ear_line(self):
        """Орбу — её вход и сколько она ещё слушает: строка ear службе при смене входа или срока тишины (quiet_at).
        Длина окна — от сейчас до срока: строка уходит сразу после действия, которое его взвело."""
        q = self.conv.quiet_at
        target, woke = getattr(self.ear, "target", ""), self._wake_state()
        if (target, q, woke) == self.ear_sent:
            return
        self.ear_sent = (target, q, woke)
        left = None if q is None else max(0.0, q - self.conv.clock())
        self.send({"cmd": "ear", "ear_in": target, "until": None if left is None else time.time() + left, "secs": left or 0.0,
                   "wake": woke})

    def _model(self):
        if self.has_model is None:  # один раз: модель ставит install.sh, а после него службу перезапускают
            self.has_model = wake.available()
        return self.has_model

    def _wake_ready(self):
        return self.wake_on and self.ac is not False and self._model()

    def _wake_state(self):
        """Побудка для строки состояния: on или почему нет; error — в покое слух на имя не открылся или умер сам."""
        if not self.wake_on:
            return "off"
        if not self._model():
            return "no_model"
        if self.ac is False:
            return "battery"
        if self.no_mic:
            return "no_mic"
        if not self.conv.active and not (self.ear.open and self.ear.waking):
            return "error"
        return self.mic_why or "on"

    async def _wake_open(self):
        self.ear.speakers = None  # слух на имя — по сервису присутствия: «Ты тут?» в динамики (listen) — не его случай
        try:
            await asyncio.to_thread(self.ear.wake_load)
        except Exception as e:  # модель Vosk не грузится — побудки нет до перезапуска Омы, слух — как раньше
            self.has_model = False
            self._warn(f"слово-побудка: модель Vosk: {e!r}")
            await self.ear.stop()
            return
        try:
            self.no_mic = not await self.ear.start(wake=True)
        except Exception as e:  # pw-record, pactl, VAD — повтор на следующем тике ac (раз в 30 с)
            self._warn(f"слово-побудка: {e!r}")
            await self.ear.stop()

    async def _wake_sync(self):
        """Слух в покое: открыт на «Ома», если побудка разрешена и можно, иначе закрыт; разговор слух не отдаёт."""
        if not self.conv.active:
            if self._wake_ready():
                if not (self.ear.open and self.ear.waking):  # открыт и слушает имя — pw-record не трогать
                    await self._wake_open()
            elif self.ear.open:
                await self.ear.stop()
        self._ear_line()

    async def _woke(self, text, at):
        """Вторая ступень подтвердила «Ома»: «тут», «да» — «я на месте» без разговора; иначе разговор, как по клику, на том
        же входе — фразу с именем слух распознает заново целиком (take). Безопасность поручений та же: реплика голосовая."""
        c = self.conv
        if c.active or not self.ear.waking or not self.wake_on or await asyncio.to_thread(locked):
            return  # пока думала вторая ступень — клик, окно ответа, тишина или блокировка
        print("agent_voice: слово-побудка", file=sys.stderr, flush=True)  # текст — нет: в нём бывает чужая речь
        leave = self.brain.key.startswith("agent:")  # с Омой-помощником — в общем чате, как по клику
        acts = c.wake(strip_wake(text))
        if c.active:
            await self.ear.take(at)
            c.echo = self.ear.echo
        await self.act(acts)
        if c.active:
            await self.follow(await self.general() if leave else None)
            self.to_brain(self.brain.warm)

    async def act(self, acts):
        try:
            await self._act(acts)
        finally:
            self._ear_line()

    async def _act(self, acts):
        for a in acts:
            k = a[0]
            if k == "ear":
                if a[1]:
                    await self.ear.start()
                    self.conv.echo = getattr(self.ear, "echo", False)  # в динамиках её голос просачивается в слух
                    if self.conv.mic != self.mic_now:  # слух на имя уже знал: mute или чужая запись — разговор на паузе
                        await self._act(self.conv.mic_busy(self.mic_now))
                    if not self.whisper.ready:  # от батареи: грузится по первому включению слуха
                        self.warm()
                elif self._wake_ready():  # конец разговора — слух снова слушает имя
                    await self._wake_open()
                else:
                    await self.ear.stop()
            elif k == "send":
                if self.conv.voiced and not self.conv.quiet:
                    self.send({"cmd": "chime"})  # «услышала» сразу: до ответа мозга секунды
                parts, self.sending = self.sending, []  # звук этой реплики — её: следующий черновик его не тронет
                pre, self.pre_sending = self.pre_sending, None
                n = self.conv.turn_no
                self.to_brain(self._deliver, a[1], a[2], self.conv.voiced, parts, pre, self._sound(), n, self.ask_id, turn=n)
            elif k == "presence":
                self.send({"cmd": "presence", "state": a[1]})
            elif k == "chime":  # побудка услышала имя
                self.send({"cmd": "chime"})
            elif k == "interrupt":  # после отправки, что ещё в очереди: перебить ход, а не реплику до него
                self.to_brain(self.brain.interrupt)
            elif k == "stop":  # перебивание: финалы агентов, удержанные за Омой, демон не выбрасывает
                self.send({"cmd": "hush"})
            elif k == "pause_voice":  # перебив: её речь на паузе / дальше — решает распознанный текст
                self.send({"cmd": "oma_pause", "on": a[1]})
            elif k == "halt":  # «отбой»: замолчать совсем — и её ответ, и чтение, и новости
                self.send({"cmd": "stop"})
            elif k == "say":
                if self.send({"cmd": "voice", "text": a[1]}) is not False:  # не дошла — служба её и не засчитает
                    self.says += 1
            elif k == "show":
                self._show(a[1])
            elif k == "draft":
                self.emit({"bubble": {"id": "draft", "who": "Ты", "text": a[1], "kind": "draft", "hearing": a[2], "append": False}})
            elif k == "drop_draft":
                self.utter, self.sending = [], self.utter  # отправка следом заберёт звук; без неё — забыт новым черновиком
                self.pre_sending, self.pre = self.pre, None
                self.emit({"bubble": {"id": "draft", "who": "Ты", "text": "", "kind": "draft", "append": False, "drop": True}})
            elif k == "state":
                s = "loading" if a[1] == "listening" and not self.whisper.ready else a[1]
                self.send({"cmd": "voice_state", "state": s, "talking": self.conv.active})


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
        oma = Oma(Conversation(), Ear(events, whisper, wake=wake.VoskWake(), watch=MIC_WATCH), Brain(events, lambda: agents, send=link.send, orders=orders, fresh=link.turn), whisper,
                  orders=orders, events=events)

        def on_agents(lst):
            agents[:] = lst  # снимок для инструментов мозга

        last = {}

        def on_status(d):
            oma.brain.memory = d.get("memory") is not False  # настройка памяти — с нового подключения мозга
            for ev in status_events(d, last):
                events.put_nowait(ev)

        async def daemon():
            with contextlib.suppress(OSError):  # демона ещё нет или он закрыл сокет — keep переподключит
                await link.watch(on_status, on_agents)

        async def power():
            while True:
                ac = on_ac()
                events.put_nowait(("ac", ac))  # побудка от сети; заодно слух на «Ома», упавший сам, поднимется
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

        reader = asyncio.StreamReader(limit=1 << 22)  # ask со вставленным логом бывает длинным
        await loop.connect_read_pipe(lambda: asyncio.StreamReaderProtocol(reader), sys.stdin)
        loop.add_signal_handler(signal.SIGTERM, reader.feed_eof)  # voice_stop: как EOF
        loops = [asyncio.create_task(keep(n, f)) for n, f in (("daemon", daemon), ("power", power), ("ticker", ticker), ("lock", lock))]
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
