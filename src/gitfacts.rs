//! Факты git о ходе агента — кодом, без модели: «Закоммичено», «Не закоммичены 3 файла», в окне ещё «не запушено: 2»
//! (дизайн «Джарвис», done-facts). Только чтение; без необязательных замков git: index.lock под ногами у агента не берём.

use crate::mux;
use crate::text::plural;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, UNIX_EPOCH};

/// Срок каждой команды git.
const TIMEOUT: Duration = Duration::from_secs(5);
/// Больше тронутых в ходе файлов — «много незакоммиченных файлов».
pub const MANY: usize = 500;
/// Запас на часы и округление mtime: файл, тронутый за пару секунд до промпта, — тоже ход.
const SLACK: u64 = 2;

/// Git после хода. Default — о git ни слова.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Facts {
    pub commits: usize,          // коммитов за ход
    pub dirty: usize,            // незакоммиченных файлов, тронутых в ходе
    pub unpushed: Option<usize>, // коммитов ветки не в upstream; None — upstream нет
}

impl Facts {
    /// О git в ходе, строчными, без точки; None — в ходе git не менялся.
    fn said(&self) -> Option<String> {
        match self.dirty {
            0 if self.commits > 0 => Some("закоммичено".into()),
            0 => None,
            n if n > MANY => Some("много незакоммиченных файлов".into()),
            n => Some(format!("не {} {n} {}", plural(n, ["закоммичен", "закоммичены", "закоммичены"]), plural(n, ["файл", "файла", "файлов"]))),
        }
    }

    /// Фраза после пересказа итога: «Закоммичено.», «Не закоммичены 3 файла.»; "" — молчим.
    pub fn voice(&self) -> String {
        self.said().map(|s| {
            let mut c = s.chars();
            c.next().map(|f| f.to_uppercase().chain(c).collect::<String>() + ".").unwrap_or_default()
        }).unwrap_or_default()
    }

    /// Пересказ итога и фраза фактов за ним — как он звучит и стоит в пузыре.
    pub fn after(&self, say: &str) -> String {
        let v = self.voice();
        if v.is_empty() { say.to_string() } else { format!("{say} {v}") }
    }

    /// В пачке итогов звучит только тревожное: «В agent-speak не закоммичены 3 файла.»; None — нечего.
    pub fn alarm(&self, project: &str) -> Option<String> {
        (self.dirty > 0).then(|| format!("В {project} {}.", self.said().unwrap_or_default()))
    }

    /// Строке агента в окне: «не закоммичены 3 файла · не запушено: 2»; "" — в ходе git не менялся.
    pub fn window(&self) -> String {
        let Some(s) = self.said() else { return String::new() };
        match self.unpushed {
            Some(n) if n > 0 => format!("{s} · не запушено: {n}"),
            _ => s,
        }
    }
}

/// Строки `git status --porcelain -z`: (путь от корня репозитория, файла там больше нет). У переименования и копии
/// за новым путём идёт старый — он пропускается; переименование — как удаление: mtime файла оно не меняет, только папки.
pub fn parse(out: &str) -> Vec<(&str, bool)> {
    let mut fields = out.split('\0').filter(|f| !f.is_empty());
    let mut got = Vec::new();
    while let Some(f) = fields.next() {
        let (xy, path) = (f.get(..2).unwrap_or_default(), f.get(3..).unwrap_or_default());
        if xy.contains(['R', 'C']) {
            fields.next();
        }
        got.push((path, xy.contains(['D', 'R'])));
    }
    got
}

/// Факты хода в папке cwd с промпта since (эпоха, с). Не репозиторий, ошибка или таймаут git — None и строка в журнал.
/// «Изменён в ходе» — mtime не раньше since − SLACK (у удалённого и переименованного — mtime папки, где он был или стал):
/// чужие правки, сделанные до хода, не в счёт; и при больших списках status — тоже по mtime.
pub fn collect(cwd: &Path, since: u64) -> Option<Facts> {
    let git = |args: &[&str]| mux::run(Command::new("git").arg("--no-optional-locks").arg("-C").arg(cwd).args(args), TIMEOUT);
    let facts = || -> Result<Facts, String> {
        let top = git(&["rev-parse", "--show-toplevel"])?;
        let top = Path::new(top.trim_end_matches('\n'));
        // shortcut: путь не в UTF-8 обрывает чтение вывода (mux::run читает строку) — тогда ошибка и без фактов
        let status = git(&["status", "--porcelain", "-z", "--untracked-files=all"])?;
        // удаление и переименование меняют mtime папки, а не файла: у них — ближайшая существующая папка-предок
        let dirty = parse(&status).iter()
            .filter(|(p, gone)| top.join(p).ancestors().skip(*gone as usize).find_map(mtime).is_none_or(|t| t + SLACK >= since))
            .count();
        let commits = git(&["rev-list", "--count", &format!("--since=@{since}"), "HEAD"])?;
        let commits = commits.trim().parse().map_err(|e| format!("rev-list: {e}"))?;
        // нет upstream — git отвечает ошибкой: «не запушено» не знаем
        let unpushed = git(&["rev-list", "--count", "@{u}..HEAD"]).ok().and_then(|n| n.trim().parse().ok());
        Ok(Facts { commits, dirty, unpushed })
    };
    facts().map_err(|e| eprintln!("agent-speak: факты git ({}): {e}", cwd.display())).ok()
}

/// mtime файла, эпоха, с; ссылка — своя, не цели.
fn mtime(p: &Path) -> Option<u64> {
    let t = std::fs::symlink_metadata(p).and_then(|m| m.modified()).ok()?;
    t.duration_since(UNIX_EPOCH).ok().map(|d| d.as_secs())
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::SystemTime;

    /// git для подготовки репозитория теста: без глобальных настроек пользователя (подпись, хуки), коммит с датой date.
    pub fn git(dir: &Path, date: u64, args: &[&str]) {
        let ok = Command::new("git").args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "init.defaultBranch=main", "-C"]).arg(dir).args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null").env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_DATE", format!("@{date} +0000")).env("GIT_COMMITTER_DATE", format!("@{date} +0000"))
            .stdout(std::process::Stdio::null()).status().unwrap().success();
        assert!(ok, "git {args:?}");
    }

    pub fn now() -> u64 {
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
    }

    /// Свежий репозиторий в /tmp: файлы a, b и sub/x закоммичены давно (2001 год).
    pub fn repo(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("oma-gitfacts-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        for f in ["a", "b", "sub/x"] {
            std::fs::write(dir.join(f), "1").unwrap();
        }
        git(&dir, 1_000_000_000, &["init", "-q"]);
        git(&dir, 1_000_000_000, &["add", "-A"]);
        git(&dir, 1_000_000_000, &["commit", "-q", "-m", "старое"]);
        dir
    }

    #[test]
    fn parse_takes_new_path_of_rename_and_marks_gone() {
        let out = "D  b\0R  c\0a\0?? d e\0?? n/f\0 M ру с\0C  k\0j\0";
        assert_eq!(parse(out), [("b", true), ("c", true), ("d e", false), ("n/f", false), ("ру с", false), ("k", false)]);
        assert!(parse("").is_empty());
    }

    #[test]
    fn phrases_by_count() {
        let f = |commits, dirty| Facts { commits, dirty, unpushed: None };
        assert_eq!(f(0, 0).voice(), "", "в ходе git не менялся — о git ни слова");
        assert_eq!(f(2, 0).voice(), "Закоммичено.");
        assert_eq!(f(2, 1).voice(), "Не закоммичен 1 файл.");
        assert_eq!(f(0, 3).voice(), "Не закоммичены 3 файла.");
        assert_eq!(f(0, 5).voice(), "Не закоммичены 5 файлов.");
        assert_eq!(f(0, 21).voice(), "Не закоммичен 21 файл.");
        assert_eq!(f(0, MANY + 1).voice(), "Много незакоммиченных файлов.");
        assert_eq!((f(2, 0).alarm("plug"), f(0, 3).alarm("plug").as_deref()), (None, Some("В plug не закоммичены 3 файла.")));
        assert_eq!(Facts { unpushed: Some(2), ..f(0, 3) }.window(), "не закоммичены 3 файла · не запушено: 2");
        assert_eq!(Facts { unpushed: Some(0), ..f(1, 0) }.window(), "закоммичено");
        assert_eq!(Facts { unpushed: Some(2), ..f(0, 0) }.window(), "", "«не запушено» — только о ходе, который трогал git");
    }

    #[test]
    fn collect_counts_only_this_turn() {
        let dir = repo("turn");
        let since = now() - 100;
        assert_eq!(collect(&dir, since), Some(Facts::default()), "чисто, коммитов за ход нет");
        // чужая правка до хода не в счёт; правка и удаление в ходе — да; cwd — подпапка: пути status от корня
        let old = std::fs::File::create(dir.join("до хода")).unwrap();
        old.set_modified(UNIX_EPOCH + Duration::from_secs(since - 1000)).unwrap();
        std::fs::write(dir.join("sub/x"), "2").unwrap();
        std::fs::remove_file(dir.join("b")).unwrap();
        let f = collect(&dir.join("sub"), since).unwrap();
        assert_eq!((f.commits, f.dirty, f.voice().as_str()), (0, 2, "Не закоммичены 2 файла."));
        // закоммитил в ходе; upstream — локальная ветка на старом коммите
        git(&dir, 1_000_000_000, &["branch", "base"]);
        git(&dir, 1_000_000_000, &["branch", "-q", "-u", "base"]);
        git(&dir, now(), &["commit", "-q", "-am", "ход"]);
        let f = collect(&dir, since).unwrap();
        assert_eq!((f.commits, f.dirty, f.unpushed), (1, 0, Some(1)));
        assert_eq!((f.voice().as_str(), f.window().as_str()), ("Закоммичено.", "закоммичено · не запушено: 1"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn deleted_and_renamed_before_the_turn_are_not_its_facts() {
        let dir = repo("before");
        let since = now() + 3600; // ход начнётся через час: всё, что ниже, — до него
        std::fs::remove_file(dir.join("b")).unwrap();
        git(&dir, 1_000_000_000, &["mv", "sub/x", "sub/y"]);
        // удаление и переименование меняют mtime папки, а не файла: папки — тоже до хода
        for d in [dir.clone(), dir.join("sub")] {
            std::fs::File::open(&d).unwrap().set_modified(UNIX_EPOCH + Duration::from_secs(since - 1000)).unwrap();
        }
        assert_eq!(collect(&dir, since), Some(Facts::default()), "старое удаление и git mv — не этот ход");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn many_untracked_files_from_before_the_turn_are_not_its_facts() {
        let dir = repo("many");
        let since = now();
        for i in 0..=MANY {
            let f = std::fs::File::create(dir.join(format!("old{i}"))).unwrap();
            f.set_modified(UNIX_EPOCH + Duration::from_secs(since - 1000)).unwrap();
        }
        assert_eq!(collect(&dir, since).map(|f| f.dirty), Some(0), "больше MANY строк status — всё равно по mtime");
        std::fs::write(dir.join("new"), "1").unwrap();
        assert_eq!(collect(&dir, since).map(|f| f.voice()).as_deref(), Some("Не закоммичен 1 файл."));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn not_a_repo_has_no_facts() {
        let dir = std::env::temp_dir().join(format!("oma-gitfacts-none-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(collect(&dir, now()), None);
        assert_eq!(collect(&dir.join("нет такой"), now()), None);
        let _ = std::fs::remove_dir_all(dir);
    }
}
