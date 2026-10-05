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
ln -sfn "$PWD/systemd/agent-speakd.service" ~/.config/systemd/user/agent-speakd.service
# Плагин панели Omarchy; значок в группе индикаторов копирует хук patch_indicators (omarchy-dotfiles)
mkdir -p ~/.config/omarchy/plugins
ln -sfn "$PWD/plugin" ~/.config/omarchy/plugins/predmaxim.agent-speak

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

systemctl --user daemon-reload
systemctl --user enable --now agent-speakd.service
systemctl --user restart agent-speakd.service
