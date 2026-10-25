"""Задания и ответы агентам от Омы: опасное, сказанное голосом, уходит только после «подтверждаю …».
Совпадение проверяет код по распознанному тексту, не модель: модель могла бы «услышать» подтверждение в ослышке.
Передаёт CLI `agent-speak tell | launch | projects` — исход мозгу нужен сразу."""
import asyncio
import os
import re
import time

from .conversation import words

CONFIRM_WAIT = 60  # с: «подтверждаю …» позже — не подтверждение
DANGER = (  # (что, слово фразы «подтверждаю …», основы слов в тексте, основы в подтверждении)
    ("пуш", "пуш", ("push", "пуш", "запуш", "отпуш"), ("пуш", "push")),
    ("удаление", "удаление", ("удал", "delete", "rm", "сотри", "стери", "снеси"), ("удален",)),
    ("деплой", "деплой", ("deploy", "депло", "задепло", "выкат", "release", "релиз", "publish", "опублик"), ("депло", "deploy")),
    ("миграция", "миграцию", ("migrat", "миграц"), ("миграц",)),
    ("сброс", "сброс", ("reset", "сброс", "сбрось"), ("сброс",)),
    ("force", "форс", ("force", "форс"), ("форс", "force")),
    ("запись в базу", "запись", ("drop", "truncate", "insert", "базу", "бд"), ("запис",)),
)
NEGATIVE = {"нет", "не", "отмена", "стоп"}


def _has(ws, stems):
    return any(w.startswith(stems) for w in ws)


def danger(text):
    """Опасное в тексте: строка DANGER или None."""
    ws = words(text or "")
    return next((d for d in DANGER if _has(ws, d[2])), None)


def negative(text):
    ws = words(text or "")
    return bool(ws) and ws[0] in NEGATIVE


def confirms(text, d):
    ws = words(text)
    return 2 <= len(ws) <= 4 and _has(ws, ("подтвержд",)) and _has(ws, d[3])


def asked(entry):
    """Вопрос агента, на который отвечают: варианты из снимка, иначе последнее «…?» его последней реплики."""
    if entry.get("ask"):
        return entry["ask"]
    last = next((l for l in reversed(entry.get("feed") or []) if l["kind"] in ("question", "final")), None)
    q = re.findall(r"[^.!?]*\?", last["text"]) if last else []
    return q[-1].strip() if q else ""


async def cli(*args, timeout=40):
    """`agent-speak <args>`: (успех, stdout) или (False, stderr — строка ошибки по-русски)."""
    try:
        p = await asyncio.create_subprocess_exec(os.environ.get("AGENT_SPEAK_BIN", "agent-speak"), *args,
                                                 stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE)
    except OSError as e:
        return False, f"agent-speak не запустился: {e}"
    try:
        out, err = await asyncio.wait_for(p.communicate(), timeout)
    except asyncio.TimeoutError:
        p.kill()
        return False, "терминал не ответил"
    return (True, out.decode().strip()) if p.returncode == 0 else (False, err.decode().strip() or f"код {p.returncode}")


def _name(path):
    return os.path.basename(path.rstrip("/")) or path


class Orders:
    """Передача агентам. turn() — перед каждой репликой мозгу: сказана ли она голосом и что именно."""

    def __init__(self, run=cli, clock=time.monotonic):
        self.run, self.clock = run, clock
        self.voiced, self.heard = False, ""
        self.pending = None  # опасное ждёт фразы: {args, what, said, until}
        self.note = ""       # мозгу — в начало следующей реплики: подтверждённое Ома передала сама

    def turn(self, voiced, heard):
        self.voiced, self.heard = voiced, heard

    def gate(self, sent, ask=""):
        """Опасное, сказанное голосом: строка DANGER; набранное в окне и отказ («нет, не пушь») — None."""
        if not self.voiced:
            return None
        d = danger(self.heard) or danger(sent) or danger(ask)
        if d and negative(self.heard) and negative(sent) and not danger(self.heard):
            return None
        return d

    async def _order(self, args, d, said, ok_reply):
        if d:
            self.pending = {"args": args, "what": d, "said": said, "until": self.clock() + CONFIRM_WAIT}
            return (f"Не передано: это {d[0]}, такое уходит только после подтверждения голосом. "
                    f"Скажи пользователю ровно: «Это {d[0]}. Скажи: подтверждаю {d[1]}».")
        ok, out = await self.run(*args)
        return ok_reply if ok else f"Не передано: {out}. Скажи пользователю коротко, что не вышло."

    async def tell(self, entry, text, gist, option=0):
        p = entry["project"]
        return await self._order(("tell", entry["id"], str(option), text), self.gate(text, asked(entry)),
                                 f"Передала проекту {p}: {gist}.",
                                 f"Передано проекту {p}. Скажи пользователю одной фразой, кому и что: "
                                 f"«Передала проекту {p}: …» — смысл, не текст задания.")

    async def launch(self, path, task, gist):
        name = _name(path)
        return await self._order(("launch", path, task), self.gate(task), f"Запустила агента в проекте {name}: {gist}.",
                                 f"Агент запущен в проекте {name}. Скажи пользователю одной фразой: "
                                 f"«Запустила агента в проекте {name}: …» — смысл задания.")

    async def projects(self):
        ok, out = await self.run("projects")
        if not ok:
            return f"Список проектов недоступен: {out}."
        return "\n".join(f"{_name(l)} — {l}" for l in out.splitlines() if l.strip()) or "Проектов нет."

    async def confirm(self, text):
        """Реплика голосом, пока опасное ждёт: фраза совпала — передать и вернуть фразу Омы; иначе None.
        Ожидание снимается в любом случае: подтверждает только следующая реплика."""
        p, self.pending = self.pending, None
        if not p or self.clock() > p["until"] or not confirms(text, p["what"]):
            return None
        ok, out = await self.run(*p["args"])
        said = p["said"] if ok else f"Не вышло: {out}."
        self.note = f"(Пользователь подтвердил голосом, Ома сама: {said}) "
        return said
