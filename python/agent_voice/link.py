"""Сокет демона agent-speak: команды — строка JSON; подписка — строки состояния."""
import asyncio
import json
import os
import socket

SOCK = os.path.join(os.environ.get("XDG_RUNTIME_DIR", "/tmp"), "agent-speak.sock")


_conn = None  # одно соединение: порядок сообщений (демон читает построчно, поток на соединение)


def send(cmd):
    global _conn
    data = (json.dumps(cmd, ensure_ascii=False) + "\n").encode()
    for _ in range(2):  # демон мог закрыть соединение — одно переподключение
        try:
            if _conn is None:
                _conn = socket.socket(socket.AF_UNIX)
                _conn.connect(SOCK)
            _conn.sendall(data)
            return True
        except OSError:
            if _conn is not None:
                _conn.close()
            _conn = None
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
