// The CI config hold (#124): an agent step's commit that touches a protected
// path isn't pushed until chrismo has looked. libexec/sd-act's hold_* and
// lib/sheepdog.sh's protected_paths still do the same for ci-fix, ship and
// restack --resolve: change both.

use super::{die, repo_remote};
use crate::git::{git, git_out, is_ancestor, rev};

/// A path covers itself and what's under it.
const PROTECTED_DEFAULT: [&str; 7] =
    [".github/workflows/", ".github/actions/", ".rwx/", ".captain/", ".circleci/", ".gitlab-ci.yml", ".buildkite/"];
const PROTECTED_FILE: &str = ".sheepdog/protected-paths";

/// settings.protectedPaths when set (an empty list protects nothing); else
/// .sheepdog/protected-paths in the <base> commit, which the branch's agent
/// can't rewrite (one per line, # comments); else the defaults. An empty
/// <base> skips the file, as step_guidance does. Unless the setting is set,
/// the file is itself protected: an agent emptying it would unprotect every
/// branch after it merges.
fn protected_paths(b: &str, base: &str) -> Vec<String> {
    let mut paths: Vec<String> = match crate::config::get_text("settings.protectedPaths", b, false) {
        Ok(set) => return set.lines().filter(|p| !p.is_empty()).map(str::to_string).collect(),
        Err(lines) if !lines.is_empty() => {
            for l in lines {
                eprintln!("sd config: {l}");
            }
            die("sd config failed")
        }
        Err(_) => {
            let spec = format!("{base}:{PROTECTED_FILE}");
            if !base.is_empty() && git_out(".", &["cat-file", "-t", &spec]).is_some_and(|t| t == "blob") {
                file_paths(&git(".", &["cat-file", "blob", &spec]).stdout)
            } else {
                PROTECTED_DEFAULT.iter().map(|p| p.to_string()).collect()
            }
        }
    };
    paths.push(PROTECTED_FILE.to_string());
    paths
}

/// .sheepdog/protected-paths' paths: comments, blanks and the space around
/// them dropped.
fn file_paths(text: &str) -> Vec<String> {
    text.lines()
        .map(|l| l.split('#').next().unwrap_or("").trim())
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

/// The <files> a protected path covers, trailing / or not.
fn protected_hits(paths: &[String], files: &[String]) -> Vec<String> {
    let covers = |p: &str, f: &str| {
        let p = p.strip_suffix('/').unwrap_or(p);
        f == p || f.strip_prefix(p).is_some_and(|rest| rest.starts_with('/'))
    };
    files.iter().filter(|f| !f.is_empty() && paths.iter().any(|p| !p.is_empty() && covers(p, f))).cloned().collect()
}

fn say(paths: &[String]) -> String {
    format!("the agent changed CI config ({}): review, then sd act push", paths.join(", "))
}

/// The paths an active hold names; None with none. A hold whose commit is on
/// the remote was published some other way, so it is cleared (reconcile, not
/// enforce). One that can't be read refuses <doing>.
fn hold_paths(b: &str, remote: &str, doing: &str) -> Option<Vec<String>> {
    let key = format!("branch.{b}.sd-held");
    let unreadable = || die(format!("not {doing}: can't read {key}, the CI config hold"));
    let o = git(".", &["config", "--get", &key]);
    if o.code == 1 {
        return None;
    }
    if !o.ok {
        unreadable();
    }
    let v = o.stdout.strip_suffix('\n').unwrap_or(&o.stdout);
    let (commit, paths) = v.split_once('\n').unwrap_or((v, v));
    let rsha = rev(".", &format!("refs/remotes/{remote}/{b}"));
    if !rsha.is_empty() && is_ancestor(".", commit, &rsha) {
        if !git(".", &["config", "--unset", &key]).ok {
            unreadable();
        }
        return None;
    }
    Some(paths.lines().map(str::to_string).collect())
}

/// After an agent step's edits are committed, the <files> they changed
/// against the protected paths (read from <base>). On a match, hold the
/// branch: branch.<b>.sd-held is HEAD, then the paths, an active hold's
/// included, one per line. The automated pushes refuse while it holds;
/// chrismo running sd act push is the approval, and clears it. A HEAD already
/// on the remote-tracking ref was pushed during the step, by the agent (the
/// deny on git push is no guarantee either): a hold would only be cleared as
/// published, unseen, so that fails loudly instead.
fn hold_after(b: &str, base: &str, files: &[String]) {
    let hits = protected_hits(&protected_paths(b, base), files);
    if hits.is_empty() {
        return;
    }
    let remote = repo_remote();
    let head = format!("refs/heads/{b}");
    let rsha = rev(".", &format!("refs/remotes/{remote}/{b}"));
    if !rsha.is_empty() && is_ancestor(".", &head, &rsha) {
        die(format!(
            "the agent pushed CI config itself ({}): it is on {remote}/{b} unreviewed; review it now (git log -p {remote}/{b} -- {})",
            hits.join(", "),
            hits.join(" ")
        ));
    }
    let mut held = hold_paths(b, &remote, &format!("holding {b}")).unwrap_or_default();
    held.extend(hits);
    held.retain(|p| !p.is_empty());
    held.sort();
    held.dedup();
    let o = git(".", &["config", &format!("branch.{b}.sd-held"), &format!("{}\n{}", rev(".", &head), held.join("\n"))]);
    if !o.ok {
        eprint!("{}", o.stderr);
        std::process::exit(o.code);
    }
    println!("held: {}", say(&held));
}

/// hold_after over <since>..the branch, if it moved. Each agent step runs it
/// right after the harness returns, failed or not, before any outcome can die
/// (the agent may commit itself: the deny is no guarantee), and again from
/// there over the pipeline's own commit.
pub fn hold_since(b: &str, base: &str, since: &str) {
    let head = format!("refs/heads/{b}");
    if rev(".", &head) == since {
        return;
    }
    let diff = git(".", &["diff", "-z", "--name-only", "--no-renames", since, &head]).stdout;
    let files: Vec<String> = diff.split('\0').filter(|f| !f.is_empty()).map(str::to_string).collect();
    hold_after(b, base, &files);
}

/// Refuse <doing> while a hold is active; <then> follows the message.
pub fn hold_gate(b: &str, doing: &str, then: &str) {
    let Some(paths) = hold_paths(b, &repo_remote(), doing) else { return };
    die(format!("not {doing}: {}. {then}", say(&paths)));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    // Was lib/sheepdog.bats: "protected_hits: a path covers itself and what's under it, trailing / or not"
    #[test]
    fn protected_hits_a_path_covers_itself_and_whats_under_it_trailing_slash_or_not() {
        let paths = strings(&[".github/workflows/", ".rwx", ".gitlab-ci.yml"]);
        let files = strings(&[
            ".github/workflows/ci.yml", ".github/workflowsx", ".rwx/tests.yml", ".rwx", ".rwxy/a", ".gitlab-ci.yml",
            ".gitlab-ci.yml.bak", "src/main.rs", "",
        ]);
        assert_eq!(protected_hits(&paths, &files), strings(&[".github/workflows/ci.yml", ".rwx/tests.yml", ".rwx", ".gitlab-ci.yml"]));
        assert!(protected_hits(&[], &files).is_empty());
    }

    #[test]
    fn file_paths_drops_comments_blanks_and_the_space_around() {
        let text = "# CI\n.github/\n\n  deploy/pipeline.yml  # the deploy\n   \n#.rwx/\n";
        assert_eq!(file_paths(text), strings(&[".github/", "deploy/pipeline.yml"]));
    }

    #[test]
    fn say_names_the_paths_and_the_way_out() {
        assert_eq!(
            say(&strings(&[".circleci/config.yml", ".rwx/tests.yml"])),
            "the agent changed CI config (.circleci/config.yml, .rwx/tests.yml): review, then sd act push"
        );
    }
}
