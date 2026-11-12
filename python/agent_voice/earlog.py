"""Диагностика слуха — выключена, пока нет каталога DIR (`$XDG_RUNTIME_DIR/agent-speak-ear`, tmpfs, только пользователю).
Есть — на каждую распознанную фразу Ома кладёт туда папку: звук куска (phrase.wav), последние RING с микрофона до конца
куска (stream.wav — видно, съедено ли начало до куска), распознанное, решение разговора (принята, эхо, не принята; её
слова и состояние), реплику целиком. Держит последние KEEP. Включить — `mkdir -m700 $XDG_RUNTIME_DIR/agent-speak-ear`,
выключить — удалить каталог. Пользователь разрешил запись 2026-11-12: «съедается начало фразы» иначе не разобрать."""
import collections
import json
import os
import shutil
import time
import wave

DIR = os.path.join(os.environ.get("XDG_RUNTIME_DIR") or "/tmp", "agent-speak-ear")
KEEP = 10
RATE, FRAME = 16000, 512
RING = 20  # с звука с микрофона до конца куска

_ring = collections.deque(maxlen=RING * RATE // FRAME)
_checked, _on = 0.0, False
_last = {}  # номер речи → её папка


def on(now=None):
    """Каталог есть — раз в секунду, а не на каждый кадр (31 кадр в секунду)."""
    global _checked, _on
    now = time.monotonic() if now is None else now
    if now - _checked >= 1.0:
        _checked, _on = now, os.path.isdir(DIR)
    return _on


def frame(b):
    if on():
        _ring.append(b)
    elif _ring:
        _ring.clear()


def _wav(path, pcm):
    with wave.open(path, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(RATE)
        w.writeframes(pcm)


def _info(path, **kw):
    f = os.path.join(path, "info.json")
    try:
        with open(f) as fh:
            d = json.load(fh)
    except (OSError, ValueError):
        d = {}
    d.update(kw)
    with open(f, "w") as fh:
        json.dump(d, fh, ensure_ascii=False, indent=1)


def phrase(n, pcm, text):
    """Кусок распознан: его звук, звук микрофона до его конца и текст."""
    if not on():
        return
    try:
        path = os.path.join(DIR, f"{time.strftime('%Y%m%d-%H%M%S')}-{n:06d}")  # имена сортируются по порядку: старые — первыми
        os.makedirs(path, exist_ok=True)
        _wav(os.path.join(path, "phrase.wav"), pcm)
        _wav(os.path.join(path, "stream.wav"), b"".join(_ring))
        _info(path, n=n, at=time.strftime("%H:%M:%S"), text=text, phrase_s=round(len(pcm) / 2 / RATE, 2))
        _last[n] = path
        old = sorted(os.listdir(DIR))[:-KEEP]
        for d in old:
            shutil.rmtree(os.path.join(DIR, d), ignore_errors=True)
        for k in [k for k, p in _last.items() if not os.path.isdir(p)]:
            del _last[k]
    except OSError:
        pass  # диагностика не мешает слуху


def verdict(n, **kw):
    """Что разговор сделал с куском n: принята / эхо / не принята, её слова и состояние."""
    path = _last.get(n)
    if path and on():
        try:
            _info(path, **kw)
        except OSError:
            pass


def whole(by_parts, text):
    """Реплика при отправке: по кускам и целиком — в папку последнего куска."""
    if _last:
        verdict(max(_last), sent_by_parts=by_parts, sent_whole=text)
