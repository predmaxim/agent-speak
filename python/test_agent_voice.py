# python -m pytest python/test_agent_voice.py  (из voice-venv или любого python3 с pytest)
import sys, os
sys.path.insert(0, os.path.dirname(__file__))
from agent_voice.segment import Segmenter
from agent_voice.chunker import Chunker
from agent_voice.conversation import Conversation


def feed(seg, probs):
    out = []
    for i, p in enumerate(probs):
        out += seg.feed(p, bytes([i % 256]) * 1024)
    return out


def test_short_blip_is_not_speech():
    assert feed(Segmenter(), [0.9] * 7 + [0.1] * 40) == []


def test_phrase_after_pause_with_preroll():
    seg = Segmenter()
    ev = feed(seg, [0.1] * 10 + [0.9] * 20 + [0.1] * 22)
    assert ev[0] == ("start",)
    assert ev[1][0] == "phrase"
    # предзахват: 8 кадров до старта + 20 речи + 22 тишины
    assert len(ev[1][1]) == (8 + 20 + 22) * 1024
    assert len(ev) == 2


def test_short_pause_keeps_one_phrase():
    ev = feed(Segmenter(), [0.9] * 10 + [0.1] * 10 + [0.9] * 10 + [0.1] * 22)
    assert [e[0] for e in ev] == ["start", "phrase"]


def test_chunker_sentences_and_min_len():
    c = Chunker()
    assert c.feed("Да. Сейчас посмотрю файл конфигурации") == []  # «Да.» короче 20 — ждёт
    assert c.feed(" сервиса. Ещё") == ["Да. Сейчас посмотрю файл конфигурации сервиса."]
    assert c.flush() == ["Ещё"]
    assert c.flush() == []


def test_chunker_newline_ends_sentence():
    c = Chunker()
    assert c.feed("Первая строка достаточно длинная\nвторая") == ["Первая строка достаточно длинная"]


def test_conversation_turn():
    cv = Conversation()
    assert cv.phrase(" а ") == []  # короче 2 символов после strip — ничего
    assert cv.phrase("Что в этом проекте?") == [("send", "Что в этом проекте?"), ("state", "thinking")]
    assert cv.delta("Это демон озвучки агентов, написан на Rust. И") == [
        ("say", "Это демон озвучки агентов, написан на Rust."), ("state", "speaking")]
    cv.audio(True)
    assert cv.done() == [("say", "И")]
    assert cv.audio(False) == [("state", "listening")]


def test_barge_in_while_speaking_stops_and_interrupts():
    cv = Conversation()
    cv.phrase("Расскажи подробно")
    cv.delta("Длинный ответ про устройство демона. Дальше")
    cv.audio(True)
    assert cv.speech_start() == [("stop",), ("interrupt",), ("state", "listening")]
    assert cv.delta(" хвост старого хода.") == []  # прерванный ход не озвучивается
    assert cv.done() == []


def test_barge_in_while_thinking_only_interrupts():
    cv = Conversation()
    cv.phrase("Подумай")
    assert cv.speech_start() == [("interrupt",), ("state", "listening")]


def test_speech_while_listening_does_nothing():
    assert Conversation().speech_start() == []


def test_barge_in_after_turn_done_only_stops_audio():
    cv = Conversation()
    cv.phrase("Скажи")
    cv.delta("Готово, всё проверил полностью. ")
    cv.audio(True)  # ответ ещё звучит, когда ход закончился
    assert cv.done() == []
    assert cv.speech_start() == [("stop",), ("state", "listening")]


def test_short_answer_under_min_len():
    cv = Conversation()
    cv.phrase("Ты тут?")
    cv.delta("Да.")
    assert cv.done() == [("say", "Да."), ("state", "speaking")]


import json
from types import SimpleNamespace
from agent_voice.agents import CodexProtocol, claude_events


def test_codex_requests_and_turn_filter():
    p = CodexProtocol()
    rid, line = p.request("turn/start", {"threadId": "t1", "input": [{"type": "text", "text": "привет"}]})
    assert json.loads(line) == {"id": rid, "method": "turn/start", "params": {"threadId": "t1", "input": [{"type": "text", "text": "привет"}]}}
    assert line.endswith("\n")
    assert p.handle(json.dumps({"id": rid, "result": {"turn": {"id": "u1", "items": [], "status": "inProgress"}}})) == [("response", rid, {"turn": {"id": "u1", "items": [], "status": "inProgress"}})]
    p.turn_id = "u1"
    d = lambda turn, text: json.dumps({"method": "item/agentMessage/delta", "params": {"delta": text, "itemId": "i", "threadId": "t1", "turnId": turn}})
    assert p.handle(d("u1", "Привет")) == [("delta", "Привет")]
    assert p.handle(d("u0", "старое")) == []  # дельта прерванного хода
    done = lambda turn: json.dumps({"method": "turn/completed", "params": {"threadId": "t1", "turn": {"id": turn, "items": [], "status": "completed"}}})
    assert p.handle(done("u0")) == []
    assert p.handle(done("u1")) == [("done", None)]
    assert p.turn_id is None
    assert p.handle("не json") == []


def test_codex_error_response():
    p = CodexProtocol()
    rid, _ = p.request("thread/start", {})
    assert p.handle(json.dumps({"id": rid, "error": {"code": -1, "message": "not logged in"}})) == [("error", "not logged in")]


def test_claude_events_text_and_skip():
    st = {"skip": False}
    delta = lambda t: SimpleNamespace(event={"type": "content_block_delta", "delta": {"type": "text_delta", "text": t}})
    ResultMessage = type("ResultMessage", (), {})
    assert claude_events(delta("Да"), st) == [("delta", "Да")]
    assert claude_events(SimpleNamespace(event={"type": "message_start"}), st) == []
    assert claude_events(ResultMessage(), st) == [("done", None)]
    st["skip"] = True  # после interrupt: хвост прерванного хода не нужен
    assert claude_events(delta("хвост"), st) == []
    assert claude_events(ResultMessage(), st) == []
    assert st["skip"] is False
