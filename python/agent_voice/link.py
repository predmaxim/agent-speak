"""Сокет демона agent-speak: команды — строка JSON; подписка — строки состояния и агентов."""
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


async def watch(on_status, on_agents, sock=SOCK):
    """Подписка на демона: строка состояния → on_status(dict), строка агентов → on_agents(list),
    прочие строки (пузыри, чаты, мусор) пропускаются. Демон закрыл поток — возврат."""
    r, w = await asyncio.open_unix_connection(sock, limit=1 << 22)  # строка агентов с лентами бывает длинной
    try:
        w.write(b'{"cmd":"subscribe"}\n')
        await w.drain()
        async for line in r:
            try:
                d = json.loads(line)
            except ValueError:
                continue
            if not isinstance(d, dict):
                continue
            if "running" in d:
                on_status(d)
            elif "agents" in d:
                on_agents(d["agents"])
    finally:
        w.close()


async def request(cmd, sock=SOCK, timeout=1.0):
    """Запрос с ответом — своим соединением: строка туда, строка обратно (dict); службы нет, молчит или ответила
    не JSON — None. Не через send: непрочитанный ответ служба ждёт секунду и закрывает соединение с командами."""
    try:
        r, w = await asyncio.wait_for(asyncio.open_unix_connection(sock, limit=1 << 22), timeout)
        try:
            w.write((json.dumps(cmd, ensure_ascii=False) + "\n").encode())
            await w.drain()
            return json.loads(await asyncio.wait_for(r.readline(), timeout))  # фокус служба спрашивает ~0,16 с
        finally:
            w.close()
    except (OSError, ValueError, asyncio.TimeoutError):
        return None


async def turn(sock=SOCK):
    """Ход мозга начинается: служба заново спрашивает окно в фокусе и отвечает строкой агентов — снимок для реплики."""
    d = await request({"cmd": "turn"}, sock)
    return d.get("agents") if isinstance(d, dict) else None
