// Stamps the commit sd is built from as SD_BUILD_COMMIT (#440): HEAD's sha,
// "-dirty" after it when the tracked files the binary is built from differ
// from HEAD, or "unknown" with no git. src/stamp.rs reads it.

use std::path::Path;
use std::process::Command;

/// What the binary is built from (everything include_str! embeds is under
/// src): the dirty check covers these and only these, and each is watched so
/// an edit or its revert redoes the check.
const INPUTS: [&str; 4] = ["src", "build.rs", "Cargo.toml", "Cargo.lock"];

fn git(args: &[&str]) -> Option<String> {
    let o = Command::new("git").args(args).output().ok()?;
    o.status.success().then(|| String::from_utf8_lossy(&o.stdout).trim().to_string())
}

fn commit() -> String {
    let Some(sha) = git(&["rev-parse", "HEAD"]) else { return "unknown".into() };
    // A build must not take index.lock from a git commit running beside it.
    let mut args = vec!["--no-optional-locks", "status", "--porcelain", "--untracked-files=no", "--"];
    args.extend(INPUTS);
    match git(&args) {
        Some(s) if !s.is_empty() => format!("{sha}-dirty"),
        _ => sha,
    }
}

/// A branch ref only in packed-refs has no file, and the commit that writes
/// one leaves HEAD and packed-refs alone, so then the ref's nearest directory
/// that exists, no higher than refs/, is watched instead.
fn ref_watch(path: &str) -> Option<String> {
    for a in Path::new(path).ancestors() {
        if a.exists() {
            return Some(a.to_string_lossy().into_owned());
        }
        if a.file_name().is_some_and(|n| n == "refs") {
            return None;
        }
    }
    None
}

/// --git-path finds HEAD and refs in a worktree too, where .git is a file.
/// Cargo reruns on every build for a path that doesn't exist, so none is
/// watched.
fn watched() -> Vec<String> {
    let path = |n: &str| git(&["rev-parse", "--git-path", n]);
    let mut out: Vec<String> = ["HEAD", "packed-refs"].into_iter().filter_map(path).collect();
    out.extend(INPUTS.map(String::from));
    out.retain(|p| Path::new(p).exists());
    out.extend(git(&["symbolic-ref", "-q", "HEAD"]).and_then(|r| path(&r)).and_then(|p| ref_watch(&p)));
    out
}

fn main() {
    println!("cargo:rustc-env=SD_BUILD_COMMIT={}", commit());
    for p in watched() {
        println!("cargo:rerun-if-changed={p}");
    }
}
