//! Память Омы между днями: служба дописывает пересказы дня в `news/<день>.jsonl`, сжимает день задача
//! `agent_voice.memory` по таймеру systemd (python/agent_voice/memory.py). День — с 05:00 до 05:00: пользователь
//! работает после полуночи.

use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::Path;

/// Граница дня, с от полуночи; та же — DAY_START в memory.py.
const DAY_START: i64 = 5 * 3600;

/// День памяти момента epoch: местная дата часом `epoch − 05:00`.
pub fn day(epoch: u64) -> String {
    // SAFETY: localtime_r потокобезопасна, tm — локальный буфер
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let t = (epoch as i64 - DAY_START) as libc::time_t;
    unsafe { libc::localtime_r(&t, &mut tm) };
    format!("{:04}-{:02}-{:02}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday)
}

/// Строка пересказа в `<dir>/news/<день>.jsonl`; каталоги — 0700, файл — 0600. Сбой записи — строка в журнал:
/// память — не повод ронять пересказ.
pub fn log_news(dir: &Path, epoch: u64, project: &str, ask: &str, say: &str) {
    let news = dir.join("news");
    let line = serde_json::json!({"t": epoch, "project": project, "ask": ask, "say": say}).to_string() + "\n";
    let res = std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&news).and_then(|_| {
        std::fs::OpenOptions::new().create(true).append(true).mode(0o600).open(news.join(format!("{}.jsonl", day(epoch))))
            .and_then(|mut f| f.write_all(line.as_bytes()))
    });
    if let Err(e) = res {
        eprintln!("agent-speak: память: пересказ не записан: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Эпоха местного времени 2026-11-12 ЧЧ:ММ.
    fn local(h: i32, m: i32) -> u64 {
        // SAFETY: mktime читает и нормализует свой tm
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        (tm.tm_year, tm.tm_mon, tm.tm_mday, tm.tm_hour, tm.tm_min, tm.tm_isdst) = (126, 10, 12, h, m, -1);
        unsafe { libc::mktime(&mut tm) as u64 }
    }

    #[test]
    fn day_starts_at_five() {
        assert_eq!(day(local(4, 59)), "2026-11-11", "после полуночи — ещё вчерашний день");
        assert_eq!(day(local(5, 0)), "2026-11-12");
        assert_eq!(day(local(23, 59)), "2026-11-12");
    }

    #[test]
    fn news_line_appended_to_day_file() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("agent-speak-memory-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        log_news(&dir, local(12, 0), "site", "Final", "Миграция откачена.");
        log_news(&dir, local(13, 0), "почта", "Question", "Куда пушить?");
        let f = dir.join("news/2026-11-12.jsonl");
        let lines: Vec<serde_json::Value> =
            std::fs::read_to_string(&f).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(lines.len(), 2);
        assert_eq!((lines[0]["project"].as_str(), lines[0]["ask"].as_str()), (Some("site"), Some("Final")));
        assert_eq!((lines[1]["say"].as_str(), lines[1]["t"].as_u64()), (Some("Куда пушить?"), Some(local(13, 0))));
        assert_eq!(std::fs::metadata(&f).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::metadata(dir.join("news")).unwrap().permissions().mode() & 0o777, 0o700);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
