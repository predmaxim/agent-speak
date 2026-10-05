"""Агенты голосового разговора: общий вид — start/send/interrupt/close и очередь events
с ("delta", текст), ("done", None), ("error", причина). Права — только чтение."""
import asyncio
import itertools
import json

VOICE_PROMPT = ("Это голосовой разговор. Отвечай коротко, разговорным языком, без markdown, "
                "списков и кода вслух; код и пути упоминай словами.")


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


class ClaudeAgent:
    def __init__(self, events):
        self.events, self.st, self.client, self.reader = events, {"skip": False}, None, None

    async def start(self, cwd):
        from claude_agent_sdk import ClaudeAgentOptions, ClaudeSDKClient
        opts = ClaudeAgentOptions(
            cwd=cwd, permission_mode="plan", include_partial_messages=True,
            system_prompt={"type": "preset", "preset": "claude_code", "append": VOICE_PROMPT})
        self.client = ClaudeSDKClient(opts)
        await self.client.connect()
        self.reader = asyncio.create_task(self._read())

    async def _read(self):
        try:
            async for m in self.client.receive_messages():
                for e in claude_events(m, self.st):
                    await self.events.put(e)
        except Exception as e:  # SDK упал — разговор кончается
            await self.events.put(("error", f"Claude: {e}"))

    async def send(self, text):
        await self.client.query(text)

    async def interrupt(self):
        # ponytail: interrupt сразу после ResultMessage съест следующий ход; счётчик ходов, если это всплывёт
        self.st["skip"] = True
        await self.client.interrupt()

    async def close(self):
        if self.client:
            await self.client.disconnect()


class CodexProtocol:
    """JSON-RPC codex app-server (строка = сообщение) без ввода-вывода."""
    def __init__(self):
        self.ids = itertools.count(1)
        self.thread_id = None
        self.turn_id = None
        self.turn_error = None

    def request(self, method, params):
        rid = next(self.ids)
        return rid, json.dumps({"id": rid, "method": method, "params": params}, ensure_ascii=False) + "\n"

    def handle(self, line):
        try:
            m = json.loads(line)
        except ValueError:
            return []
        if "id" in m and "method" not in m:
            if "error" in m:
                return [("error", m["error"].get("message", str(m["error"])))]
            return [("response", m["id"], m.get("result"))]
        p = m.get("params") or {}
        if m.get("method") == "item/agentMessage/delta" and p.get("turnId") == self.turn_id:
            return [("delta", p.get("delta", ""))]
        if m.get("method") == "error" and p.get("turnId") == self.turn_id:
            self.turn_error = (p.get("error") or {}).get("message")
        if m.get("method") == "turn/completed" and (p.get("turn") or {}).get("id") == self.turn_id:
            self.turn_id, err, self.turn_error = None, self.turn_error, None
            if p["turn"].get("status") == "failed":
                return [("error", f"Codex: {err or 'ход не удался'}")]
            return [("done", None)]
        return []


class CodexAgent:
    def __init__(self, events):
        self.events, self.p, self.proc, self.waiting = events, CodexProtocol(), None, {}

    async def _call(self, method, params):
        rid, line = self.p.request(method, params)
        fut = asyncio.get_running_loop().create_future()
        self.waiting[rid] = fut
        self.proc.stdin.write(line.encode())
        await self.proc.stdin.drain()
        return await fut

    async def start(self, cwd):
        self.proc = await asyncio.create_subprocess_exec(
            "codex", "app-server", stdin=asyncio.subprocess.PIPE, stdout=asyncio.subprocess.PIPE)
        self.reader = asyncio.create_task(self._read())
        await self._call("initialize", {"clientInfo": {"name": "agent-speak", "version": "1"}})
        self.proc.stdin.write(b'{"method":"initialized"}\n')
        r = await self._call("thread/start", {
            "cwd": cwd, "sandbox": "read-only", "approvalPolicy": "never",
            "developerInstructions": VOICE_PROMPT, "ephemeral": True})
        self.p.thread_id = r["thread"]["id"]

    async def _read(self):
        async for raw in self.proc.stdout:
            for e in self.p.handle(raw.decode(errors="replace")):
                if e[0] == "response":
                    fut = self.waiting.pop(e[1], None)
                    if fut and not fut.done():
                        fut.set_result(e[2])
                else:
                    await self.events.put(e)
        await self.events.put(("error", "Codex: app-server завершился"))

    async def send(self, text):
        r = await self._call("turn/start", {"threadId": self.p.thread_id, "input": [{"type": "text", "text": text}]})
        self.p.turn_id = r["turn"]["id"]

    async def interrupt(self):
        turn, self.p.turn_id = self.p.turn_id, None  # дальнейшие дельты этого хода отсекаются
        if turn:
            await self._call("turn/interrupt", {"threadId": self.p.thread_id, "turnId": turn})

    async def close(self):
        if self.proc and self.proc.returncode is None:
            self.proc.kill()
            await self.proc.wait()


def make_agent(name, events):
    return {"claude": ClaudeAgent, "codex": CodexAgent}[name](events)
