# python -m pytest python/test_agent_voice.py  (из voice-venv или любого python3 с pytest)
import sys, os, time
sys.path.insert(0, os.path.dirname(__file__))
from agent_voice.segment import Segmenter
from agent_voice.chunker import Chunker
from agent_voice.conversation import Conversation, SEND_PAUSE, SILENCE_END, WINDOW_END, is_ack, is_yes


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
    # предзахват: 10 кадров тишины до старта (запас 16, есть 10) + 20 речи + 22 тишины
    assert len(ev[1][1]) == (10 + 20 + 22) * 1024
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
    assert cv.tick() == [("drop_draft",), ("send", "какая погода в Москве", ""), ("state", "thinking")]


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
    assert cv.speech_start(2) == [("pause_voice", True), ("draft", "", True)], "в наушниках — пауза, не обрыв: решит текст"


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
    assert cv.ask("который час") == [("drop_draft",), ("send", "который час", ""), ("state", "thinking")]
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
    assert cv.tick() == [("drop_draft",), ("send", "какая погода", ""), ("state", "thinking")]


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
    cv.listen(5, "question", "плуг: готово"); clk.t += 3
    cv.speech_start(1); cv.phrase("да, удаляй", 1)
    clk.t += SEND_PAUSE
    assert ("send", "да, удаляй", "Ома только что сказала: «плуг: готово». ") in cv.tick()


def _say(clk, cv, text, n=1):
    cv.speech_start(n); cv.phrase(text, n)
    clk.t += SEND_PAUSE
    return cv.tick()


def test_ack_after_news_closes_without_send():
    clk, cv = _conv()
    cv.listen(5, "news", "плуг: готово")
    acts = _say(clk, cv, "Ок.")
    assert ("presence", "here") in acts and ("ear", False) in acts and not any(a[0] == "send" for a in acts)
    assert cv.state == "idle" and not cv.active


def test_ack_after_question_sent_then_closes():
    clk, cv = _conv()
    cv.listen(5, "question", "плуг: удалить?")
    ctx = "Ома только что сказала: «плуг: удалить?». "
    assert ("send", "Ок", ctx) in _say(clk, cv, "Ок")
    cv.delta("Принято. "); cv.audio(True)
    cv.done()
    assert cv.audio(False)[-2:] == [("ear", False), ("state", "idle")]


def test_ack_reply_silent_in_quiet_mode_closes_on_done():
    clk, cv = _conv()
    cv.listen(5, "question", "плуг: удалить?")
    _say(clk, cv, "Ок"); cv.set_quiet(True)
    cv.delta("Принято. ")
    assert ("ear", False) in cv.done()


def test_auto_window_closes_after_reply():
    # окно открыла сама Ома: после её ответа микрофон закрыт — разговор в комнате не уходит в Claude; продолжить — кликом
    clk, cv = _conv()
    cv.listen(5, "question", "плуг: баг")
    _say(clk, cv, "расскажи подробнее")
    cv.delta("А что за баг? "); cv.audio(True); cv.done()
    cv.speech_start(2)  # в комнате говорят поверх её ответа
    assert ("ear", False) in cv.audio(False) or ("ear", False) in cv.tick()


def test_are_you_there_room_talk_with_yes_inside_is_ignored():
    clk, cv = _conv()
    cv.listen(5, "here", "Эй, ты тут?")
    acts = _say(clk, cv, "Которые под вид кожи? Налили пиво? Нет, не под вид кожи, а голубые")
    assert not any(a[0] in ("send", "presence") for a in acts), "ответ Оме — короткий и начинается с ответа"
    assert ("presence", "here") in _say(clk, cv, "через минуту", n=2)


def test_are_you_there_yes_closes():
    clk, cv = _conv()
    cv.listen(5, "here", "Ты тут?")
    acts = _say(clk, cv, "да, я тут")
    assert ("presence", "here") in acts and ("ear", False) in acts and not any(a[0] == "send" for a in acts)


def test_are_you_there_other_goes_to_brain():
    clk, cv = _conv()
    cv.listen(5, "here", "Ты тут?")
    assert ("send", "через минуту", "Ома спросила: «Эй, ты тут? Есть новости». ") in _say(clk, cv, "через минуту")


def test_are_you_there_ignores_talk_not_to_oma():
    clk, cv = _conv()
    cv.listen(5, "here", "Эй, ты тут?")
    acts = _say(clk, cv, "Бабы, бабы, бабы.")
    assert not any(a[0] in ("send", "presence") for a in acts) and cv.active, "ребёнок в комнате — не ответ Оме, слушаем дальше"
    acts = _say(clk, cv, "да, я тут", n=2)
    assert ("presence", "here") in acts


def test_click_conversation_ack_ends_it():
    # «ок» и тишина — пользователь закончил: мозгу не уходит, разговор закрывается сразу, а не через SILENCE_END
    clk, cv = _conv()
    cv.talk()
    acts = _say(clk, cv, "ок")
    assert not any(a[0] in ("send", "presence") for a in acts) and ("ear", False) in acts and cv.state == "idle"


def test_is_ack_table():
    assert is_ack("Ок, спасибо.") and is_ack("OK") and is_ack("Окей!") and is_ack("Принято")
    assert not is_ack("ок давай пушь") and not is_ack("")
    assert is_yes("да, я тут") and not is_yes("через минуту")


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
    return out, Ear(out, Whisper(load=lambda: "m"), cmd=cmd, vad_load=lambda: None, mic=lambda: "")


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
from agent_voice.brain import general_chats, find_agent_chat, messages_of, format_agents, pick_agent, agent_prompt, OMA_PROMPT

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
    assert pick_agent(E, "почтов")[0]["id"] == "s1" and pick_agent(E, "s2")[0]["id"] == "s2"
    assert pick_agent(E, "")[0]["id"] == "s2" and pick_agent(E, "нет такого") == (None, "Агент не найден.")
    assert format_agents([], 0) == "Агентов нет."


def test_format_agents_shows_watch_health():
    sick = dict(E[1], health="завис")
    assert format_agents([sick], now=1000).endswith("Запускаю тесты; сторож: завис")
    assert "сторож" not in format_agents([dict(E[1], health="")], now=1000)


def test_tools_read_only_and_agent_by_id():
    from agent_voice import brain
    assert "WebSearch" in brain.TOOLS and "WebFetch" not in brain.TOOLS  # наружу — только поиск
    tools = {t.name: t.description for t in brain.oma_tools(lambda: E, None)}
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
        assert [brain.strip_note(t) for t in made[1].sent] == ["два"] and [await ev.get(), await ev.get()] == [("opened", "chat:sid-2", "new"), ("done", None)]
        await made[1].q.put(None)
        assert await ev.get() == ("error", "Claude: сессия оборвалась")
        await b.interrupt()
        assert made[1].interrupts == 0 and not b.st["skip"]
        await b.send("три")
        assert [brain.strip_note(t) for t in made[2].sent] == ["три"] and made[2].opts.resume == "sid-2"
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
        key, texts, warmed = "new", [], False
        async def send(self, t): self.texts.append(t)
        async def warm(self): self.warmed = True
        async def open(self, k): self.key = k; return []
        async def interrupt(self): pass
        def chats(self): return []

    clk = Clock()
    oma = Oma(Conversation(clock=clk), FakeEar(), FakeBrain(), SimpleNamespace(ready=True),
              send=lambda c: (emitted if c["cmd"] == "emit" else sent).append(c), turn_end=SimpleNamespace(complete=lambda pcm: None))
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


def test_general_chat_for_talk_and_window():
    # Ома-помощник — в общем чате: клик по орбу из чата агента и строка «Ома» в окне ведут в последний
    # общий чат (моложе двух часов), иначе в новый; окно принимает настоящий ключ по was
    lines, opened = [], []

    class FakeBrain:
        key = "agent:a1"
        updated = time.time() - 600
        async def open(self, k): opened.append(k); self.key = k; return []
        async def interrupt(self): pass
        async def warm(self): pass
        def chats(self): return [{"key": "chat:g", "updated": self.updated}]

    oma = Oma(Conversation(), SimpleNamespace(open=False, speakers=None, start=_async_none, stop=_async_none),
              FakeBrain(), SimpleNamespace(ready=True), send=lambda c: c["cmd"] == "emit" and lines.append(c["line"]))

    async def go():
        await oma.command({"do": "talk"})
        assert opened == ["chat:g"] and lines[0]["chat"]["was"] == "*", "окно идёт за разговором"
        await oma.command({"do": "talk"})  # слушала — клик закрывает разговор, чат тот же
        assert opened == ["chat:g"]
        oma.brain.key, oma.brain.updated = "agent:a1", time.time() - 3 * 3600
        await oma.command({"do": "open", "key": "general"})
        assert opened[-1] == "new" and lines[-1]["chat"]["key"] == "new" and lines[-1]["chat"]["was"] == "general"
        await oma.command({"do": "open", "key": "general"})  # уже в общем — он и есть
        assert opened[-1] == "new" and len(opened) == 2 and lines[-1]["chat"] == {"key": "new", "was": "general", "messages": []}
    asyncio.run(go())


async def _async_none(*_):
    return None


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
    return out, Ear(out, whisper, cmd=["sh", "-c", f"head -c {64 * 1024} /dev/zero; sleep 60"], vad_load=lambda: vad, mic=lambda: "")


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
        e = Ear(asyncio.Queue(), Whisper(load=lambda: "m"), cmd=["sleep", "60"], vad_load=lambda: loads.append(1) or "vad", mic=lambda: "")
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


def _presence(state):
    from agent_voice import brain
    sent = []
    t = next(t for t in brain.oma_tools(lambda: E, sent.append) if t.name == "presence")
    asyncio.run(t.handler({"state": state}))
    return sent


def test_presence_tool_sends_state():
    assert _presence("busy") == [{"cmd": "presence", "state": "busy"}]


def test_presence_tool_rejects_unknown():
    assert _presence("sleep") == []


def test_prompt_mentions_presence_and_stress():
    from agent_voice import brain
    assert "presence" in brain.OMA_PROMPT and "U+0301" in brain.OMA_PROMPT
    assert "mcp__oma__presence" in brain.OMA_TOOLS


def test_mic_picks_clear_input_by_where_oma_speaks():
    from agent_voice.context import pick_mic, buds_out
    srcs = "43\tmic_aec\tPipeWire\n46\tmic_denoise\tPipeWire\n67\talsa_input.pci-0000_00_1f.3.analog-stereo\tPipeWire\n80\tbluez_input.00_11_22_33_44_55.0\tPipeWire\n"
    raw, buds = "alsa_input.pci-0000_00_1f.3.analog-stereo", "bluez_output.00_11_22_33_44_55.1"
    # сырой микрофон ноутбука, как у voxtype: шумодав режет слова, эхоподавление webrtc их портит (docs/recognition.md)
    # Ома в наушниках — эха нет
    assert pick_mic("mic_denoise", buds, srcs, out=False) == (raw, False, "")
    # Ома в динамиках — тоже сырой, но её голос в нём есть: echo — эхо отсекает разговор по тексту
    assert pick_mic("mic_denoise", "alsa_output.pci-x", srcs, out=False) == (raw, True, "mic_aec")
    # наушники на столе — Ома в динамиках, даже если вход по умолчанию — гарнитура
    assert pick_mic("bluez_input.00_11_22_33_44_55.0", buds, srcs, out=True) == (raw, True, "mic_aec")
    # гарнитура в ушах — её микрофон, вход по умолчанию
    assert pick_mic("bluez_input.00_11_22_33_44_55.0", buds, srcs, out=False) == ("", False, "")
    assert pick_mic("x", "alsa_output.x", "", out=False) == ("", True, "")
    assert buds_out('{"at": 1000, "buds": "out"}', 1050)
    assert not buds_out('{"at": 1000, "buds": "out"}', 1091)  # снимок устарел — сервиса нет
    assert not buds_out('{"at": 1000, "buds": "in"}', 1010)
    assert not buds_out("{", 1010)


def test_music_caption_is_whisper_hallucination():
    from agent_voice.ear import HALLUCINATION
    assert HALLUCINATION.search("Играет музыка.") and HALLUCINATION.search("[музыка]")
    assert not HALLUCINATION.search("включи музыку в наушниках")


def test_rec_cmd_targets_mic_before_stdout():
    from agent_voice.ear import REC_CMD, rec_cmd
    assert rec_cmd(REC_CMD, "") == REC_CMD
    assert rec_cmd(REC_CMD, "mic_denoise")[-3:] == ["--target", "mic_denoise", "-"]


def test_prompt_demands_short_answers():
    # «Умеешь ли ты …?» — «Да» или «Нет», без перечня умений: Ома говорит голосом, лишнее утомляет
    from agent_voice import brain
    assert "«Да» или «Нет»" in brain.OMA_PROMPT and "одной-тремя" not in brain.OMA_PROMPT


def test_cancel_word_stops_everything_and_ends_conversation():
    # «отбой»/«отмена» — не реплика мозгу: всё, что звучит (и чтение), обрывается, разговор кончается, микрофон выключен
    clk, cv = _conv()
    cv.talk()
    cv.ask("расскажи про агентов")
    cv.delta("Агент правит Ому. "); cv.audio(True)
    start = cv.speech_start(1)
    assert ("interrupt",) in start  # ход мозга обрывается уже на начале речи
    cv.phrase("Отбой.", 1)
    clk.t += SEND_PAUSE
    acts = cv.tick()
    assert ("halt",) in acts and ("ear", False) in acts
    assert not any(a[0] == "send" for a in acts)
    assert cv.state == "idle" and not cv.active


def test_stop_word_silences_and_keeps_listening():
    # «стоп» — замолчи и слушай дальше; конец разговора — «отбой»
    clk, cv = _conv()
    cv.talk()
    cv.ask("расскажи про агентов")
    cv.delta("Агент правит Ому. "); cv.audio(True)
    cv.speech_start(1)
    cv.phrase("Стоп.", 1)
    clk.t += SEND_PAUSE
    acts = cv.tick()
    assert ("halt",) in acts and ("ear", False) not in acts and not any(a[0] == "send" for a in acts)
    assert cv.active and cv.state == "listening"
    assert any(a[0] == "send" for a in _say(clk, cv, "расскажи только про почту", n=2))


def test_stop_words_table():
    from agent_voice.conversation import is_hold
    assert is_hold("Стоп.") and is_hold("хватит") and is_hold("Подожди!") and is_hold("стоп, стоп")
    assert not is_hold("стоп, а что там с тестами") and not is_hold("отбой")


def test_cancel_words_table():
    from agent_voice.conversation import is_cancel
    assert is_cancel("Отмена!") and is_cancel("отбой, отбой") and is_cancel("отмени") and not is_cancel("Стоп.")
    assert is_cancel("отмену.") and is_cancel("Отменить") and is_cancel("отбою")  # whisper пишет любую форму слова
    assert not is_cancel("отмена встречи в пятницу") and not is_cancel("отменить встречу") and not is_cancel("стоп, а что там с тестами") and not is_cancel("")


from agent_voice.orders import Orders, danger, asked


def _orders(clk=None, ok=True, out=""):
    calls = []

    async def fake(*args):
        calls.append(args)
        return ok, out

    return Orders(run=fake, clock=clk or Clock()), calls


run = asyncio.run


def test_danger_words_both_scripts():
    assert danger("запушь ветку")[0] == "пуш" and danger("git push")[0] == "пуш"
    assert danger("удали старые логи")[0] == "удаление" and danger("rm -rf build")[0] == "удаление"
    assert danger("займись тестами") is None and danger("сотрудник написал") is None


def test_voice_danger_waits_for_phrase_typed_goes():
    o, calls = _orders()
    o.turn(True, "пусть запушит")
    r = run(o.tell(E[0], "Запушь изменения", "запушить"))
    assert "подтверждаю пуш" in r and calls == []
    o.turn(False, "удали старую ветку")
    assert "Передано проекту omarchy-mail" in run(o.tell(E[0], "Удали старую ветку", "удалить ветку"))
    assert calls == [("tell", "s1", "0", "Удали старую ветку")]


def test_negative_answer_needs_no_confirmation():
    o, calls = _orders()
    agent = dict(E[0], ask="Ждёт ответа на AskUserQuestion: Запушить? Варианты: Да; Нет")
    o.turn(True, "нет")
    r = run(o.tell(agent, "Нет, не пушь", "не пушить"))
    assert "да, передай" in r and "подтверждаю" not in r and not calls, "отказ не опасен, но голосом — тоже после «да, передай»"
    o.turn(True, "да")
    assert "подтверждаю пуш" in run(o.tell(agent, "Да", "пушить")), "опасность в вопросе агента"


def test_confirm_only_same_kind_in_time():
    clk = Clock()
    for heard, wait, sent in (("подтверждаю пуш", 1, True), ("Подтверждаю, push.", 1, True), ("Подтверждаю душ.", 1, True),
                              ("подтверждаю удаление", 1, False), ("подтверждаю пуш", 61, False),
                              ("ну я вроде подтверждаю этот пуш наверное", 1, False)):
        o, calls = _orders(clk)
        o.turn(True, "запушь")
        run(o.tell(E[0], "Запушь", "запушить"))
        clk.t += wait
        said = run(o.confirm(heard))
        assert bool(calls) == sent and (said == "Передала проекту omarchy-mail: запушить.") == sent, heard
    o, calls = _armed(clk)
    run(o.confirm("подтверждаю пуш"))
    assert run(o.confirm("подтверждаю пуш")) is None, "ожидание одно: после ответа его нет"


def test_missed_confirmation_keeps_waiting():
    # whisper ослышался в слове — пользователь повторит; ожидание не пропадает, мозг не зовётся
    o, calls = _armed()
    assert run(o.confirm("подтверждаю удаление")) == "Не расслышала. Скажи: подтверждаю пуш." and o.pending and not calls
    assert run(o.confirm("подтверждаю пуш")) == "Передала проекту omarchy-mail: запушить." and calls


def test_confirmed_order_leaves_note_for_brain():
    o, calls = _orders()
    o.turn(True, "запусти агента в почте, пусть удалит старые черновики")
    assert "подтверждаю удаление" in run(o.launch("/p/mail", "Удали старые черновики", "удалить черновики"))
    assert run(o.confirm("подтверждаю удаление")) == "Запустила агента в проекте mail: удалить черновики."
    assert calls == [("launch", "/p/mail", "Удали старые черновики", "orca")]
    assert "Запустила агента в проекте mail" in o.note


def test_asked_takes_last_question_sentence():
    final = dict(E[1], feed=[{"kind": "final", "text": "Всё запушено. Взяться за тесты?"}])
    assert asked(final) == "Взяться за тесты?" and danger(asked(final)) is None
    assert asked(dict(E[0], ask="Ждёт ответа на ExitPlanMode: план")) == "Ждёт ответа на ExitPlanMode: план"
    assert asked(E[1]) == ""


def test_cli_error_reaches_brain_and_projects_listed():
    o, _ = _orders(ok=False, out="агент не в Orca и не в herdr — передать не могу")
    o.turn(False, "скажи почте привет")
    assert run(o.tell(E[0], "Привет", "привет")) == "Не передано: агент не в Orca и не в herdr — передать не могу. Скажи пользователю коротко, что не вышло."
    o, calls = _orders(out="/p/mail\n/home/u/Projects/agent-speak\n")
    assert run(o.projects()) == "mail — /p/mail\nagent-speak — /home/u/Projects/agent-speak"
    assert run(o.projects("herdr")) and calls == [("projects", "orca"), ("projects", "herdr")]
    o, _ = _orders(ok=False, out="Orca не запущена — открой её, и я запущу")
    assert run(o.projects()) == "Список проектов недоступен: Orca не запущена — открой её, и я запущу."


class FakeOrders:
    def __init__(self):
        self.told, self.voiced = [], False

    async def tell(self, entry, text, gist, option=0):
        self.told.append((entry["id"], text, gist, option))
        return "Передано"


def _tool(name, entries, orders):
    from agent_voice import brain
    return next(t for t in brain.oma_tools(lambda: entries, None, orders) if t.name == name)


def test_tell_refuses_ambiguous_agent():
    two = [E[0], dict(E[0], id="s3", title="Почта: тесты")]
    o = FakeOrders()
    r = run(_tool("tell", two, o).handler({"agent": "mail", "text": "Привет", "gist": "привет"}))["content"][0]["text"]
    assert "несколько агентов" in r and "s1" in r and "s3" in r and o.told == []
    run(_tool("tell", two, o).handler({"agent": "s3", "text": "Привет", "gist": "привет", "option": 2}))
    run(_tool("tell", E, o).handler({"text": "Дальше", "gist": "дальше"}))
    assert o.told == [("s3", "Привет", "привет", 2), ("s2", "Дальше", "дальше", 0)], "точный id; пусто — агент в фокусе"


def test_agent_by_part_of_id():
    a = dict(E[0], id="37b11a5e-f794-4e4a-8723-172cbbf96ea7")
    b = dict(E[1], id="37b2c0de-0000-4e4a-8723-000000000000")
    o = FakeOrders()
    run(_tool("tell", [a, b], o).handler({"agent": "37b11a5e", "text": "Привет", "gist": "привет"}))
    assert o.told == [(a["id"], "Привет", "привет", 0)], "префикс id"
    r = run(_tool("agent_feed", [a, b], o).handler({"agent": "F794"}))["content"][0]["text"]
    assert r == "status: Читаю план", "часть id без учёта регистра"
    e, why = pick_agent([a, b], "37b")
    assert e is None and a["id"] in why and b["id"] in why
    assert pick_agent([a, b], "почтов")[0] is a, "название раньше id"


def _voice(oma, clk, text, n):
    async def go():
        await oma.event(("start", n))
        await oma.event(("phrase", text, n))
        clk.t += SEND_PAUSE
        await oma.event(("tick",))
    run(go())


def test_typed_order_needs_no_confirmation():
    sent, emitted = [], []
    clk, oma = _oma(sent, emitted)
    run(oma.command({"do": "ask", "text": "удали старую ветку"}))
    assert oma.orders.voiced is False and oma.orders.heard == "удали старую ветку"
    run(oma.command({"do": "talk"}))
    _voice(oma, clk, "удали старую ветку", 1)
    assert oma.orders.voiced is True


def test_oma_confirmation_skips_brain():
    sent, emitted = [], []
    clk, oma = _oma(sent, emitted)
    oma.orders, calls = _orders(clk)
    oma.brain.texts = []
    oma.orders.turn(True, "запушь")
    run(oma.orders.tell(E[0], "Запушь", "запушить"))
    run(oma.command({"do": "talk"}))
    _voice(oma, clk, "подтверждаю пуш", 1)
    assert oma.brain.texts == [] and calls == [("tell", "s1", "0", "Запушь")]
    oma_said = [e["line"]["bubble"]["text"] for e in emitted if e["line"].get("bubble", {}).get("kind") == "oma"]
    assert oma_said == ["Передала проекту omarchy-mail: запушить."]
    states = [c["state"] for c in sent if c["cmd"] == "voice_state"]
    assert states[-1] != "thinking", states
    run(oma.event(("audio", True)))
    run(oma.event(("audio", False)))
    _voice(oma, clk, "что дальше", 2)
    assert oma.brain.texts == ["(Пользователь подтвердил голосом, Ома сама: Передала проекту omarchy-mail: запушить.) что дальше"]


def test_format_agents_shows_question_options():
    waiting = dict(E[0], ask="Ждёт ответа на AskUserQuestion: Какой цвет? Варианты: Красный; Синий")
    assert format_agents([waiting], now=1000).endswith("; ждёт ответа: Ждёт ответа на AskUserQuestion: Какой цвет? Варианты: Красный; Синий")


def test_prompt_knows_orders():
    from agent_voice import brain
    assert all(w in brain.OMA_PROMPT for w in ("tell", "launch", "projects")) and "пока не умеешь" not in brain.OMA_PROMPT
    assert {"mcp__oma__tell", "mcp__oma__projects", "mcp__oma__launch"} <= set(brain.OMA_TOOLS)
    assert 'tell("s1"' in agent_prompt(E[0])


def _armed(clk=None):
    o, calls = _orders(clk)
    o.turn(True, "запушь")
    run(o.tell(E[0], "Запушь", "запушить"))
    return o, calls


def test_confirm_rejects_negation():
    for heard in ("не подтверждаю пуш", "нет, не подтверждаю пуш"):
        o, calls = _armed()
        assert run(o.confirm(heard)) == "Хорошо, не передаю." and calls == [] and o.pending is None, heard


PUSH_MENU = "Ждёт ответа на AskUserQuestion: Запушить сейчас? Варианты: Запушить — сразу на GitHub; Нет — потом"


def test_option_gated_by_chosen_variant():
    agent = dict(E[0], ask="Агент: тесты прошли.\n\n" + PUSH_MENU)
    o, calls = _orders()
    o.turn(True, "нет")
    assert "подтверждаю пуш" in run(o.tell(agent, "Нет", "не пушить", option=1)), "уходит вариант 1 — пуш, а не текст"
    o.turn(True, "нет")
    r = run(o.tell(agent, "", "не пушить", option=2))
    assert "да, передай" in r and "подтверждаю" not in r
    plan = dict(E[0], ask="Ждёт ответа на ExitPlanMode: 1. тесты 2. git push")
    o.turn(True, "нет")
    assert "подтверждаю пуш" in run(o.tell(plan, "Нет", "одобрить", option=1)), "одобрение плана — без поблажки отказу"


def test_refusal_does_not_cover_added_danger():
    o, calls = _orders()
    for sent in ("Нет, вместо этого сделай git reset --hard", "Не пушь, а удали ветку", "Нет, сначала прогони тесты, потом запушь"):
        o.turn(True, "нет")
        assert "подтверждаю" in run(o.tell(E[0], sent, "нет")), sent
    o.turn(True, "нет")
    r = run(o.tell(E[0], "Нет, не пушь", "не пушить"))
    assert "да, передай" in r and "подтверждаю" not in r


def test_asked_is_only_the_question_part():
    agent = dict(E[0], ask="Всё запушено, старые логи удалил.\n\nЖдёт ответа на AskUserQuestion: Взяться за тесты? Варианты: Да; Нет")
    assert asked(agent) == "Ждёт ответа на AskUserQuestion: Взяться за тесты? Варианты: Да; Нет"
    o, calls = _orders()
    o.turn(True, "да")
    r = run(o.tell(agent, "Да", "взяться за тесты"))
    assert "да, передай" in r and "подтверждаю" not in r


def test_danger_dictionary_gaps():
    for text, what in (("убери старую ветку", "удаление"), ("remove the old branch", "удаление"), ("git clean -fdx", "удаление"),
                       ("почисти логи", "удаление"), ("стереть кэш", "удаление"), ("откати последний коммит", "сброс"),
                       ("git rebase -i", "сброс"), ("git revert HEAD", "сброс"), ("залей на гитхаб", "пуш"),
                       ("UPDATE users SET a=1", "запись в базу"), ("запиши в базе", "запись в базу")):
        assert (danger(text) or ("",))[0] == what, text


def test_same_turn_after_danger_sends_nothing():
    o, calls = _orders()
    o.turn(True, "пусть отправит изменения на гитхаб")  # опасное слово — только в тексте мозга
    assert "подтверждаю пуш" in run(o.tell(E[0], "Запушь изменения", "запушить"))
    assert run(o.tell(E[0], "Отправь изменения на GitHub", "отправить")).startswith("Не передано") and calls == []
    o.turn(True, "займись тестами")
    r = run(o.tell(E[0], "Займись тестами", "тесты"))
    assert "да, передай" in r and "подтверждаю" not in r


def test_cli_timeout_warns_it_may_have_arrived(monkeypatch):
    from agent_voice.orders import cli
    monkeypatch.setenv("AGENT_SPEAK_BIN", "sleep")
    ok, out = run(cli("5", timeout=0.2))
    assert not ok and "не знаю, дошло ли" in out


def test_flickering_onset_keeps_first_syllable():
    # под вентиляторами VAD «мигает» на первом слоге: начало не должно вытесниться из запаса
    probs = [0.1] * 10 + [0.9] * 6 + [0.1] + [0.9] * 6 + [0.1] + [0.9] * 8 + [0.1] * 22
    ev = feed(Segmenter(), probs)
    audio = next(e[1] for e in ev if e[0] == "phrase")
    assert bytes([10]) * 1024 in audio, "первый кадр речи на месте"


def _oma_hearing(sent, emitted, heard):
    clk, oma = _oma(sent, emitted)
    oma.brain.texts = []
    oma.ear.audio = {}

    async def hear(pcm):
        heard.append(pcm)
        return "Какая погода в Москве?"

    oma.hear = hear
    return clk, oma


def test_whole_utterance_retranscribed_at_send():
    sent, emitted, heard = [], [], []
    clk, oma = _oma_hearing(sent, emitted, heard)

    async def go():
        await oma.command({"do": "talk"})
        for n, (text, pcm) in enumerate((("какая погода", b"a"), ("в москве", b"b")), 1):
            await oma.event(("start", n))
            oma.ear.audio[n] = pcm
            await oma.event(("phrase", text, n))
        clk.t += SEND_PAUSE
        await oma.event(("tick",))
    run(go())
    assert heard == [b"ab"] and oma.brain.texts == ["Какая погода в Москве?"]
    me = [e["line"]["bubble"]["text"] for e in emitted if e["line"].get("bubble", {}).get("kind") == "me"]
    assert me == ["Какая погода в Москве?"], "в чате — то, что ушло мозгу"


def test_single_phrase_not_retranscribed_and_dropped_audio_forgotten():
    sent, emitted, heard = [], [], []
    clk, oma = _oma_hearing(sent, emitted, heard)

    async def go():
        await oma.command({"do": "talk"})
        await oma.event(("start", 1))
        oma.ear.audio[1] = b"x"
        await oma.event(("phrase", "лишнее", 1))
        await oma.command({"do": "cancel"})  # Esc: фраза выброшена — и её звук
        await oma.event(("start", 2))
        oma.ear.audio[2] = b"y"
        await oma.event(("phrase", "который час", 2))
        clk.t += SEND_PAUSE
        await oma.event(("tick",))
    run(go())
    assert heard == [] and oma.brain.texts == ["который час"] and oma.ear.audio == {}


def _speaking_through_speakers(text):
    clk = Clock()
    cv = Conversation(clock=clk)
    cv.echo = True  # Ома говорит в динамики, слух — через эхоподавление: её голос всё равно просачивается
    cv.talk()
    cv.ask("что ты умеешь")
    cv.delta(text)
    cv.done()
    cv.audio(True)
    return clk, cv


def test_own_voice_through_speakers_is_not_barge_in():
    clk, cv = _speaking_through_speakers("Могу смотреть, что делают твои агенты, и запускать новых.")
    assert cv.speech_start(1) == [], "её же голос в микрофоне — не повод обрывать её"
    assert cv.phrase("Могу смотреть.", 1) == [] and cv.draft == ""
    acts = cv.audio(False)
    clk.t += 1
    assert cv.phrase("и запускать новых", 2) in ([], [("drop_draft",)]), "хвост её голоса после конца речи — тоже эхо"
    clk.t += SEND_PAUSE
    assert not any(a[0] == "send" for a in cv.tick())


def test_real_barge_in_through_speakers_still_stops():
    clk, cv = _speaking_through_speakers("Могу смотреть, что делают твои агенты.")
    assert cv.speech_start(1) == []
    acts = cv.phrase("стоп, расскажи про почту", 1)
    assert ("pause_voice", True) in acts and ("stop",) not in acts and cv.draft == "стоп, расскажи про почту"


def test_oma_learns_echo_from_ear():
    sent, emitted = [], []
    clk, oma = _oma(sent, emitted)
    oma.ear.echo = True
    run(oma.command({"do": "talk"}))
    assert oma.conv.echo


def test_echo_words_table():
    from agent_voice.conversation import is_echo
    assert is_echo("Не совсем понимаю.", "Не совсем поняла. Ты хочешь, чтобы я посмотрела?")
    assert is_echo("Могу смотреть.", "Могу смотреть, что делают твои агенты.")
    assert not is_echo("расскажи про почту", "Могу смотреть, что делают твои агенты.")
    assert not is_echo("да", "")


VPN = ("От вас нужно удалить в reg.ru запись `vpl2.predmax.ru`. Из-за неё трафик к серверу уходит мимо VPN. "
       "Ещё на сервере остался временный sudo без пароля, агент уберёт его в конце.")


def test_own_tail_before_user_words_is_cut_off():
    # 2026-10-29: пользователь заговорил, пока она договаривала, — её последнее предложение и его вопрос
    # одной фразой; её слов в ней 8 из 15 (меньше ECHO_SHARE), и фраза ушла мозгу целиком
    clk, cv = _speaking_through_speakers(VPN)
    assert cv.speech_start(1) == []
    acts = cv.phrase("...со временной суда без пароля. Агент уберет его в конце. А что там с директусом?", 1)
    assert ("pause_voice", True) in acts and cv.draft == "А что там с директусом?"


def test_echo_judged_by_phrase_onset_not_by_transcript_time():
    # фраза началась под её голос, а распознана через 4 с после того, как она смолкла (пользователь договаривал):
    # окно эха считается от начала речи
    clk, cv = _speaking_through_speakers(VPN)
    assert cv.speech_start(1) == []
    cv.audio(False)
    clk.t += 4
    cv.phrase("...со временной суда без пароля. Агент уберет его в конце. А что там с директусом?", 1)
    assert cv.draft == "А что там с директусом?"


def test_strip_echo_table():
    from agent_voice.conversation import strip_echo
    assert strip_echo("...со временной суда без пароля. Агент уберет его в конце. А что там с директусом?", VPN) \
        == "А что там с директусом?"
    assert strip_echo("Агент уберёт его в конце, а sudo оставь", VPN) == "а sudo оставь"
    assert strip_echo("Удалить запись? Да, удали.", VPN) == "Удалить запись? Да, удали.", "два слова подряд — не эхо"
    assert strip_echo("а что там с директусом", VPN) == "а что там с директусом"
    assert strip_echo("расскажи про почту, агент уберёт его в конце", VPN) \
        == "расскажи про почту, агент уберёт его в конце", "её слова не в начале — уже не эхо"


def test_echo_cut_phrase_skips_whole_retranscription():
    # звук фразы с её голосом в начале нельзя распознавать целиком: эхо вернулось бы в реплику
    sent, emitted, heard = [], [], []
    clk, oma = _oma_hearing(sent, emitted, heard)
    oma.ear.echo = True

    async def go():
        await oma.command({"do": "talk"})
        oma.conv.echo = True
        oma.conv.ask("что с vpn")
        oma.conv.delta(VPN)
        oma.conv.done()
        await oma.event(("audio", True))
        await oma.event(("start", 1))
        oma.ear.audio[1] = b"x"
        await oma.event(("phrase", "Агент уберет его в конце. А что там с директусом?", 1))
        await oma.event(("audio", False))
        await oma.event(("start", 2))
        oma.ear.audio[2] = b"y"
        await oma.event(("phrase", "и почту проверь", 2))
        clk.t += SEND_PAUSE
        await oma.event(("tick",))
    run(go())
    assert heard == [] and oma.brain.texts[-1] == "А что там с директусом? и почту проверь"


def test_ear_keeps_echo_cancelled_stream_through_playback_tail():
    # демон шлёт «audio false», когда записал последний кусок, а pw-cat доигрывает ещё ~0,7 с:
    # сырой микрофон в это время слышит её голос без эхоподавления
    sent, emitted = [], []
    clk, oma = _oma(sent, emitted)
    oma.ear.speaking = False

    async def go():
        await oma.event(("audio", True))
        assert oma.ear.speaking
        await oma.event(("audio", False))
        assert oma.ear.speaking, "хвост pw-cat ещё звучит"
        await asyncio.sleep(main_TAIL() / 2)
        await oma.event(("audio", True))
        await oma.event(("audio", False))
        await asyncio.sleep(main_TAIL() * 0.75)
        assert oma.ear.speaking, "новая фраза продлила хвост"
        await asyncio.sleep(main_TAIL() / 2)
        assert not oma.ear.speaking
    run(go())


def test_listen_through_speakers_uses_echo_mic(monkeypatch):
    from agent_voice import __main__ as main_mod
    monkeypatch.setattr(main_mod, "locked", lambda: False)
    monkeypatch.setattr(main_mod, "mic_outputs", lambda: [])
    sent, emitted = [], []
    clk, oma = _oma(sent, emitted)
    run(oma.command({"do": "listen", "secs": 5, "kind": "here", "said": "Эй, ты тут?", "speakers": True}))
    assert oma.ear.speakers is True, "спросила в динамики — слушать через эхоподавление"
    run(oma.command({"do": "talk"}))
    assert oma.ear.speakers is None, "обычный разговор — вход по тому, куда она говорит"


def test_mic_target_forced_to_speakers(monkeypatch):
    from agent_voice import context
    calls = []
    def fake_run(args, **kw):
        out = {"get-default-source": "bluez_input.x\n", "get-default-sink": "bluez_output.x\n",
               "list": "1\tmic_aec\tx\n2\talsa_input.pci\tx\n"}[args[1]]
        return SimpleNamespace(stdout=out)
    assert context.mic_target(fake_run) == ("", False, ""), "наушники в ушах — их микрофон"
    assert context.mic_target(fake_run, out=True) == ("alsa_input.pci", True, "mic_aec"), "спросила в динамики — сырой ноутбука, её голос в нём есть"


def main_TAIL():
    from agent_voice import __main__ as m
    return m.TAIL


def test_ear_hears_echo_cancelled_stream_while_oma_speaks(monkeypatch):
    # в динамиках: пока она говорит — кадры mic_aec (её голос вырезан, перебив слышно), молчит — сырой микрофон
    from agent_voice import ear as earmod
    from agent_voice.ear import Ear
    probs = iter([0.9] * 10 + [0.1] * 22 + [0.1] * 10000)
    vad = lambda x, rate: SimpleNamespace(item=lambda: next(probs))
    monkeypatch.setattr(earmod, "transcribe", lambda m, pcm: "стоп")
    script = 'if [ "$1" = mic_aec ]; then c=B; else c=A; fi; yes $c | tr -d "\\n" | head -c 65536; sleep 60'
    out = asyncio.Queue()
    e = Ear(out, Whisper(load=lambda: "m"), cmd=["sh", "-c", script, "sh"], vad_load=lambda: vad,
            mic=lambda: ("", True, "mic_aec"))
    e.speaking = True

    async def go():
        await e.start()
        while True:
            ev = await asyncio.wait_for(out.get(), 5)
            if ev[0] == "phrase":
                break
        await e.stop()
    run(go())
    assert set(e.audio[1]) == {ord("B")}, "фраза — из потока без её голоса"


def test_ear_stop_does_not_hang_on_unread_stream():
    # mic_aec молчит, сырой поток льётся: его не читают, буфер полон, транспорт на паузе и EOF не видит —
    # Process.wait() ждал бы его вечно, а с ним и stop(): диспетчер Омы вставал, клик по орбу не выключал слух
    from agent_voice.ear import Ear
    script = 'if [ "$1" = mic_aec ]; then exec sleep 60; else exec cat /dev/zero; fi'
    e = Ear(asyncio.Queue(), Whisper(load=lambda: "m"), cmd=["sh", "-c", script, "sh"], vad_load=lambda: None,
            mic=lambda: ("", True, "mic_aec"))

    async def go():
        await e.start()
        await asyncio.sleep(0.5)
        await asyncio.wait_for(e.stop(), 5)
    run(go())
    assert not e.open


# --- разговорное поведение: «ок» — конец, шум — не реплика, поддакивание не обрывает, молчание по решению мозга ---

def _answered(text):
    """Разговор по клику: вопрос ушёл, ответ Омы прозвучал, она слушает."""
    clk, cv = _conv()
    cv.talk(); cv.speech_start(1); cv.phrase("как дела у агентов", 1)
    clk.t += SEND_PAUSE; cv.tick()
    cv.delta(text); cv.done(); cv.audio(True); cv.audio(False)
    assert cv.state == "listening"
    return clk, cv


def _reply(clk, cv, text, n=2):
    cv.speech_start(n); cv.phrase(text, n)
    clk.t += SEND_PAUSE
    return cv.tick()


def test_ack_after_answer_ends_conversation():
    clk, cv = _answered("Оба агента простаивают.")
    acts = _reply(clk, cv, "Окей.")
    assert ("ear", False) in acts and not any(a[0] == "send" for a in acts) and cv.state == "idle"


def test_ack_after_question_is_an_answer():
    clk, cv = _answered("Передать агенту, чтобы запускал тесты?")
    acts = _reply(clk, cv, "Хорошо.")
    assert ("send", "Хорошо.", "") in acts and cv.active


def test_noise_is_not_a_reply():
    from agent_voice.conversation import is_noise
    for t in ["Кхм.", "Эм...", "Ммм", "Хм?", "[кашель]", "(вздыхает)", "*кашляет*", "Э-э-э", "Ну...", "А?", "Кхе-кхе."]:
        assert is_noise(t), t
    for t in ["Да.", "Нет", "Ага.", "Стоп.", "Ну что там с почтой?", "Окей"]:
        assert not is_noise(t), t
    clk, cv = _answered("Оба агента простаивают.")
    assert not any(a[0] == "send" for a in _reply(clk, cv, "Кхм.")) and cv.draft == "" and cv.active


def test_backchannel_through_speakers_does_not_stop_her():
    clk, cv = _speaking_through_speakers("Могу смотреть, что делают твои агенты, и запускать новых.")
    cv.speech_start(1)
    assert cv.phrase("Угу.", 1) == [] and cv.state == "speaking"
    cv.speech_start(2)
    assert cv.phrase("Кхм.", 2) == [] and cv.state == "speaking"


def test_cough_while_thinking_keeps_the_answer():
    clk, cv = _conv()
    cv.talk(); cv.speech_start(1); cv.phrase("как дела у агентов", 1)
    clk.t += SEND_PAUSE; cv.tick()
    assert cv.state == "thinking"
    assert ("interrupt",) not in cv.speech_start(2), "начало звука — ещё не перебив"
    assert ("interrupt",) not in cv.phrase("Кхм.", 2) and cv.turn
    assert any(a[0] == "say" for a in cv.delta("Оба агента сейчас простаивают и ждут тебя. "))


def test_words_while_thinking_interrupt():
    clk, cv = _conv()
    cv.talk(); cv.speech_start(1); cv.phrase("как дела у агентов", 1)
    clk.t += SEND_PAUSE; cv.tick()
    cv.speech_start(2)
    acts = cv.phrase("и ещё про почту", 2)
    assert ("interrupt",) in acts and cv.draft == "и ещё про почту" and cv.state == "listening"


def test_words_while_thinking_interrupt_even_if_she_started_speaking():
    clk, cv = _conv()
    cv.talk(); cv.speech_start(1); cv.phrase("как дела у агентов", 1)
    clk.t += SEND_PAUSE; cv.tick()
    cv.speech_start(2)
    cv.delta("Оба агента сейчас простаивают и ждут тебя. "); cv.audio(True)
    acts = cv.phrase("подожди, а почта", 2)
    assert ("pause_voice", True) in acts and cv.draft == "подожди, а почта"


def _thinking(text):
    clk, cv = _conv()
    cv.talk(); cv.speech_start(1); cv.phrase(text, 1)
    clk.t += SEND_PAUSE; cv.tick()
    return cv


def test_silent_mark_says_nothing():
    # инструмент silent модель звала по кругу (8 раз на «Кхм.»): метка в ответе — без лишних кругов
    from agent_voice.brain import OMA_PROMPT
    assert "[молчу]" in OMA_PROMPT and "[конец]" in OMA_PROMPT
    cv = _thinking("да, мам, сейчас приду")
    assert cv.delta(" [мол") == [] and cv.delta("чу]") == []
    assert cv.done() == [("state", "listening")] and cv.active


def test_end_mark_closes_conversation():
    cv = _thinking("ну всё, спасибо, свободна")
    assert cv.delta("[конец]") == []
    assert ("ear", False) in cv.done() and cv.state == "idle"


def test_bracket_that_is_not_a_mark_is_spoken():
    cv = _thinking("что в скобках")
    cv.delta("[Пример] — это пример текста в скобках.")
    assert any(a == ("say", "Пример — это пример текста в скобках.") for a in cv.done())


def test_marks_hidden_from_chat_history():
    from agent_voice.brain import messages_of
    m = lambda t, x: SimpleNamespace(type=t, message={"content": [{"type": "text", "text": x}]})
    assert messages_of([m("user", "кхм"), m("assistant", "[молчу]"), m("user", "как дела"), m("assistant", "Хорошо.")]) == \
        [{"role": "me", "text": "кхм"}, {"role": "me", "text": "как дела"}, {"role": "oma", "text": "Хорошо."}]


def test_quick_ack_right_after_her_last_word_ends_conversation():
    # 2026-10-29: «ок» сразу после её фразы через динамики пропадало — срез через секунду после конца её звука
    # (_cut_later) выбрасывал фразу, начатую под её голос, целиком; теперь её хвост снимает текст (strip_echo)
    clk, cv = _speaking_through_speakers("Два агента работают, а исследование простаивает.")
    cv.speech_start(1)
    cv.done(); cv.audio(False)
    cv.phrase("Простаивает. Ок.", 1)
    assert cv.draft == "Ок."
    clk.t += SEND_PAUSE
    assert ("ear", False) in cv.tick() and cv.state == "idle"


def test_strip_echo_her_last_word_only_when_phrase_began_under_her_voice():
    from agent_voice.conversation import strip_echo
    said = "Два агента работают, а исследование простаивает."
    assert strip_echo("Простаивает. Ок.", said, tail=True) == "Ок."
    assert strip_echo("Простаивает. Ок.", said) == "Простаивает. Ок.", "одно слово без начала под её голос — не эхо"
    assert strip_echo("Работают. Ок.", said, tail=True) == "Работают. Ок.", "не последнее её слово — не хвост"


# --- отклик быстрее: щелчок «услышала», слова на инструментах, снимок агентов в реплике, claude по клику, первая запятая ---

def test_voiced_send_chimes_first():
    sent, emitted = [], []
    clk, oma = _oma(sent, emitted)

    async def go():
        await oma.command({"do": "talk"})
        await oma.event(("start", 1))
        await oma.event(("phrase", "как дела у агентов", 1))
        clk.t += SEND_PAUSE
        await oma.event(("tick",))
    run(go())
    assert {"cmd": "chime"} in sent and oma.brain.texts


def test_typed_or_quiet_send_does_not_chime():
    sent, emitted = [], []
    clk, oma = _oma(sent, emitted)
    run(oma.command({"do": "ask", "text": "как дела"}))
    assert {"cmd": "chime"} not in sent
    clk, oma = _oma(sent, emitted)

    async def go():
        await oma.event(("quiet", True))
        await oma.command({"do": "talk"})
        await oma.event(("start", 1))
        await oma.event(("phrase", "как дела у агентов", 1))
        clk.t += SEND_PAUSE
        await oma.event(("tick",))
    run(go())
    assert {"cmd": "chime"} not in sent


def test_tool_call_says_what_she_does():
    cv = _thinking("как дела у агентов")
    acts = cv.tool("mcp__oma__agents")
    assert ("say", "Смотрю агентов.") in acts and ("show", "Смотрю агентов.") in acts and cv.state == "speaking"
    cv.audio(True)
    assert cv.tool("mcp__oma__agent_tail") == [], "она ещё говорит — тишины нет"
    cv.audio(False)
    assert ("say", "Читаю переписку агента.") in cv.tool("mcp__oma__agent_tail"), "каждый инструмент — своё слово"
    assert cv.tool("thinking") == [], "раздумье после сказанного — без слова"
    assert cv.tool("mcp__oma__presence") == []


def test_thinking_first_says_a_word():
    cv = _thinking("сложный вопрос")
    assert any(a[0] == "say" for a in cv.tool("thinking"))
    quiet = _thinking("тихо")
    quiet.quiet = True
    assert not any(a[0] == "say" for a in quiet.tool("mcp__oma__agents"))


def test_claude_events_report_tool_and_thinking_start():
    st = {"skip": False}
    ev = lambda block: SimpleNamespace(event={"type": "content_block_start", "content_block": block})
    assert claude_events(ev({"type": "tool_use", "name": "mcp__oma__agents"}), st) == [("tool", "mcp__oma__agents")]
    assert claude_events(ev({"type": "thinking"}), st) == [("tool", "thinking")]
    assert claude_events(ev({"type": "text"}), st) == []


def test_oma_routes_tool_event_to_voice():
    sent, emitted = [], []
    clk, oma = _oma(sent, emitted)

    async def go():
        await oma.command({"do": "talk"})
        oma.conv.ask("как дела у агентов")
        await oma.event(("tool", "mcp__oma__agents"))
    run(go())
    assert {"cmd": "voice", "text": "Смотрю агентов."} in sent


def test_agents_snapshot_rides_with_reply_and_hides_from_chat():
    from agent_voice.brain import with_agents, strip_note, messages_of, general_chats
    entries = [{"title": "Ома", "project": "agent-speak", "id": "a1", "state": "working", "since": 0, "status": "правит слух"}]
    text = with_agents("как дела у агентов", entries, 60)
    assert text.startswith("как дела у агентов") and "правит слух" in text and strip_note(text) == "как дела у агентов"
    m = lambda t, x: SimpleNamespace(type=t, message={"content": [{"type": "text", "text": x}]})
    assert messages_of([m("user", text)]) == [{"role": "me", "text": "как дела у агентов"}]
    info = SimpleNamespace(session_id="s", custom_title=None, summary=None, first_prompt=text, last_modified=0, tag=None)
    assert general_chats([info])[0]["title"] == "как дела у агентов"


def test_brain_warms_once_and_send_reuses_it():
    from agent_voice.brain import Brain
    b = Brain(asyncio.Queue(), lambda: [])
    calls, queries = [], []

    class Client:
        async def query(self, t): queries.append(t)

    async def connect():
        calls.append(1)
        await asyncio.sleep(0.01)
        b.client, b.reader = Client(), asyncio.get_running_loop().create_future()

    b._connect = connect

    async def go():
        await asyncio.gather(b.warm(), b.warm(), b.send("привет"))
    run(go())
    assert calls == [1] and queries and queries[0].startswith("привет")


def test_talk_warms_brain():
    sent, emitted = [], []
    clk, oma = _oma(sent, emitted)

    async def go():
        await oma.command({"do": "talk"})
        await asyncio.sleep(0)
    run(go())
    assert oma.brain.warmed


def test_long_first_sentence_starts_at_comma():
    ch = Chunker()
    assert ch.feed("Агент по Directus закрыл пять задач из шести, осталась пятая, и ему") == \
        ["Агент по Directus закрыл пять задач из шести,"]
    assert ch.feed(" нужно ваше решение. Дальше") == ["осталась пятая, и ему нужно ваше решение."], "дальше — предложениями"
    short = Chunker()
    assert short.feed("Да, конечно, сейчас") == [], "короткое — ждёт конца предложения"


# --- голосом агентам ничего не уходит без «да, передай»: 2026-10-29 Ома сама составила задание из обрывков
# («Стой. Сделай. Короче. Стоп.») и приняла «продолжай» за согласие — текст набрался в строке ввода агента ---

def _ordinary(clk=None):
    o, calls = _orders(clk)
    shown = []
    o.show = shown.append
    o.turn(True, "скажи агенту, пусть делает обе оптимизации")
    r = run(o.tell(E[0], "Делай обе оптимизации", "делать обе оптимизации"))
    return o, calls, shown, r


def test_voice_order_waits_for_yes_and_reads_back():
    o, calls, shown, r = _ordinary()
    assert calls == [] and "«Передать проекту omarchy-mail: делать обе оптимизации? Скажи: да, передай.»" in r
    assert shown == ["Передам проекту omarchy-mail: «Делай обе оптимизации»"], "полный текст — в окне"
    assert run(o.confirm("Да, передай.")) == "Передала проекту omarchy-mail: делать обе оптимизации."
    assert calls == [("tell", "s1", "0", "Делай обе оптимизации")] and "подтвердил" in o.note


def test_continue_is_not_consent():
    for heard in ("продолжай", "У меня.", "давай", "ага", "да, и ещё скажи ему про тесты потом"):
        o, calls, _, _ = _ordinary()
        assert run(o.confirm(heard)) is None and o.pending is None and calls == [], heard
        assert "ничего не передала" in o.note, "мозг знает, что не ушло"


def test_half_yes_asks_again_and_no_cancels():
    o, calls, _, _ = _ordinary()
    assert run(o.confirm("да")) == "Не расслышала. Скажи: да, передай." and o.pending and not calls
    assert run(o.confirm("передай")) == "Не расслышала. Скажи: да, передай." and o.pending
    assert run(o.confirm("нет, не надо")) == "Хорошо, не передаю." and o.pending is None and not calls


def test_voice_launch_waits_for_yes_launch():
    o, calls = _orders()
    o.turn(True, "запусти агента в почте, пусть займётся тестами")
    r = run(o.launch("/p/mail", "Займись тестами", "заняться тестами"))
    assert "«Запустить агента в проекте mail: заняться тестами? Скажи: да, запускай.»" in r and not calls
    assert run(o.confirm("Да, запускай!")) == "Запустила агента в проекте mail: заняться тестами." and calls


def test_danger_ignores_plain_yes():
    o, calls = _armed()
    assert run(o.confirm("да, передай")) == "Не расслышала. Скажи: подтверждаю пуш." and o.pending and not calls


def test_option_read_back_names_the_variant():
    agent = dict(E[0], ask=PUSH_MENU)
    o, calls = _orders()
    shown = []
    o.show = shown.append
    o.turn(True, "нет, потом")
    run(o.tell(agent, "", "не пушить", option=2))
    assert shown == ["Передам проекту omarchy-mail: вариант 2 — Нет — потом"]


def test_prompt_forbids_composing_from_fragments():
    from agent_voice import brain
    assert "из обрывков" in brain.OMA_PROMPT and "«Продолжай»" in brain.OMA_PROMPT


def test_oma_shows_pending_order_in_window():
    sent, emitted = [], []
    clk, oma = _oma(sent, emitted)
    oma.orders.show("Передам проекту x: «текст»")
    assert any(e["line"].get("bubble", {}).get("text") == "Передам проекту x: «текст»" for e in emitted)


def test_typed_reply_drops_pending_voice_order():
    sent, emitted = [], []
    clk, oma = _oma(sent, emitted)
    oma.orders, calls = _orders(clk)
    oma.orders.turn(True, "пусть делает тесты")
    run(oma.orders.tell(E[0], "Делай тесты", "тесты"))
    run(oma.command({"do": "ask", "text": "а что с почтой"}))
    assert oma.orders.pending is None and calls == []



# --- конец реплики по звуку (Smart Turn) и реплика целиком заранее ---

def test_turn_end_sets_send_pause():
    from agent_voice.conversation import WAIT_MORE
    for done, wait in ((0.95, 0.0), (0.7, SEND_PAUSE), (0.2, WAIT_MORE), (None, SEND_PAUSE)):
        clk, cv = _conv()
        cv.talk(); cv.speech_start(1); cv.phrase("скажи агенту, пусть", 1, done)
        clk.t += max(wait - 0.05, 0)
        if wait:
            assert not any(a[0] == "send" for a in cv.tick()), done
            clk.t += 0.1
        assert any(a[0] == "send" for a in cv.tick()), done


def test_oma_asks_turn_end_about_whole_utterance():
    sent, emitted, heard = [], [], []
    clk, oma = _oma_hearing(sent, emitted, heard)
    seen = []
    oma.turn_end = SimpleNamespace(complete=lambda pcm: seen.append(pcm) or 0.97)

    async def go():
        await oma.command({"do": "talk"})
        for n, (text, pcm) in enumerate((("какая погода", b"aa"), ("в москве", b"bb")), 1):
            await oma.event(("start", n))
            oma.ear.audio[n] = pcm
            await oma.event(("phrase", text, n))
        await oma.event(("tick",))  # договорил — без паузы
    run(go())
    assert seen == [b"aa", b"aabb"] and oma.brain.texts == ["Какая погода в Москве?"]


def test_turn_end_failure_falls_back_to_pause():
    sent, emitted = [], []
    clk, oma = _oma(sent, emitted)
    oma.turn_end = SimpleNamespace(complete=lambda pcm: 1 / 0)
    oma.ear.audio = {1: b"aa"}

    async def go():
        await oma.command({"do": "talk"})
        await oma.event(("start", 1))
        await oma.event(("phrase", "как дела", 1))
        await oma.event(("tick",))
        assert oma.brain.texts == []
        clk.t += SEND_PAUSE
        await oma.event(("tick",))
    run(go())
    assert oma.brain.texts == ["как дела"]


def test_smart_turn_features_and_missing_model():
    import numpy as np
    from agent_voice.turn import SmartTurn, features
    from faster_whisper.feature_extractor import FeatureExtractor
    f = features(np.zeros(16000, np.float32) + 0.1, FeatureExtractor(feature_size=80, chunk_length=8))
    assert f.shape == (1, 80, 800) and f.dtype == np.float32
    assert SmartTurn("/нет/такой/модели.onnx").complete(b"\0\0" * 1600) is None


def test_whole_utterance_heard_ahead_while_pausing():
    # реплика целиком распознаётся, пока идёт пауза, а не при отправке: отправка её не ждёт
    sent, emitted, heard = [], [], []
    clk, oma = _oma_hearing(sent, emitted, heard)

    async def go():
        await oma.command({"do": "talk"})
        for n, (text, pcm) in enumerate((("какая", b"a"), ("погода", b"b"), ("в москве", b"c")), 1):
            await oma.event(("start", n))
            oma.ear.audio[n] = pcm
            await oma.event(("phrase", text, n))
            await asyncio.sleep(0)
        assert heard == [b"ab", b"abc"], "распознаётся по мере роста реплики"
        clk.t += SEND_PAUSE
        await oma.event(("tick",))
    run(go())
    assert heard == [b"ab", b"abc"] and oma.brain.texts == ["Какая погода в Москве?"]



# --- перебив — пауза, а не обрыв: кашель — она продолжает; слова не ей («[молчу]») — продолжает; ответ по делу —
# прерванный ответ выбрасывается ---

def _speaking_in_buds():
    clk, cv = _conv()
    _ask_answer(clk, cv)  # ответ звучит, ход мозга ещё идёт
    assert cv.state == "speaking" and cv.turn
    return clk, cv


def test_cough_in_buds_pauses_then_resumes():
    clk, cv = _speaking_in_buds()
    assert cv.speech_start(2) == [("pause_voice", True), ("draft", "", True)]
    assert cv.audio(False) == [] and cv.state == "speaking", "пауза — не конец её речи"
    acts = cv.phrase("Кхм.", 2)
    assert ("pause_voice", False) in acts and ("interrupt",) not in acts and cv.state == "speaking" and cv.turn


def _suspended(text="да, мам, сейчас приду"):
    clk, cv = _speaking_in_buds()
    cv.speech_start(2)
    cv.audio(False)  # демон: на паузе она молчит
    acts = cv.phrase(text, 2)
    assert ("interrupt",) in acts and ("stop",) not in acts and ("pause_voice", False) not in acts
    assert cv.state == "listening" and cv.draft == text
    clk.t += SEND_PAUSE
    assert any(a[0] == "send" for a in cv.tick())
    return clk, cv


def test_words_not_for_her_resume_her_answer():
    clk, cv = _suspended()
    assert cv.delta("[молчу]") == []
    assert cv.done() == [("pause_voice", False), ("state", "speaking")], "продолжает прерванный ответ"
    assert cv.audio(True) == [] and cv.audio(False) == [("state", "listening")]


def test_real_answer_after_barge_drops_old_answer():
    clk, cv = _suspended("подожди, а что с почтой")
    acts = cv.delta("Почта пуста, новых писем нет. ")
    assert acts[:2] == [("stop",), ("pause_voice", False)] and ("say", "Почта пуста, новых писем нет.") in acts


def test_tool_after_barge_drops_old_answer_first():
    clk, cv = _suspended("подожди, а что с почтой")
    acts = cv.tool("mcp__oma__agents")
    assert acts[:2] == [("stop",), ("pause_voice", False)] and ("say", "Смотрю агентов.") in acts


def test_end_mark_after_barge_ends_and_unpauses():
    clk, cv = _suspended("всё, свободна")
    cv.delta("[конец]")
    acts = cv.done()
    assert ("stop",) in acts and ("pause_voice", False) in acts and cv.state == "idle"


def test_cancel_while_paused_unpauses():
    clk, cv = _speaking_in_buds()
    cv.speech_start(2)
    acts = cv.cancel()
    assert ("stop",) in acts and ("pause_voice", False) in acts and cv.state == "listening"


def test_speakers_barge_pauses_too():
    clk, cv = _speaking_through_speakers("Могу смотреть, что делают твои агенты, и запускать новых.")
    cv.speech_start(1)
    acts = cv.phrase("да, мам, сейчас", 1)
    assert ("pause_voice", True) in acts and ("stop",) not in acts and cv.state == "listening"


def test_oma_sends_her_pause_to_daemon():
    sent, emitted = [], []
    clk, oma = _oma(sent, emitted)
    run(oma.act([("pause_voice", True), ("pause_voice", False)]))
    assert [c for c in sent if c["cmd"] == "oma_pause"] == [{"cmd": "oma_pause", "on": True}, {"cmd": "oma_pause", "on": False}]
