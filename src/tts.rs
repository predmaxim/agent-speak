//! Дочерний процесс silero_tts.py: модель в памяти, перезапуск при сбое.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

/// Файл модели Silero в ~/.local/share/agent-speak; его качает install.sh.
pub const MODEL: &str = "v5_6_ru.pt";

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

    /// tone и lead — смысл, разметку SSML строит адаптер (python/silero_tts.py `ssml`); прежний сервер их не читает.
    fn request(&self, text: &str, speaker: &str, rate: &str, tone: &str, lead: &str) -> std::io::Result<Option<Vec<u8>>> {
        let mut s = UnixStream::connect(&self.sock)?;
        s.set_read_timeout(Some(Duration::from_secs(60)))?;
        let req = serde_json::json!({"text": text, "speaker": speaker, "rate": rate, "tone": tone, "lead": lead});
        s.write_all(format!("{req}\n").as_bytes())?;
        read_reply(&mut s)
    }

    pub fn synth(&mut self, text: &str, speaker: &str, rate: &str, tone: &str, lead: &str) -> Result<Vec<u8>, Failed> {
        for _ in 0..3 {
            if self.ensure() {
                match self.request(text, speaker, rate, tone, lead) {
                    Ok(Some(pcm)) => {
                        self.failures = 0;
                        return Ok(pcm);
                    }
                    Ok(None) => return Err(Failed::Text), // сервер жив: перезапуск не поможет
                    Err(_) => {}
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
        Err(Failed::Down)
    }
}

/// Почему нет звука: сервер синтеза не отвечает (фраза подождёт) или не смог этот текст (повтор не поможет).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Failed {
    Down,
    Text,
}

/// Ответ silero_tts.py: u32 длина + PCM; длина u32::MAX — ошибка синтеза (None), 0 — читать нечего.
fn read_reply(r: &mut impl Read) -> std::io::Result<Option<Vec<u8>>> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let n = u32::from_le_bytes(len);
    if n == u32::MAX {
        return Ok(None);
    }
    let mut pcm = vec![0u8; n as usize];
    r.read_exact(&mut pcm)?;
    Ok(Some(pcm))
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
    fn error_reply_is_not_empty_sound() {
        let mut ok = std::io::Cursor::new([2u8, 0, 0, 0, 7, 8]);
        assert_eq!(read_reply(&mut ok).unwrap(), Some(vec![7, 8]));
        let mut empty = std::io::Cursor::new([0u8; 4]);
        assert_eq!(read_reply(&mut empty).unwrap(), Some(vec![]), "читать нечего — пусто, не ошибка");
        let mut err = std::io::Cursor::new([0xFFu8; 4]);
        assert_eq!(read_reply(&mut err).unwrap(), None);
    }

    #[test]
    #[ignore] // нужна модель: cargo test tts -- --ignored
    fn synthesizes_and_restarts() {
        let data = std::path::PathBuf::from(std::env::var("HOME").unwrap()).join(".local/share/agent-speak");
        let sock = std::env::temp_dir().join(format!("tts-test-{}.sock", std::process::id()));
        let mut t = Tts::new(
            data.join("venv/bin/python"),
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("python/silero_tts.py"),
            data.join(MODEL),
            sock,
        );
        assert!(t.synth("Проверка связи.", "xenia", "medium", "", "").unwrap().len() > 48000);
        assert!(matches!(t.synth("Привет.", "nobody", "medium", "", ""), Err(Failed::Text)));
        t.kill();
        assert!(t.synth("Снова работает.", "xenia", "medium", "", "").unwrap().len() > 48000);
        assert!(t.synth("куда пушить?", "kseniya", "medium", "ask", "агент спик").unwrap().len() > 48000, "интонация и проект");
    }
}
