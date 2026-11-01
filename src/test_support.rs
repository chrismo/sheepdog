// Test fixtures shared across modules' unit tests (cfg(test) in lib.rs).

use crate::git::{git, strip_nl};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Once;

/// A temp dir's repo as lib/sheepdog.bats's setup made it: main with
/// one commit, and feat, checked out, one past it. git's global and system
/// config are a test file and none, for every git these tests run.
pub(crate) struct Repo {
    root: PathBuf,
    pub(crate) dir: String,
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

impl Repo {
    pub(crate) fn new() -> Repo {
        static ENV: Once = Once::new();
        static N: AtomicUsize = AtomicUsize::new(0);
        let base = std::env::temp_dir().join(format!("sd-git-test-{}", std::process::id()));
        ENV.call_once(|| {
            fs::create_dir_all(&base).unwrap();
            let cfg = base.join("gitconfig-global");
            fs::write(&cfg, "[user]\n\temail = t@example.com\n\tname = Tester\n[init]\n\tdefaultBranch = main\n").unwrap();
            std::env::set_var("GIT_CONFIG_GLOBAL", &cfg);
            std::env::set_var("GIT_CONFIG_SYSTEM", "/dev/null");
            for v in ["GIT_DIR", "GIT_WORK_TREE", "GIT_COMMON_DIR", "GIT_INDEX_FILE"] {
                std::env::remove_var(v);
            }
        });
        let root = base.join(N.fetch_add(1, Ordering::SeqCst).to_string());
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("repo")).unwrap();
        let dir = fs::canonicalize(root.join("repo")).unwrap().to_string_lossy().into_owned();
        let r = Repo { root, dir };
        r.git(&["init", "-q"]);
        r.git(&["commit", "-q", "--allow-empty", "-m", "init"]);
        r.git(&["switch", "-qc", "feat"]);
        r.git(&["commit", "-q", "--allow-empty", "-m", "one"]);
        r
    }

    /// git <args> in the repo, which must succeed; its stdout, chomped.
    pub(crate) fn git(&self, args: &[&str]) -> String {
        let o = git(&self.dir, args);
        assert!(o.ok, "git {args:?} failed: {}", o.stderr);
        strip_nl(o.stdout)
    }
}

/// (file, line number, line) for each line of src/ outside a #[cfg(test)] module.
pub(crate) fn production_lines(dir: &Path, out: &mut Vec<(PathBuf, usize, String)>) {
    for e in fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            production_lines(&p, out);
            continue;
        }
        if p.extension().is_none_or(|x| x != "rs") {
            continue;
        }
        let text = fs::read_to_string(&p).unwrap();
        let mut lines = text.lines().enumerate().peekable();
        while let Some((i, l)) = lines.next() {
            if l == "#[cfg(test)]" && lines.peek().is_some_and(|(_, m)| m.contains("mod ") && m.ends_with('{')) {
                lines.by_ref().find(|(_, l)| *l == "}");
                continue;
            }
            out.push((p.clone(), i + 1, l.to_string()));
        }
    }
}
