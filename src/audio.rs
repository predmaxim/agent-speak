//! Постоянный pw-cat: PCM пишется в stdin; reset — мгновенно оборвать звук.

use std::io::Write;
use std::sync::{Arc, Mutex};
use std::process::{Child, Command, Stdio};

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

    pub fn reset(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

/// Динамики ноутбука: первый выход не Bluetooth из `pactl list short sinks`; нет — пусто (вывод по умолчанию).
pub fn pick_speakers(list: &str) -> String {
    list.lines().filter_map(|l| l.split('\t').nth(1)).find(|n| !n.starts_with("bluez_")).unwrap_or_default().to_string()
}

pub fn speakers() -> String {
    Command::new("pactl").args(["list", "short", "sinks"]).output().map(|o| pick_speakers(&String::from_utf8_lossy(&o.stdout))).unwrap_or_default()
}

/// Звук по умолчанию идёт в Bluetooth-наушники.
pub fn bluetooth_out() -> bool {
    Command::new("pactl").arg("get-default-sink").output().is_ok_and(|o| o.stdout.starts_with(b"bluez_"))
}

fn needs_respawn(has_child: bool, spawned_sink: &str, sink: &str) -> bool {
    !has_child || spawned_sink != sink
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn respawns_when_sink_changes() {
        assert!(needs_respawn(false, "", ""));
        assert!(!needs_respawn(true, "", ""));
        assert!(needs_respawn(true, "", "echo-cancel-sink"));
        assert!(needs_respawn(true, "echo-cancel-sink", ""));
    }

    #[test]
    fn speakers_are_first_non_bluetooth_sink() {
        let list = "12619\tbluez_output.00_11_22_33_44_55.1\tPipeWire\ts16le 2ch 44100Hz\tSUSPENDED\n66\talsa_output.pci-0000_00_1f.3.analog-stereo\tPipeWire\ts32le 2ch 48000Hz\tSUSPENDED\n";
        assert_eq!(pick_speakers(list), "alsa_output.pci-0000_00_1f.3.analog-stereo");
        assert_eq!(pick_speakers("1\tbluez_output.x.1\tPipeWire\n"), "");
        assert_eq!(pick_speakers(""), "");
    }
}
