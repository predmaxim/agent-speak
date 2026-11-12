"""Проверка сервера синтеза: python test_silero_tts.py (нужна модель)."""
import json, os, socket, struct, subprocess, sys, time

HERE = os.path.dirname(os.path.abspath(__file__))
DATA = os.path.expanduser("~/.local/share/agent-speak")
SOCK = "/tmp/agent-speak-tts-test.sock"
MODEL = f"{DATA}/v5_6_ru.pt"
if not os.path.exists(MODEL) and __name__ != "__main__":
    import pytest  # модель ставит install.sh; без неё pytest пропускает проверку, а запуск скриптом падает
    pytest.skip(f"нет {MODEL}: сначала ./install.sh", allow_module_level=True)


def ask(req):
    s = socket.socket(socket.AF_UNIX)
    s.connect(SOCK)
    s.sendall((json.dumps(req) + "\n").encode())
    n = struct.unpack("<I", s.recv(4, socket.MSG_WAITALL))[0]
    if n == 0xFFFFFFFF:  # ошибка синтеза — не пустой звук
        return None
    data = b""
    while len(data) < n:
        data += s.recv(n - len(data))
    return data


sys.path.insert(0, HERE)
import torch
from silero_tts import pcm16

# у голоса eugene пик 1,155: без ограничения int16 заворачивается (1,2 → -26216) — треск
assert struct.unpack("<3h", pcm16(torch.tensor([1.2, -1.3, 0.5]))) == (32767, -32767, 16383)

if os.path.exists(SOCK):
    os.unlink(SOCK)
p = subprocess.Popen([f"{DATA}/venv/bin/python", f"{HERE}/silero_tts.py", MODEL, SOCK])
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
    assert ask({"text": "hello 123", "speaker": "xenia", "rate": "medium"}) == b""  # нечего читать — пусто, это не ошибка
    # интонация и проект с паузой (ssml): модель принимает разметку при любом тоне
    for tone in ("ask", "alarm", "done"):
        said = ask({"text": "нужен ответ, куда пушить?", "speaker": "kseniya", "rate": "medium", "tone": tone, "lead": "агент спик"})
        assert said and len(said) > 48000, (tone, said and len(said))
    assert ask({"text": "Привет.", "speaker": "nobody", "rate": "medium"}) is None
    assert len(ask({"text": "Снова работает.", "speaker": "xenia", "rate": "medium"})) > 48000  # ошибка сервер не роняет
    print("ok")
finally:
    p.terminate()
