"""Контекст для Омы: хвост транскрипта агента, выделение, захват микрофона, питание."""
import json
import os
import subprocess
import time


INJECTED = ("<system-reminder>", "<command-", "<local-command")  # вставки Claude Code в реплики пользователя


def _blocks(content):
    return [{"type": "text", "text": content}] if isinstance(content, str) else (content or [])


def transcript_tail(path: str, limit: int = 4000) -> str:
    """Диалог из jsonl-транскрипта Claude: строки «ты/агент/результат», последние limit символов."""
    try:
        with open(path, "rb") as f:
            f.seek(0, os.SEEK_END)
            f.seek(max(0, f.tell() - 256 * 1024))
            raw = f.read().decode("utf-8", "replace")
    except OSError:
        return ""
    lines = []
    for row in raw.splitlines():
        try:
            d = json.loads(row)
            kind = d["type"]
            blocks = _blocks(d["message"]["content"])
        except (ValueError, KeyError, TypeError):
            continue
        if kind == "user" and d.get("isMeta"):  # служебные строки, а не слова пользователя
            continue
        for b in blocks:
            if not isinstance(b, dict):
                continue
            if b.get("type") == "text" and kind in ("user", "assistant"):
                t = str(b.get("text", ""))
                if kind == "user" and t.lstrip().startswith(INJECTED):
                    continue
                if len(t) > 1000:
                    t = t[:1000] + "…"
                lines.append(("ты: " if kind == "user" else "агент: ") + t)
            elif b.get("type") == "tool_result":
                parts = [x.get("text", "") for x in _blocks(b.get("content")) if isinstance(x, dict)]
                lines.append(("результат: " + " ".join(parts))[:500])
    out = []
    size = 0
    for l in reversed(lines):  # целые строки с конца, пока влезают
        if size + len(l) + 1 > limit:
            break
        out.append(l)
        size += len(l) + 1
    return "\n".join(reversed(out))


def selection(run=subprocess.run) -> str:
    """Текущее выделение мыши (primary), до 2000 символов."""
    try:
        r = run(["wl-paste", "--primary", "--no-newline"], capture_output=True, text=True, timeout=1)
        return r.stdout[:2000] if r.returncode == 0 else ""
    except Exception:
        return ""


def mic_outputs() -> list:
    try:
        r = subprocess.run(["pactl", "-f", "json", "list", "source-outputs"],
                           capture_output=True, text=True, timeout=2)
        return json.loads(r.stdout)
    except Exception:
        return []


# Имя узла записи Омы: PipeWire не отдаёт pid у pw-record, свой поток узнаём по нему.
OWN_NODE = "oma-ear"


def foreign_capture(outputs: list, own: set) -> bool:
    """Микрофон слушает не виртуальный поток чужого процесса (монитор выхода — cava, OBS — не в счёт)."""
    own = {str(x) for x in own}
    for o in outputs:
        p = o.get("properties", {})
        if o.get("corked") or "true" in (p.get("stream.monitor"), p.get("stream.capture.sink")) or p.get("node.name") == OWN_NODE:
            continue
        if p.get("node.virtual") != "true" and p.get("application.process.id") not in own:
            return True
    return False


PRESENCE = os.path.join(os.environ.get("XDG_RUNTIME_DIR", "/tmp"), "predmaxim-presence.json")
STALE_AFTER = 90  # с: снимок старше — сервиса присутствия нет (как presence::STALE_AFTER демона)


def buds_out(snapshot: str, now: float) -> bool:
    """По свежему снимку сервиса присутствия оба наушника вынуты."""
    try:
        d = json.loads(snapshot)
        return d.get("buds") == "out" and now - float(d.get("at", 0)) <= STALE_AFTER
    except (ValueError, TypeError, AttributeError):
        return False


def pick_mic(source: str, sink: str, sources: str, out: bool) -> str:
    """Вход для слуха Омы. Шумодав (mic_denoise, RNNoise) под вентиляторами вырезает целые слова — его Ома не слушает.
    Говорит в наушники — эха нет: сырой микрофон ноутбука; в динамики (наушников нет или вынуты, out) — mic_aec,
    эхоподавление без шумодава; гарнитура в ушах — её микрофон: "" — вход по умолчанию."""
    if source.startswith("bluez_") and not out:
        return ""
    names = [p[1] for p in (l.split("\t") for l in sources.splitlines()) if len(p) > 1]
    names = [n for n in names if not n.startswith("bluez_") and not n.endswith(".monitor")]
    speakers = out or not sink.startswith("bluez_")
    for prefix in (("mic_aec", "alsa_input.") if speakers else ("alsa_input.",)):
        for n in names:
            if n.startswith(prefix):
                return n
    return ""


def mic_target(run=subprocess.run) -> str:
    """Куда слушать — по тому, куда Ома говорит (pick_mic); сбой — вход по умолчанию."""
    try:
        try:
            with open(PRESENCE) as f:
                out = buds_out(f.read(), time.time())
        except OSError:
            out = False  # сервиса присутствия нет
        q = lambda *a: run(["pactl", *a], capture_output=True, text=True, timeout=2).stdout
        return pick_mic(q("get-default-source").strip(), q("get-default-sink").strip(), q("list", "short", "sources"), out)
    except Exception:
        return ""


def locked() -> bool:
    """Экран заблокирован: omarchy-hyprland-session-locked вернул 0 (1 — нет, 2 — не определить, как «нет»)."""
    try:
        return subprocess.run(["omarchy-hyprland-session-locked"], capture_output=True, timeout=3).returncode == 0
    except Exception:
        return False


def on_ac(root: str = "/sys/class/power_supply") -> bool:
    """От сети ли питание (Mains или USB online); без таких источников (настольный) — True."""
    ac = []
    try:
        names = os.listdir(root)
    except OSError:
        return True
    for n in names:
        try:
            with open(f"{root}/{n}/type") as f:
                typ = f.read().strip()
            if typ in ("Mains", "USB"):
                with open(f"{root}/{n}/online") as f:
                    ac.append(f.read().strip() == "1")
        except OSError:
            pass
    return any(ac) if ac else True
