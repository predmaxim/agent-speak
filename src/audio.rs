//! Постоянный pw-cat: PCM пишется в stdin; reset — мгновенно оборвать звук.

use std::io::Write;
use std::process::{Child, Command, Stdio};

pub struct Player {
    child: Option<Child>,
}

impl Player {
    pub fn new() -> Player {
        Player { child: None }
    }

    pub fn write(&mut self, pcm: &[u8]) -> bool {
        if self.child.is_none() {
            self.child = Command::new("pw-cat")
                .args(["--playback", "--raw", "--format", "s16", "--rate", "48000", "--channels", "1", "-"])
                .stdin(Stdio::piped())
                .spawn()
                .ok();
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
