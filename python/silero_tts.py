"""Сервер синтеза Silero: модель в памяти, запрос — JSON-строка, ответ — u32 длина + PCM s16le 48 кГц моно
(длина ERROR — ошибка синтеза)."""
import json
import os
import re
import socket
import struct
import sys
import warnings

import torch

warnings.filterwarnings("ignore")
torch.set_num_threads(4)
MAX = 800  # Silero не берёт длинный текст за раз
ERROR = 0xFFFFFFFF  # длина-ответ «синтез не удался»; её же ждёт src/tts.rs


def chunks(text):
    buf = ""
    for s in re.split(r"(?<=[.!?…])\s+", text):
        if buf and len(buf) + len(s) > MAX:
            yield buf
            buf = ""
        buf = f"{buf} {s}".strip()
    if buf:
        yield buf


RATES = ["x-slow", "slow", "medium", "fast", "x-fast"]
STEP = {"alarm": -1, "done": 1}  # беда — медленнее, итог — быстрее: на шаг от скорости пользователя
PITCH = {"ask": "high", "alarm": "low"}  # вопрос агента — выше, беда — ниже
PAUSE = "250ms"  # после проекта


def escape(s):
    return s.replace("&", " и ").replace("<", " ").replace(">", " ")


def ssml(text, lead="", tone="", rate="medium"):
    """Смысл от демона (tone: ask | alarm | done | "", lead — проект перед паузой) → простой текст или SSML Silero.
    Только документированное (ssml.md silero-models): <speak>, <prosody pitch|rate> уровнями, <break time>. Без <p> и <s>:
    с ними модель теряла вопрос «тут?» (каталог, 2026-11-11). Двоеточие перед <break> обязательно: без знака модель
    дописывает к проекту точку и читает его отдельным повествовательным предложением."""
    i = RATES.index(rate) if rate in RATES else RATES.index("medium")
    j = i + STEP.get(tone, 0)
    rate = RATES[j] if 0 <= j < len(RATES) else RATES[i]  # у края списка скорость не меняется
    attrs = "".join(f' {k}="{v}"' for k, v in (("pitch", PITCH.get(tone, "medium")), ("rate", rate)) if v != "medium")
    if not attrs and not lead:
        return text
    body = escape(text)
    if lead:
        body = f'{escape(lead)}:<break time="{PAUSE}"/> {body}'
    return f"<speak><prosody{attrs}>{body}</prosody></speak>" if attrs else f"<speak>{body}</speak>"


def synth(model, text, speaker, rate, tone="", lead=""):
    out = []
    for part in chunks(text):
        if not re.search(r"[а-яёА-ЯЁ]", part):
            continue
        kw = dict(speaker=speaker, sample_rate=48000, put_accent=True, put_yo=True)
        said = ssml(part, lead, tone, rate)
        lead = ""  # проект — только перед первым куском
        if said.startswith("<speak>"):
            audio = model.apply_tts(ssml_text=said, **kw)
        else:
            audio = model.apply_tts(text=said, **kw)
        out.append(pcm16(audio))
    return b"".join(out)


def pcm16(audio):
    """float → s16le; у голоса eugene пики выше 1 — без clamp int16 заворачивается в треск."""
    return (audio.clamp(-1, 1) * 32767).to(torch.int16).numpy().tobytes()


def main():
    model_path, sock_path = sys.argv[1], sys.argv[2]
    model = torch.package.PackageImporter(model_path).load_pickle("tts_models", "model")
    if os.path.exists(sock_path):
        os.unlink(sock_path)
    srv = socket.socket(socket.AF_UNIX)
    srv.bind(sock_path)
    srv.listen(4)
    while True:
        conn, _ = srv.accept()
        with conn:
            try:
                req = json.loads(conn.makefile("rb").readline())
                pcm = synth(model, req["text"], req.get("speaker", "kseniya"), req.get("rate", "medium"), req.get("tone", ""), req.get("lead", ""))
                reply = struct.pack("<I", len(pcm)) + pcm
            except Exception as e:  # плохой запрос не должен ронять сервер
                print(f"silero_tts: {e}", file=sys.stderr, flush=True)
                reply = struct.pack("<I", ERROR)  # не пустой звук: тот значит «читать нечего»
            try:
                conn.sendall(reply)
            except OSError:
                pass


if __name__ == "__main__":
    main()
