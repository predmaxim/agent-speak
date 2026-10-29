#!/bin/bash
# Сборка и установка agent-speak: бинарник, сервер синтеза, стартовый словарь, служба systemd.
set -euo pipefail
cd "$(dirname "$0")"
command -v cargo >/dev/null || { echo "нужен cargo (rustup или pacman -S rust)" >&2; exit 1; }
DATA=~/.local/share/agent-speak

cargo build --release
mkdir -p ~/.local/bin "$DATA" ~/.config/systemd/user
ln -sfn "$PWD/target/release/agent-speak" ~/.local/bin/agent-speak
ln -sfn "$PWD/python/silero_tts.py" "$DATA/silero_tts.py"
[[ -f $DATA/terms.tsv ]] || cp data/terms.tsv "$DATA/terms.tsv"
# копия, не ссылка: `systemctl disable` (agent-speak off) снимает ссылку на юнит вместе с автозапуском
install -m644 systemd/agent-speakd.service ~/.config/systemd/user/agent-speakd.service
# Плагин Omarchy — панель Омы (орб и окно); включается: omarchy plugin enable predmaxim.agent-speak
mkdir -p ~/.config/omarchy/plugins
ln -sfn "$PWD/plugin" ~/.config/omarchy/plugins/predmaxim.agent-speak
# «Ома» в меню приложений: включает службу и открывает окно
mkdir -p ~/.local/share/applications
sed -e "s|@BIN@|$HOME/.local/bin/agent-speak|" -e "s|@ICON@|$PWD/data/oma.svg|" data/oma.desktop >~/.local/share/applications/oma.desktop

# Silero: venv и модель, если ещё нет
if ! "$DATA/venv/bin/python" -c 'import torch, numpy, scipy' 2>/dev/null; then
  python3 -m venv "$DATA/venv"
  "$DATA/venv/bin/pip" install -q torch numpy scipy --index-url https://download.pytorch.org/whl/cpu
fi
[[ -f $DATA/v5_ru.pt ]] || {
  curl -fsSL -o "$DATA/v5_ru.pt.tmp" https://models.silero.ai/models/tts/ru/v5_ru.pt && mv "$DATA/v5_ru.pt.tmp" "$DATA/v5_ru.pt"
}

# Голосовой разговор: отдельный venv на Python 3.12 (колёса faster-whisper/ctranslate2), whisper на GPU
ln -sfn "$PWD/python/agent_voice" "$DATA/agent_voice"
if ! "$DATA/voice-venv/bin/python" -c 'import faster_whisper, silero_vad, claude_agent_sdk' 2>/dev/null; then
  uv venv -q --python 3.12 "$DATA/voice-venv"
  uv pip install -q --python "$DATA/voice-venv/bin/python" torch --index-url https://download.pytorch.org/whl/cpu
  uv pip install -q --python "$DATA/voice-venv/bin/python" faster-whisper silero-vad claude-agent-sdk \
    nvidia-cublas-cu12 'nvidia-cudnn-cu12==9.*' pytest
fi

# Конец реплики по звуку: Smart Turn v3.2 (pipecat-ai/smart-turn, BSD-2), 8 МБ; нет файла — Ома ждёт паузу, как раньше
ST=$DATA/smart-turn-v3.2-cpu.onnx
[[ -f $ST ]] || {
  curl -fsSL -o "$ST.tmp" https://huggingface.co/pipecat-ai/smart-turn-v3/resolve/main/smart-turn-v3.2-cpu.onnx &&
    echo "2bb026316b14a660486a75b1733cd3fbab8c2fd0314dc9af7be49f8cca967e4f  $ST.tmp" | sha256sum -c --quiet && mv "$ST.tmp" "$ST"
} || { rm -f "$ST.tmp"; echo "Smart Turn не скачался — Ома будет ждать паузу" >&2; }

systemctl --user daemon-reload
systemctl --user enable --now agent-speakd.service
systemctl --user restart agent-speakd.service
