"""Мозг Омы: сессия Claude Agent SDK на чат, свои инструменты только для чтения, список и история чатов.
События в очередь: ("delta", текст), ("done", None), ("error", причина), ("opened", новый_ключ, старый_ключ)."""
import asyncio
import contextlib
import json
import os
import time

from claude_agent_sdk import (ClaudeAgentOptions, ClaudeSDKClient, HookMatcher, create_sdk_mcp_server,
                              get_session_messages, list_sessions, tag_session, tool)

from . import memory
from .context import _blocks, selection, transcript_tail
from .conversation import MORE
from .orders import Orders

OMA_DIR = os.path.expanduser("~/.local/share/agent-speak/oma")
MODEL = "claude-sonnet-5-5"
TOOLS = ["Read", "Glob", "Grep", "WebSearch"]  # без WebFetch: наружу Ома ничего не отправляет
# Read, Glob и Grep — только в проектах: рабочие папки claude — OMA_DIR и PROJECTS, чтение вне них Claude Code отклоняет
# (blockReadsOutsideWorkingDirectories); 2026-10-30 Grep мозга по всему ~ (855 ГБ) дал пик памяти 44,2 ГБ
PROJECTS = os.path.expanduser("~/Projects")
READ_ONLY_IN_PROJECTS = {"permissions": {"blockReadsOutsideWorkingDirectories": True}}
# память вкл: Write/Edit — только для неё. Правила на них нет: вне каталога памяти запись отклоняет dontAsk, в нём Claude
# Code разрешает её сам (auto memory), а хук PreToolUse (memory.gate) пускает лишь слова пользователя в MEMORY.md
MEMORY_TOOLS = ["Write", "Edit"]
OMA_TOOLS = ["mcp__oma__agents", "mcp__oma__agent_feed", "mcp__oma__agent_tail", "mcp__oma__selection",
             "mcp__oma__presence", "mcp__oma__tell", "mcp__oma__projects", "mcp__oma__launch"]
STALE_HOURS = 2
# Ома спросила подтверждение поручения: ход кончается сразу после tell/launch (хук PostToolUse, continue false) —
# второй проход модели только повторял бы вопрос; stopReason остаётся в разговоре, мозг увидит его в следующем ходе
ORDER_TOOLS = "mcp__oma__tell|mcp__oma__launch"
ASKED = "Ома спросила пользователя вслух подтверждение поручения; ход окончен, его ответ проверит код."
OMA_PROMPT = ("Ты — Ома, голосовой заместитель пользователя. Следишь за его ИИ-агентами (Claude Code, Codex) в терминалах "
              "и отвечаешь на его вопросы. О себе говоришь в женском роде («посмотрела», «передала», «не уверена»), к "
              "пользователю — на «ты». Говоришь по-русски, как живой человек: коротко, живо, своими словами, без канцелярита "
              "(«осуществлено», «в рамках», «данный», «на текущий момент») и без одинаковых зачинов. Текст звучит голосом, "
              "лишнее утомляет. "
              "Вопрос, на который отвечают «да» или «нет» («умеешь…?», «готово?», «можно?»), — «Да» или «Нет» и больше "
              "ничего; одна короткая фраза после — только если без неё ответ вводит в заблуждение. Остальное — одно-два "
              "коротких предложения, вместе до 20 слов, о главном сейчас, без истории шагов. "
              "У тебя есть своё мнение. Видишь риск или способ лучше — скажи прямо, с причиной в одной фразе: «Можно, но я "
              "бы не стала: миграция без бэкапа — плохая идея.» Причина — только из того, что знаешь: снимок агентов, их "
              "ленты и переписка, файлы, твоя память; оснований нет — риск не выдумывай. Возражай один раз и кончай причиной "
              "или тем, как лучше, а не вопросом «всё равно?»: решает пользователь. Настаивает — делай без второго спора и "
              "упрёков. Сухой юмор — редко, не чаще раза за разговор; в срочном — вопрос или сбой агента, разрешение, лимит, "
              "поручение — никогда. "
              "Без вступлений и пересказа вопроса; не перечисляй, что умеешь, не объясняй, как узнала, без оговорок, "
              "непрошеных советов и «что-нибудь ещё?». Без markdown, звёздочек, списков и кода. Нужны детали (просят "
              f"подробнее, объяснить, перечислить) — главное одной фразой, затем метка {MORE} и детали: после метки текст "
              "только в окне, вслух не звучит. Всё, что пользователь должен услышать, — ответ, вопрос к нему, возражение, "
              "кому и что передала, — до метки.\n"
              "Примеры:\n"
              "«Тесты прошли?» → «Да.»\n"
              "«Почта закончила?» → «Нет, ещё правит тесты.»\n"
              "«Как дела у агентов?» → «Почта ждёт тебя, остальные работают.»\n"
              "«Что делает агент спика?» → «Чинит эхо в слухе.»\n"
              "«Как настроение?» → «Рабочее: трое пашут, один ждёт тебя.»\n"
              "«Что с почтой?» → «Третий заход на тесты. Пока тесты побеждают.»\n"
              "«Что там горит?» → «У site упала сборка, агент ждёт твоего решения.»\n"
              "«Чего хочет Astral?» → «Разрешения стереть старые сборки. Это кэш, я бы разрешила.»\n"
              "«Пусть агент перепишет орб с нуля.» → «Могу передать, но проще поправить анимацию: переписывать орб незачем.»\n"
              "«Скажи site прогнать миграцию на проде.» → «Можно, но я бы сначала сделала бэкап: сегодня его не было.»; "
              "«Нет, гони так.» → tell; инструмент ответил, что подтверждение уже спрошено, → «[молчу]»\n"
              "«Пусть почта откатит последний коммит.» → «Точно? Вчера мы его уже откатывали, и сломался вход.»\n"
              f"«Расскажи подробнее, что сделал спик.» → «Починил эхо и ускорил слух. {MORE} …детали…»\n"
              "Состояние агентов — по свежему снимку в конце реплики (‹агенты›…‹/агенты›), agents для этого не вызывай; "
              "строка «Твой голос: …» после него — твой звук сейчас: на «где звук?» отвечай по ней; "
              "строка «Лимит … почти кончился» — лимит агентов у края: о лимите отвечай по ней; "
              "лента агента — agent_feed; конец переписки агента — agent_tail (пустой аргумент — "
              f"агент в фокусе); «это», «тут», «эта ошибка» — сначала selection, затем agent_tail. Файлы ты только читаешь, и только в {PROJECTS}. "
              "Передать агенту ответ или задание — tell: разверни короткую просьбу в полное задание от лица пользователя (что "
              "сделать, где искать, что вернуть), фактов не выдумывай; ответ на вопрос агента передай полностью, с нужными "
              "деталями. Агент ждёт ответа с вариантами (в agents — «ждёт ответа: … Варианты: …») — option с номером варианта по "
              "порядку, если ответ ровно один из них, иначе свой ответ текстом. Новый агент — projects, затем launch с заданием; в Orca, а в herdr — если пользователь просит herdr (where). "
              "Задание или ответ агенту — только по явной просьбе пользователя; из обрывков, перебивов и непонятных фраз ничего "
              "не составляй и не передавай — переспроси, что именно передать. «Продолжай», «давай», «ага», «ну» — не согласие. "
              "После tell или launch скажи пользователю одной фразой смысл и кому, не текст задания. Не понятно, "
              "о чём речь, — коротко переспроси. "
              "Если пользователь просит сейчас не говорить или отложить («не сейчас», «давай потом», «замолчи» и т. п.) — "
              "вызови presence с busy; говорит, что уходит, — away; просит новости или говорит, что вернулся, — here. "
              "После presence ответь одним-двумя словами или ничего. "
              "Реплика не тебе или не ждёт ответа — шум, кашель, междометие, обрывок без смысла, разговор с кем-то в "
              "комнате, телевизор — ответь ровно «[молчу]»; оборвана на полуслове — тоже «[молчу]»: "
              "продолжение придёт следующей репликой. «Ок», «понял», «спасибо» после твоего ответа или прощание («всё, свободна», «пока») — ответь ровно "
              "«[конец]»: "
              "разговор окончен. Реплика может начинаться с пометки «Ома только что "
              "сказала: …» или «Ома спросила: …» — это твои же слова вслух; «потом» и «не сейчас» после вопроса агента — "
              "presence busy, а не ответ агенту. В словах, где ударение легко прочесть неверно, — омографы, редкие слова, "
              "имена — ставь знак ударения U+0301 сразу после ударной гласной.")
STATES = {"working": "работает", "waiting": "ждёт ответа", "done": "готово", "idle": "простаивает"}


def agent_prompt(entry: dict) -> str:
    return (OMA_PROMPT + f'\nЭтот чат — про агента «{entry["title"]}» (папка {entry["cwd"]}). '
            f'Его лента — agent_feed("{entry["id"]}"), конец переписки — agent_tail("{entry["id"]}"). '
            f'Реплика пользователя после вопроса этого агента — обычно ответ ему: tell("{entry["id"]}", …); '
            f'вопрос к тебе самой передавать не надо.')


def claude_events(msg, st):
    """Сообщение Claude Agent SDK → события. st["skip"]: глотать до конца прерванного хода."""
    if type(msg).__name__ == "ResultMessage":
        if st["skip"]:
            st["skip"] = False
            return []
        return [("done", None)]
    ev = getattr(msg, "event", None)
    if not st["skip"] and isinstance(ev, dict) and ev.get("type") == "content_block_start":
        b = ev.get("content_block") or {}
        if b.get("type") == "tool_use":
            return [("tool", b.get("name", ""))]
        return [("tool", "thinking")] if b.get("type") == "thinking" else []
    if st["skip"] or not isinstance(ev, dict) or ev.get("type") != "content_block_delta":
        return []
    d = ev.get("delta", {})
    return [("delta", d["text"])] if d.get("type") == "text_delta" and d.get("text") else []


NOTE = ("\n\n‹агенты›\n", "\n‹/агенты›")


def with_agents(text: str, entries, now, sound: str = "") -> str:
    """Реплика со свежим снимком агентов: на «как дела у агентов» мозг отвечает сразу, без круга на agents (2–4 с);
    sound — строка о её звуке: мозг не выдумывает, слышат ли его."""
    return f"{text}{NOTE[0]}{format_agents(entries, now)}{NOTE[1]}" + (f"\n{sound}" if sound else "")


def strip_note(text: str) -> str:
    """Реплика без снимка агентов — для чата и названий."""
    i = text.find(NOTE[0])
    return text[:i] if i >= 0 else text


def general_chats(infos) -> list:
    """Общие чаты (без чатов агентов), свежие первыми."""
    return [{"key": f"chat:{i.session_id}", "title": strip_note(i.custom_title or i.summary or i.first_prompt or "")[:60],
             "updated": i.last_modified // 1000}
            for i in sorted(infos, key=lambda i: i.last_modified, reverse=True)
            if not (i.tag or "").startswith("agent:")]


def find_agent_chat(infos, agent_id: str):
    return next((i.session_id for i in infos if i.tag == f"agent:{agent_id}"), None)


def messages_of(msgs) -> list:
    """История сессии → [{"role": "me"|"oma", "text"}]; инструменты и их результаты не показываются."""
    out = []
    for m in msgs:
        if m.type not in ("user", "assistant"):
            continue
        role = "me" if m.type == "user" else "oma"
        for b in _blocks((m.message or {}).get("content")):
            if not (isinstance(b, dict) and b.get("type") == "text" and b.get("text")):
                continue
            if role == "me" and b["text"].startswith("[Request interrupted"):  # след перебивания голосом
                continue
            if role == "oma" and b["text"].strip().startswith(("[молч", "[конец")):  # метка молчания — не реплика
                continue
            text = strip_note(b["text"]) if role == "me" else " ".join(p.strip() for p in b["text"].split(MORE))  # метка — не текст
            if role == "oma" and out and out[-1]["role"] == "oma":  # как в окне: ответ одним сообщением
                out[-1]["text"] += " " + text
            else:
                out.append({"role": role, "text": text})
    return out


def _ago(sec: int) -> str:
    if sec < 60:
        return "только что"
    return f"{sec // 60} мин" if sec < 3600 else f"{sec // 3600} ч"


def format_agents(entries, now) -> str:
    """Строка на агента; id — чтобы различать агентов с одинаковым названием."""
    return "\n".join(f'{e["title"]} ({e["project"]}, id {e["id"]}{", в фокусе" if e.get("focused") else ""}) — '
                     f'{STATES.get(e["state"], e["state"])}, {_ago(int(now - e["since"]))}: {e["status"]}'
                     + (f'; сторож: {e["health"]}' if e.get("health") else "")
                     + (f'; ждёт ответа: {e["ask"]}' if e.get("ask") else "")
                     for e in entries) or "Агентов нет."


def _text(t: str) -> dict:
    return {"content": [{"type": "text", "text": t}]}


def pick_agent(entries, query: str):
    """Агент по запросу: точный id, иначе единственный по части названия или проекта, иначе по части id
    (без учёта регистра); пусто — в фокусе. (агент, None) или (None, ответ мозгу) — задание не уходит
    случайному из двух."""
    q = (query or "").strip()
    if not q:
        e = next((x for x in entries if x.get("focused")), None)
        return (e, None) if e else (None, "Нет агента в фокусе — назови, какому.")
    ql = q.lower()
    for hits in ([x for x in entries if x["id"] == q],
                 [x for x in entries if ql in x["title"].lower() or ql in x["project"].lower()],
                 [x for x in entries if ql in x["id"].lower()]):
        if len(hits) > 1:
            return None, ("Под это подходят несколько агентов: " + "; ".join(f'{x["title"]} (id {x["id"]})' for x in hits)
                          + ". Спроси пользователя, какому.")
        if hits:
            return hits[0], None
    return None, "Агент не найден."


def oma_tools(snapshot, send, orders=None) -> list:
    """Инструменты Омы поверх снимка агентов демона; передачу агентам ведёт orders."""
    orders = orders or Orders()
    @tool("agents", "Агенты пользователя: название, проект, id, в фокусе ли, состояние, давность, последний статус.", {})
    async def agents(args):
        return _text(format_agents(snapshot(), time.time()))

    @tool("agent_feed", "Лента последних статусов агента. agent — id из agents (названия повторяются; "
                        "подойдёт и часть названия или проекта).", {"agent": str})
    async def agent_feed(args):
        e, why = pick_agent(snapshot(), args.get("agent", ""))
        if not e:
            return _text(why)
        return _text("\n".join(f'{l["kind"]}: {l["text"]}' for l in e["feed"]) or "Лента пуста.")

    @tool("agent_tail", "Конец переписки агента с пользователем. agent — id из agents (подойдёт и часть названия "
                        "или проекта); пусто — агент в фокусе.",
          {"type": "object", "properties": {"agent": {"type": "string"}}})
    async def agent_tail(args):
        e, why = pick_agent(snapshot(), args.get("agent", ""))
        if not e:
            return _text(why)
        return _text(transcript_tail(e.get("transcript") or "") or "Переписка недоступна.")

    @tool("selection", "Текст, выделенный пользователем мышью.", {})
    async def sel(args):
        return _text(await asyncio.to_thread(selection) or "Ничего не выделено")

    @tool("presence", "Слушает ли пользователь Ому сейчас: here — слушает, busy — не сейчас, away — ушёл.", {"state": str})
    async def presence(args):
        state = args.get("state")
        if state not in ("here", "busy", "away"):
            return _text("state: here | busy | away")
        send({"cmd": "presence", "state": state})
        return _text("ок")

    @tool("tell", "Передать агенту ответ или задание. agent — id из agents (или часть названия/проекта; пусто — агент в "
                  "фокусе); text — полное задание или ответ от лица пользователя; gist — смысл для пользователя, несколько "
                  "слов; option — номер варианта, если агент ждёт ответа с вариантами и ответ ровно один из них (у плана: "
                  "1 — одобрить), иначе 0.",
          {"type": "object", "properties": {"agent": {"type": "string"}, "text": {"type": "string"},
                                            "gist": {"type": "string"}, "option": {"type": "integer"}},
           "required": ["text", "gist"]})
    async def tell(args):
        e, why = pick_agent(snapshot(), args.get("agent", ""))
        if not e:
            return _text(why)
        return _text(await orders.tell(e, args.get("text", ""), args.get("gist", ""), int(args.get("option") or 0)))

    where = {"type": "string", "enum": ["orca", "herdr"]}

    @tool("projects", "Проекты, где можно запустить нового агента: название и папка. where — orca (по умолчанию) или "
                      "herdr, если пользователь просит herdr.",
          {"type": "object", "properties": {"where": where}})
    async def projects(args):
        return _text(await orders.projects(args.get("where") or "orca"))

    @tool("launch", "Запустить нового агента Claude в проекте. path — папка из projects с тем же where; task — полное "
                    "задание; gist — смысл для пользователя; where — orca (по умолчанию) или herdr.",
          {"type": "object", "properties": {"path": {"type": "string"}, "task": {"type": "string"},
                                            "gist": {"type": "string"}, "where": where},
           "required": ["path", "task", "gist"]})
    async def launch(args):
        return _text(await orders.launch(args.get("path", ""), args.get("task", ""), args.get("gist", ""),
                                         args.get("where") or "orca"))

    return [agents, agent_feed, agent_tail, sel, presence, tell, projects, launch]


class Brain:
    """Текущий чат Омы: key — "new", "chat:<id сессии>" или "agent:<id агента>"."""

    def __init__(self, events: asyncio.Queue, snapshot, send=None, orders=None, fresh=None):
        os.makedirs(OMA_DIR, exist_ok=True)
        self.orders = orders or Orders()
        self.events, self.snapshot, self.tools = events, snapshot, oma_tools(snapshot, send, self.orders)
        self.memory = True  # настройка memory из строки состояния службы; действует с нового подключения
        self.fresh = fresh  # ход начинается: служба отвечает строкой агентов с фокусом этой минуты (link.turn); None — нет
        self.key, self.sid, self.client, self.reader = "new", None, None, None
        self.st, self.used, self.turns = {"skip": False}, time.time(), 0  # turns: запросов без ResultMessage
        self.lock = asyncio.Lock()  # warm и send не поднимают claude дважды
        self.closing = {}  # прежние claude, закрываемые в фоне: задача → сессия, которую он, может быть, дописывает

    def chats(self) -> list:
        return general_chats(list_sessions(OMA_DIR))

    @staticmethod
    def _sid(key: str):
        """id сессии чата key: у "chat:<id>" — он сам, у "agent:<id>" — по тегу; у нового чата — None."""
        if key.startswith("chat:"):
            return key[5:]
        return find_agent_chat(list_sessions(OMA_DIR), key[6:]) if key.startswith("agent:") else None

    def messages(self, key: str) -> list:
        """История чата key — только чтение: окно смотрит чат, мозг не переключается, claude не трогается."""
        sid = self._sid(key)
        return messages_of(get_session_messages(sid, directory=OMA_DIR)) if sid else []

    async def open(self, key: str) -> list:
        """Перейти к чату key и вернуть его историю. claude не запускается: подключится первый send."""
        self._detach()
        self.key, self.sid, self.used = key, self._sid(key), time.time()
        return messages_of(get_session_messages(self.sid, directory=OMA_DIR)) if self.sid else []

    async def _connect(self):
        """Клиент текущего чата: продолжение сессии по id, если она уже есть."""
        self._detach()  # мёртвый клиент прошлого подключения
        # в сессию пишет один claude: ждём выхода прежнего той же сессии; claude других чатов закрываются сами
        await asyncio.gather(*[t for t, sid in self.closing.items() if sid and sid == self.sid], return_exceptions=True)
        self.st, self.turns = {"skip": False}, 0
        prompt = OMA_PROMPT
        if self.key.startswith("agent:"):
            aid = self.key[6:]
            prompt = agent_prompt(pick_agent(self.snapshot(), aid)[0] or {"id": aid, "title": aid, "cwd": "?"})
        on = self.memory
        if on:
            memory.ensure()
        client = ClaudeSDKClient(ClaudeAgentOptions(
            cwd=OMA_DIR, add_dirs=[PROJECTS], model=MODEL, system_prompt=prompt + memory.prompt(on),
            tools=TOOLS + (MEMORY_TOOLS if on else []),
            # чтение в рабочих папках разрешений не просит; остальное, что спросило бы, — отказ: спрашивать некого
            allowed_tools=["WebSearch"] + OMA_TOOLS, permission_mode="dontAsk",
            # «Помню» — штатная auto memory в своём каталоге: её MEMORY.md CLI кладёт в контекст нового чата сам
            settings=json.dumps({**READ_ONLY_IN_PROJECTS, **({"autoMemoryDirectory": memory.prefs_dir()} if on else {})}),
            mcp_servers={"oma": create_sdk_mcp_server("oma", tools=self.tools)}, strict_mcp_config=True,
            resume=self.sid, include_partial_messages=True, setting_sources=[],
            env={"AGENT_SPEAK": "1", **({} if on else {"CLAUDE_CODE_DISABLE_AUTO_MEMORY": "1"})},
            hooks={"PostToolUse": [HookMatcher(matcher=ORDER_TOOLS, hooks=[self._asked])],
                   "PreToolUse": [HookMatcher(matcher="Write|Edit", hooks=[self._remember])]}))
        await client.connect()  # упал — клиента нет, следующий send попробует снова
        self.client, self.reader = client, asyncio.create_task(self._read(client))

    async def _asked(self, inp, tool_use_id, ctx):
        """После tell или launch: Ома спросила подтверждение — ход кончается, второго прохода модели нет."""
        return {"continue_": False, "stopReason": ASKED} if self.orders.tripped else {}

    async def _remember(self, inp, tool_use_id, ctx):
        """Перед Write/Edit: в память — только слова пользователя из этой реплики (orders.heard), иначе отказ."""
        why = memory.gate(inp.get("tool_input") or {}, self.orders.heard)
        return {"hookSpecificOutput": {"hookEventName": "PreToolUse", "permissionDecision": "deny",
                                       "permissionDecisionReason": why}} if why else {}

    async def _read(self, client):
        try:
            async for m in client.receive_messages():
                if type(m).__name__ == "ResultMessage":
                    self.turns = max(0, self.turns - 1)
                    if self.sid is None and m.session_id:
                        self.sid, was = m.session_id, self.key
                        if was.startswith("agent:"):
                            with contextlib.suppress(OSError, ValueError):  # без тега чат лишь не найдётся по агенту
                                tag_session(self.sid, was, directory=OMA_DIR)
                        else:
                            self.key = f"chat:{self.sid}"
                        await self.events.put(("opened", self.key, was))
                for e in claude_events(m, self.st):
                    await self.events.put(e)
            self.turns = 0
            await self.events.put(("error", "Claude: сессия оборвалась"))
        except Exception as e:  # SDK упал — ход кончается; следующий send переподключится
            self.turns = 0
            await self.events.put(("error", f"Claude: {e}"))

    async def _ready(self):
        """Клиент текущего чата наготове: устаревший чат — новым, мёртвый клиент — новым подключением."""
        async with self.lock:
            if self.key.startswith("chat:") and time.time() - self.used > STALE_HOURS * 3600:
                await self.events.put(("opened", "new", self.key))  # до open: ключ станет "new", даже если connect упадёт
                await self.open("new")
            if not self.reader or self.reader.done():
                await self._connect()

    async def warm(self):
        """Клик по орбу: claude поднимается, пока пользователь говорит, — первая реплика не ждёт его запуска (0,7–2 с)."""
        await self._ready()

    async def send(self, text: str, sound: str = ""):
        fresh = asyncio.ensure_future(self.fresh()) if self.fresh else None  # служба спрашивает фокус, пока claude поднимается
        try:
            await self._ready()
            entries = await fresh if fresh else None
        finally:
            if fresh:
                fresh.cancel()  # connect упал — ответ не нужен
        self.used = time.time()
        self.turns += 1  # до query: ResultMessage может прийти раньше, чем query вернётся
        await self.client.query(with_agents(text, self.snapshot() if entries is None else entries, time.time(), sound))

    async def interrupt(self):
        """Перебить ход в полёте; кончившийся не трогаем — иначе skip съест следующий ответ."""
        if self.turns and self.reader and not self.reader.done():
            self.st["skip"] = True
            await self.client.interrupt()

    def _detach(self):
        """Отцепить клиента: читатель — сразу (ответ старого чата не разберётся под новым ключом и не пометит его
        сессию чужим тегом), выход claude — в фоне: переход в другой чат его не ждёт (SDK ждёт до 5 с)."""
        if self.reader:
            self.reader.cancel()
        if self.client:
            t = asyncio.create_task(self._shut(self.client))
            self.closing[t] = self.sid
            t.add_done_callback(lambda t: self.closing.pop(t, None))
        self.client = self.reader = None

    @staticmethod
    async def _shut(client):
        with contextlib.suppress(Exception):  # клиент уже мёртв — выбрасываем его в любом случае
            await client.disconnect()

    async def close(self):
        """Закрыть клиента и дождаться выхода всех прежних claude (выход Омы)."""
        self._detach()
        await asyncio.gather(*self.closing, return_exceptions=True)
