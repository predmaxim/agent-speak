"""Сервер синтеза Silero: модель в памяти, запрос — JSON-строка, ответ — u32 длина + PCM s16le 48 кГц моно."""
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


def chunks(text):
    buf = ""
    for s in re.split(r"(?<=[.!?…])\s+", text):
        if buf and len(buf) + len(s) > MAX:
            yield buf
            buf = ""
        buf = f"{buf} {s}".strip()
    if buf:
        yield buf


def synth(model, text, speaker, rate):
    out = []
    for part in chunks(text):
        if not re.search(r"[а-яёА-ЯЁ]", part):
            continue
        kw = dict(speaker=speaker, sample_rate=48000, put_accent=True, put_yo=True)
        if rate == "medium":
            audio = model.apply_tts(text=part, **kw)
        else:
            safe = part.replace("&", " и ").replace("<", " ").replace(">", " ")
            audio = model.apply_tts(ssml_text=f'<speak><prosody rate="{rate}">{safe}</prosody></speak>', **kw)
        out.append((audio * 32767).to(torch.int16).numpy().tobytes())
    return b"".join(out)


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
                pcm = synth(model, req["text"], req.get("speaker", "xenia"), req.get("rate", "medium"))
            except Exception as e:  # плохой запрос не должен ронять сервер
                print(f"silero_tts: {e}", file=sys.stderr, flush=True)
                pcm = b""
            try:
                conn.sendall(struct.pack("<I", len(pcm)) + pcm)
            except OSError:
                pass


if __name__ == "__main__":
    main()
