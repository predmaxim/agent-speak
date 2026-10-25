"""Мозг Омы: сессия Claude Agent SDK на чат, свои инструменты только для чтения, список и история чатов.
События в очередь: ("delta", текст), ("done", None), ("error", причина), ("opened", новый_ключ, старый_ключ)."""
import asyncio
import contextlib
import os
import time

from claude_agent_sdk import (ClaudeAgentOptions, ClaudeSDKClient, create_sdk_mcp_server, get_session_messages,
                              list_sessions, tag_session, tool)

from .context import _blocks, selection, transcript_tail

OMA_DIR = os.path.expanduser("~/.local/share/agent-speak/oma")
MODEL = "claude-sonnet-5-5"
TOOLS = ["Read", "Glob", "Grep", "WebSearch"]  # без WebFetch: наружу Ома ничего не отправляет
OMA_TOOLS = ["mcp__oma__agents", "mcp__oma__agent_feed", "mcp__oma__agent_tail", "mcp__oma__selection",
             "mcp__oma__presence"]
STALE_HOURS = 2
OMA_PROMPT = ("Ты — Ома, голосовой заместитель пользователя. Следишь за его ИИ-агентами (Claude Code, Codex) в терминалах "
              "и отвечаешь на его вопросы. Отвечай по-русски и как можно короче: текст звучит голосом, лишнее утомляет. "
              "Вопрос, на который отвечают «да» или «нет» («умеешь…?», «готово?», «можно?»), — «Да» или «Нет» и больше "
              "ничего; одна короткая фраза после — только если без неё ответ вводит в заблуждение. Остальное — одна фраза, "
              "две — только если иначе не ответить; «что делает агент», «как дела» — одной фразой о главном сейчас, без "
              "истории шагов. Не перечисляй, что умеешь, не объясняй, как узнала, не добавляй "
              "оговорок, советов и вопросов «что-нибудь ещё?»; подробности — только когда попросят. Без вступлений, "
              "markdown, списков и кода. "
              "Состояние агентов — инструментами agents и agent_feed; конец переписки агента — agent_tail (пустой аргумент — "
              "агент в фокусе); «это», «тут», «эта ошибка» — сначала selection, затем agent_tail. Ты только читаешь: ничего "
              "не меняешь и не передаёшь агентам; если просят передать или сделать — скажи, что пока не умеешь. Не понятно, "
              "о чём речь, — коротко переспроси. "
              "Если пользователь просит сейчас не говорить или отложить («не сейчас», «давай потом», «замолчи» и т. п.) — "
              "вызови presence с busy; говорит, что уходит, — away; просит новости или говорит, что вернулся, — here. "
              "После presence ответь одним-двумя словами или ничего. Реплика может начинаться с пометки «Ома только что "
              "сказала: …» или «Ома спросила: …» — это твои же слова вслух; «потом» и «не сейчас» после вопроса агента — "
              "presence busy, а не ответ агенту. В словах, где ударение легко прочесть неверно, — омографы, редкие слова, "
              "имена — ставь знак ударения U+0301 сразу после ударной гласной.")
STATES = {"working": "работает", "waiting": "ждёт ответа", "done": "готово", "idle": "простаивает"}


def agent_prompt(entry: dict) -> str:
    return (OMA_PROMPT + f'\nЭтот чат — про агента «{entry["title"]}» (папка {entry["cwd"]}). '
            f'Его лента — agent_feed("{entry["id"]}"), конец переписки — agent_tail("{entry["id"]}").')


def claude_events(msg, st):
    """Сообщение Claude Agent SDK → события. st["skip"]: глотать до конца прерванного хода."""
    if type(msg).__name__ == "ResultMessage":
        if st["skip"]:
            st["skip"] = False
            return []
        return [("done", None)]
    ev = getattr(msg, "event", None)
    if st["skip"] or not isinstance(ev, dict) or ev.get("type") != "content_block_delta":
        return []
    d = ev.get("delta", {})
    return [("delta", d["text"])] if d.get("type") == "text_delta" and d.get("text") else []


def general_chats(infos) -> list:
    """Общие чаты (без чатов агентов), свежие первыми."""
    return [{"key": f"chat:{i.session_id}", "title": (i.custom_title or i.summary or i.first_prompt or "")[:60],
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
            if role == "oma" and out and out[-1]["role"] == "oma":  # как в окне: ответ одним сообщением
                out[-1]["text"] += " " + b["text"]
            else:
                out.append({"role": role, "text": b["text"]})
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
                     for e in entries) or "Агентов нет."


def find_entry(entries, query: str):
    """Агент по id или по части названия/проекта без учёта регистра."""
    q = (query or "").strip().lower()
    if not q:
        return None
    return (next((e for e in entries if e["id"] == query.strip()), None)
            or next((e for e in entries if q in e["title"].lower() or q in e["project"].lower()), None))


def _text(t: str) -> dict:
    return {"content": [{"type": "text", "text": t}]}


def oma_tools(snapshot, send) -> list:
    """Инструменты Омы поверх снимка агентов демона."""
    @tool("agents", "Агенты пользователя: название, проект, id, в фокусе ли, состояние, давность, последний статус.", {})
    async def agents(args):
        return _text(format_agents(snapshot(), time.time()))

    @tool("agent_feed", "Лента последних статусов агента. agent — id из agents (названия повторяются; "
                        "подойдёт и часть названия или проекта).", {"agent": str})
    async def agent_feed(args):
        e = find_entry(snapshot(), args.get("agent", ""))
        if not e:
            return _text("Агент не найден.")
        return _text("\n".join(f'{l["kind"]}: {l["text"]}' for l in e["feed"]) or "Лента пуста.")

    @tool("agent_tail", "Конец переписки агента с пользователем. agent — id из agents (подойдёт и часть названия "
                        "или проекта); пусто — агент в фокусе.",
          {"type": "object", "properties": {"agent": {"type": "string"}}})
    async def agent_tail(args):
        entries = snapshot()
        q = args.get("agent", "")
        e = find_entry(entries, q) if q.strip() else next((x for x in entries if x.get("focused")), None)
        if not e:
            return _text("Агент не найден." if q.strip() else "Нет агента в фокусе.")
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

    return [agents, agent_feed, agent_tail, sel, presence]


class Brain:
    """Текущий чат Омы: key — "new", "chat:<id сессии>" или "agent:<id агента>"."""

    def __init__(self, events: asyncio.Queue, snapshot, send=None):
        os.makedirs(OMA_DIR, exist_ok=True)
        self.events, self.snapshot, self.tools = events, snapshot, oma_tools(snapshot, send)
        self.key, self.sid, self.client, self.reader = "new", None, None, None
        self.st, self.used, self.turns = {"skip": False}, time.time(), 0  # turns: запросов без ResultMessage

    def chats(self) -> list:
        return general_chats(list_sessions(OMA_DIR))

    async def open(self, key: str) -> list:
        """Перейти к чату key и вернуть его историю. claude не запускается: подключится первый send."""
        await self.close()
        self.key, self.sid, self.used = key, None, time.time()
        if key.startswith("chat:"):
            self.sid = key[5:]
        elif key.startswith("agent:"):
            self.sid = find_agent_chat(list_sessions(OMA_DIR), key[6:])
        return messages_of(get_session_messages(self.sid, directory=OMA_DIR)) if self.sid else []

    async def _connect(self):
        """Клиент текущего чата: продолжение сессии по id, если она уже есть."""
        await self.close()  # мёртвый клиент прошлого подключения
        self.st, self.turns = {"skip": False}, 0
        prompt = OMA_PROMPT
        if self.key.startswith("agent:"):
            aid = self.key[6:]
            prompt = agent_prompt(find_entry(self.snapshot(), aid) or {"id": aid, "title": aid, "cwd": "?"})
        client = ClaudeSDKClient(ClaudeAgentOptions(
            cwd=OMA_DIR, model=MODEL, system_prompt=prompt, tools=TOOLS, allowed_tools=TOOLS + OMA_TOOLS,
            mcp_servers={"oma": create_sdk_mcp_server("oma", tools=self.tools)}, strict_mcp_config=True,
            resume=self.sid, include_partial_messages=True, setting_sources=[], env={"AGENT_SPEAK": "1"}))
        await client.connect()  # упал — клиента нет, следующий send попробует снова
        self.client, self.reader = client, asyncio.create_task(self._read(client))

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

    async def send(self, text: str):
        if self.key.startswith("chat:") and time.time() - self.used > STALE_HOURS * 3600:
            await self.events.put(("opened", "new", self.key))  # до open: ключ станет "new", даже если connect упадёт
            await self.open("new")
        if not self.reader or self.reader.done():
            await self._connect()
        self.used = time.time()
        self.turns += 1  # до query: ResultMessage может прийти раньше, чем query вернётся
        await self.client.query(text)

    async def interrupt(self):
        """Перебить ход в полёте; кончившийся не трогаем — иначе skip съест следующий ответ."""
        if self.turns and self.reader and not self.reader.done():
            self.st["skip"] = True
            await self.client.interrupt()

    async def close(self):
        if self.reader:
            self.reader.cancel()
        if self.client:
            with contextlib.suppress(Exception):  # клиент уже мёртв — выбрасываем его в любом случае
                await self.client.disconnect()
        self.client = self.reader = None
