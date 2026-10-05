"""Поток дельт ответа → предложения для озвучки."""
import re

END = re.compile(r"(?<=[.!?…])\s+|\n+")


class Chunker:
    def __init__(self, min_len=20):
        self.min_len, self.buf = min_len, ""

    def feed(self, delta):
        self.buf += delta
        parts = END.split(self.buf)
        self.buf = parts.pop()  # недописанное — ждёт
        out, acc = [], ""
        for p in parts:
            acc = f"{acc} {p.strip()}".strip()
            if len(acc) >= self.min_len:
                out.append(acc)
                acc = ""
        if acc:
            self.buf = f"{acc} {self.buf}" if self.buf else acc
        return out

    def flush(self):
        rest, self.buf = self.buf.strip(), ""
        return [rest] if rest else []

    def reset(self):
        self.buf = ""
