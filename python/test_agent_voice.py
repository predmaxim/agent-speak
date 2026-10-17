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


def test_talk_draft_send():
    cv = Conversation()
    assert cv.talk() == [("ear", True), ("state", "listening")]
    assert cv.phrase("какая погода") == [("draft", "какая погода")]
    assert cv.phrase("в Москве") == [("draft", "какая погода в Москве")]
    assert cv.talk() == [("drop_draft",), ("ear", False), ("send", "какая погода в Москве"), ("state", "thinking")]


def test_talk_without_draft_cancels():
    cv = Conversation()
    cv.talk()
    assert cv.talk() == [("drop_draft",), ("ear", False), ("state", "idle")]


def test_answer_shown_and_said_then_idle():
    cv = Conversation()
    cv.talk(); cv.phrase("вопрос тут"); cv.send()
    acts = cv.delta("Солнечно, плюс двадцать градусов. ")
    assert ("show", "Солнечно, плюс двадцать градусов.") in acts and ("say", "Солнечно, плюс двадцать градусов.") in acts
    assert acts[-1] == ("state", "speaking")
    cv.audio(True)
    assert cv.done() == []
    assert cv.audio(False) == [("state", "idle")]


def test_quiet_shows_without_saying():
    cv = Conversation()
    cv.set_quiet(True)
    cv.ask("который час")
    acts = cv.delta("Сейчас ровно три часа дня. ")
    assert ("show", "Сейчас ровно три часа дня.") in acts and not any(a[0] == "say" for a in acts)
    assert cv.done() == [("state", "idle")]


def test_dialog_sends_each_phrase_and_barges_in():
    cv = Conversation()
    assert cv.set_dialog(True) == [("ear", True), ("state", "listening")]
    assert cv.talk() == []
    assert cv.phrase("как дела у почты") == [("send", "как дела у почты"), ("state", "thinking")]
    cv.delta("Почта ждёт твоего ответа сейчас. ")
    cv.audio(True)
    assert cv.speech_start() == [("stop",), ("interrupt",), ("state", "listening")]
    assert cv.set_dialog(False) == [("ear", False), ("state", "idle")]


def test_click_while_speaking_interrupts_and_listens():
    cv = Conversation()
    cv.ask("расскажи")
    cv.delta("Длинный ответ про всё сразу. ")
    cv.audio(True)
    assert cv.talk() == [("stop",), ("interrupt",), ("ear", True), ("state", "listening")]


def test_statuses_audio_does_not_end_draft():
    cv = Conversation()
    cv.talk(); cv.phrase("подожди")
    assert cv.audio(True) == [] and cv.audio(False) == []
    assert (cv.state, cv.draft) == ("listening", "подожди")


def test_mic_busy_pauses_listening():
    cv = Conversation()
    cv.set_dialog(True)
    assert cv.mic_busy(True) == [("state", "paused")]
    assert cv.phrase("это для voxtype") == []
    assert cv.mic_busy(False) == [("state", "listening")]


def test_short_phrases_dropped():
    cv = Conversation()
    cv.talk()
    assert cv.phrase(" а ") == [] and cv.ask("ж") == []

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
    assert p.handle(json.dumps({"id": rid, "error": {"code": -1, "message": "not logged in"}})) == [("response_error", rid, "not logged in")]


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


def test_codex_failed_turn_is_error():
    p = CodexProtocol()
    p.turn_id = "u1"
    err = {"method": "error", "params": {"error": {"message": "model not supported"}, "threadId": "t1", "turnId": "u1", "willRetry": False}}
    assert p.handle(json.dumps(err)) == []
    failed = {"method": "turn/completed", "params": {"threadId": "t1", "turn": {"id": "u1", "items": [], "status": "failed"}}}
    assert p.handle(json.dumps(failed)) == [("error", "Codex: model not supported")]
    assert p.turn_id is None
    p.turn_id = "u2"
    failed["params"]["turn"]["id"] = "u2"
    assert p.handle(json.dumps(failed)) == [("error", "Codex: ход не удался")]


import asyncio
from agent_voice.agents import CodexAgent


def _fake_agent():
    a = CodexAgent(asyncio.Queue())
    out = []
    a.proc = SimpleNamespace(
        stdin=SimpleNamespace(write=out.append, drain=lambda: asyncio.sleep(0)),
        stdout=asyncio.StreamReader())
    a.reader = asyncio.create_task(a._read())
    return a, out


def test_codex_plumbing_error_response_and_eof():
    async def run():
        a, out = _fake_agent()
        call = asyncio.create_task(a._call("thread/start", {}))
        await asyncio.sleep(0)
        rid = json.loads(out[0])["id"]
        a.proc.stdout.feed_data(json.dumps({"id": rid, "error": {"message": "no auth"}}).encode() + b"\n")
        try:
            await call
            assert False, "must raise"
        except RuntimeError as e:
            assert "no auth" in str(e)
        pending = asyncio.create_task(a._call("turn/start", {}))
        await asyncio.sleep(0)
        a.proc.stdout.feed_eof()
        try:
            await asyncio.wait_for(pending, 1)
            assert False, "must raise"
        except RuntimeError:
            pass
        assert (await a.events.get())[0] == "error"
    asyncio.run(run())


def test_codex_interrupt_during_send():
    async def run():
        a, out = _fake_agent()
        a.p.thread_id = "t1"
        send = asyncio.create_task(a.send("привет"))
        await asyncio.sleep(0)
        intr = asyncio.create_task(a.interrupt())
        await asyncio.sleep(0)
        rid = json.loads(out[0])["id"]
        a.proc.stdout.feed_data(json.dumps({"id": rid, "result": {"turn": {"id": "u1"}}}).encode() + b"\n")
        await asyncio.sleep(0.01)
        req = json.loads(out[1])
        assert req["method"] == "turn/interrupt" and req["params"]["turnId"] == "u1"
        a.proc.stdout.feed_data(json.dumps({"id": req["id"], "result": {}}).encode() + b"\n")
        await asyncio.wait_for(asyncio.gather(send, intr), 1)
        assert a.p.turn_id is None
        a.proc.stdout.feed_eof()
    asyncio.run(run())


def test_real_vad_one_phrase_from_synthetic_speech():
    # 512 отсчётов float32 @16 кГц → вероятность; реальный Silero VAD + Segmenter, без микрофона
    import pytest
    pytest.importorskip("silero_vad")
    import numpy as np, torch
    from silero_vad import load_silero_vad
    vad = load_silero_vad()
    # речь — внешний файл (s16 моно 16 кГц, напр. из Silero TTS): AGENT_VOICE_SPEECH_PCM
    silence = vad(torch.zeros(512), 16000).item()
    assert silence < 0.5
    path = os.environ.get("AGENT_VOICE_SPEECH_PCM")
    if not path or not os.path.exists(path):
        pytest.skip("AGENT_VOICE_SPEECH_PCM не задан")
    pcm = np.frombuffer(open(path, "rb").read(), np.int16)
    pcm = np.concatenate([np.zeros(8000, np.int16), pcm, np.zeros(16000, np.int16)])
    seg, evs = Segmenter(), []
    for i in range(len(pcm) // 512):
        b = pcm[i * 512:(i + 1) * 512].tobytes()
        x = torch.from_numpy(np.frombuffer(b, np.int16).astype(np.float32) / 32768)
        evs += seg.feed(vad(x, 16000).item(), b)
    assert [e[0] for e in evs] == ["start", "phrase"]


def test_link_reconnects_after_server_closes(tmp_path):
    import socket
    from agent_voice import link
    path = str(tmp_path / "s.sock")
    srv = socket.socket(socket.AF_UNIX)
    srv.bind(path)
    srv.listen(2)
    link.SOCK, link._conn = path, None
    assert link.send({"a": 1})
    c1, _ = srv.accept()
    assert link.send({"a": 2})  # то же соединение
    assert c1.recv(100) == b'{"a": 1}\n{"a": 2}\n'
    c1.close()  # демон закрыл молчуна
    ok = [link.send({"a": i}) for i in (3, 4)]  # первая запись может «уйти» в закрытый сокет
    assert all(ok)
    c2, _ = srv.accept()
    assert b'{"a": 4}' in c2.recv(100)


# --- контекст Омы ---
from agent_voice.context import transcript_tail, foreign_capture, on_ac, selection


def test_transcript_tail_keeps_dialogue_and_trims(tmp_path):
    rows = [
        {"type": "user", "message": {"content": "почини сборку"}},
        {"type": "assistant", "message": {"content": [{"type": "text", "text": "Смотрю ошибку."}, {"type": "tool_use", "name": "Bash", "input": {}}]}},
        {"type": "user", "message": {"content": [{"type": "tool_result", "content": "error: E0425 " + "x" * 900}]}},
        {"type": "assistant", "message": {"content": [{"type": "text", "text": "Не хватает импорта."}]}},
    ]
    p = tmp_path / "t.jsonl"
    p.write_text("\n".join(json.dumps(r, ensure_ascii=False) for r in rows) + "\nобрывок")
    t = transcript_tail(str(p))
    assert t.splitlines()[0] == "ты: почини сборку"
    assert "агент: Смотрю ошибку." in t and "результат: error: E0425" in t
    assert t.endswith("агент: Не хватает импорта.")
    assert max(len(l) for l in t.splitlines()) <= 520
    assert len(transcript_tail(str(p), limit=40)) <= 40
    assert transcript_tail(str(tmp_path / "нет.jsonl")) == ""


def test_foreign_capture_ignores_virtual_and_own():
    out = lambda pid: {"properties": {"application.process.id": str(pid)}}
    assert not foreign_capture([{"properties": {"node.virtual": "true"}}], {42})
    assert not foreign_capture([out(42)], {42})
    assert foreign_capture([out(7)], {42})
    assert not foreign_capture([], set())


def test_on_ac(tmp_path):
    def supply(name, typ, online=None):
        d = tmp_path / name
        d.mkdir()
        (d / "type").write_text(typ + "\n")
        if online is not None:
            (d / "online").write_text(online + "\n")
    assert on_ac(str(tmp_path))
    supply("BAT0", "Battery")
    supply("ADP0", "Mains", "0")
    assert not on_ac(str(tmp_path))
    (tmp_path / "ADP0" / "online").write_text("1\n")
    assert on_ac(str(tmp_path))


def test_selection_trims_and_survives_errors():
    assert len(selection(run=lambda *a, **k: SimpleNamespace(returncode=0, stdout="я" * 3000))) == 2000
    def boom(*a, **k):
        raise FileNotFoundError
    assert selection(run=boom) == ""


def test_transcript_tail_caps_long_message(tmp_path):
    row = {"type": "assistant", "message": {"content": [{"type": "text", "text": "я" * 5000}]}}
    p = tmp_path / "t.jsonl"
    p.write_text(json.dumps(row, ensure_ascii=False))
    last = transcript_tail(str(p)).splitlines()[-1]
    assert last.startswith("агент: ") and len(last) == len("агент: ") + 1001 and last.endswith("…")


def test_foreign_capture_skips_corked():
    assert not foreign_capture([{"corked": True, "properties": {"application.process.id": "7"}}], set())


def test_on_ac_usb_counts(tmp_path):
    for n, typ, on in (("A", "Mains", "0"), ("U", "USB", "1")):
        (tmp_path / n).mkdir()
        (tmp_path / n / "type").write_text(typ)
        (tmp_path / n / "online").write_text(on)
    assert on_ac(str(tmp_path))


def test_ask_from_talk_listening_closes_ear():
    cv = Conversation()
    cv.talk(); cv.phrase("черновик тут")
    acts = cv.ask("который час")
    assert ("ear", False) in acts
    cv.delta("Сейчас ровно три часа дня. ")
    cv.audio(True); cv.done()
    cv.audio(False)
    assert cv.state == "idle"


def test_click_stops_status_audio_and_keeps_draft():
    cv = Conversation()
    cv.talk(); cv.phrase("подожди меня"); cv.audio(True)
    acts = cv.talk()
    assert acts[0] == ("stop",) and ("send", "подожди меня") in acts
