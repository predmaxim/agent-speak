# Issue / PR в Omarchy: хук `screenrecord start|stop`

Готовый текст issue или PR для `basecamp/omarchy`. Публикует автор, не агент. Зачем это Оме: при показе экрана она молчит, а пузыри у орба идут без текста (README, «Показ экрана»). Показ через портал она видит по потоку PipeWire. Запись экрана Omarchy по умолчанию идёт мимо портала: gpu-screen-recorder с kms (`omarchy-capture-screenrecording`:305-315) или wf-recorder (wlr-screencopy). Ни потока PipeWire, ни события для плагинов нет: бар узнаёт о записи командой `omarchy-shell -q omarchy.indicators refresh` (:508-510), а состояние отдают только `--status` и pid-файл `$XDG_RUNTIME_DIR/omarchy-screenrecord-pid`. Опрашивать их — костыль (признак 3), поэтому запись Omarchy agent-speak сейчас не покрывает.

Проверено 2026-11-12 на `omarchy 4.5.0-3` (`/usr/share/omarchy/bin/omarchy-capture-screenrecording`, 617 строк; совпадает с клоном `~/Projects/omarchy/omarchy`):
- запись пошла — `toggle_screenrecording_indicator` на :384, после записи pid и имени файла;
- запись остановлена — :421 в `stop_screenrecording`;
- старт отменён, пока запись сохранялась как начатая, — :604;
- `omarchy-hook` (`/usr/share/omarchy/bin/omarchy-hook`) запускает `~/.config/omarchy/hooks/<имя>` и `<имя>.d/*` по очереди, синхронно; при отсутствии хуков ничего не делает.
- Всё тело скрипта идёт под `flock` на fd 9 (`} 9>>"$RUNTIME_DIR/omarchy-screenrecord.lock"`, :617), рекордер запускается с `9>&-` (:358). Хук тоже получает `9>&-`: иначе фоновый процесс из хука держал бы блокировку и следующий запуск записи ждал бы его.
- `pid`-файл и `--status` патч не трогает.
- `bash -n` на изменённом скрипте проходит, `patch --dry-run` на файл 4.5.0-3 — тоже. Вживую не запускал.

Что останется непокрытым и с хуком (записать в README agent-speak, когда хук появится): рекордер, умерший сам, стоп-хука не вызовет; запись, начатая до установки хука; gliff (remote-session), клиенты wlr-screencopy без портала, Sunshine.

Что сделает agent-speak после слияния: хук из `~/.config/omarchy/hooks/screenrecord.d/` шлёт службе факт записи, и она становится второй причиной показа экрана рядом с потоком портала. Способ доставки (команда `agent-speak` или факт presence) выбрать при сборке.

---

## Title

screenrecord: run `omarchy-hook screenrecord start|stop`

## Body

Tools that should go quiet while the screen is recorded (a voice assistant that shows text bubbles, a notification filter) can see screen sharing through the portal's PipeWire stream, but Omarchy's own screen recording uses gpu-screen-recorder's kms capture (or wf-recorder) by default. That leaves no stream and no event: the bar is refreshed by `omarchy-shell -q omarchy.indicators refresh`, and the only way for anything else to know is to poll `omarchy-capture-screenrecording --status` or the pid file.

This runs the existing hook mechanism at the points where the indicator is refreshed:

- `omarchy-hook screenrecord start` once the recorder is running and its pid is saved;
- `omarchy-hook screenrecord stop` once the recording has stopped, and when a start is cancelled while it was being saved as started.

The hook runs with fd 9 closed, like the recorder, so a hook that leaves a background process cannot hold the recording lock. `--status` and the pid file are unchanged. A recorder that dies on its own still runs no stop hook; the comment says so.

```diff
--- a/bin/omarchy-capture-screenrecording
+++ b/bin/omarchy-capture-screenrecording
@@ -24,6 +24,12 @@
 # it is installed. wf-recorder encodes on the CPU and captures one audio
 # device, so desktop and microphone audio together go through a temporary null
 # sink. Portal capture and --resolution are gpu-screen-recorder only.
+#
+# Hook: `omarchy-hook screenrecord start` runs once a recording is running,
+# `omarchy-hook screenrecord stop` once it has stopped or its start was
+# cancelled, so other tools (a voice assistant, a notification filter) can
+# keep quiet while the screen is recorded. A recorder that dies on its own
+# runs no stop hook.
 
 [[ -f ~/.config/user-dirs.dirs ]] && source ~/.config/user-dirs.dirs
 OUTPUT_DIR="${OMARCHY_SCREENRECORD_DIR:-${XDG_VIDEOS_DIR:-$HOME/Videos}}"
@@ -382,6 +388,7 @@
       echo "$pid" >"$REC_PID_FILE"
       echo "$filename" >"$RECORDING_FILE"
       toggle_screenrecording_indicator
+      omarchy-hook screenrecord start 9>&-
       return 0
     fi
 
@@ -419,6 +426,7 @@
   done
 
   toggle_screenrecording_indicator
+  omarchy-hook screenrecord stop 9>&-
   cleanup_webcam
   wf_audio_cleanup
 
@@ -602,6 +610,7 @@
           cleanup_webcam
           rm -f "$(<"$RECORDING_FILE")" "$REC_PID_FILE" "$RECORDING_FILE"
           toggle_screenrecording_indicator
+          omarchy-hook screenrecord stop 9>&-
           rm -f "$CANCEL_FILE"
         fi
         # A request after that check is left for its requester, who stops this
```

### Testing

1. `mkdir -p ~/.config/omarchy/hooks/screenrecord.d` and a hook that logs its argument: `echo 'echo "$(date +%T) $1" >> /tmp/screenrecord-hook.log' > ~/.config/omarchy/hooks/screenrecord.d/log`.
2. `omarchy screenrecord`, then stop it: the log has `start`, then `stop`.
3. A second `omarchy screenrecord` while the first is still starting cancels that start (`CANCEL_FILE`): if the recorder was already running, the log has one `start` and one `stop`; Esc in the picker logs nothing, as no recorder started.
4. A hook ending in `sleep 60 &`: the next `omarchy screenrecord` is not blocked by the lock.
5. No `screenrecord` hooks: recording behaves as before.
