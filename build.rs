// Stamps the commit sd is built from as SD_BUILD_COMMIT (#440): HEAD's sha,
// "-dirty" after it when tracked files differ from HEAD, or "unknown" with no
// git. src/stamp.rs reads it.

use std::path::Path;
use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let o = Command::new("git").args(args).output().ok()?;
    o.status.success().then(|| String::from_utf8_lossy(&o.stdout).trim().to_string())
}

fn commit() -> String {
    let Some(sha) = git(&["rev-parse", "HEAD"]) else { return "unknown".into() };
    match git(&["status", "--porcelain", "--untracked-files=no"]) {
        Some(s) if !s.is_empty() => format!("{sha}-dirty"),
        _ => sha,
    }
}

/// --git-path finds HEAD and refs in a worktree too, where .git is a file.
/// src is watched so an edit there redoes the dirty check.
fn watched() -> Vec<String> {
    let mut names = vec!["HEAD".to_string(), "packed-refs".into()];
    names.extend(git(&["symbolic-ref", "-q", "HEAD"]));
    let mut out: Vec<String> = names.iter().filter_map(|n| git(&["rev-parse", "--git-path", n])).collect();
    out.push("src".into());
    out.retain(|p| Path::new(p).exists());
    out
}

fn main() {
    println!("cargo:rustc-env=SD_BUILD_COMMIT={}", commit());
    for p in watched() {
        println!("cargo:rerun-if-changed={p}");
    }
}
