# agent-speak

Озвучка ИИ-агентов (Claude Code, Codex) по ходу работы: промежуточные статусы звучат сразу, по-русски, голосом Silero.

- Сервис `agent-speakd` (`systemctl --user status agent-speakd`), журнал — `journalctl --user -u agent-speakd`.
- Команды: `agent-speak read | stop | pause | mode`; хоткеи — в `omarchy-dotfiles` (`hyprland.lua`).
- Хуки: `~/.claude/settings.json` (Stop, UserPromptSubmit, Notification, MessageDisplay), `notify` в `~/.codex/config.toml`.
- Настройки: `~/.config/agent-speak/config.toml` (`mode`, `speaker`, `rate`, `max_age_secs`, `read_intermediate`); сервис перечитывает файл сам.
- Плагин панели Omarchy `predmaxim.agent-speak` (`plugin/`, ставится ссылкой `install.sh`): значок в центральной группе индикаторов (левый клик — окно настроек, правый — пауза/продолжение), окно меняет настройки командой `set` в сокет сервиса. Тесты: `node plugin/test.js`.
- Словарь произношений: `~/.local/share/agent-speak/terms.tsv` (`слово<TAB>произношение`, `+` — ударение); незнакомые слова дописывает фоновая модель.
- Установка: `./install.sh`. Тесты: `cargo test`, с моделью — `cargo test -- --ignored` и `python/test_silero_tts.py`.
- Проект: `docs/superpowers/specs/2026-10-02-agent-speak-design.md`.
