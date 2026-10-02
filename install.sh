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

# Silero: venv и модель, если ещё нет
if ! "$DATA/venv/bin/python" -c 'import torch, numpy, scipy' 2>/dev/null; then
  python3 -m venv "$DATA/venv"
  "$DATA/venv/bin/pip" install -q torch numpy scipy --index-url https://download.pytorch.org/whl/cpu
fi
[[ -f $DATA/v5_ru.pt ]] || {
  curl -fsSL -o "$DATA/v5_ru.pt.tmp" https://models.silero.ai/models/tts/ru/v5_ru.pt && mv "$DATA/v5_ru.pt.tmp" "$DATA/v5_ru.pt"
}

systemctl --user daemon-reload
systemctl --user enable --now agent-speakd.service
systemctl --user restart agent-speakd.service
