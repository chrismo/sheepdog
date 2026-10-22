use std::fs;
use std::process::{Command, Stdio};

pub fn output(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).stderr(Stdio::null()).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout);
    Some(s.strip_suffix('\n').unwrap_or(&s).to_string())
}

pub fn succeeds(args: &[&str]) -> bool {
    Command::new("git")
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

pub fn in_repo() -> bool {
    succeeds(&["rev-parse", "--git-dir"])
}

pub fn branch_exists(b: &str) -> bool {
    succeeds(&["rev-parse", "-q", "--verify", &format!("refs/heads/{b}")])
}

pub fn common_dir() -> Option<String> {
    output(&["rev-parse", "--path-format=absolute", "--git-common-dir"])
}

/// Mid-rebase, the branch being rebased (restack --resolve runs detached, #179).
pub fn current_branch() -> Option<String> {
    match output(&["branch", "--show-current"]) {
        Some(b) if !b.is_empty() => Some(b),
        _ => rebase_head(),
    }
}

fn rebase_head() -> Option<String> {
    let gd = output(&["rev-parse", "--absolute-git-dir"])?;
    let head = ["rebase-merge", "rebase-apply"]
        .iter()
        .find_map(|d| fs::read_to_string(format!("{gd}/{d}/head-name")).ok())?;
    head.trim_end().strip_prefix("refs/heads/").map(str::to_string)
}
