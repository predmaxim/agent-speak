//! Постоянный pw-cat: PCM пишется в stdin; reset — мгновенно оборвать звук.

use std::io::{BufRead, Write};
use std::os::fd::AsRawFd;
use std::sync::{Arc, Mutex};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

/// Задержка узла pw-cat (`--latency`, как по умолчанию): столько звука сверх канала он держит у себя.
// shortcut: задержка графа и устройства (ALSA, кодек BT) в left() не входит — pactl даёт 0, pw-dump — кванты без кванта графа;
// точнее — свой поток PipeWire (pw_stream_get_time_n) вместо pw-cat, если эхо в хвосте начнёт проходить
const LATENCY: Duration = Duration::from_millis(100);
/// Байт звука в секунду: s16, 48 кГц, моно — формат pw-cat ниже.
const BYTES_PER_SEC: f64 = 96000.0;

pub struct Player {
    child: Option<Child>,
    spawned_sink: String, // вывод, с которым запущен child: --target читается только при spawn
    sink: Arc<Mutex<String>>,
}

impl Player {
    pub fn new(sink: Arc<Mutex<String>>) -> Player {
        Player { child: None, spawned_sink: String::new(), sink }
    }

    pub fn write(&mut self, pcm: &[u8]) -> bool {
        let sink = self.sink.lock().unwrap().clone();
        if needs_respawn(self.child.as_ref().is_some_and(|c| c.stdin.is_some()), &self.spawned_sink, &sink) {
            self.reset();
            let mut cmd = Command::new("pw-cat");
            cmd.args(["--playback", "--raw", "--format", "s16", "--rate", "48000", "--channels", "1"]);
            cmd.arg(format!("--latency={}ms", LATENCY.as_millis()));
            if !sink.is_empty() {
                cmd.args(["--target", &sink]); // нет такого узла — PipeWire подключит к выходу по умолчанию
            }
            self.spawned_sink = sink;
            self.child = cmd.arg("-").stdin(Stdio::piped()).spawn().ok();
        }
        let ok = self.child.as_mut().and_then(|c| c.stdin.as_mut()).is_some_and(|s| s.write_all(pcm).is_ok());
        if !ok {
            self.reset();
        }
        ok
    }

    /// Закрыть stdin: pw-cat доиграет буфер и выйдет сам. Простаивать ему нельзя: он ждёт
    /// stdin в fread прямо в главном цикле и не отвечает PipeWire — пропали наушники,
    /// поток переводят на другой выход, и этот выход виснет вместе со всеми потоками.
    pub fn close(&mut self) {
        if let Some(mut c) = self.child.take() {
            drop(c.stdin.take());
            std::thread::spawn(move || { let _ = c.wait(); }); // иначе вышедший pw-cat висит зомби до следующей фразы
        }
    }

    /// Сколько ещё звучит уже записанное: байты в канале pw-cat и его задержка; pw-cat нет (оборван, закрыт) — ноль.
    pub fn left(&self) -> Duration {
        let Some(fd) = self.child.as_ref().and_then(|c| c.stdin.as_ref()).map(AsRawFd::as_raw_fd) else { return Duration::ZERO };
        Duration::from_secs_f64(queued(fd).unwrap_or(0) as f64 / BYTES_PER_SEC) + LATENCY
    }

    pub fn reset(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

/// Байт в канале, ещё не прочитанных другим концом (FIONREAD работает и на пишущем конце).
fn queued(fd: std::os::fd::RawFd) -> Option<usize> {
    let mut n: libc::c_int = 0;
    // SAFETY: FIONREAD пишет int по указателю на локальную переменную; fd открыт у вызывающего
    (unsafe { libc::ioctl(fd, libc::FIONREAD, &mut n) } == 0).then_some(n as usize)
}

/// Динамики ноутбука: первый выход не Bluetooth из `pactl list short sinks`; нет — пусто (вывод по умолчанию).
pub fn pick_speakers(list: &str) -> String {
    list.lines().filter_map(|l| l.split('\t').nth(1)).find(|n| !n.starts_with("bluez_")).unwrap_or_default().to_string()
}

/// Строка `pactl subscribe` о смене выходов: «change on server» — сменился выход по умолчанию (pipewire-pulse шлёт его
/// при смене метаданных default), «new/remove on sink» — выход появился или пропал (подключили наушники).
pub fn outs_changed(line: &str) -> bool {
    ["on server", "'new' on sink #", "'remove' on sink #"].iter().any(|e| line.contains(e))
}

/// Выход по умолчанию и динамики ноутбука сейчас; pactl не ответил за 5 с — None (прежние значения в силе).
fn outs() -> Option<(String, String)> {
    let pactl = |args: &[&str]| crate::mux::run(Command::new("pactl").args(args), Duration::from_secs(5)).ok();
    Some((pactl(&["get-default-sink"])?.trim().to_string(), pick_speakers(&pactl(&["list", "short", "sinks"])?)))
}

/// Поток выходов: send(выход по умолчанию, динамики) — при старте и при каждой смене (события `pactl subscribe`), а не
/// опросом на каждую строку состояния. send вернул false — цикл службы кончился. pactl subscribe вышел (pipewire-pulse
/// перезапущен или не пускает) — подписка заново: через секунду, если сервер ответил при прошлой, иначе всё реже, до
/// раза в 30 с, и одна строка в журнале на серию отказов; pactl нет — поток кончается, выход остаётся «динамики».
pub fn watch_outs(send: impl Fn(String, String) -> bool + Send + 'static) {
    std::thread::spawn(move || {
        let mut fails = 0;
        loop {
            let mut sub = match Command::new("pactl").arg("subscribe").stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn() {
                Ok(c) => c,
                Err(e) => return eprintln!("agent-speak: не слежу за выходами звука (pactl subscribe): {e}"),
            };
            let lines = std::io::BufReader::new(sub.stdout.take().unwrap()).lines();
            // shortcut: смена выхода по умолчанию, пока pactl subscribe подключается, событием не придёт (при подключении он
            // ничего не печатает); заметят — выход по умолчанию брать из `pw-metadata -m -n default`: сначала снимок, затем
            // смены, без гонки; новые и пропавшие выходы — по-прежнему pactl subscribe
            let report = |o: Option<(String, String)>| o.is_none_or(|(d, s)| send(d, s));
            // принял ли сервер подписку, она не скажет: судим по ответу на чтение выходов сразу после неё
            let first = outs();
            let answered = first.is_some();
            let alive = report(first) && lines.map_while(Result::ok).filter(|l| outs_changed(l)).all(|_| report(outs()));
            let _ = sub.kill();
            let status = sub.wait();
            if !alive {
                return;
            }
            fails = if answered { 0 } else { fails + 1 };
            if fails == 1 {
                let status = status.map_or_else(|e| e.to_string(), |s| s.to_string());
                eprintln!("agent-speak: выходы звука: pipewire-pulse не отвечает (pactl subscribe: {status}) — пробую снова, всё реже, до раза в 30 с");
            }
            std::thread::sleep(resubscribe_after(fails));
        }
    });
}

/// Пауза перед новой подпиской после fails отказов подряд: секунда, затем вдвое дольше, не дольше 30 с.
fn resubscribe_after(fails: u32) -> Duration {
    Duration::from_secs((1u64 << fails.saturating_sub(1).min(5)).min(30))
}

/// Щелчок «услышала»: 90 мс, 880 Гц, края сглажены — без треска. s16 48 кГц моно, как у Player.
fn chime_pcm() -> Vec<u8> {
    let n = 48000 * 90 / 1000;
    (0..n)
        .map(|i| {
            let t = i as f32 / 48000.0;
            let fade = (std::f32::consts::PI * i as f32 / n as f32).sin(); // 0 → 1 → 0
            ((2.0 * std::f32::consts::PI * 880.0 * t).sin() * fade * 0.25 * i16::MAX as f32) as i16
        })
        .flat_map(i16::to_le_bytes)
        .collect()
}

/// Сыграть щелчок отдельным pw-cat в выход sink (пусто — по умолчанию), не задерживая вызывающего.
pub fn chime(sink: &str) {
    let sink = sink.to_string();
    std::thread::spawn(move || {
        let mut cmd = Command::new("pw-cat");
        cmd.args(["--playback", "--raw", "--format", "s16", "--rate", "48000", "--channels", "1"]);
        if !sink.is_empty() {
            cmd.args(["--target", &sink]);
        }
        if let Ok(mut c) = cmd.arg("-").stdin(Stdio::piped()).spawn() {
            if let Some(mut s) = c.stdin.take() {
                let _ = s.write_all(&chime_pcm());
            }
            let _ = c.wait(); // stdin закрыт: pw-cat доиграет и выйдет
        }
    });
}

fn needs_respawn(has_child: bool, spawned_sink: &str, sink: &str) -> bool {
    !has_child || spawned_sink != sink
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chime_is_short_and_fades() {
        let pcm = chime_pcm();
        assert_eq!(pcm.len(), 48000 * 90 / 1000 * 2);
        let s = |i: usize| i16::from_le_bytes([pcm[2 * i], pcm[2 * i + 1]]).unsigned_abs();
        assert!(s(0) < 50 && s(pcm.len() / 2 - 1) < 50, "края тихие — без щелчка-треска");
        assert!((0..pcm.len() / 2).map(s).max().unwrap() > 5000);
    }

    #[test]
    fn queued_bytes_are_read_on_the_writing_end() {
        use std::os::fd::AsRawFd;
        let (_r, mut w) = std::io::pipe().unwrap();
        assert_eq!(queued(w.as_raw_fd()), Some(0));
        // не больше PIPE_BUF: его пустой канал вмещает всегда — и урезанный до страницы лимитом pipe-user-pages-soft
        // (pipe(7)); 9600 байт в таком канале без читателя вешали тест навсегда
        w.write_all(&[0; libc::PIPE_BUF]).unwrap();
        assert_eq!(queued(w.as_raw_fd()), Some(libc::PIPE_BUF), "остаток канала pw-cat — по его пишущему концу");
    }

    #[test]
    fn respawns_when_sink_changes() {
        assert!(needs_respawn(false, "", ""));
        assert!(!needs_respawn(true, "", ""));
        assert!(needs_respawn(true, "", "echo-cancel-sink"));
        assert!(needs_respawn(true, "echo-cancel-sink", ""));
    }

    #[test]
    fn resubscribe_backs_off_to_half_a_minute() {
        let secs: Vec<u64> = (0..8).map(|f| resubscribe_after(f).as_secs()).collect();
        assert_eq!(secs, [1, 1, 2, 4, 8, 16, 30, 30], "сервер ответил — через секунду; не отвечает — всё реже, до 30 с");
        assert_eq!(resubscribe_after(u32::MAX).as_secs(), 30);
    }

    #[test]
    #[ignore] // живой pipewire-pulse
    fn outs_are_reported_at_start() {
        let (tx, rx) = std::sync::mpsc::channel();
        watch_outs(move |d, s| tx.send((d, s)).is_ok());
        let (default, _) = rx.recv_timeout(Duration::from_secs(5)).expect("выходы при старте");
        assert!(!default.is_empty());
    }

    #[test]
    fn speakers_are_first_non_bluetooth_sink() {
        let list = "12619\tbluez_output.00_11_22_33_44_55.1\tPipeWire\ts16le 2ch 44100Hz\tSUSPENDED\n66\talsa_output.pci-0000_00_1f.3.analog-stereo\tPipeWire\ts32le 2ch 48000Hz\tSUSPENDED\n";
        assert_eq!(pick_speakers(list), "alsa_output.pci-0000_00_1f.3.analog-stereo");
        assert_eq!(pick_speakers("1\tbluez_output.x.1\tPipeWire\n"), "");
        assert_eq!(pick_speakers(""), "");
    }
}
