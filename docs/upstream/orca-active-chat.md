# Issue в Orca: какой чат сейчас на экране

Готовый текст issue для `stablyai/orca`. Публикует автор, не агент. Зачем это Оме: новости агента, на чей чат пользователь смотрит, не звучат голосом. У терминалов Orca это решено, а у чатов документированного способа нет. Поэтому сейчас, пока окно Orca в фокусе, «заняты» все её чаты: их новости идут только пузырями (README, «Агенты в чатах Orca»).

Проверено 2026-11-12 на Orca 1.4.256 (`orca --version`):
- `orca chat --help` перечисляет одну команду: `rename`.
- `orca terminal show` и `terminal switch` принимают только `--terminal <handle>`. В гайде `orca skills get orca-cli` сказано: «A chat tab has no terminal handle».
- `orca agent-context --json` (326 команд) не содержит команды, которая назвала бы видимый чат.

После того как Orca даст такой способ, agent-speak считает «занятым» только видимый чат, а не все чаты окна Orca.

---

## Title

CLI: report which chat tab is currently visible (`orca chat current`)

## Body

External tools such as voice assistants and notifiers need to know what the user is looking at in Orca. Then they stay quiet about an agent whose chat is already on screen.

For terminal tabs this is covered: `orca terminal show` / `terminal list` and the terminal handle identify a terminal. Chat tabs (structured agent sessions) have no terminal handle. `orca chat` has only `rename`. As of Orca 1.4.256, no documented command or event tells which chat is visible.

Request: a documented way to read the visible chat, for example:

- `orca chat current --json` → `{ "visible": true, "worktree": "<id>", "sessionId": "<agent session id>" }` for the chat in the focused pane of the active workspace, or `null` when no chat is shown. The `sessionId` would be the same id that `agent launch` prints and `chat rename --session` takes.
- or a subscribable event when the visible tab changes.

It also helps if a chat agent process can tell its own session id (the one `chat rename --session` takes) through a documented environment variable. Then a tool can match the visible chat to a running process.
