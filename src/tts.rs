//! Дочерний процесс silero_tts.py: модель в памяти, перезапуск при сбое.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

pub struct Tts {
    python: PathBuf,
    script: PathBuf,
    model: PathBuf,
    sock: PathBuf,
    child: Option<Child>,
    failures: u32,
    last_notice: Option<Instant>,
}

impl Tts {
    pub fn new(python: PathBuf, script: PathBuf, model: PathBuf, sock: PathBuf) -> Tts {
        Tts { python, script, model, sock, child: None, failures: 0, last_notice: None }
    }

    pub fn kill(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        let _ = std::fs::remove_file(&self.sock);
    }

    fn ensure(&mut self) -> bool {
        if let Some(c) = &mut self.child {
            if c.try_wait().ok().flatten().is_none() {
                return true;
            }
        }
        self.kill();
        let Ok(child) = Command::new(&self.python).arg(&self.script).arg(&self.model).arg(&self.sock).spawn() else {
            return false;
        };
        self.child = Some(child);
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_secs(30) {
            if self.sock.exists() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        false
    }

    fn request(&self, text: &str, speaker: &str, rate: &str) -> std::io::Result<Vec<u8>> {
        let mut s = UnixStream::connect(&self.sock)?;
        s.set_read_timeout(Some(Duration::from_secs(60)))?;
        let req = serde_json::json!({"text": text, "speaker": speaker, "rate": rate});
        s.write_all(format!("{req}\n").as_bytes())?;
        let mut len = [0u8; 4];
        s.read_exact(&mut len)?;
        let mut pcm = vec![0u8; u32::from_le_bytes(len) as usize];
        s.read_exact(&mut pcm)?;
        Ok(pcm)
    }

    pub fn synth(&mut self, text: &str, speaker: &str, rate: &str) -> Option<Vec<u8>> {
        for _ in 0..3 {
            if self.ensure() {
                if let Ok(pcm) = self.request(text, speaker, rate) {
                    self.failures = 0;
                    return Some(pcm);
                }
            }
            self.kill();
            self.failures += 1;
        }
        if self.failures >= 3 {
            // не чаще раза в минуту: speaker повторяет синтез каждые ~10 с
            if self.last_notice.is_none_or(|t| t.elapsed() > Duration::from_secs(60)) {
                crate::notice::notify("Синтез упал", "silero_tts.py не отвечает");
                self.last_notice = Some(Instant::now());
            }
            self.failures = 0;
        }
        None
    }
}

impl Drop for Tts {
    fn drop(&mut self) {
        self.kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore] // нужна модель: cargo test tts -- --ignored
    fn synthesizes_and_restarts() {
        let data = std::path::PathBuf::from(std::env::var("HOME").unwrap()).join(".local/share/agent-speak");
        let sock = std::env::temp_dir().join(format!("tts-test-{}.sock", std::process::id()));
        let mut t = Tts::new(
            data.join("venv/bin/python"),
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("python/silero_tts.py"),
            data.join("v5_ru.pt"),
            sock,
        );
        assert!(t.synth("Проверка связи.", "xenia", "medium").unwrap().len() > 48000);
        t.kill();
        assert!(t.synth("Снова работает.", "xenia", "medium").unwrap().len() > 48000);
    }
}
