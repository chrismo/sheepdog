// The CI config hold (#124): an agent step's commit that touches a protected
// path isn't pushed until chrismo has looked. And pr_gate (#270): nothing is
// pushed or posted onto a PR that merged or closed while the step ran.

use super::{die, repo_remote};
use crate::git::{git, git_out, is_ancestor, rev};
use crate::proc;

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
    let set = match crate::config::get_text("settings.protectedPaths", b, false) {
        Ok(set) => Some(set),
        Err(lines) if !lines.is_empty() => {
            for l in lines {
                eprintln!("sd config: {l}");
            }
            die("sd config failed")
        }
        Err(_) => None,
    };
    paths_in(".", set.as_deref(), base)
}

/// protected_paths in the repo at <dir>, given the setting's lines if set.
fn paths_in(dir: &str, set: Option<&str>, base: &str) -> Vec<String> {
    if let Some(set) = set {
        return set.lines().filter(|p| !p.is_empty()).map(str::to_string).collect();
    }
    let spec = format!("{base}:{PROTECTED_FILE}");
    let mut paths: Vec<String> = if !base.is_empty() && git_out(dir, &["cat-file", "-t", &spec]).is_some_and(|t| t == "blob") {
        file_paths(&git(dir, &["cat-file", "blob", &spec]).stdout)
    } else {
        PROTECTED_DEFAULT.iter().map(|p| p.to_string()).collect()
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
pub(super) fn hold_after(b: &str, base: &str, files: &[String]) {
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

/// Refuse <doing> while a hold is active; <then>, if any, follows the message.
pub fn hold_gate(b: &str, doing: &str, then: &str) {
    let Some(paths) = hold_paths(b, &repo_remote(), doing) else { return };
    die(refusal(doing, &paths, then));
}

fn refusal(doing: &str, paths: &[String], then: &str) -> String {
    let then = if then.is_empty() { String::new() } else { format!(". {then}") };
    format!("not {doing}: {}{then}", say(paths))
}

/// Refuse <doing> unless GitHub says PR #<number> (<url>) is still open: one
/// that merged or closed while <step> ran stays as it is (on merge, a push
/// would recreate the head branch GitHub deleted). A read that fails refuses
/// too; the next run retries. <left> says what the step leaves behind ("the
/// commit is kept in <wt>").
pub fn pr_gate(number: &str, url: &str, step: &str, doing: &str, left: &str) {
    let pr = if url.is_empty() { number } else { url };
    let o = proc::run("gh", &["pr", "view", pr, "--json", "state", "--jq", ".state"], None, None);
    let read = if o.ok { Ok(o.stdout.trim().to_string()) } else { Err(o.stderr.trim().replace('\n', " ")) };
    if let Some(why) = pr_refusal(number, step, doing, left, read) {
        die(why);
    }
}

fn pr_refusal(number: &str, step: &str, doing: &str, left: &str, read: Result<String, String>) -> Option<String> {
    let kept = format!("not {doing}; {left}");
    let unread = |why: &str| format!("can't read PR #{number}'s state from GitHub ({why}): {kept}. The next run retries");
    match read.as_deref() {
        Ok("OPEN") => None,
        Ok(s @ ("MERGED" | "CLOSED")) => Some(format!("PR #{number} {} during {step}: {kept}", s.to_lowercase())),
        Ok(s) => Some(unread(&format!("it answered \"{s}\""))),
        Err(e) if e.is_empty() => Some(unread("gh failed")),
        Err(e) => Some(unread(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::Repo;

    fn strings(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    /// <p> := <text> in the repo, committed.
    fn commit(r: &Repo, p: &str, text: &str) {
        let f = std::path::Path::new(&r.dir).join(p);
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(f, text).unwrap();
        r.git(&["add", p]);
        r.git(&["commit", "-qm", p]);
    }

    // Was lib/sheepdog.bats: "protected_paths: the default list, with no setting and no .sheepdog/protected-paths; the list's file is protected too"
    #[test]
    fn protected_paths_the_default_list_with_no_setting_and_no_file_the_lists_file_is_protected_too() {
        let r = Repo::new();
        assert_eq!(
            paths_in(&r.dir, None, "HEAD"),
            strings(&[
                ".github/workflows/", ".github/actions/", ".rwx/", ".captain/", ".circleci/", ".gitlab-ci.yml", ".buildkite/",
                ".sheepdog/protected-paths"
            ])
        );
    }

    // Was lib/sheepdog.bats: "protected_paths: .sheepdog/protected-paths in the base commit, comments and blanks dropped; the branch's own copy ignored; the file itself always protected"
    #[test]
    fn protected_paths_the_file_in_the_base_commit_the_branchs_own_copy_ignored_the_file_itself_always_protected() {
        let r = Repo::new();
        commit(&r, ".sheepdog/protected-paths", "# ours\n.github/\n\n  deploy/pipeline.yml  # the deploy\n");
        let base = r.git(&["rev-parse", "HEAD"]);
        commit(&r, ".sheepdog/protected-paths", "nothing\n");
        assert_eq!(paths_in(&r.dir, None, &base), strings(&[".github/", "deploy/pipeline.yml", ".sheepdog/protected-paths"]));
        commit(&r, ".sheepdog/protected-paths", "# none\n");
        assert_eq!(paths_in(&r.dir, None, "HEAD"), strings(&[".sheepdog/protected-paths"]));
        // No base: the default list.
        assert_eq!(paths_in(&r.dir, None, ""), paths_in(&r.dir, None, "HEAD~3"));
    }

    // Was lib/sheepdog.bats: "protected_paths: settings.protectedPaths wins, and an empty list protects nothing"
    // The setting as config::get_text reads a list: one per line.
    #[test]
    fn protected_paths_settings_protectedpaths_wins_and_an_empty_list_protects_nothing() {
        let r = Repo::new();
        commit(&r, ".sheepdog/protected-paths", ".github/\n");
        assert_eq!(paths_in(&r.dir, Some("ci/\nMakefile"), "HEAD"), strings(&["ci/", "Makefile"]));
        assert!(paths_in(&r.dir, Some(""), "HEAD").is_empty());
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
    fn refusal_adds_then_only_when_there_is_one() {
        let paths = strings(&[".rwx/ci.yml"]);
        assert_eq!(refusal("shipping b", &paths, ""), format!("not shipping b: {}", say(&paths)));
        assert_eq!(refusal("pushing b", &paths, "Then this"), format!("not pushing b: {}. Then this", say(&paths)));
    }

    #[test]
    fn pr_refusal_only_an_open_pr_goes_on_and_an_unread_one_stops_270() {
        let r = |read| pr_refusal("7", "address", "pushing or posting", "the commit is kept in /wt", read);
        assert_eq!(r(Ok("OPEN".into())), None);
        assert_eq!(
            r(Ok("MERGED".into())).unwrap(),
            "PR #7 merged during address: not pushing or posting; the commit is kept in /wt"
        );
        assert_eq!(r(Ok("CLOSED".into())).unwrap(), "PR #7 closed during address: not pushing or posting; the commit is kept in /wt");
        assert_eq!(
            r(Err("HTTP 502".into())).unwrap(),
            "can't read PR #7's state from GitHub (HTTP 502): not pushing or posting; the commit is kept in /wt. The next run retries"
        );
        assert_eq!(
            r(Err("".into())).unwrap(),
            "can't read PR #7's state from GitHub (gh failed): not pushing or posting; the commit is kept in /wt. The next run retries"
        );
        assert_eq!(
            r(Ok("".into())).unwrap(),
            "can't read PR #7's state from GitHub (it answered \"\"): not pushing or posting; the commit is kept in /wt. The next run retries"
        );
    }

    #[test]
    fn say_names_the_paths_and_the_way_out() {
        assert_eq!(
            say(&strings(&[".circleci/config.yml", ".rwx/tests.yml"])),
            "the agent changed CI config (.circleci/config.yml, .rwx/tests.yml): review, then sd act push"
        );
    }
}
