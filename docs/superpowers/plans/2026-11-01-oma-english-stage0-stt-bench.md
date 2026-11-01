# Учебный режим Омы, этап 0: проверка распознавания — план

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** выбрать модель распознавания для учебного режима: прогнать фразы
пользователя с нарочными ошибками через кандидатов и показать, кто сохраняет
ошибки дословно.

**Architecture:** отдельный набор `python/stt_bench/`: список фраз (TSV),
чистые функции метрик и отчёта (`core.py`), запись голоса через `pw-record`
(`record.py`), прогон кандидатов, каждый в своём процессе, и отчёт в
`docs/stt-bench.md` (`run.py`). Свой venv, Ома и её `voice-venv` не
трогаются.

**Tech Stack:** Python 3.12, faster-whisper (large-v3-turbo, CUDA),
onnx-asr (NVIDIA Parakeet TDT 0.6B v3, onnxruntime-gpu), PipeWire
`pw-record`, pytest.

**Spec:** `docs/superpowers/specs/2026-11-01-oma-english-training-design.md`
(раздел «Этапы», пункт 0; «Распознавание в режиме»).

Это первый из нескольких планов. Планы остального (режим, цикл исправления,
английский голос, банк фраз, прогресс) пишутся после этого этапа: от выбранной
модели зависит слух.

## Global Constraints

- Пороги: сохранено **не ниже 90 %** нарочных ошибок; ложных ошибок **не
  больше 5 %** слов.
- Русские слова в смешанных фразах должны быть записаны по-русски, не
  переведены. Это отдельная метрика, без порога.
- **Цифры показать пользователю до этапа 1 при любом исходе.** Этап 1 без
  его решения не начинать.
- Ома и `~/.local/share/agent-speak/voice-venv` не меняются. Отдельный venv:
  `~/.local/share/agent-speak/bench-venv` (Python 3.12).
- Записи голоса лежат вне репозитория: `~/.local/share/agent-speak/stt-bench/rec/<id>.wav`.
  Результаты прогонов там же, в `results/<кандидат>.json`.
- Службы PipeWire и WirePlumber не перезапускать.
- Комментарии, сообщения CLI и документация пишутся по-русски, как в
  репозитории. Коммиты без ИИ-соавторов. README обновляется вместе с кодом.
  Коммит и push в `main`.
- Тесты: `~/.local/share/agent-speak/voice-venv/bin/python -m pytest python/test_stt_bench.py`.
  `core.py` пишется только на стандартной библиотеке, чтобы тесты шли в любом
  venv.

## Review Focus

1. **Кривые апострофы и кавычки** (`’`, `«»`) у моделей и в TSV дают ложные
   ошибки. `normalize` приводит `’` к `'` и убирает кавычки (Task 1, тест
   `test_normalize_quotes_and_case`).
2. **Записан не тот микрофон** (гарнитура, тишина), и все фразы пустые. После
   каждой записи `record.py` печатает пик в dBFS и предупреждает, если он ниже
   −40 (Task 2, тест `test_peak_dbfs`).
3. **Обрезано первое слово:** пользователь начинает говорить сразу после Enter,
   а `pw-record` отдаёт звук через ~50 мс. «Говори» печатается только через
   0,3 с после запуска процесса (Task 2, ручная проверка в шаге 6).
4. **В VRAM попадают чужие процессы:** Ома уже держит whisper на видеокарте.
   Считаем только память своего PID (Task 3, тест `test_parse_compute_apps`).
5. **Модель переводит или транслитерирует русские слова** в смешанных фразах.
   Это ловит метрика `cyrillic_pct` (Task 1, тест `test_summarize`).

---

### Task 1: фразы и метрики

**Files:**
- Create: `python/stt_bench/__init__.py` (пустой)
- Create: `python/stt_bench/phrases.tsv`
- Create: `python/stt_bench/core.py`
- Test: `python/test_stt_bench.py`

**Interfaces:**
- Produces:
  - `Phrase` — dataclass `(id: str, kind: str, text: str, must: list[str])`.
    `kind` — одно из `error` / `ru` / `mixed`.
  - `PHRASES: Path` — путь к `phrases.tsv`.
  - `load_phrases(path: Path = PHRASES) -> list[Phrase]`.
  - `normalize(text: str) -> list[str]`.
  - `preserved(must: list[str], hyp: str) -> list[bool]`.
  - `false_errors(ref: str, hyp: str, must: list[str]) -> tuple[int, int]`.
  - `summarize(phrases: list[Phrase], hyps: dict[str, str]) -> dict` с ключами
    `preserved_pct`, `false_error_pct`, `cyrillic_pct`, `ru_error_pct`
    (float, 0–100).
  - `THRESHOLDS = (90.0, 5.0)`.

- [ ] **Step 1: Создать `phrases.tsv`**

Столбцы через TAB: `id  kind  text  must`. `must` — n-граммы через `|`, которые
должны стоять подряд в нормализованном распознанном тексте. Для `ru` столбец
пустой. Цифр в фразах нет: модели пишут числа по-разному. Содержимое ровно
такое:

```
a01	error	Please fix bug in the login page.	fix bug in
a02	error	I need the new branch for this feature.	need the new branch
a03	error	Can you write test for this function?	write test for
a04	error	Agent should check a README first.	check a readme
s01	error	The build fail on every commit.	build fail on
s02	error	He don't know why the server is down.	he don't know
s03	error	This function return the wrong value.	function return the
s04	error	Our app work fine on my laptop.	app work fine
t01	error	I am work on this task since Monday.	i am work on
t02	error	Yesterday I have pushed the changes.	yesterday i have pushed
t03	error	I already fix this bug last week.	i already fix this
t04	error	When the agent will finish, tell me.	when the agent will finish
p01	error	It depends from the config file.	depends from the
p02	error	Let's have a call in Monday.	call in monday
p03	error	Please explain me how this works.	explain me how
w01	error	Can you tell me where is the log file?	where is the log file
w02	error	I don't understand what does this error mean.	what does this error mean
w03	error	Why you didn't run the tests?	why you didn't run
n01	error	All test is failing after the update.	all test is failing
n02	error	We have many problem with deploy.	many problem with deploy
r01	ru	Как сказать по-английски «развернуть приложение на сервере»?	
r02	ru	Как пишется слово environment?	
r03	ru	Почему здесь нужен артикль the?	
r04	ru	Как правильно сказать, что я жду ответа от агента?	
r05	ru	Повтори последнюю фразу по-русски.	
m01	mixed	I want you to развернуть the branch on staging.	развернуть
m02	mixed	Please check why the тесты are failing.	тесты
m03	mixed	Can you сделать a backup before the migration?	сделать
m04	mixed	The agent is stuck, нужно перезапустить it.	нужно перезапустить
m05	mixed	Tell me what is the причина of this error.	причина
```

- [ ] **Step 2: Написать падающие тесты**

```python
# python -m pytest python/test_stt_bench.py
import sys, os
sys.path.insert(0, os.path.dirname(__file__))
from stt_bench.core import normalize, preserved, false_errors, summarize, load_phrases, Phrase

def test_normalize_quotes_and_case():
    assert normalize("He don’t know, «why»!") == ["he", "don't", "know", "why"]

def test_normalize_russian_hyphen_yo():
    assert normalize("Как сказать по-английски? Ещё") == ["как", "сказать", "по", "английски", "еще"]

def test_preserved():
    assert preserved(["he don't know"], "He doesn't know why.") == [False]
    assert preserved(["he don't know"], "he don't know why") == [True]
    assert preserved(["fix bug in"], "Please fix a bug in the login page.") == [False]

def test_false_errors_counts_only_outside_must():
    assert false_errors("Please fix bug in the login page.", "Please fix bug in the logging page.", ["fix bug in"]) == (1, 4)
    assert false_errors("He don't know why the server is down.", "He doesn't know why the server is down.", ["he don't know"]) == (0, 5)
    assert false_errors("The build fail on every commit.", "The build fail on every single commit.", ["build fail on"]) == (1, 3)

def test_summarize():
    ph = [Phrase("s02", "error", "He don't know why.", ["he don't know"]),
          Phrase("m02", "mixed", "Check the тесты now.", ["тесты"]),
          Phrase("r05", "ru", "Повтори фразу.", [])]
    s = summarize(ph, {"s02": "He doesn't know why.", "m02": "Check the tests now.", "r05": "Повтори фразу."})
    assert s["preserved_pct"] == 0.0
    assert s["false_error_pct"] == 0.0
    assert s["cyrillic_pct"] == 0.0
    assert s["ru_error_pct"] == 0.0

def test_phrases_file_is_consistent():
    ph = load_phrases()
    kinds = [p.kind for p in ph]
    assert (kinds.count("error"), kinds.count("ru"), kinds.count("mixed")) == (20, 5, 5)
    assert len({p.id for p in ph}) == 30
    for p in ph:
        assert all(preserved([m], p.text) == [True] for m in p.must), p.id
        if p.kind == "mixed":
            assert all(any("а" <= c <= "я" for c in m) for m in p.must), p.id
```

- [ ] **Step 3: Убедиться, что тесты падают**

Run: `~/.local/share/agent-speak/voice-venv/bin/python -m pytest python/test_stt_bench.py -v`
Expected: FAIL, `ModuleNotFoundError: No module named 'stt_bench'`.

- [ ] **Step 4: Реализовать `core.py`**

- `normalize`: нижний регистр; `’` и `‘` → `'`; `ё` → `е`; дефис → пробел;
  всё, кроме букв, цифр и апострофа внутри слова, → пробел; деление по
  пробелам.
- `preserved`: n-грамма `must` после `normalize` стоит подряд в `normalize(hyp)`.
- `false_errors`: позиции слов `ref`, входящие в первое вхождение каждой
  n-граммы `must`, исключаются. Выравнивание —
  `difflib.SequenceMatcher(None, ref_tokens, hyp_tokens, autojunk=False).get_opcodes()`.
  Ошибки: слова `ref` вне исключённых позиций в `replace`/`delete` плюс слова
  `hyp` в `insert`, если вставка стоит не внутри исключённого отрезка.
  Возврат: `(ошибки, число слов ref вне исключённых)`.
- `summarize`:
  - `preserved_pct` — доля сохранённых n-грамм по `error`;
  - `false_error_pct` — сумма ошибок / сумма слов по `error`;
  - `cyrillic_pct` — доля сохранённых n-грамм по `mixed`;
  - `ru_error_pct` — `false_errors(text, hyp, [])` по `ru`.

  Если фразы нет в `hyps`, она считается распознанной как пустая строка.

- [ ] **Step 5: Тесты проходят**

Run: `~/.local/share/agent-speak/voice-venv/bin/python -m pytest python/test_stt_bench.py -v`
Expected: 6 passed.

- [ ] **Step 6: Commit**

```bash
git add python/stt_bench/__init__.py python/stt_bench/phrases.tsv python/stt_bench/core.py python/test_stt_bench.py
git commit -m "Проверка распознавания: фразы с нарочными ошибками и метрики"
```

### Task 2: запись фраз

**Files:**
- Create: `python/stt_bench/record.py`
- Test: `python/test_stt_bench.py` (дописать)

**Interfaces:**
- Consumes: `load_phrases`, `Phrase` (Task 1).
- Produces:
  - `REC_DIR = Path.home() / ".local/share/agent-speak/stt-bench/rec"`;
  - `pick_raw_mic(sources: str) -> str` — первое имя `alsa_input.*` из вывода
    `pactl list short sources`, иначе `""`;
  - `pending(ids: list[str], rec_dir: Path) -> list[str]` — id без файла
    `<id>.wav`, в исходном порядке;
  - `peak_dbfs(pcm: bytes) -> float` — пик s16 в dBFS относительно 32767, тишина → `-inf`.

- [ ] **Step 1: Падающие тесты**

```python
from stt_bench.record import pick_raw_mic, pending, peak_dbfs
import array, math

def test_pick_raw_mic():
    src = "42\tmic_aec\tPipeWire\n1849\talsa_input.pci-0000_00_1f.3.analog-stereo\tPipeWire\n"
    assert pick_raw_mic(src) == "alsa_input.pci-0000_00_1f.3.analog-stereo"
    assert pick_raw_mic("42\tmic_aec\tPipeWire\n") == ""

def test_pending(tmp_path):
    (tmp_path / "a02.wav").write_bytes(b"x")
    assert pending(["a01", "a02", "a03"], tmp_path) == ["a01", "a03"]

def test_peak_dbfs():
    assert peak_dbfs(array.array("h", [0] * 100).tobytes()) == -math.inf
    assert peak_dbfs(array.array("h", [0, 32767]).tobytes()) == 0.0
    assert round(peak_dbfs(array.array("h", [0, 3277]).tobytes())) == -20
```

- [ ] **Step 2: Тесты падают**

Run: `~/.local/share/agent-speak/voice-venv/bin/python -m pytest python/test_stt_bench.py -v`
Expected: FAIL, `ModuleNotFoundError: No module named 'stt_bench.record'`.

- [ ] **Step 3: Реализовать функции и CLI `python -m stt_bench.record [--mic ИМЯ]`**

Запускать из каталога `python/`. CLI работает так:
1. Идёт по `pending(...)`, показывает `[n/30] id: текст`.
2. Enter запускает `pw-record --target <mic> --rate 16000 --channels 1 --format s16 <REC_DIR>/<id>.wav`.
   Микрофон по умолчанию — `pick_raw_mic`, как у Омы на ноутбуке.
3. Через 0,3 с после запуска печатает «● Говори… Enter — стоп».
4. Enter останавливает запись через SIGINT, чтобы `pw-record` дописал
   заголовок WAV.
5. Печатает пик (пик берётся из WAV через модуль `wave`). Если пик ниже
   −40 dBFS, предупреждает: «Тихо — проверь микрофон».
6. Дальше выбор: Enter — следующая фраза, `r` — перезаписать, `p` — прослушать
   (`pw-play`), `q` — выйти.

Повторный запуск продолжает с первой незаписанной фразы. Только стандартная
библиотека.

- [ ] **Step 4: Тесты проходят**

Run: `~/.local/share/agent-speak/voice-venv/bin/python -m pytest python/test_stt_bench.py -v`
Expected: 9 passed.

- [ ] **Step 5: Проверка записи без голоса**

Run: `cd python && timeout -s INT 2 pw-record --target "$(pactl list short sources | awk '$2 ~ /^alsa_input\./{print $2; exit}')" --rate 16000 --channels 1 --format s16 /tmp/stt-check.wav; python3 -c "import wave; w=wave.open('/tmp/stt-check.wav'); print(w.getframerate(), w.getnchannels(), w.getnframes() > 16000)"`
Expected: `16000 1 True`.

- [ ] **Step 6: Commit**

```bash
git add python/stt_bench/record.py python/test_stt_bench.py
git commit -m "Проверка распознавания: запись фраз через pw-record"
```

### Task 3: прогон кандидатов и отчёт

**Files:**
- Create: `python/stt_bench/run.py`
- Modify: `python/stt_bench/core.py` (дописать `report`)
- Modify: `README.md` (раздел «Установка и тесты»: одна строка про
  проверку распознавания)
- Test: `python/test_stt_bench.py` (дописать)

**Interfaces:**
- Consumes: `load_phrases`, `summarize`, `THRESHOLDS` (Task 1); `REC_DIR`
  (Task 2).
- Produces:
  - `RESULTS_DIR = REC_DIR.parent / "results"`;
  - JSON результата `{"name": str, "hyps": {id: str}, "latency_s": float, "vram_mb": int, "timestamps": bool}`;
  - `core.report(results: list[dict], phrases: list[Phrase]) -> str` — markdown;
  - `run.parse_compute_apps(text: str, pid: int) -> int` — МБ своего PID из
    `nvidia-smi --query-compute-apps=pid,used_memory --format=csv,noheader,nounits`,
    нет строки → `0`.

- [ ] **Step 1: Падающие тесты**

```python
from stt_bench.core import report
from stt_bench.run import parse_compute_apps

def test_parse_compute_apps():
    assert parse_compute_apps("1234, 1650\n999, 2100\n", 999) == 2100
    assert parse_compute_apps("1234, 1650\n", 5) == 0

def test_report_marks_pass_and_fail():
    ph = [Phrase("s02", "error", "He don't know why.", ["he don't know"])]
    good = {"name": "good", "hyps": {"s02": "He don't know why."}, "latency_s": 0.4, "vram_mb": 2000, "timestamps": True}
    bad = {"name": "bad", "hyps": {"s02": "He doesn't know why."}, "latency_s": 0.3, "vram_mb": 1600, "timestamps": True}
    md = report([good, bad], ph)
    assert "| good | 100 | 0 |" in md and "| bad | 0 | 0 |" in md
    assert md.count("✅") == 1 and md.count("❌") >= 1
    assert "He doesn't know why." in md
```

- [ ] **Step 2: Тесты падают**

Run: `~/.local/share/agent-speak/voice-venv/bin/python -m pytest python/test_stt_bench.py -v`
Expected: FAIL, `ImportError: cannot import name 'report'`.

- [ ] **Step 3: Реализовать `report`**

Сводная таблица — колонки:

`| кандидат | ошибки сохранены % | ложные ошибки % | русские слова % | русские фразы, ошибки % | задержка, с | VRAM, МБ | таймкоды | итог |`

Проценты округляются до целых. Итог: `✅`, если сохранено ≥ `THRESHOLDS[0]` и
ложных ≤ `THRESHOLDS[1]`, иначе `❌`.

Ниже — таблица по фразам: `id | эталон | текст каждого кандидата`. Строку `error`
или `mixed`, где n-грамма потеряна, помечать `❌`.

- [ ] **Step 4: Создать bench-venv**

```bash
uv venv -q --python 3.12 ~/.local/share/agent-speak/bench-venv
uv pip install -q --python ~/.local/share/agent-speak/bench-venv/bin/python \
  faster-whisper 'onnx-asr[gpu,hub]' 'onnxruntime-gpu[cuda,cudnn]' nvidia-cublas-cu12 'nvidia-cudnn-cu12==9.*' pytest
```

Expected: `~/.local/share/agent-speak/bench-venv/bin/python -c "import faster_whisper, onnx_asr"`
выполняется без ошибок.

- [ ] **Step 5: Реализовать `run.py`**

Режимы CLI. Запуск — `~/.local/share/agent-speak/bench-venv/bin/python -m stt_bench.run`
из каталога `python/`.
- **Без аргументов.** Для каждого кандидата запускает себя же
  `--candidate ИМЯ` в отдельном процессе: так VRAM освобождается и считается
  отдельно. Потом выполняет `--report`.
- **`--candidate ИМЯ`.** Загружает модель и прогревает её на первой записи.
  Затем распознаёт все `REC_DIR/*.wav` (16 кГц, s16 → float32). Задержка —
  среднее время на фразу без прогрева. VRAM — максимум
  `parse_compute_apps(..., os.getpid())` после каждой фразы. Результат пишется
  в `RESULTS_DIR/ИМЯ.json`.
- **`--report`.** Пишет `report(...)` в `docs/stt-bench.md`. Над таблицей
  шапка: дата, число записей, свободная VRAM до прогона (`nvidia-smi`).

Перед загрузкой faster-whisper нужен `LD_LIBRARY_PATH` на
`site-packages/nvidia/*/lib` с перезапуском процесса через `os.execv`. Код
скопировать из `cuda_env()` в `python/agent_voice/__main__.py`. Кандидаты —
словарь имя → загрузчик, загрузчик возвращает
`(transcribe(audio: np.ndarray) -> str, timestamps: bool)`:

| имя | модель и параметры |
|---|---|
| `whisper-en` | `WhisperModel("large-v3-turbo", device="cuda", compute_type="float16")`, `language="en"`, `beam_size=1`, `condition_on_previous_text=False`, `word_timestamps=True` |
| `whisper-auto` | то же, `language=None` |
| `whisper-verbatim` | то же, `language=None`, `initial_prompt="Umm, so, like, he don't know, I- I think we should, uh, check it."` |
| `parakeet-v3` | `onnx_asr.load_model("nemo-parakeet-tdt-0.6b-v3", providers=["CUDAExecutionProvider"])`, текст и таймкоды через `.with_timestamps()` |

Если `parakeet-v3` не грузится на GPU, загрузить его с
`providers=["CPUExecutionProvider"]` и пометить в JSON `"name": "parakeet-v3-cpu"`.
Пятого кандидата добавить, только если `onnx_asr` из коробки грузит
многоязычную модель NeMo с русским, например Canary. Иначе пропустить.

- [ ] **Step 6: Тесты проходят, конвейер работает на тишине**

Run: `~/.local/share/agent-speak/voice-venv/bin/python -m pytest python/test_stt_bench.py -v`
Expected: 11 passed.

Run: `mkdir -p /tmp/stt-smoke && python3 -c "import wave; w=wave.open('/tmp/stt-smoke/x.wav','wb'); w.setnchannels(1); w.setsampwidth(2); w.setframerate(16000); w.writeframes(b'\0\0'*16000)"`, затем прогон
`--candidate` для `whisper-en` и `parakeet-v3` с подменой каталога записей
через переменную окружения `STT_BENCH_REC=/tmp/stt-smoke`. `run.py` читает её
вместо `REC_DIR`, если она задана.
Expected: оба JSON созданы; у `whisper-en` `vram_mb > 0`; у Parakeet `vram_mb > 0` или имя `parakeet-v3-cpu`.

- [ ] **Step 7: README и commit**

В README, раздел «Установка и тесты», добавить строку: проверка распознавания
для учебного режима — `python/stt_bench/`, запись `python -m stt_bench.record`,
прогон `bench-venv … -m stt_bench.run`, отчёт `docs/stt-bench.md`.

```bash
git add python/stt_bench/run.py python/stt_bench/core.py python/test_stt_bench.py README.md
git commit -m "Проверка распознавания: прогон whisper и Parakeet, отчёт в docs/stt-bench.md"
git push
```

### Task 4: запись пользователя, прогон, решение (ручная)

**Files:**
- Create: `docs/stt-bench.md` (сгенерированный отчёт + раздел «Вывод» руками)

**Interfaces:**
- Consumes: `record.py`, `run.py` (Tasks 2–3).

- [ ] **Step 1: Дождаться, когда пользователь сможет говорить вслух**

Сейчас у него тихий режим. Дай ему команду
`cd ~/Projects/omarchy/own/agent-speak/python && ~/.local/share/agent-speak/voice-venv/bin/python -m stt_bench.record`
и попроси читать фразы как есть, с ошибками, обычным темпом.

- [ ] **Step 2: Прогон**

Run: `cd python && ~/.local/share/agent-speak/bench-venv/bin/python -m stt_bench.run`
Expected: `docs/stt-bench.md` с четырьмя (или пятью) строками в сводке.

- [ ] **Step 3: Раздел «Вывод» в `docs/stt-bench.md`**

Три-пять предложений:
- кто прошёл пороги и почему;
- как модели обошлись с русскими словами;
- сколько VRAM остаётся под Kokoro (~0,5–1 ГБ) из 8 ГБ рядом с тем, что уже
  держит Ома;
- рекомендация.

- [ ] **Step 4: Commit и push**

```bash
git add docs/stt-bench.md
git commit -m "Проверка распознавания: результаты на голосе пользователя"
git push
```

- [ ] **Step 5: Показать цифры пользователю и остановиться**

Сводная таблица и рекомендация — в чат. Следующий план (этап 1) пишется
только после его решения.
