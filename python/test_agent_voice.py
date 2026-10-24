# python -m pytest python/test_agent_voice.py  (из voice-venv или любого python3 с pytest)
import sys, os
sys.path.insert(0, os.path.dirname(__file__))
from agent_voice.segment import Segmenter
from agent_voice.chunker import Chunker
from agent_voice.conversation import Conversation, SEND_PAUSE, SILENCE_END


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


class Clock:
    t = 0.0

    def __call__(self):
        return self.t


def _conv():
    clk = Clock()
    return clk, Conversation(clock=clk)


def _ask_answer(clk, cv):
    """В разговоре: вопрос → пауза → отправка → ответ звучит."""
    cv.talk(); cv.phrase("вопрос тут", 1)
    clk.t += SEND_PAUSE; cv.tick()
    cv.delta("Солнечно, плюс двадцать градусов. ")
    cv.audio(True)


def test_talk_listens_and_pause_sends():
    clk, cv = _conv()
    assert cv.talk() == [("ear", True), ("state", "listening")] and cv.active
    assert cv.speech_start(1) == [("draft", "", True)]
    assert cv.phrase("какая погода", 1) == [("draft", "какая погода", False)]
    clk.t += 1.0; assert cv.tick() == []
    assert cv.speech_start(2) == [("draft", "какая погода", True)]
    clk.t += 5; assert cv.tick() == []                      # пока слышит — не отправляет
    assert cv.phrase("в Москве", 2) == [("draft", "какая погода в Москве", False)]
    clk.t += SEND_PAUSE
    assert cv.tick() == [("drop_draft",), ("send", "какая погода в Москве"), ("state", "thinking")]


def test_answer_then_listens_again():
    clk, cv = _conv()
    cv.talk(); cv.phrase("вопрос тут", 1)
    clk.t += SEND_PAUSE; cv.tick()
    assert cv.state == "thinking"
    acts = cv.delta("Солнечно, плюс двадцать градусов. ")
    assert ("show", "Солнечно, плюс двадцать градусов.") in acts and ("say", "Солнечно, плюс двадцать градусов.") in acts
    assert acts[-1] == ("state", "speaking")
    cv.audio(True)
    assert cv.done() == []
    assert cv.audio(False) == [("state", "listening")] and cv.active


def test_click_while_listening_ends_and_drops_draft():
    clk, cv = _conv()
    cv.talk(); cv.speech_start(1); cv.phrase("черновик тут", 1)
    assert cv.talk() == [("drop_draft",), ("ear", False), ("state", "idle")] and not cv.active


def test_click_while_speaking_interrupts_and_listens():
    clk, cv = _conv()
    _ask_answer(clk, cv)
    assert cv.talk() == [("stop",), ("interrupt",), ("state", "listening")]


def test_click_while_speaking_typed_answer_starts_conversation():
    clk, cv = _conv()
    cv.ask("расскажи"); cv.delta("Длинный ответ про всё сразу. "); cv.audio(True)
    assert cv.talk() == [("stop",), ("interrupt",), ("ear", True), ("state", "listening")] and cv.active


def test_cancel_drops_phrase_in_flight():
    clk, cv = _conv()
    cv.talk(); cv.speech_start(1); cv.phrase("раз два", 1); cv.speech_start(2)
    assert cv.cancel() == [("drop_draft",)]
    assert cv.phrase("три четыре", 2) == []                 # распознано после отмены
    assert cv.speech_start(3) == [("draft", "", True)]
    assert cv.phrase("пять шесть", 3) == [("draft", "пять шесть", False)]
    cv.cancel(); assert cv.cancel() == []                    # отменять нечего


def test_silence_ends_conversation():
    clk, cv = _conv()
    cv.talk(); clk.t += SILENCE_END - 0.1; assert cv.tick() == []
    clk.t += 0.2; assert cv.tick() == [("ear", False), ("state", "idle")]
    assert cv.end() == []


def test_silence_not_counted_while_hearing_or_answering():
    clk, cv = _conv()
    cv.talk(); cv.speech_start(1); clk.t += 60; assert cv.tick() == []
    cv.phrase("вопрос тут", 1); clk.t += SEND_PAUSE; cv.tick()  # → thinking
    clk.t += 60; assert cv.tick() == []
    cv.done(); clk.t += SILENCE_END - 1; assert cv.tick() == []  # счёт — заново с «слушаю»


def test_end_stops_turn():
    clk, cv = _conv()
    cv.talk(); cv.phrase("вопрос тут", 1)
    clk.t += SEND_PAUSE; cv.tick()
    assert cv.end() == [("interrupt",), ("ear", False), ("state", "idle")]


def test_phrase_after_end_dropped():
    clk, cv = _conv()
    cv.talk(); cv.speech_start(1); cv.talk()
    assert cv.phrase("поздно уже", 1) == [] and cv.draft == ""


def test_voice_barge_in():
    clk, cv = _conv()
    cv.talk(); cv.phrase("вопрос тут", 1)
    clk.t += SEND_PAUSE; cv.tick()
    cv.delta("Сборка идёт уже десять минут. "); cv.audio(True)
    assert cv.speech_start(2) == [("stop",), ("interrupt",), ("state", "listening"), ("draft", "", True)]


def test_mic_busy_clears_hearing():
    clk, cv = _conv()
    cv.talk(); cv.speech_start(1)
    assert cv.mic_busy(True) == [("state", "paused"), ("drop_draft",)]
    assert cv.phrase("это для voxtype", 1) == []
    assert cv.mic_busy(False) == [("state", "listening")]
    clk.t += SILENCE_END; assert cv.tick() == [("ear", False), ("state", "idle")]


def test_ask_in_conversation_keeps_listening():
    clk, cv = _conv()
    cv.talk(); cv.speech_start(1); cv.phrase("черновик тут", 1)
    assert cv.ask("который час") == [("drop_draft",), ("send", "который час"), ("state", "thinking")]
    cv.delta("Сейчас ровно три часа дня. "); cv.audio(True); cv.done(); cv.audio(False)
    assert cv.state == "listening" and cv.active


def test_quiet_shows_without_saying():
    clk, cv = _conv()
    cv.set_quiet(True)
    cv.ask("который час")
    acts = cv.delta("Сейчас ровно три часа дня. ")
    assert ("show", "Сейчас ровно три часа дня.") in acts and not any(a[0] == "say" for a in acts)
    assert cv.done() == [("state", "idle")]


def test_short_phrases_dropped():
    clk, cv = _conv()
    cv.talk(); cv.speech_start(1)
    assert cv.phrase(" а ", 1) == [("drop_draft",)] and cv.ask("ж") == []


def test_status_audio_does_not_end_draft():
    clk, cv = _conv()
    cv.talk(); cv.phrase("подожди", 1)
    assert cv.audio(True) == [] and cv.audio(False) == []
    assert (cv.state, cv.draft) == ("listening", "подожди")


def test_short_phrase_after_draft_still_sends():
    clk, cv = _conv()
    cv.talk(); cv.speech_start(1); cv.phrase("какая погода", 1); cv.speech_start(2); cv.phrase(".", 2)
    cv.tick()
    clk.t += SEND_PAUSE
    assert cv.tick() == [("drop_draft",), ("send", "какая погода"), ("state", "thinking")]


def test_silence_ends_after_cancel():
    clk, cv = _conv()
    cv.talk(); cv.speech_start(1); cv.cancel(); cv.tick()
    clk.t += SILENCE_END
    assert cv.tick() == [("ear", False), ("state", "idle")]


def test_listen_window_ends_without_speech():
    clk, cv = _conv()
    assert cv.listen(5) == [("ear", True), ("state", "listening")] and cv.active
    clk.t += 4.9; assert cv.tick() == []
    clk.t += 0.2; assert cv.tick() == [("ear", False), ("state", "idle")]


def test_listen_window_speech_becomes_conversation():
    clk, cv = _conv()
    cv.listen(5); clk.t += 3
    cv.speech_start(1); cv.phrase("да, удаляй", 1)
    clk.t += SEND_PAUSE
    assert ("send", "да, удаляй") in cv.tick()


def test_listen_noop_when_quiet_mic_busy_or_talking():
    clk, cv = _conv()
    cv.set_quiet(True); assert cv.listen(5) == []
    cv.set_quiet(False); cv.mic_busy(True); assert cv.listen(5) == []
    cv.mic_busy(False); cv.talk(); assert cv.listen(5) == []


def test_cancel_outside_conversation_returns_to_idle():
    clk, cv = _conv()
    cv.ask("вопрос тут")
    assert cv.cancel() == [("interrupt",), ("state", "idle")] and cv.state == "idle"


def test_short_phrase_keeps_hearing_mark():
    clk, cv = _conv()
    cv.talk(); cv.speech_start(1); cv.speech_start(2)
    assert cv.phrase("", 1) == [("draft", "", True)]


def test_mic_busy_keeps_draft():
    clk, cv = _conv()
    cv.talk(); cv.phrase("черновик тут", 1); cv.speech_start(2)
    assert cv.mic_busy(True) == [("state", "paused"), ("draft", "черновик тут", False)]


import json
import asyncio
from types import SimpleNamespace
from agent_voice.brain import claude_events


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


def test_transcript_tail_skips_meta_and_injected_user_lines(tmp_path):
    rows = [
        {"type": "user", "isMeta": True, "message": {"content": "Caveat: служебное"}},
        {"type": "user", "message": {"content": "<system-reminder>напоминание</system-reminder>"}},
        {"type": "user", "message": {"content": [{"type": "text", "text": "<command-name>/clear</command-name>"}]}},
        {"type": "user", "message": {"content": "<local-command-stdout>ok</local-command-stdout>"}},
        {"type": "user", "message": {"content": "почини сборку"}},
        {"type": "assistant", "message": {"content": [{"type": "text", "text": "Смотрю."}]}},
    ]
    p = tmp_path / "t.jsonl"
    p.write_text("\n".join(json.dumps(r, ensure_ascii=False) for r in rows))
    assert transcript_tail(str(p)).splitlines() == ["ты: почини сборку", "агент: Смотрю."]


def test_transcript_tail_caps_long_message(tmp_path):
    row = {"type": "assistant", "message": {"content": [{"type": "text", "text": "я" * 5000}]}}
    p = tmp_path / "t.jsonl"
    p.write_text(json.dumps(row, ensure_ascii=False))
    last = transcript_tail(str(p)).splitlines()[-1]
    assert last.startswith("агент: ") and len(last) == len("агент: ") + 1001 and last.endswith("…")


def test_foreign_capture_skips_corked():
    assert not foreign_capture([{"corked": True, "properties": {"application.process.id": "7"}}], set())


def test_foreign_capture_skips_monitor_streams():
    # cava, OBS и т. п. пишут звук рабочего стола (монитор выхода), а не микрофон
    assert not foreign_capture([{"properties": {"application.process.id": "7", "stream.monitor": "true"}}], set())
    # клиенты Pulse API (parec -d <выход>.monitor) приходят с stream.capture.sink, без stream.monitor
    assert not foreign_capture([{"properties": {"application.process.id": "7", "stream.capture.sink": "true"}}], set())


def test_on_ac_usb_counts(tmp_path):
    for n, typ, on in (("A", "Mains", "0"), ("U", "USB", "1")):
        (tmp_path / n).mkdir()
        (tmp_path / n / "type").write_text(typ)
        (tmp_path / n / "online").write_text(on)
    assert on_ac(str(tmp_path))


from agent_voice.ear import Whisper, transcribe


def test_transcribe_drops_whisper_hallucinations():
    class Seg:
        def __init__(self, text): self.text = text
    class Model:
        def __init__(self, *texts): self.texts = texts
        def transcribe(self, audio, **kw): return [Seg(t) for t in self.texts], None
    pcm = b"\0\0" * 160
    assert transcribe(Model(" Покажи агентов.", " Что с тестами?"), pcm) == "Покажи агентов. Что с тестами?"
    # титры YouTube, которые whisper «слышит» в тишине; хвост «Спасибо.» — тот же шум
    assert transcribe(Model(" Субтитры создавал DimaTorzok", " Спасибо."), pcm) == ""
    assert transcribe(Model(" Продолжение следует..."), pcm) == ""
    assert transcribe(Model(" Редактор субтитров А.Синецкая Корректор А.Егорова"), pcm) == ""


def test_whisper_loads_once_and_unloads_on_battery_only():
    loads, t = [], [0.0]
    w = Whisper(load=lambda: loads.append(1) or "model", clock=lambda: t[0], idle=300)

    async def run():
        assert await asyncio.gather(w.get(), w.get()) == ["model", "model"] and loads == [1]
        t[0] = 299; w.maybe_unload(on_ac=False); assert w.ready
        t[0] = 301; w.maybe_unload(on_ac=True); assert w.ready
        w.maybe_unload(on_ac=False); assert not w.ready
        await w.get(); assert loads == [1, 1]

    asyncio.run(run())


def _ear(cmd):
    from agent_voice.ear import Ear
    out = asyncio.Queue()
    # VAD подменён заглушкой, кадры не подаются; микрофон и модель не трогаем
    return out, Ear(out, Whisper(load=lambda: "m"), cmd=cmd, vad_load=lambda: None)


def test_ear_start_stop_single_process_and_interleave():
    async def run():
        out, e = _ear(["sleep", "60"])
        await asyncio.gather(e.start(), e.start())
        assert e.open and e.pid
        proc = e._proc
        await e.stop()
        assert not e.open and proc.returncode is not None
        procs = []
        for _ in range(3):
            await asyncio.gather(e.start(), e.stop(), e.start(), e.stop())
            procs.append(e._proc)
        assert not e.open and e.pid is None
        await e.start(); last = e._proc; await e.stop()
        assert last.returncode is not None

    asyncio.run(run())


def test_ear_reports_when_recorder_exits():
    async def run():
        out, e = _ear(["true"])
        await e.start()
        ev = await asyncio.wait_for(out.get(), 5)
        assert ev[0] == "error"
        assert not e.open
        await e.start(); await asyncio.wait_for(out.get(), 5)  # повторный start работает

    asyncio.run(run())


# --- мозг Омы ---
from agent_voice.brain import general_chats, find_agent_chat, messages_of, format_agents, find_entry, agent_prompt, OMA_PROMPT

E = [{"id": "s1", "title": "Почтовый клиент", "project": "omarchy-mail", "state": "waiting", "status": "Клод задал вопрос: какой вариант?",
      "since": 940, "focused": False, "feed": [{"at": 900, "kind": "status", "text": "Читаю план"}], "transcript": "/t/s1.jsonl", "cwd": "/p/mail"},
     {"id": "s2", "title": "agent-speak", "project": "agent-speak", "state": "working", "status": "Запускаю тесты",
      "since": 990, "focused": True, "feed": [], "transcript": "", "cwd": "/p/as"}]


def info(sid, tag=None, mod=0, first=None):
    return SimpleNamespace(session_id=sid, tag=tag, last_modified=mod, custom_title=None, summary=None, first_prompt=first)


def test_general_chats_newest_first_without_agent_chats():
    infos = [info("a", mod=1000, first="как дела по проектам"), info("b", tag="agent:s1", mod=3000), info("c", mod=2000, first="x" * 100)]
    assert general_chats(infos) == [{"key": "chat:c", "title": "x" * 60, "updated": 2}, {"key": "chat:a", "title": "как дела по проектам", "updated": 1}]
    assert find_agent_chat(infos, "s1") == "b" and find_agent_chat(infos, "s9") is None


def test_messages_of_keeps_text_only():
    m = lambda t, c: SimpleNamespace(type=t, message={"content": c})
    msgs = [m("user", "как дела"), m("assistant", [{"type": "tool_use", "name": "mcp__oma__agents"}]),
            m("user", [{"type": "tool_result", "content": "…"}]), m("assistant", [{"type": "text", "text": "Почта ждёт ответа."}]),
            m("user", [{"type": "text", "text": "[Request interrupted by user]"}])]
    assert messages_of(msgs) == [{"role": "me", "text": "как дела"}, {"role": "oma", "text": "Почта ждёт ответа."}]


def test_format_agents_and_find():
    t = format_agents(E, now=1000)
    assert t.splitlines()[0] == "Почтовый клиент (omarchy-mail, id s1) — ждёт ответа, 1 мин: Клод задал вопрос: какой вариант?"
    assert "agent-speak (agent-speak, id s2, в фокусе) — работает, только что: Запускаю тесты" in t
    assert find_entry(E, "почтов")["id"] == "s1" and find_entry(E, "s2")["id"] == "s2"
    assert find_entry(E, "") is None and find_entry(E, "нет такого") is None
    assert format_agents([], 0) == "Агентов нет."


def test_tools_read_only_and_agent_by_id():
    from agent_voice import brain
    assert "WebSearch" in brain.TOOLS and "WebFetch" not in brain.TOOLS  # наружу — только поиск
    tools = {t.name: t.description for t in brain.oma_tools(lambda: E)}
    assert all("id из agents" in tools[n] for n in ("agent_feed", "agent_tail"))
    assert "id" in tools["agents"]


def test_agent_prompt_names_agent():
    p = agent_prompt(E[0])
    assert p.startswith(OMA_PROMPT) and "«Почтовый клиент»" in p and 'agent_feed("s1")' in p


def _fake_sdk(monkeypatch, tmp_path, fail=0):
    # SDK подменён: ни сети, ни процесса claude; первые fail подключений падают
    from agent_voice import brain
    made, tags, fails = [], [], [fail]
    Result = type("ResultMessage", (), {})

    class Client:
        def __init__(self, opts):
            self.opts, self.q, self.sent, self.interrupts = opts, asyncio.Queue(), [], 0
            made.append(self)
        async def connect(self):
            if fails[0]:
                fails[0] -= 1
                raise RuntimeError("нет входа")
        async def query(self, text):
            self.sent.append(text)
            r = Result()
            r.session_id = f"sid-{len(made)}"
            await self.q.put(r)
        async def receive_messages(self):
            while (m := await self.q.get()) is not None:  # None — CLI умер, поток кончился
                yield m
        async def interrupt(self): self.interrupts += 1
        async def disconnect(self): pass

    monkeypatch.setattr(brain, "OMA_DIR", str(tmp_path))
    monkeypatch.setattr(brain, "ClaudeSDKClient", Client)
    monkeypatch.setattr(brain, "list_sessions", lambda d: [])
    monkeypatch.setattr(brain, "tag_session", lambda sid, tag, directory: tags.append((sid, tag, directory)))
    monkeypatch.setattr(brain, "get_session_messages", lambda sid, directory: [SimpleNamespace(type="user", message={"content": "привет"})])
    return brain, made, tags


def test_brain_chat_keys_tag_and_stale(monkeypatch, tmp_path):
    brain, made, tags = _fake_sdk(monkeypatch, tmp_path)

    async def run():
        ev = asyncio.Queue()
        b = brain.Brain(ev, lambda: E)
        await b.send("как дела")
        assert [await ev.get(), await ev.get()] == [("opened", "chat:sid-1", "new"), ("done", None)]
        o = made[0].opts
        assert o.cwd == str(tmp_path) and o.resume is None and o.system_prompt == OMA_PROMPT
        assert o.tools == brain.TOOLS and o.allowed_tools == brain.TOOLS + brain.OMA_TOOLS and o.env == {"AGENT_SPEAK": "1"}
        assert o.strict_mcp_config and list(o.mcp_servers) == ["oma"]
        assert await b.open("agent:s1") == [] and len(made) == 1  # open только читает историю: claude не запускается
        await b.send("что там")
        assert "«Почтовый клиент»" in made[1].opts.system_prompt
        assert [await ev.get(), await ev.get()] == [("opened", "agent:s1", "agent:s1"), ("done", None)]
        assert tags == [("sid-2", "agent:s1", str(tmp_path))]
        assert await b.open("chat:sid-1") == [{"role": "me", "text": "привет"}] and len(made) == 2
        await b.send("ещё")  # подключение — с первой реплики, продолжением сессии
        assert made[2].opts.resume == "sid-1" and await ev.get() == ("done", None)
        b.used -= brain.STALE_HOURS * 3600 + 1  # чат давно не трогали
        await b.send("снова")
        assert [await ev.get(), await ev.get()] == [("opened", "new", "chat:sid-1"), ("opened", "chat:sid-4", "new")]
        assert made[3].opts.resume is None and b.key == "chat:sid-4"
        await b.close()

    asyncio.run(run())


def test_brain_reconnects_after_failed_connect_and_dead_stream(monkeypatch, tmp_path):
    import pytest
    brain, made, _ = _fake_sdk(monkeypatch, tmp_path, fail=1)

    async def run():
        ev = asyncio.Queue()
        b = brain.Brain(ev, lambda: E)
        with pytest.raises(RuntimeError):
            await b.send("раз")
        await b.interrupt()  # клиента нет — ничего
        await b.send("два")
        assert made[1].sent == ["два"] and [await ev.get(), await ev.get()] == [("opened", "chat:sid-2", "new"), ("done", None)]
        await made[1].q.put(None)
        assert await ev.get() == ("error", "Claude: сессия оборвалась")
        await b.interrupt()
        assert made[1].interrupts == 0 and not b.st["skip"]
        await b.send("три")
        assert made[2].sent == ["три"] and made[2].opts.resume == "sid-2"
        await b.close()

    asyncio.run(run())


def test_brain_interrupt_only_while_turn_in_flight(monkeypatch, tmp_path):
    brain, made, _ = _fake_sdk(monkeypatch, tmp_path)

    async def run():
        ev = asyncio.Queue()
        b = brain.Brain(ev, lambda: E)
        await b.send("раз")
        assert [await ev.get(), await ev.get()] == [("opened", "chat:sid-1", "new"), ("done", None)]
        await b.interrupt()  # ход уже кончился — перебивать нечего
        assert made[0].interrupts == 0 and not b.st["skip"]
        await b.send("два")
        assert await asyncio.wait_for(ev.get(), 1) == ("done", None)  # следующий ответ не проглочен
        await b.send("три")
        await b.interrupt()  # ход в полёте: перебит, его хвост глотается
        assert made[0].interrupts == 1
        await asyncio.sleep(0.05)
        assert ev.empty() and not b.st["skip"]
        await b.close()

    asyncio.run(run())


# --- процесс Омы ---
from agent_voice.__main__ import Oma
from agent_voice.link import watch


def _oma(sent, emitted):
    class FakeEar:
        open, pid = False, 1
        async def start(self): self.open = True
        async def stop(self): self.open = False

    class FakeBrain:
        key, texts = "new", []
        async def send(self, t): self.texts.append(t)
        async def open(self, k): self.key = k; return []
        async def interrupt(self): pass
        def chats(self): return []

    clk = Clock()
    oma = Oma(Conversation(clock=clk), FakeEar(), FakeBrain(), SimpleNamespace(ready=True),
              send=lambda c: (emitted if c["cmd"] == "emit" else sent).append(c))
    return clk, oma


def test_oma_routes_talk_draft_tick_and_quiet_answer():
    sent, emitted = [], []
    clk, oma = _oma(sent, emitted)

    async def run():
        await oma.command({"do": "talk"})
        assert oma.ear.open
        assert sent[-1] == {"cmd": "voice_state", "state": "listening", "talking": True}
        await oma.event(("start", 1))
        assert emitted[-1]["line"]["bubble"] == {"id": "draft", "who": "Ты", "text": "", "kind": "draft", "hearing": True, "append": False}
        await oma.event(("phrase", "как дела у почты", 1))
        clk.t += SEND_PAUSE
        await oma.event(("tick",))
        assert oma.brain.texts == ["как дела у почты"] and oma.ear.open
        b = [e["line"]["bubble"] for e in emitted if "bubble" in e["line"]]
        assert b[-2]["drop"] and b[-1]["kind"] == "me" and b[-1]["text"] == "как дела у почты"
        assert b[-1]["id"] == f"me-{os.getpid()}-{oma.n}"
        oma.conv.set_quiet(True)
        await oma.event(("delta", "Почта ждёт твоего ответа сейчас. "))
        assert not any(c["cmd"] == "voice" for c in sent)
        assert any(e["line"].get("bubble", {}).get("kind") == "oma" for e in emitted)

    asyncio.run(run())


def test_oma_locked_ends_conversation():
    sent, emitted = [], []
    clk, oma = _oma(sent, emitted)

    async def run():
        await oma.command({"do": "talk"})
        await oma.event(("locked",))
        assert not oma.ear.open and sent[-1]["talking"] is False

    asyncio.run(run())


def test_oma_listen_command_opens_window_unless_locked(monkeypatch):
    import agent_voice.__main__ as m
    sent, emitted = [], []
    clk, oma = _oma(sent, emitted)

    async def run():
        monkeypatch.setattr(m, "mic_outputs", lambda: [])
        monkeypatch.setattr(m, "locked", lambda: True)
        await oma.command({"do": "listen", "secs": 5})
        assert not oma.ear.open  # экран заблокирован: чужие уши не включаем
        monkeypatch.setattr(m, "locked", lambda: False)
        await oma.command({"do": "listen", "secs": 5})
        assert oma.ear.open and sent[-1] == {"cmd": "voice_state", "state": "listening", "talking": True}
        clk.t += 5.1
        await oma.event(("tick",))
        assert not oma.ear.open and sent[-1]["talking"] is False

    asyncio.run(run())


def test_oma_listen_with_key_switches_to_asking_agents_chat(monkeypatch):
    import agent_voice.__main__ as m
    sent, emitted = [], []
    clk, oma = _oma(sent, emitted)
    monkeypatch.setattr(m, "locked", lambda: False)
    monkeypatch.setattr(m, "mic_outputs", lambda: [])

    async def run():
        await oma.command({"do": "listen", "secs": 5, "key": "agent:s"})
        assert oma.brain.key == "agent:s" and oma.ear.open  # ответ на вопрос уйдёт в чат спросившего
        assert {"chat": {"key": "agent:s", "messages": []}} in [e["line"] for e in emitted]

    asyncio.run(run())


def test_oma_listen_command_skips_when_foreign_mic_capture(monkeypatch):
    import agent_voice.__main__ as m
    sent, emitted = [], []
    clk, oma = _oma(sent, emitted)
    monkeypatch.setattr(m, "locked", lambda: False)
    monkeypatch.setattr(m, "mic_outputs", lambda: [{"properties": {"application.process.id": "7"}}])  # voxtype пишет

    async def run():
        await oma.command({"do": "listen", "secs": 5})
        assert not oma.ear.open and not oma.conv.active and sent == []  # слух закрыт — conv.mic ещё не знает о чужой записи
        monkeypatch.setattr(m, "mic_outputs", lambda: [{"corked": True, "properties": {"application.process.id": "7"}}])
        await oma.command({"do": "listen", "secs": 5})
        assert oma.ear.open  # поток на паузе — не помеха

    asyncio.run(run())


def test_oma_cancel_command():
    sent, emitted = [], []
    clk, oma = _oma(sent, emitted)

    async def run():
        await oma.command({"do": "talk"})
        await oma.event(("start", 1))
        await oma.command({"do": "cancel"})
        assert emitted[-1]["line"]["bubble"]["drop"] and emitted[-1]["line"]["bubble"]["id"] == "draft"

    asyncio.run(run())


def test_oma_stop_action_hushes_daemon():
    sent = []
    oma = Oma(Conversation(), None, None, SimpleNamespace(ready=True), send=sent.append)
    asyncio.run(oma.act([("stop",)]))
    assert sent == [{"cmd": "hush"}]  # перебивание Омы: финалы агентов в очереди остаются


def test_watch_ignores_unknown_lines_and_eof_ends(tmp_path):
    import socket, threading
    path = str(tmp_path / "s.sock")
    srv = socket.socket(socket.AF_UNIX)
    srv.bind(path)
    srv.listen(1)

    def serve():
        c, _ = srv.accept()
        c.recv(100)
        c.sendall('{"running":true,"speaking":true,"quiet":false}\n{"bubble":{"id":"x"}}\n{"agents":[{"id":"a"}]}\nмусор\n'.encode())
        c.close()

    threading.Thread(target=serve, daemon=True).start()
    st, ag = [], []
    asyncio.run(asyncio.wait_for(watch(st.append, ag.append, sock=path), 2))
    assert [s["speaking"] for s in st] == [True] and ag == [[{"id": "a"}]]


def test_oma_stale_chat_line_lands_in_new_chat_and_done_syncs():
    lines = []

    class FakeBrain:
        key = "new"
        async def open(self, k): self.key = k; return [{"role": "me", "text": "давно"}]
        async def send(self, t):
            if self.key.startswith("chat:"):  # как Brain при устаревшем чате: "opened" в очередь, ключ уже "new"
                self.pending, self.key = ("opened", "new", self.key), "new"
            else:  # новый чат получил id посреди ответа
                self.key = "chat:sid"
        def chats(self): return [{"key": "chat:old"}]

    oma = Oma(Conversation(), None, FakeBrain(), SimpleNamespace(ready=True),
              send=lambda c: c["cmd"] == "emit" and lines.append(c["line"]))
    me = {"role": "me", "text": "снова привет"}

    async def run():
        await oma.command({"do": "open", "key": "chat:old"})
        assert lines[-1] == {"chat": {"key": "chat:old", "messages": [{"role": "me", "text": "давно"}]}}
        await oma.command({"do": "ask", "text": "снова привет", "key": "chat:old"})
        await oma.event(oma.brain.pending)
        assert lines[-1] == {"chat": {"key": "new", "was": "chat:old", "messages": [me]}}
        await oma.event(("delta", "Привет, чем помочь сегодня? "))
        await oma.event(("done", None))
        assert lines[-2:] == [{"chat": {"key": "new", "messages": [me, {"role": "oma", "text": "Привет, чем помочь сегодня?"}]}},
                              {"chats": [{"key": "chat:old"}]}]
        await oma.command({"do": "ask", "text": "и ещё", "key": "new"})
        assert oma.brain.key == "chat:sid" and len(oma.history) == 3 and oma.history[-1] == {"role": "me", "text": "и ещё"}

    asyncio.run(run())


def test_oma_open_without_key_reports_current_chat():
    lines, opened = [], []

    class FakeBrain:
        key = "chat:x"
        async def open(self, k): opened.append(k); return []

    oma = Oma(Conversation(), None, FakeBrain(), SimpleNamespace(ready=True),
              send=lambda c: c["cmd"] == "emit" and lines.append(c["line"]))
    oma.history = [{"role": "me", "text": "голосом"}]
    asyncio.run(oma.command({"do": "open"}))
    assert lines == [{"chat": {"key": "chat:x", "was": "*", "messages": [{"role": "me", "text": "голосом"}]}}]
    assert opened == [] and oma.brain.key == "chat:x"


def test_oma_error_notifies_new_text_or_after_5_min():
    seen, t = [], [0.0]
    oma = Oma(Conversation(), SimpleNamespace(open=False), None, SimpleNamespace(ready=True),
              send=lambda c: None, notify=seen.append, clock=lambda: t[0])

    async def run():
        for _ in range(3):
            await oma.event(("error", "recognize: boom"))
        t[0] = 299
        await oma.event(("error", "recognize: boom"))
        await oma.event(("error", "Claude: сессия оборвалась"))
        await oma.event(("error", "Claude: сессия оборвалась"))
        t[0] = 600
        await oma.event(("error", "Claude: сессия оборвалась"))

    asyncio.run(run())
    assert seen == ["recognize: boom", "Claude: сессия оборвалась", "Claude: сессия оборвалась"]


def test_serve_eof_cuts_hung_dispatch():
    from agent_voice.__main__ import serve
    hung = []

    class HangBrain:
        key = "new"
        async def send(self, t):
            hung.append(t)
            await asyncio.Event().wait()  # connect/query повис навсегда

    async def run():
        oma = Oma(Conversation(), None, HangBrain(), SimpleNamespace(ready=True), send=lambda c: None)
        r = asyncio.StreamReader()
        r.feed_data('{"do":"ask","text":"привет"}\n'.encode())
        task = asyncio.create_task(serve(oma, asyncio.Queue(), r))
        await asyncio.sleep(0.1)
        assert hung == ["привет"] and not task.done()
        r.feed_eof()
        await asyncio.wait_for(task, 1)

    asyncio.run(run())


def _numbered_ear(monkeypatch, whisper):
    from agent_voice import ear as earmod
    from agent_voice.ear import Ear
    # две фразы: 10 кадров речи, 22 тишины, снова; VAD — по заранее заданному списку
    probs = iter([0.9] * 10 + [0.1] * 22 + [0.9] * 10 + [0.1] * 22)
    vad = lambda x, rate: SimpleNamespace(item=lambda: next(probs))
    monkeypatch.setattr(earmod, "transcribe", lambda m, pcm: "слово")
    out = asyncio.Queue()
    return out, Ear(out, whisper, cmd=["sh", "-c", f"head -c {64 * 1024} /dev/zero; sleep 60"], vad_load=lambda: vad)


def _drain(out, kinds):
    async def run():
        got = []
        while len(got) < kinds:
            ev = await asyncio.wait_for(out.get(), 5)
            if ev[0] != "error" or ev[1].startswith("recognize"):
                got.append(ev)
        return got
    return run


def test_ear_numbers_speech(monkeypatch):
    out, e = _numbered_ear(monkeypatch, Whisper(load=lambda: "m"))

    async def run():
        await e.start()
        got = await _drain(out, 4)()
        await e.stop()
        return got

    got = asyncio.run(run())
    assert sorted(got, key=lambda x: (x[-1], x[0] != "start")) == [("start", 1), ("phrase", "слово", 1), ("start", 2), ("phrase", "слово", 2)]
    assert e.seq == 2


def test_ear_failed_recognition_still_emits_empty_phrase(monkeypatch):
    def boom():
        raise RuntimeError("нет модели")
    out, e = _numbered_ear(monkeypatch, Whisper(load=boom))

    async def run():
        await e.start()
        got = []
        while len(got) < 6:
            got.append(await asyncio.wait_for(out.get(), 5))
        await e.stop()
        return got

    got = asyncio.run(run())
    assert ("phrase", "", 1) in got and ("phrase", "", 2) in got
    assert any(g[0] == "error" and g[1].startswith("recognize") for g in got)


def test_locked_false_without_helper(monkeypatch):
    from agent_voice.context import locked
    monkeypatch.setenv("PATH", "/nonexistent")
    assert locked() is False


def test_ear_warm_loads_vad_once():
    from agent_voice.ear import Ear
    loads = []

    async def run():
        e = Ear(asyncio.Queue(), Whisper(load=lambda: "m"), cmd=["sleep", "60"], vad_load=lambda: loads.append(1) or "vad")
        await e.warm()
        await e.start()
        await e.stop()
        await e.warm()

    asyncio.run(run())
    assert loads == [1]


def test_foreign_capture_skips_own_named_stream():
    # PipeWire не отдаёт pid у pw-record: свой поток узнаём по имени узла
    from agent_voice.context import OWN_NODE, foreign_capture
    from agent_voice.ear import REC_CMD
    assert not foreign_capture([{"properties": {"node.name": OWN_NODE}}], {42})
    assert foreign_capture([{"properties": {"node.name": "pw-record"}}], {42})
    assert OWN_NODE in " ".join(REC_CMD)
