# Слово-побудка «Ома» (дизайн 2026-11-12, «wake-word»): python -m pytest python/test_wake.py
import asyncio
import json
import sys
from types import SimpleNamespace

from test_agent_voice import Clock, _oma, feed, run  # noqa: F401 — общие заглушки разговора и Омы
from agent_voice import wake as wakemod
from agent_voice.conversation import Conversation
from agent_voice.ear import Whisper, transcribe
from agent_voice.segment import Segmenter
from agent_voice.wake import CLIP, PRE, VoskWake, Waker, named, strip_wake


def test_segmenter_keep_caps_the_phrase_in_progress():
    # под телевизором речь не кончается: буфер слуха на «Ома» — не больше keep кадров, а фраза кончается как всегда
    seg = Segmenter(keep=30)
    ev = feed(seg, [0.9] * 200)
    assert ev == [("start",)] and len(seg.buf) == 30
    ev = feed(seg, [0.1] * 22)
    assert ev[0][0] == "phrase" and len(ev[0][1]) == 30 * 1024
    assert len(feed(Segmenter(), [0.9] * 200 + [0.1] * 22)[1][1]) == 222 * 1024, "без keep — вся фраза, как раньше"


def test_name_first_word_table():
    for t in ("Ома.", "Ома, что у Jira?", "ома тут", "Oma, привет", "  — Ома!", "ОМА"):
        assert named(t), t
    for t in ("Дома.", "Кома, ты тут?", "Она тут?", "Омар", "Закон Ома.", "", "Омский поезд"):
        assert not named(t), t
    assert strip_wake("Ома, что у Jira?") == "что у Jira?"
    assert strip_wake("Ома.") == "" and strip_wake("Oma! тут") == "тут"
    assert strip_wake("Закон Ома.") == "Закон Ома.", "имя — только в начале"


class FakeDet:
    """Детектор первой ступени: попадание — на hit_at-м кадре речи (от start) или в конце речи, время — hit_s."""

    def __init__(self, hit_at=None, hit_s=0.0, at_end=False):
        self.hit_at, self.hit_s, self.at_end, self.n, self.starts = hit_at, hit_s, at_end, 0, 0

    def start(self):
        self.starts += 1
        self.n = 0

    def feed(self, f):
        self.n += 1
        return self.hit_s if self.n == self.hit_at and not self.at_end else None

    def end(self):
        return self.hit_s if self.at_end else None


def _frames(w, probs, k=0):
    out = []
    for i, p in enumerate(probs):
        out += w.frame(k + i, p, (k + i).to_bytes(2, "little") * 512)
    return out


def test_waker_feeds_only_speech_and_clips_from_half_a_second_before_the_name():
    det = FakeDet(hit_at=30, hit_s=1.0)  # «ома» с 1,0 с ленты детектора: кадр 31 от её начала
    w = Waker(det)
    assert _frames(w, [0.1] * 40) == [] and det.starts == 0, "тишина детектору не идёт"
    clips = _frames(w, [0.9] * 20 + [0.1] * 22, k=40)
    assert det.starts == 1 and w.k0 == 40 - 16, "речь — с предзахвата"
    at, pcm = clips[0]
    assert at == w.k0 + 31 - PRE
    assert len(pcm) == 1024 * (40 + 20 + 22 - at), "речь кончилась раньше 3 с — клип до конца речи"
    assert pcm[:2] == at.to_bytes(2, "little")
    assert w.busy and _frames(w, [0.9] * 40 + [0.1] * 22, k=200) == [], "вторая ступень думает — новых клипов нет"


def test_waker_clip_is_at_most_three_seconds_and_name_at_the_end_of_speech():
    w = Waker(FakeDet(hit_at=10, hit_s=0.0))
    clips = _frames(w, [0.9] * 300)  # телевизор: речь не кончается
    assert len(clips) == 1 and clips[0][0] == 0 and len(clips[0][1]) == CLIP * 1024
    w = Waker(FakeDet(hit_s=0.7, at_end=True))  # Vosk отдал «ома» в FinalResult
    clips = _frames(w, [0.1] * 20 + [0.9] * 10 + [0.1] * 22)
    assert clips and clips[0][0] == w.k0 + int(0.7 * 16000 / 512) - PRE
    assert [x[0] for x in w.since(clips[0][0])] == list(range(clips[0][0], 52)), "разговор после побудки — с клипа"


def test_vosk_wake_reads_result_at_every_boundary_and_finds_the_name():
    calls = []

    class Rec:
        def __init__(self, *a):
            calls.append(("new", a[2]))

        def SetWords(self, on):
            pass

        def AcceptWaveform(self, f):
            return f == b"end"

        def Result(self):
            calls.append("Result")
            return json.dumps({"result": [{"word": "[unk]", "start": 0.1}, {"word": "ома", "start": 0.62, "conf": 1.0}]})

        def FinalResult(self):
            calls.append("Final")
            return json.dumps({"text": ""})

    sys.modules["vosk"] = SimpleNamespace(KaldiRecognizer=Rec)
    try:
        v = VoskWake("/nonexistent")
        v.start()
        assert v.feed(b"x") is None and v.feed(b"end") == 0.62
        assert v.end() is None and v.end() is None, "второй end без речи — без распознавателя"
        assert calls == [("new", '["ома", "[unk]"]'), "Result", "Final"]
    finally:
        del sys.modules["vosk"]
    assert not wakemod.available("/nonexistent")


def test_transcribe_passes_hotwords():
    got = {}

    class Model:
        def transcribe(self, audio, **kw):
            got.update(kw)
            return [], None
    transcribe(Model(), b"\0\0" * 160, "Ома")
    assert got["hotwords"] == "Ома" and got["language"] == "ru"
    transcribe(Model(), b"\0\0" * 160)
    assert got["hotwords"] is None, "обычная фраза — без подсказки"


def test_wake_mic_is_the_builtin_one_even_with_a_headset_in_the_ears():
    from agent_voice.context import pick_mic
    srcs = "43\tmic_aec\tPipeWire\n67\talsa_input.pci-0000_00_1f.3.analog-stereo\tPipeWire\n80\tbluez_input.00_11.0\tPipeWire\n"
    raw = "alsa_input.pci-0000_00_1f.3.analog-stereo"
    assert pick_mic("bluez_input.00_11.0", "bluez_output.00_11.1", srcs, out=False) == ("", False, ""), "разговор — гарнитура"
    assert pick_mic("bluez_input.00_11.0", "bluez_output.00_11.1", srcs, out=False, wake=True) == (raw, False, "")
    assert pick_mic("mic_denoise", "alsa_output.x", srcs, out=False, wake=True) == (raw, True, "mic_aec")
    assert pick_mic("bluez_input.00_11.0", "bluez_output.00_11.1", "80\tbluez_input.00_11.0\tPipeWire\n", out=False, wake=True)[0] == "", \
        "встроенного нет — побудки нет, Bluetooth не берём"


def test_mic_watch_events_table():
    from agent_voice.context import MIC_EVENT
    # change on source-output — cork/uncork чужого потока записи: открытый на паузе поток заговорил (foreign_capture)
    for line in ("Event 'new' on source-output #12", "Event 'remove' on source-output #12", "Event 'change' on source-output #12",
                 "Event 'change' on source #45", "Event 'change' on server #-1"):
        assert MIC_EVENT.search(line + "\n"), line
    for line in ("Event 'change' on client #1693", "Event 'new' on sink-input #3", "Event 'change' on sink #2"):
        assert not MIC_EVENT.search(line + "\n"), line


# Слух: pw-record подменён sh (живёт, пока его не убьют; --target и "-" — его аргументы), VAD — заглушка по списку
LIVE = ["sh", "-c", "sleep 60", "-"]


class LoadDet(FakeDet):
    def load(self):
        pass


def test_ear_wake_listens_on_the_builtin_mic_and_switches_mode_without_restart():
    from agent_voice.ear import Ear
    talk = [("alsa_input.pci", True, "mic_aec")]
    wake_mic = [("alsa_input.pci", True, "mic_aec")]
    e = Ear(asyncio.Queue(), Whisper(load=lambda: "m"), cmd=LIVE, vad_load=lambda: None, muted=lambda t: False,
            mic=lambda wake=False, out=None: wake_mic[0] if wake else talk[0], wake=LoadDet())

    async def go():
        assert await e.start(wake=True) and e.waking and e.target == "alsa_input.pci"
        assert e._aec is None, "эхоподавление ради побудки не держим"
        proc = e._proc
        assert await e.start() and e._proc is proc and not e.waking, "тот же вход — без перезапуска: начало фразы не съедено"
        assert e._aec is not None and e.echo, "разговор в динамиках — со входом эхоподавления"
        assert await e.start(wake=True) and e._proc is not proc and e._aec is None, "из разговора с mic_aec — заново"
        talk[0] = ("", False, "")  # гарнитура в ушах: разговор — её микрофон
        proc = e._proc
        assert await e.start() and e._proc is not proc and e.target == "" and not e.waking
        wake_mic[0] = ("", False, "")
        assert await e.start(wake=True) is False and not e.open, "встроенного нет — слух закрыт"
        await e.stop()
    run(go())


def _frames_cmd(*batches, pause=0.3):
    """pw-record: пачки нулевых кадров с паузой между ними, затем живёт."""
    src = "import sys, time\n" + "".join(f"sys.stdout.buffer.write(bytes({n * 1024})); sys.stdout.flush(); time.sleep({pause})\n"
                                        for n in batches) + "time.sleep(60)"
    return [sys.executable, "-c", src, "-"]


def test_ear_wake_confirms_the_name_and_the_conversation_hears_the_whole_phrase(monkeypatch):
    from agent_voice import ear as earmod
    from agent_voice.ear import Ear
    probs = iter([0.1] * 20 + [0.9] * 20 + [0.1] * 22 + [0.1] * 40)
    asked = []

    def whisper_text(m, pcm, hotwords=None):
        asked.append((hotwords, len(pcm)))
        return "Ома, что нового?"
    monkeypatch.setattr(earmod, "transcribe", whisper_text)
    out = asyncio.Queue()
    e = Ear(out, Whisper(load=lambda: "m"), cmd=_frames_cmd(62, 40), vad_load=lambda: lambda x, r: SimpleNamespace(item=lambda: next(probs)),
            mic=lambda wake=False, out=None: ("alsa_input.pci", False, ""), muted=lambda t: False, wake=LoadDet(hit_at=10, hit_s=0.9))

    async def go():
        await e.start(wake=True)
        ev = await asyncio.wait_for(out.get(), 5)
        assert ev[0] == "wake" and ev[1] == "Ома, что нового?" and asked == [("Ома", 1024 * (63 - ev[2]))], \
            "вторая ступень — с подсказкой, по клипу до конца речи; событий start в побудке нет"
        assert await e.take(ev[2]) and not e.waking
        got = [await asyncio.wait_for(out.get(), 5) for _ in range(2)]
        await e.stop()
        return ev[2], got
    at, got = run(go())
    assert got[0][:2] == ("start", 1) and got[1] == ("phrase", "Ома, что нового?", 1), "фраза с именем — заново, целиком"
    assert at == 17 and asked[1] == (None, 1024 * (63 - at)), "разговор распознаёт без подсказки, с кадра клипа"


def test_ear_wake_forgets_a_near_miss_and_skips_her_voice_in_the_speakers(monkeypatch):
    import torch  # noqa: F401 — как в службе: torch уже загружен прогревом VAD, кадры читаются вовремя
    from agent_voice import ear as earmod
    from agent_voice.ear import Ear
    monkeypatch.setattr(earmod, "transcribe", lambda m, pcm, hotwords=None: "Дома, что нового?")
    probs = iter([0.9] * 20 + [0.1] * 22 + [0.9] * 20 + [0.1] * 22)
    out, det = asyncio.Queue(), LoadDet(hit_at=10, hit_s=0.3)
    e = Ear(out, Whisper(load=lambda: "m"), cmd=_frames_cmd(42, 42, pause=1.0),
            vad_load=lambda: lambda x, r: SimpleNamespace(item=lambda: next(probs)),
            mic=lambda wake=False, out=None: ("alsa_input.pci", True, "mic_aec"), muted=lambda t: False, wake=det)

    async def go():
        await e.start(wake=True)
        await asyncio.sleep(0.5)
        assert out.empty() and det.starts == 1, "«Дома» — вторая ступень не подтвердила: звук забыт, событий нет"
        e.speaking = True  # её голос в динамиках (echo) — кадры побудки не слушаются
        await asyncio.sleep(1.2)
        assert det.starts == 1 and out.empty()
        await e.stop()
    run(go())


def test_mic_watch_checks_once_per_burst_of_pactl_events():
    from agent_voice.ear import Ear
    lines = "\\n".join(["Event 'change' on client #1", "Event 'new' on source-output #7", "Event 'change' on source #45"])
    out = asyncio.Queue()
    e = Ear(out, Whisper(load=lambda: "m"), cmd=LIVE, vad_load=lambda: None, mic=lambda: ("", False, ""), muted=lambda t: False,
            watch=["sh", "-c", f'printf "{lines}\\n"; sleep 60'])

    async def go():
        await e.start()
        assert await asyncio.wait_for(out.get(), 5) == ("mic_check",)
        await asyncio.sleep(0.3)
        assert out.empty(), "проверка ещё в очереди Омы — пачка событий её не множит"
        sub = e._sub
        await e.stop()
        assert sub.returncode is not None, "слух закрыт — подписки нет"
    run(go())


# --- Ома: побудка в покое ---
class WakeEar:
    def __init__(self, mic=True):
        self.open = self.waking = False
        self.mic, self.calls, self.target, self.echo, self.mic_pending, self.pid = mic, [], "alsa_input.pci", False, False, 1

    async def start(self, wake=False):
        self.calls.append("wake" if wake else "talk")
        self.open, self.waking = (self.mic, self.mic) if wake else (True, False)
        return self.open

    def wake_load(self):
        pass

    async def take(self, at):
        self.calls.append(("take", at))
        self.waking = False
        return True

    async def stop(self):
        self.calls.append("stop")
        self.open = self.waking = False


def _woke_oma(monkeypatch, mic=True):
    import agent_voice.__main__ as m
    monkeypatch.setattr(m, "locked", lambda: False)
    sent, emitted, ears = [], [], []
    clk, oma = _oma(sent, emitted, ears)
    oma.ear, oma.has_model, oma.ac = WakeEar(mic), True, True
    return m, sent, ears, oma


def test_oma_wake_follows_the_daemon_power_and_the_mic(monkeypatch):
    m, sent, ears, oma = _woke_oma(monkeypatch)

    async def go():
        await oma.command({"do": "wake", "on": True})
        assert oma.ear.calls == ["wake"] and ears[-1]["wake"] == "on"
        await oma.event(("ac", True))
        assert oma.ear.calls == ["wake"], "открыт и слушает имя — pw-record не трогаем"
        await oma.event(("ac", False))
        assert oma.ear.calls[-1] == "stop" and ears[-1]["wake"] == "battery"
        await oma.event(("ac", True))
        monkeypatch.setattr(m, "mic_outputs", lambda: [])
        monkeypatch.setattr(m, "muted", lambda t: True)
        await oma.event(("mic_check",))
        assert ears[-1]["wake"] == "muted" and oma.events.get_nowait() == ("mic", True)
        monkeypatch.setattr(m, "muted", lambda t: False)
        await oma.event(("mic_check",))
        assert ears[-1]["wake"] == "on"
        await oma.command({"do": "wake", "on": False})  # тихо, «Не беспокоить», показ экрана, блокировка, настройка
        assert oma.ear.calls[-1] == "stop" and ears[-1]["wake"] == "off"
        oma.has_model = False
        await oma.command({"do": "wake", "on": True})
        assert ears[-1]["wake"] == "no_model" and not oma.ear.open
    run(go())
    m, sent, ears, oma = _woke_oma(monkeypatch, mic=False)
    run(oma.command({"do": "wake", "on": True}))
    assert ears[-1]["wake"] == "no_mic"


def test_wake_open_failure_is_no_model_only_when_vosk_does_not_load(monkeypatch):
    m, sent, ears, oma = _woke_oma(monkeypatch)
    start = oma.ear.start

    async def boom(wake=False):
        oma.ear.calls.append("wake")
        raise OSError("pw-record не запустился")

    def bad():
        raise OSError("модель битая")

    async def go():
        oma.ear.start = boom
        await oma.command({"do": "wake", "on": True})
        assert oma.has_model and ears[-1]["wake"] != "no_model", "сбой слуха — не «нет модели»"
        oma.ear.start = start
        await oma.event(("ac", True))
        assert oma.ear.calls[-1] == "wake" and oma.ear.waking, "повтор на тике ac"
        await oma.ear.stop()
        oma.ear.wake_load = bad
        await oma.event(("ac", True))
        assert oma.has_model is False and ears[-1]["wake"] == "no_model"
    run(go())


def test_oma_conversation_end_goes_back_to_the_name(monkeypatch):
    m, sent, ears, oma = _woke_oma(monkeypatch)

    async def go():
        await oma.command({"do": "wake", "on": True})
        await oma.command({"do": "talk"})
        assert oma.ear.calls == ["wake", "talk"] and not oma.ear.waking
        await oma.command({"do": "talk"})  # клик во время слушания — конец разговора
        assert oma.ear.calls[-1] == "wake" and oma.ear.waking, "слух не закрыт, а снова слушает имя"
    run(go())


def test_mute_while_she_hears_her_name_does_not_close_are_you_there(monkeypatch):
    # mute в покое: слух на имя глохнет (кадры выбрасываются), а окно «Ты тут?» открывается, как без побудки, — на паузе,
    # с перечёркнутым микрофоном; разговор кончился на паузе — следующее окно не отказывает
    m, sent, ears, oma = _woke_oma(monkeypatch)
    monkeypatch.setattr(m, "mic_outputs", lambda: [])
    monkeypatch.setattr(m, "muted", lambda t: True)

    async def go():
        await oma.command({"do": "wake", "on": True})
        await oma.event(("mic_check",))
        await oma.event(oma.events.get_nowait())  # ("mic", True)
        assert oma.ear.paused and not oma.conv.mic and not oma.conv.active
        await oma.command({"do": "listen", "secs": 20, "kind": "here", "said": "Эй, ты тут?"})
        assert oma.conv.active and oma.conv.state == "paused", "окно открыто, но на паузе: микрофон выключен"
        await oma.command({"do": "talk"})  # конец разговора
        assert not oma.conv.active and not oma.conv.mic
        await oma.command({"do": "listen", "secs": 5, "kind": "news", "said": "Новости."})
        assert oma.conv.active, "окно ответа не застревает на прошлой занятости"
    monkeypatch.setattr(m, "locked", lambda: False)
    run(go())


def test_mic_watch_eof_is_an_error_and_closes_the_ear():
    # подписка pactl кончилась сама (перезапустили pipewire-pulse) — без неё mute и чужую запись никто не заметит:
    # слух закрывается, как при смерти pw-record; побудку снова откроет тик ac
    from agent_voice.ear import Ear
    out = asyncio.Queue()
    e = Ear(out, Whisper(load=lambda: "m"), cmd=LIVE, vad_load=lambda: None, mic=lambda: ("", False, ""), muted=lambda t: False,
            watch=["sh", "-c", "exit 0"])

    async def go():
        await e.start()
        proc = e._proc
        assert await asyncio.wait_for(out.get(), 5) == ("mic_check",)
        ev = await asyncio.wait_for(out.get(), 5)
        assert ev[0] == "error" and "MicWatch" in ev[1]
        await asyncio.wait_for(proc.wait(), 5)  # pw-record не пишет без присмотра
        assert not e.open and e._sub is None
    run(go())


def test_wake_ear_death_is_its_own_event():
    # слух на имя умер сам (перезапуск звукового стека) — не "error": Ома пишет его в журнал, без уведомления;
    # смерть слуха в разговоре — "error", как раньше
    from agent_voice.ear import Ear

    async def died(wake, cmd, watch):
        out = asyncio.Queue()
        e = Ear(out, Whisper(load=lambda: "m"), cmd=cmd, vad_load=lambda: None, mic=lambda wake=False, out=None: ("alsa_input.pci", False, ""),
                muted=lambda t: False, wake=LoadDet(), watch=watch)
        await e.start(wake=wake)
        while (ev := await asyncio.wait_for(out.get(), 5)) == ("mic_check",):
            pass
        assert not e.open and not e.waking
        return ev
    assert run(died(True, LIVE, ["sh", "-c", "exit 0"])) == ("wake_died", "MicWatch: pactl subscribe завершился")
    assert run(died(True, ["sh", "-c", "exit 0", "-"], None)) == ("wake_died", "pw-record завершился")
    assert run(died(False, ["sh", "-c", "exit 0", "-"], None)) == ("error", "pw-record завершился")


def test_wake_ear_failure_is_error_in_the_orb_and_its_death_is_no_notification(monkeypatch):
    # побудка разрешена, а слух на имя не открылся или умер сам — орб «Не слышит имя: сбой слуха», а не «Слышит имя»;
    # смерть слуха на имя — в журнал, без уведомления: тик ac откроет его снова; смерть в разговоре — уведомление, как раньше
    m, sent, ears, oma = _woke_oma(monkeypatch)
    notes, start = [], oma.ear.start
    oma.notify = notes.append

    async def boom(wake=False):
        raise OSError("pw-record не запустился")

    async def go():
        oma.ear.start = boom
        await oma.command({"do": "wake", "on": True})
        assert ears[-1]["wake"] == "error" and len(notes) == 1
        oma.ear.start = start
        await oma.event(("ac", True))
        assert ears[-1]["wake"] == "on"
        oma.ear.open = oma.ear.waking = False  # pw-record умер сам: Ear уже закрыт
        await oma.event(("wake_died", "pw-record завершился"))
        assert ears[-1]["wake"] == "error" and len(notes) == 1, "смерть слуха на имя — только в журнал"
        await oma.event(("ac", True))
        assert ears[-1]["wake"] == "on"
        await oma.command({"do": "talk"})
        assert oma.conv.active and ears[-1]["wake"] == "on", "в разговоре слух не на имени — это не сбой"
        oma.ear.open = False
        await oma.event(("error", "pw-record завершился"))
        assert len(notes) == 2 and not oma.conv.active, "смерть слуха в разговоре — уведомление, как раньше"
        assert ears[-1]["wake"] == "on", "конец разговора — снова слышит имя"
    run(go())


def test_ear_close_rechecks_the_mic_as_free():
    # слух закрыт — микрофон для Омы свободен, как раньше по опросу: проверка в очереди, MicWatch уже нет
    from agent_voice.ear import Ear
    out = asyncio.Queue()
    e = Ear(out, Whisper(load=lambda: "m"), cmd=LIVE, vad_load=lambda: None, mic=lambda: ("", False, ""), muted=lambda t: False,
            watch=["sh", "-c", "sleep 60"])

    async def go():
        await e.start()
        assert await asyncio.wait_for(out.get(), 5) == ("mic_check",)
        e.mic_pending = False  # так её снимает Ома
        await e.stop()
        assert out.get_nowait() == ("mic_check",) and e.pid is None
    run(go())


def test_oma_name_alone_listens_with_a_chime_and_name_with_a_question_takes_the_phrase(monkeypatch):
    m, sent, ears, oma = _woke_oma(monkeypatch)

    async def go():
        await oma.command({"do": "wake", "on": True})
        await oma.event(("wake", "Ома.", 40))
        assert oma.conv.active and oma.ear.calls[-1] == ("take", 40)
        assert {"cmd": "chime"} in sent and sent[-1] == {"cmd": "voice_state", "state": "listening", "talking": True}
        await oma.command({"do": "talk"})
        sent.clear()
        await oma.event(("wake", "Ома, что у Jira?", 70))
        assert oma.conv.active and oma.ear.calls[-1] == ("take", 70) and {"cmd": "chime"} not in sent, "с вопросом — без щелчка"
        assert oma.brain.warmed, "claude поднимается, пока она слушает"
    run(go())


def test_oma_name_with_here_answers_are_you_there_without_a_conversation(monkeypatch):
    m, sent, ears, oma = _woke_oma(monkeypatch)

    async def go():
        await oma.command({"do": "wake", "on": True})
        await oma.event(("wake", "Ома, я тут.", 40))
        assert not oma.conv.active and sent[-2:] == [{"cmd": "presence", "state": "here"}, {"cmd": "chime"}]
        assert ("take", 40) not in oma.ear.calls and oma.ear.waking, "мозг не зовётся, слух слушает имя дальше"
        monkeypatch.setattr(m, "locked", lambda: True)
        n = len(sent)
        await oma.event(("wake", "Ома, что нового?", 50))
        assert not oma.conv.active and len(sent) == n, "экран заблокирован — побудка не открывает разговор"
        monkeypatch.setattr(m, "locked", lambda: False)
        await oma.command({"do": "wake", "on": False})
        await oma.event(("wake", "Ома, что нового?", 60))
        assert not oma.conv.active, "пока думала вторая ступень, побудку запретили — забыть"
    run(go())


def test_conversation_wake_and_name_stripped_from_phrases():
    clk = Clock()
    c = Conversation(clock=clk)
    assert c.wake("тут") == [("presence", "here"), ("chime",)] and not c.active
    assert c.wake("") == [("chime",), ("state", "listening")] and c.active
    assert c.wake("") == [], "разговор уже идёт"
    c.speech_start(1)
    c.phrase("Ома, что у Jira?", 1)
    assert c.draft == "что у Jira?"
    c = Conversation(clock=clk)
    c.set_quiet(True)
    assert c.wake("") == [], "тихо — побудки нет"
    # окно «Ты тут?»: «Ома, тут» — это «тут»: окно закрыто, «здесь», мозг не зовётся (раньше «ома» уходила мозгу)
    c = Conversation(clock=clk)
    c.listen(20, "here", "Эй, ты тут?")
    c.speech_start(1)
    c.phrase("Ома, тут.", 1)
    clk.t += 2
    acts = c.tick()
    assert ("presence", "here") in acts and not c.active and not any(a[0] == "send" for a in acts)


def test_phrase_after_a_wake_keeps_its_sound_for_the_whole_utterance(monkeypatch):
    # черновик — без имени («что у Jira?»), а слух прислал «Ома, что у Jira?»: звук фразы — в реплику, а не None,
    # как у срезанной эхом, — иначе реплику целиком не распознать и Smart Turn её не слышит
    m, sent, ears, oma = _woke_oma(monkeypatch)
    oma.ear.audio = {1: b"pcm"}

    async def go():
        await oma.command({"do": "wake", "on": True})
        await oma.event(("wake", "Ома, что у Jira?", 70))
        await oma.event(("start", 1))
        await oma.event(("phrase", "Ома, что у Jira?", 1))
        assert oma.conv.draft == "что у Jira?" and oma.utter == [b"pcm"]
    run(go())


def test_name_listening_follows_presence_after_are_you_there_in_the_speakers(monkeypatch):
    # «Ты тут?» в динамики слушает ноутбуком (speakers True); слух на имя после него — снова по сервису присутствия:
    # иначе в наушниках он брал бы mic_aec и выбрасывал кадры, пока она говорит («Ома, стоп» не слышен)
    m, sent, ears, oma = _woke_oma(monkeypatch)
    monkeypatch.setattr(m, "mic_outputs", lambda: [])

    async def go():
        await oma.command({"do": "wake", "on": True})
        await oma.command({"do": "listen", "secs": 20, "kind": "here", "said": "Эй, ты тут?", "speakers": True})
        assert oma.ear.speakers is True
        oma.conv.clock.t += 21
        await oma.event(("tick",))  # окно кончилось тишиной — слух снова на имя
        assert not oma.conv.active and oma.ear.calls[-1] == "wake" and oma.ear.speakers is None
    run(go())


def test_whole_utterance_from_a_wake_drops_the_name(monkeypatch):
    m, sent, ears, oma = _woke_oma(monkeypatch)
    oma.whisper = SimpleNamespace(ready=True, get=lambda: _value("m"))
    monkeypatch.setattr("agent_voice.ear.transcribe", lambda model, pcm, hotwords=None: "Ома, скажи агенту, чтобы запустил тесты")
    got = run(oma._whole("скажи агенту, чтобы", [b"a", b"b"], None))
    assert got == "скажи агенту, чтобы запустил тесты"


async def _value(v):
    return v
