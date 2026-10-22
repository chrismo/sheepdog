// The guidance part of an agent step's prompt, and the shipped prompts it
// falls back to. lib/sheepdog.sh's step_guidance is the bash copy, for the
// steps still in bash.

use crate::config;
use crate::git::{git, git_out, strip_nl};
use std::fs;
use std::path::{Path, PathBuf};

/// The repo's lib/, from the binary (target/release/sd, or cargo test's
/// target/debug/sd). lib/prompts/*.md and lib/review-rules.md are read at
/// run time, so editing one needs no rebuild.
pub fn lib_dir() -> PathBuf {
    let exe = std::env::current_exe().and_then(fs::canonicalize).unwrap_or_default();
    exe.ancestors().nth(3).unwrap_or(Path::new("/")).join("lib")
}

pub fn read(p: &Path) -> Result<String, String> {
    fs::read(p).map(|b| String::from_utf8_lossy(&b).into_owned()).map_err(|e| format!("can't read {}: {e}", p.display()))
}

/// step_guidance: the guidance part of a step's prompt: the file
/// settings.<setting> names; else .sheepdog/<name>.md in the <base> commit,
/// <name> being the setting without "Prompt", kebab-cased (reviewFixPrompt:
/// .sheepdog/review-fix.md); else <default>. A relative path is read from the
/// <base> commit, which the branch's agent can't rewrite; an absolute one
/// from disk. A named file that is missing is an error, never the default.
/// An empty <base> means none can be trusted: no .sheepdog/ lookup, and a
/// relative path is refused. Every configurable step prompt goes through
/// here. The text is as $( ) gave it, trailing newlines stripped; the error
/// is what to print.
pub fn step_guidance(setting: &str, b: &str, base: &str, default: &Path) -> Result<String, String> {
    let path = config::get_text(&format!("settings.{setting}"), b, false)
        .map_err(|lines| lines.iter().map(|l| format!("sd config: {l}")).collect::<Vec<_>>().join("\n"))?;
    guidance(".", setting, &path, base, default)
}

/// step_guidance run in <dir>, with the setting's value <path>.
fn guidance(dir: &str, setting: &str, path: &str, base: &str, default: &Path) -> Result<String, String> {
    // From the top, so ./ means the worktree's top wherever the step runs.
    let o = git(dir, &["rev-parse", "--show-toplevel"]);
    if !o.ok {
        return Err(strip_nl(o.stderr));
    }
    let top = strip_nl(o.stdout);
    let blob = |p: &str| {
        let spec = format!("{base}:{p}");
        let is_blob = git_out(&top, &["cat-file", "-t", &spec]).is_some_and(|t| t == "blob");
        is_blob.then(|| git_out(&top, &["cat-file", "blob", &spec])).flatten()
    };
    let text = if path.is_empty() && base.is_empty() {
        read(default)?
    } else if path.is_empty() {
        match blob(&convention(setting)) {
            Some(t) => t,
            None => read(default)?,
        }
    } else if path.starts_with('/') {
        if !Path::new(path).is_file() {
            return Err(format!("settings.{setting}: no such file: {path}"));
        }
        read(Path::new(path))?
    } else if base.is_empty() {
        return Err(format!("settings.{setting}: no base commit to read {path} from; use an absolute path"));
    } else {
        let short: String = base.chars().take(7).collect();
        blob(path).ok_or_else(|| format!("settings.{setting}: no such file in {short}: {path}"))?
    };
    Ok(strip_nl(text))
}

/// .sheepdog/<name>.md for <setting>: reviewFixPrompt's is .sheepdog/review-fix.md.
fn convention(setting: &str) -> String {
    let name = setting.strip_suffix("Prompt").unwrap_or(setting);
    let mut kebab = String::new();
    for c in name.chars() {
        if c.is_ascii_uppercase() {
            kebab.push('-');
        }
        kebab.push(c.to_ascii_lowercase());
    }
    format!(".sheepdog/{kebab}.md")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A temp dir holding default.md ("shipped") and repo/, a repo on feat
    /// one commit past main, as lib/sheepdog.bats's guidance_repo made it.
    struct Repo {
        root: PathBuf,
        dir: String,
    }

    impl Drop for Repo {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    impl Repo {
        fn new() -> Repo {
            static N: AtomicUsize = AtomicUsize::new(0);
            let root = std::env::temp_dir()
                .join(format!("sd-guidance-test-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(root.join("repo")).unwrap();
            let root = fs::canonicalize(root).unwrap();
            fs::write(root.join("default.md"), "shipped\n").unwrap();
            fs::write(root.join("gitconfig"), "[user]\n\temail = t@example.com\n\tname = Tester\n[init]\n\tdefaultBranch = main\n")
                .unwrap();
            let r = Repo { dir: root.join("repo").to_string_lossy().into_owned(), root };
            r.git(&["init", "-q"]);
            r.git(&["commit", "-q", "--allow-empty", "-m", "init"]);
            r.git(&["switch", "-qc", "feat"]);
            r.git(&["commit", "-q", "--allow-empty", "-m", "one"]);
            r
        }

        /// git <args> in the repo, under the test's own git config.
        fn git(&self, args: &[&str]) -> String {
            let o = Command::new("git")
                .arg("-C")
                .arg(&self.dir)
                .args(args)
                .env("GIT_CONFIG_GLOBAL", self.root.join("gitconfig"))
                .env("GIT_CONFIG_SYSTEM", "/dev/null")
                .output()
                .unwrap();
            assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
            strip_nl(String::from_utf8_lossy(&o.stdout).into_owned())
        }

        fn commit(&self, p: &str, text: &str) {
            let f = Path::new(&self.dir).join(p);
            fs::create_dir_all(f.parent().unwrap()).unwrap();
            fs::write(f, text).unwrap();
            self.git(&["add", p]);
            self.git(&["commit", "-qm", p]);
        }

        fn guidance(&self, setting: &str, path: &str, base: &str) -> Result<String, String> {
            guidance(&self.dir, setting, path, base, &self.root.join("default.md"))
        }
    }

    // Was lib/sheepdog.bats: "step_guidance: empty setting, no convention file: the shipped default"
    #[test]
    fn empty_setting_no_convention_file_the_shipped_default() {
        let r = Repo::new();
        assert_eq!(r.guidance("reviewPrompt", "", "HEAD"), Ok("shipped".to_string()));
    }

    // Was lib/sheepdog.bats: "step_guidance: empty setting: .sheepdog/<name>.md in the base commit, kebab-cased"
    #[test]
    fn empty_setting_the_convention_file_in_the_base_commit_kebab_cased() {
        let r = Repo::new();
        r.commit(".sheepdog/review-fix.md", "repo review-fix\n");
        let base = r.git(&["rev-parse", "HEAD"]);
        r.commit(".sheepdog/review-fix.md", "branch rewrite\n");
        assert_eq!(r.guidance("reviewFixPrompt", "", &base), Ok("repo review-fix".to_string()));
    }

    // Was lib/sheepdog.bats: "step_guidance: a set path wins over the convention file"
    #[test]
    fn a_set_path_wins_over_the_convention_file() {
        let r = Repo::new();
        r.commit(".sheepdog/review.md", "convention\n");
        r.commit(".sheepdog/mine.md", "named\n");
        assert_eq!(r.guidance("reviewPrompt", ".sheepdog/mine.md", "HEAD"), Ok("named".to_string()));
    }

    // Was lib/sheepdog.bats: "step_guidance: no trusted base: the convention is skipped, a relative path refused"
    #[test]
    fn no_trusted_base_the_convention_is_skipped_a_relative_path_refused() {
        let r = Repo::new();
        r.commit(".sheepdog/review.md", "branch-written\n");
        assert_eq!(r.guidance("reviewPrompt", "", ""), Ok("shipped".to_string()));
        let e = r.guidance("reviewPrompt", ".sheepdog/review.md", "").unwrap_err();
        assert!(e.contains("no base commit"), "{e}");
    }

    #[test]
    fn a_relative_path_is_read_from_the_base_commit_and_missing_there_is_an_error() {
        let r = Repo::new();
        r.commit("g.md", "base\n");
        let base = r.git(&["rev-parse", "HEAD"]);
        r.commit("g.md", "rewritten\n");
        assert_eq!(r.guidance("reviewPrompt", "g.md", &base), Ok("base".to_string()));
        assert_eq!(r.guidance("reviewPrompt", "./g.md", &base), Ok("base".to_string()));
        let e = r.guidance("reviewPrompt", "nope.md", &base).unwrap_err();
        assert_eq!(e, format!("settings.reviewPrompt: no such file in {}: nope.md", &base[..7]));
        r.commit("sub/x.md", "x\n");
        assert!(r.guidance("reviewPrompt", "sub", "HEAD").is_err(), "a directory isn't a guidance file");
    }

    #[test]
    fn an_absolute_path_is_read_from_disk_whatever_the_base() {
        let r = Repo::new();
        let f = r.root.join("mine.md");
        fs::write(&f, "on disk\n\n").unwrap();
        let p = f.to_string_lossy();
        assert_eq!(r.guidance("reviewPrompt", &p, ""), Ok("on disk".to_string()));
        assert_eq!(r.guidance("reviewPrompt", &p, "HEAD"), Ok("on disk".to_string()));
        let missing = r.root.join("nope.md").to_string_lossy().into_owned();
        assert_eq!(r.guidance("reviewPrompt", &missing, "HEAD"), Err(format!("settings.reviewPrompt: no such file: {missing}")));
        let dir = r.root.to_string_lossy().into_owned();
        assert!(r.guidance("reviewPrompt", &dir, "HEAD").is_err());
    }

    #[test]
    fn convention_drops_prompt_and_kebab_cases() {
        assert_eq!(convention("reviewPrompt"), ".sheepdog/review.md");
        assert_eq!(convention("reviewFixPrompt"), ".sheepdog/review-fix.md");
        assert_eq!(convention("ciFixPrompt"), ".sheepdog/ci-fix.md");
    }
}
