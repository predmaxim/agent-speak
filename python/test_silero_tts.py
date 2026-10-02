"""Проверка сервера синтеза: python test_silero_tts.py (нужна модель)."""
import json, os, socket, struct, subprocess, sys, time

HERE = os.path.dirname(os.path.abspath(__file__))
DATA = os.path.expanduser("~/.local/share/agent-speak")
SOCK = "/tmp/agent-speak-tts-test.sock"


def ask(req):
    s = socket.socket(socket.AF_UNIX)
    s.connect(SOCK)
    s.sendall((json.dumps(req) + "\n").encode())
    n = struct.unpack("<I", s.recv(4, socket.MSG_WAITALL))[0]
    data = b""
    while len(data) < n:
        data += s.recv(n - len(data))
    return data


if os.path.exists(SOCK):
    os.unlink(SOCK)
p = subprocess.Popen([f"{DATA}/venv/bin/python", f"{HERE}/silero_tts.py", f"{DATA}/v5_ru.pt", SOCK])
try:
    for _ in range(100):
        if os.path.exists(SOCK):
            break
        time.sleep(0.1)
    pcm = ask({"text": "Привет, это проверка.", "speaker": "xenia", "rate": "medium"})
    assert len(pcm) > 48000, len(pcm)  # больше полсекунды звука
    assert len(pcm) % 2 == 0
    fast = ask({"text": "Привет, это проверка.", "speaker": "xenia", "rate": "fast"})
    assert 0 < len(fast) < len(pcm), (len(fast), len(pcm))
    assert ask({"text": "hello 123", "speaker": "xenia", "rate": "medium"}) == b""
    print("ok")
finally:
    p.terminate()
