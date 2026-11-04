// The commit sd was built from (#440): build.rs stamps it, and sd watch
// status compares it with the HEAD of the checkout that built it.

use crate::git::git_out;

pub const COMMIT: &str = env!("SD_BUILD_COMMIT");

/// The checkout that built sd; in a debug build SHEEPDOG_BUILD_REPO names
/// another, so tests can pick the repo's HEAD.
fn repo() -> String {
    let seam = if cfg!(debug_assertions) { std::env::var("SHEEPDOG_BUILD_REPO").ok() } else { None };
    seam.unwrap_or_else(|| env!("CARGO_MANIFEST_DIR").into())
}

pub fn short(built: &str) -> String {
    let (sha, dirty) = built.split_once('-').unwrap_or((built, ""));
    let sha = &sha[..sha.len().min(7)];
    if dirty.is_empty() { sha.into() } else { format!("{sha}-{dirty}") }
}

pub fn status_line() -> String {
    line(COMMIT, git_out(&repo(), &["rev-parse", "HEAD"]).as_deref())
}

/// <built> is a sha, "<sha>-dirty" or "unknown".
pub fn line(built: &str, head: Option<&str>) -> String {
    let from = if built == "unknown" { "an unknown commit".into() } else { short(built) };
    let (sha, dirty) = (built.split('-').next(), built.ends_with("-dirty"));
    match head {
        None => format!("sd: built from {from}, repo HEAD unreadable"),
        // The edits a dirty build saw may since have changed or been reverted.
        Some(h) if sha == Some(h) && dirty => {
            format!("sd: built from {from}, with uncommitted changes the tree may no longer have: rebuild with ./install.sh")
        }
        Some(h) if sha == Some(h) => format!("sd: built from {from}, up to date"),
        Some(h) => format!("sd: built from {from}, repo HEAD is {}: rebuild with ./install.sh", short(h)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OLD: &str = "fd451310000000000000000000000000000000aa";
    const NEW: &str = "1a2b3c40000000000000000000000000000000bb";

    #[test]
    fn line_says_up_to_date_on_the_same_commit_and_rebuild_on_an_older_one() {
        assert_eq!(line(OLD, Some(OLD)), "sd: built from fd45131, up to date");
        assert_eq!(line(OLD, Some(NEW)), "sd: built from fd45131, repo HEAD is 1a2b3c4: rebuild with ./install.sh");
    }

    #[test]
    fn line_never_calls_a_dirty_build_up_to_date() {
        assert_eq!(
            line(&format!("{OLD}-dirty"), Some(OLD)),
            "sd: built from fd45131-dirty, with uncommitted changes the tree may no longer have: rebuild with ./install.sh"
        );
        assert_eq!(
            line(&format!("{OLD}-dirty"), Some(NEW)),
            "sd: built from fd45131-dirty, repo HEAD is 1a2b3c4: rebuild with ./install.sh"
        );
    }

    #[test]
    fn line_says_so_when_the_build_or_the_repos_head_is_unknown() {
        assert_eq!(line("unknown", Some(NEW)), "sd: built from an unknown commit, repo HEAD is 1a2b3c4: rebuild with ./install.sh");
        assert_eq!(line(OLD, None), "sd: built from fd45131, repo HEAD unreadable");
    }
}
