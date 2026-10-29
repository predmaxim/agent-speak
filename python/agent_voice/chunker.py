"""Поток дельт ответа → предложения для озвучки."""
import re

END = re.compile(r"(?<=[.!?…])\s+|\n+")
FIRST_LEN, COMMA_AT = 60, 25  # длинное первое предложение звучит с первой запятой после 25 знаков: отклик раньше


class Chunker:
    def __init__(self, min_len=20):
        self.min_len, self.buf, self.first = min_len, "", True

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
        if self.first and not out and len(self.buf) >= FIRST_LEN:
            i = self.buf.find(", ", COMMA_AT)
            if i > 0:
                out, self.buf = [self.buf[:i + 1]], self.buf[i + 2:]
        self.first = self.first and not out
        return out

    def flush(self):
        rest, self.buf = self.buf.strip(), ""
        return [rest] if rest else []

    def reset(self):
        self.buf, self.first = "", True
