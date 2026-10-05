"""agent_voice: голосовой разговор с агентом. Запускает демон agent-speak:
python -m agent_voice --agent claude|codex --cwd DIR"""
import argparse
import asyncio
import glob
import os
import subprocess
import sys


def cuda_env():
    """CUDA-библиотеки из pip (nvidia-*) видны ctranslate2 только через LD_LIBRARY_PATH при старте."""
    libs = glob.glob(os.path.join(sys.prefix, "lib/python3*/site-packages/nvidia/*/lib"))
    have = os.environ.get("LD_LIBRARY_PATH", "")
    if libs and libs[0] not in have:
        os.environ["LD_LIBRARY_PATH"] = ":".join(libs + ([have] if have else []))
        os.execv(sys.executable, [sys.executable, "-m", "agent_voice", *sys.argv[1:]])


def notify(text):
    subprocess.run(["notify-send", "-t", "4000", "-a", "Озвучка", "Голос", text])


async def main(agent_name, cwd):
    from . import link
    from .agents import make_agent
    from .conversation import Conversation
    from .ear import listen, load_whisper

    loop = asyncio.get_running_loop()
    # EOF на stdin — демон умер: выходим, микрофон не остаётся открытым
    loop.add_reader(sys.stdin.fileno(), lambda: sys.stdin.buffer.read1(1) or os._exit(0))

    events = asyncio.Queue()
    agent = make_agent(agent_name, events)
    cv = Conversation()
    model = await asyncio.to_thread(load_whisper)
    await agent.start(cwd)

    async def act(actions):
        for a in actions:
            if a[0] == "send":
                try:
                    await agent.send(a[1])
                except Exception as e:  # сбой адаптера (например RPC-ошибка Codex)
                    events.put_nowait(("error", f"agent: {e}"))
            elif a[0] == "interrupt":
                await agent.interrupt()
            elif a[0] == "stop":
                link.send({"cmd": "stop"})
            elif a[0] == "say":
                link.send({"cmd": "voice", "text": a[1]})
            elif a[0] == "state":
                link.send({"cmd": "voice_state", "state": a[1]})

    async def guard(coro, name):  # исключение фоновой задачи → в главный цикл
        try:
            await coro
        except Exception as e:
            events.put_nowait(("error", f"{name}: {e}"))

    tasks = [  # держим ссылки: иначе задачи может собрать GC
        asyncio.create_task(guard(link.watch_speaking(lambda busy: events.put_nowait(("audio", busy))), "watch_speaking")),
        asyncio.create_task(guard(listen(events, model), "listen")),
    ]
    link.send({"cmd": "voice_state", "state": "listening"})
    while True:
        ev = await events.get()
        kind = ev[0]
        if kind == "error":
            raise RuntimeError(ev[1])
        if kind == "warn":
            notify(ev[1])
        elif kind == "start":
            await act(cv.speech_start())
        elif kind == "phrase":
            print(f"agent_voice: > {ev[1]}", flush=True)
            await act(cv.phrase(ev[1]))
        elif kind == "delta":
            await act(cv.delta(ev[1]))
        elif kind == "done":
            await act(cv.done())
        elif kind == "audio":
            await act(cv.audio(ev[1]))


if __name__ == "__main__":
    cuda_env()
    ap = argparse.ArgumentParser()
    ap.add_argument("--agent", choices=["claude", "codex"], required=True)
    ap.add_argument("--cwd", required=True)
    a = ap.parse_args()
    try:
        asyncio.run(main(a.agent, a.cwd))
    except Exception as e:
        print(f"agent_voice: {e!r}", file=sys.stderr, flush=True)
        notify(str(e)[:200])
        sys.exit(1)
