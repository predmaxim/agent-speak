"""Разметка интонации адаптера Silero (`ssml`) и её путь в модель — без модели."""
import os
import sys

import torch

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from silero_tts import MAX, ssml, synth  # noqa: E402

BREAK = '<break time="250ms"/>'


def test_plain_text_when_nothing_to_mark():
    assert ssml("Привет.") == "Привет."
    assert ssml("готово.", tone="done", rate="slow") == "готово.", "шаг быстрее от slow — medium: разметки нет"
    assert ssml("Привет.", tone="nonsense") == "Привет."


def test_rate_alone_as_before():
    assert ssml("Привет.", rate="fast") == '<speak><prosody rate="fast">Привет.</prosody></speak>'


def test_tones():
    assert ssml("куда пушить?", tone="ask") == '<speak><prosody pitch="high">куда пушить?</prosody></speak>'
    assert ssml("висит.", tone="alarm") == '<speak><prosody pitch="low" rate="slow">висит.</prosody></speak>'
    assert ssml("готово.", tone="done") == '<speak><prosody rate="fast">готово.</prosody></speak>'


def test_rate_step_stops_at_the_edges():
    assert ssml("висит.", tone="alarm", rate="x-slow") == '<speak><prosody pitch="low" rate="x-slow">висит.</prosody></speak>'
    assert ssml("готово.", tone="done", rate="x-fast") == '<speak><prosody rate="x-fast">готово.</prosody></speak>'
    assert ssml("готово.", tone="done", rate="fast") == '<speak><prosody rate="x-fast">готово.</prosody></speak>'


def test_lead_gets_colon_and_pause():
    assert ssml("нужен ответ?", lead="агент спик") == f"<speak>агент спик:{BREAK} нужен ответ?</speak>"
    assert ssml("нужен ответ?", lead="плуг", tone="ask") == f'<speak><prosody pitch="high">плуг:{BREAK} нужен ответ?</prosody></speak>'


def test_escaped_and_no_paragraphs():
    out = ssml("a < b & c > d", lead="x<y", tone="ask")
    assert "<y" not in out and "< b" not in out and "&" not in out and "> d" not in out
    for case in [ssml("Да.", lead="п", tone=t, rate=r) for t in ("", "ask", "alarm", "done") for r in ("x-slow", "medium", "x-fast")]:
        assert "<p>" not in case and "<s>" not in case, "обёртка <p><s> теряла вопрос (каталог 2026-11-11)"


class FakeModel:
    def __init__(self):
        self.calls = []

    def apply_tts(self, **kw):
        self.calls.append(kw)
        return torch.zeros(4)


def test_synth_sends_ssml_and_lead_only_once():
    m = FakeModel()
    synth(m, "Первое предложение. " * (MAX // 10), "kseniya", "medium", "ask", "плуг")
    assert len(m.calls) > 1
    assert "ssml_text" in m.calls[0] and "плуг:" in m.calls[0]["ssml_text"]
    assert all("плуг" not in c["ssml_text"] for c in m.calls[1:]), "проект — только перед первым куском"
    m = FakeModel()
    synth(m, "Привет.", "kseniya", "medium")
    assert m.calls == [dict(text="Привет.", speaker="kseniya", sample_rate=48000, put_accent=True, put_yo=True)]
