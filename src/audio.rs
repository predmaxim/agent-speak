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
        if needs_respawn(self.child.is_some(), &self.spawned_sink, &sink) {
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

    pub fn reset(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
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
}
