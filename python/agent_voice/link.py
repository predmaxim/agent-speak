"""Сокет демона agent-speak: команды — строка JSON; подписка — строки состояния."""
import asyncio
import json
import os
import socket

SOCK = os.path.join(os.environ.get("XDG_RUNTIME_DIR", "/tmp"), "agent-speak.sock")


def send(cmd):
    try:
        with socket.socket(socket.AF_UNIX) as s:
            s.connect(SOCK)
            s.sendall((json.dumps(cmd, ensure_ascii=False) + "\n").encode())
        return True
    except OSError:
        return False


async def watch_speaking(cb):
    """cb(bool) при каждой смене «говорит» у демона."""
    r, w = await asyncio.open_unix_connection(SOCK)
    w.write(b'{"cmd":"subscribe"}\n')
    await w.drain()
    last = None
    async for line in r:
        try:
            speaking = bool(json.loads(line).get("speaking"))
        except ValueError:
            continue
        if speaking != last:
            last = speaking
            cb(speaking)
