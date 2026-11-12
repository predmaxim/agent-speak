# PR в Omarchy: сигнал `dndChanged` у цели `notifications`

Готовый текст PR для `basecamp/omarchy`. Публикует автор, не агент. Зачем это Оме: «Не беспокоить» должен давать ей тишину (пункт 28 дизайна `docs/superpowers/specs/2026-11-12-oma-jarvis-wave-design.md`). Сейчас подписки на смену DND нет. Есть только запрос `omarchy-shell notifications isDnd`, а опрашивать его — костыль (признак 3). Файл `quiet.json` — хранилище службы, а не контракт (признак 4). Поэтому agent-speak DND пока не учитывает: поле `dnd_quiet` строки состояния — всегда `false`, переключателя нет.

Проверено 2026-11-12 на `omarchy 4.5.0-3` (`/usr/share/omarchy/shell`; файл совпадает с клоном `~/Projects/omarchy/omarchy`, ветка `fix-indicators-clone-services`) и `qs` из `quickshell 0.3.2.r7.g2b4501a-1`:
- `doNotDisturb` и `setDoNotDisturb` — `shell/plugins/notifications/Service.qml:53-58`; `ShellIpc` цели `notifications` — `:1396`, `isDnd` — `:1402`; загрузка из `quiet.json` — `applyQuiet`, `:1275-1282`.
- `qs ipc --help` перечисляет `listen` («Listen for IpcHandler signals») и `wait`.
- `IpcRegistry.declaredFunctions` (`shell/Commons/IpcRegistry.qml:56-60`) убирает из вызываемых через `omarchy-shell` имена на `Changed`. Сигнал `dndChanged` поэтому не станет «функцией», которую можно вызвать и тем испустить. Имя без `Changed` (например, `dndToggled`) стало бы вызываемым.
- У живой оболочки цель `notifications` отвечает клон `predmaxim.notifications` (`~/.config/omarchy/plugins/predmaxim.notifications/Service.qml:1468`). Ту же правку получает и он: `isDnd` там на `:1472`.

Не проверял: вывод `qs ipc listen` на сигнал со строковым аргументом (формат строки) — сигналов у целей оболочки сейчас нет (`qs ipc -p /usr/share/omarchy/shell show`). Это первый шаг проверки PR.

Что сделает agent-speak после слияния: служба запускает `qs ipc -p $OMARCHY_PATH/shell listen notifications dndChanged` и один раз при старте зовёт `omarchy-shell notifications isDnd`. Поле `quiet_why` получит значение `dnd`, `dnd_quiet` станет настройкой «Тихо при «Не беспокоить»». Правила тишины по причине и сводка «Пока было тихо» уже есть (README, «Показ экрана»).

---

## Title

notifications: emit `dndChanged` over IPC when Do Not Disturb changes

## Body

Other tools that want to go quiet with Do Not Disturb (a voice assistant, a dictation tool, a status script) can ask `omarchy-shell notifications isDnd`, but nothing tells them when it changes, so the only way to follow it is to poll. `quiet.json` is the service's storage, not an interface, and its path has already moved once.

This adds one IPC signal to the `notifications` target. It fires with `isDnd()`'s value (`on` / `off`) on every change of `doNotDisturb`, from any source: the bell, `toggleDnd` / `setDnd`, `omarchy-toggle-notification-silencing`, meeting mode, and the load from `quiet.json`. A listener runs `qs ipc -p $OMARCHY_PATH/shell listen notifications dndChanged` and calls `isDnd` once at start.

The name ends in `Changed` on purpose: `IpcRegistry.declaredFunctions` leaves such names out, so `omarchy-shell notifications dndChanged` cannot be used to emit it.

```diff
--- a/shell/plugins/notifications/Service.qml
+++ b/shell/plugins/notifications/Service.qml
@@ -51,6 +51,8 @@
   // ------------------------------------------------------------- quiet
   //
   property bool doNotDisturb: false
+  // IPC listeners follow Do Not Disturb without polling isDnd.
+  onDoNotDisturbChanged: ipcHandler.dndChanged(doNotDisturb ? "on" : "off")
   function setDoNotDisturb(value) {
     var on = !!value
     if (on === doNotDisturb) return
@@ -1397,6 +1399,11 @@
     id: ipcHandler
     target: "notifications"
 
+    // Every Do Not Disturb change, with isDnd()'s value. A name ending in
+    // Changed stays out of IpcRegistry's callable functions, so omarchy-shell
+    // cannot emit it.
+    signal dndChanged(state: string)
+
     function ping(): string { return "ok" }
 
     function isDnd(): string { return service.doNotDisturb ? "on" : "off" }
--- a/docs/notifications.md
+++ b/docs/notifications.md
@@ -69,3 +69,3 @@
 
-Do Not Disturb is a boolean in `quiet.json`, toggled via `omarchy-shell notifications toggleDnd` / `setDnd` / `isDnd`; `omarchy-toggle-notification-silencing` wraps the toggle.
+Do Not Disturb is a boolean in `quiet.json`, toggled via `omarchy-shell notifications toggleDnd` / `setDnd` / `isDnd`; `omarchy-toggle-notification-silencing` wraps the toggle. To follow it without polling, listen for the `dndChanged` signal (`on` / `off`): `qs ipc -p $OMARCHY_PATH/shell listen notifications dndChanged`, and call `isDnd` once at start for the current state.
 
```

### Testing

1. `qs ipc -p $OMARCHY_PATH/shell listen notifications dndChanged` in one terminal.
2. `omarchy-shell notifications toggleDnd` twice, then the bell in the bar: each change prints one line with `on` or `off`.
3. `omarchy-shell notifications dndChanged` is refused like an unknown function (no line in the listener).
4. `omarchy restart shell` with DND on: the listener reconnects or exits (note which), and `isDnd` answers `on`.
