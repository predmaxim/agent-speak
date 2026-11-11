"""Задания и ответы агентам от Омы: голосом ничего не уходит без подтверждения отдельной репликой — обычное после
«да, передай», опасное после «подтверждаю …». Совпадение проверяет код по распознанному тексту, не модель: модель
могла бы «услышать» подтверждение в ослышке или принять за него «продолжай» (так и было 2026-10-29).
Передаёт CLI `agent-speak tell | launch | projects` — исход мозгу нужен сразу."""
import asyncio
import os
import re
import time

from .conversation import words

CONFIRM_WAIT = 60  # с: «подтверждаю …» позже — не подтверждение
DANGER = (  # (что, слово фразы «подтверждаю …», основы слов в тексте, основы в подтверждении)
    ("пуш", "пуш", ("push", "пуш", "запуш", "отпуш", "залей", "залить", "залил"), ("пуш", "push")),
    ("удаление", "удаление", ("удал", "delete", "rm", "сотри", "стер", "снеси", "убер", "убра", "remov", "clean", "почист",
                              "очист"), ("удален",)),
    ("деплой", "деплой", ("deploy", "депло", "задепло", "выкат", "release", "релиз", "publish", "опублик"), ("депло", "deploy")),
    ("миграция", "миграцию", ("migrat", "миграц"), ("миграц",)),
    ("сброс", "сброс", ("reset", "сброс", "сбрось", "откат", "revert", "rebase"), ("сброс",)),
    ("force", "форс", ("force", "форс"), ("форс", "force")),
    ("запись в базу", "запись", ("drop", "truncate", "insert", "update", "базу", "базе", "база", "database", "бд"), ("запис",)),
)
# Обычное поручение голосом: подтверждение — «да» и глагол передачи вместе, одиночное «да» бывает поддакиванием и ослышкой
ORDINARY = ("поручение", "передай", (), ())
GO = ("переда", "отправ", "запуск", "запуст")
NO = {"нет", "не"}
NEGATIVE = ("отмен", "стоп", "отлож", "пропуст", "позже", "потом")  # начала отказа и в вариантах меню («Отложить»)
PLAN = "Ждёт ответа на ExitPlanMode"  # вопрос агента — одобрение плана: пунктов в вызове нет, они только на экране


def _has(ws, stems):
    return any(w.startswith(stems) for w in ws)


def danger(text):
    """Опасное в тексте: строка DANGER или None."""
    ws = words(text or "")
    return next((d for d in DANGER if _has(ws, d[2])), None)


def negative(text):
    ws = words(text or "")
    return bool(ws) and (ws[0] in NO or ws[0].startswith(NEGATIVE))


def unnegated(text):
    """Слова без тех, что стоят сразу после «не»: в «нет, не пушь» опасного нет, в «не пушь, а удали» — есть."""
    ws = words(text or "")
    return " ".join(w for i, w in enumerate(ws) if i == 0 or ws[i - 1] != "не")


def _near(a, b):
    """Одна буква не та, лишняя или пропущена: whisper слышит «подтверждаю душ» вместо «пуш»."""
    if abs(len(a) - len(b)) > 1:
        return False
    if len(a) == len(b):
        return sum(x != y for x, y in zip(a, b)) <= 1
    short, long = sorted((a, b), key=len)
    return any(long[:i] + long[i + 1:] == short for i in range(len(long)))


def confirms(text, d):
    """«Подтверждаю <слово>» в 2–4 словах; слово — с точностью до буквы: ослышка «подтверждаю …» из ничего не бывает."""
    ws = words(text)
    if not 2 <= len(ws) <= 4 or NO & set(ws) or not _has(ws, ("подтвержд",)):  # «не подтверждаю» — отказ
        return False
    return any(w.startswith(c) or (len(c) >= 3 and _near(w[:len(c)], c)) for w in ws if not w.startswith("подтвержд") for c in d[3])


def asked(entry):
    """Вопрос агента, на который отвечают: варианты из снимка, иначе последнее «…?» его последней реплики."""
    ask = entry.get("ask") or ""
    if ask:
        i = ask.rfind("Ждёт ответа на")  # перед ним — последняя реплика агента: «всё запушено» — не вопрос
        return ask[i:] if i >= 0 else ask
    last = next((l for l in reversed(entry.get("feed") or []) if l["kind"] in ("question", "final")), None)
    q = re.findall(r"[^.!?]*\?", last["text"]) if last else []
    return q[-1].strip() if q else ""


def chosen(ask, option):
    """Вариант option («метка — описание») из единственного вопроса с вариантами; иначе пусто."""
    parts = ask.split("Варианты: ")
    if option < 1 or len(parts) != 2:
        return ""
    items = parts[1].strip().split("; ")
    return items[option - 1] if option <= len(items) else ""


def menu_item(out, option):
    """Метка пункта option из вывода `agent-speak menu` («n. метка» построчно); нет такого — пусто."""
    return next((l.split(". ", 1)[1] for l in out.splitlines() if l.startswith(f"{option}. ")), "")


async def cli(*args, timeout=90):
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
        await p.wait()
        # вызов мультиплексора мог успеть: повтор дал бы второй пуш или второго агента
        return False, "терминал не ответил — не знаю, дошло ли; проверь, прежде чем повторять"
    return (True, out.decode().strip()) if p.returncode == 0 else (False, err.decode().strip() or f"код {p.returncode}")


def _name(path):
    return os.path.basename(path.rstrip("/")) or path


class Orders:
    """Передача агентам. turn() — перед каждой репликой мозгу: сказана ли она голосом и что именно."""

    def __init__(self, run=cli, clock=time.monotonic, ask=None):
        self.run, self.clock = run, clock
        self.ask = ask  # ask — вопрос подтверждения голосом: его говорит код, второй проход мозга лишь повторял его (1–5 с)
        self.voiced, self.heard = False, ""
        self.pending = None  # поручение ждёт фразы: {args, what, verb, said, until, n, shown}; ставит его Ома, когда вопрос прозвучал
        self.tripped = False  # в этом ходе мозга уже было опасное: до следующей реплики не уходит ничего
        self.n = 0           # номер хода реплики (Conversation.turn_no): вопрос прерванного хода не звучит и не ждёт
        self.note = ""       # мозгу — в начало следующей реплики: подтверждённое Ома передала сама

    def turn(self, voiced, heard, n=0):
        self.voiced, self.heard, self.tripped, self.n = voiced, heard, False, n

    def gate(self, sent, ask="", option=0):
        """Что ждёт подтверждения: набранное в окне — None; голосом — строка DANGER, иначе ORDINARY (отказ «нет, не пушь»
        не опасен, но и он — после «да, передай»). Проверяется то, что уйдёт: при option — сам вариант; одобрение плана
        (вариантов нет) отказом не считается."""
        if not self.voiced:
            return None
        label = chosen(ask, option)
        d = danger(self.heard) or danger(sent) or danger(label) or danger(ask)
        refusal = negative(self.heard) and not danger(self.heard) and (option == 0 or label) and all(
            negative(x) and not danger(unnegated(x)) for x in (sent, label.split(" — ")[0]) if x)
        return ORDINARY if not d or refusal else d

    async def _order(self, args, d, n, said, ok_reply, ask, verb, shown):
        if n != self.n:  # пока tell читал экран, начался другой ход: прерванный не передаёт и не трогает новый
            return "Не передано: ход прерван новой репликой пользователя."
        if self.tripped:  # мозг перефразировал поручение в том же ходе — передавать нельзя ничего
            return "Не передано: поручение уже ждёт подтверждения, Ома о нём спросила. Ответь ровно «[молчу]» и больше ничего не передавай."
        if d:
            self.tripped = True
            question = f"{ask} Скажи: да, {verb}." if d is ORDINARY else f"{ask} Это {d[0]}. Скажи: подтверждаю {d[1]}."
            if self.ask:  # ждать фразы поручение начнёт, когда вопрос прозвучит в своём ходе: ставит его Ома; shown — его полный текст в окно
                self.ask(question, {"args": args, "what": d, "verb": verb, "said": said, "until": self.clock() + CONFIRM_WAIT, "n": n,
                                    "shown": shown})
            why = "голосом поручение уходит" if d is ORDINARY else f"это {d[0]}, такое уходит"
            return f"Не передано: {why} только после подтверждения. Ома уже спросила вслух: «{question}» Ответь ровно «[молчу]»."
        ok, out = await self.run(*args)
        if not ok:
            return f"Не передано: {out}. Скажи пользователю коротко, что не вышло."
        return ok_reply + (f" Нажат пункт «{out}» — назови его пользователю." if out else "")  # tell: метка с экрана

    async def tell(self, entry, text, gist, option=0):
        n, p, ask = self.n, entry["project"], asked(entry)
        d = self.gate(text, ask, option)  # до чтения экрана: по реплике хода, в котором мозг вызвал tell
        item, question = chosen(ask, option), f"Передать проекту {p}: {gist}?"
        expect = item.split(" — ")[0]  # метка варианта, как её видел мозг: tell жмёт, только если в вызове агента та же
        if option and ask.startswith(PLAN):  # у плана метка пункта — только на экране: читаем до вопроса и называем её
            ok, out = await self.run("menu", entry["id"])
            item = expect = menu_item(out, option) if ok else ""
            if not item:
                return f"Не передано: {out if not ok else f'на экране нет пункта {option}'}. Скажи пользователю коротко, что не вышло."
            question = f"Одобрить план проекта {p} — пункт «{item}»?"
        shown = f"Передам проекту {p}: " + (f"вариант {option} — {item}" if option else f"«{text}»")
        args = ("tell", entry["id"], str(option), text) + ((expect,) if option and expect else ())
        return await self._order(args, d, n, f"Передала проекту {p}: {gist}.",
                                 f"Передано проекту {p}. Скажи пользователю одной фразой, кому и что: "
                                 f"«Передала проекту {p}: …» — смысл, не текст задания.",
                                 question, "передай", shown)

    async def launch(self, path, task, gist, where="orca"):
        name = _name(path)
        return await self._order(("launch", path, task, where), self.gate(task), self.n, f"Запустила агента в проекте {name}: {gist}.",
                                 f"Агент запущен в проекте {name}. Скажи пользователю одной фразой: "
                                 f"«Запустила агента в проекте {name}: …» — смысл задания.",
                                 f"Запустить агента в проекте {name}: {gist}?", "запускай", f"Запущу агента в проекте {name}: «{task}»")

    async def projects(self, where="orca"):
        ok, out = await self.run("projects", where)
        if not ok:
            return f"Список проектов недоступен: {out}."
        return "\n".join(f"{_name(l)} — {l}" for l in out.splitlines() if l.strip()) or "Проектов нет."

    async def confirm(self, text):
        """Реплика голосом, пока поручение ждёт: фраза совпала — передать и вернуть фразу Омы. Попытка подтвердить
        не той фразой — переспросить, ожидание держится; «нет», «не подтверждаю» — отказ. Другая реплика («продолжай»)
        снимает ожидание, ничего не уходит: None, реплика — мозгу с пометкой."""
        p = self.pending
        if not p or self.clock() > p["until"]:
            self.pending = None
            return None
        ws, d = words(text), p["what"]
        if NO & set(ws) or (ws and ws[0].startswith(NEGATIVE)):
            self.pending = None
            self.note = "(Пользователь не подтвердил — Ома ничего не передала.) "
            return "Хорошо, не передаю."
        short = len(ws) <= 4
        ok = short and "да" in ws and _has(ws, GO) if d is ORDINARY else confirms(text, d)
        if not ok:
            if short and ("да" in ws or _has(ws, GO + ("подтвержд",))):
                return f"Не расслышала. Скажи: да, {p['verb']}." if d is ORDINARY else f"Не расслышала. Скажи: подтверждаю {d[1]}."
            self.pending = None
            self.note = "(Пользователь не подтвердил — Ома ничего не передала.) "
            return None
        self.pending = None
        ok, out = await self.run(*p["args"])
        said = (p["said"] + (f" Нажала «{out}»." if out else "")) if ok else f"Не вышло: {out}."  # tell: метка с экрана
        self.note = f"(Пользователь подтвердил голосом, Ома сама: {said}) "
        return said
