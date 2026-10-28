// enroll, unenroll, forget and unregister: what sd watch and sd state keep
// for a branch and a repo.

use super::{die, no_base_why, state_json};
use crate::git::{self, git, git_input, git_ok, git_out, is_ancestor, main_worktree, rev, stacked_on, stacked_unclear, worktree_for};
use crate::proc::Out;
use crate::running;
use crate::val::{at, sj, t};
use serde_json::Value;
use std::path::Path;

/// A git write that must land: on failure, git's error and its exit status,
/// as set -e had it.
fn must(o: Out) -> Out {
    if !o.ok {
        eprint!("{}", o.stderr);
        std::process::exit(o.code);
    }
    o
}

/// git config --unset(-all), where 5 (it wasn't set) is fine and a real
/// config error exits 1, as bash's || (( $? == 5 )) did.
fn unset(args: &[&str]) {
    let o = git(".", args);
    if !o.ok && o.code != 5 {
        eprint!("{}", o.stderr);
        std::process::exit(1);
    }
}

/// A lineage write: Err with git's error, which enroll reports and goes on.
fn write(args: &[&str]) -> Result<(), String> {
    let o = git(".", args);
    if o.ok { Ok(()) } else { Err(o.stderr.trim_end().to_string()) }
}

fn short(sha: &str) -> &str {
    sha.get(..7).unwrap_or(sha)
}

pub fn enroll(b: &str, remote: &str) {
    enroll_mark(b);
    println!("enrolled {b}");
    let Ok(state) = state_json(".", b) else { return };
    if let Err(e) = record_lineage(b, &state, remote) {
        eprintln!("sd act: {e}");
    }
}

/// Mark <b> for sd watch and register this repo; the caller says so (start
/// on stderr, its stdout being only the path).
pub fn enroll_mark(b: &str) {
    if !git::branch_exists(b) {
        die(format!("no such branch: {b}"));
    }
    must(git(".", &["config", &format!("branch.{b}.sheepdog"), "true"]));
    let main = main_worktree(".");
    let repos = git_out(".", &["config", "--global", "--get-all", "sheepdog.repo"]).unwrap_or_default();
    if !repos.lines().any(|r| r == main) {
        must(git(".", &["config", "--global", "--add", "sheepdog.repo", &main]));
    }
}

/// What sd state's record says to store as a branch's lineage.
#[derive(Debug, PartialEq)]
enum Lineage {
    /// The PR's base names a stacked parent; old_base is empty when sd state
    /// found no base.
    Pr { parent: String, old_base: String },
    /// No PR and no recorded parent: maybe a plain git branch from another's
    /// tip (#94), looked for off this default branch.
    Infer(String),
    None,
}

fn lineage(state: &Value) -> Lineage {
    let (source, parent, def) = (t(state, "parent.source"), t(state, "parent.name"), t(state, "sync.default"));
    if source == "pr" && !parent.is_empty() && !def.is_empty() && parent != def {
        return Lineage::Pr { parent, old_base: t(state, "sync.old_base") };
    }
    // threads.owed_bot is null when gh failed: then a PR may name the parent.
    if at(state, "pr").is_null() && source == "default" && !at(state, "threads.owed_bot").is_null() && !def.is_empty() {
        return Lineage::Infer(def);
    }
    Lineage::None
}

/// While the PR's base still names a stacked parent, store it and the base
/// pin; GitHub retargets the PR to the default branch on merge. With no PR
/// and no recorded parent, the branch it was made from by a plain git branch.
/// Err when git finds candidates but can't pick one (a tie, or a parent that
/// moved on), or when git won't write it. <state> is the caller's sd state
/// --json, so a second gh lookup can't disagree.
pub fn record_lineage(b: &str, state: &Value, remote: &str) -> Result<(), String> {
    let (parent, old_base) = match lineage(state) {
        Lineage::Pr { parent, old_base } => (parent, old_base),
        Lineage::Infer(def) => match stacked_on(".", b, remote, &def) {
            Ok(found) => found,
            Err((2, names)) => {
                return Err(format!("{b} looks stacked on {}: record one with git config branch.{b}.sd-parent <name>", names.replace(' ', " or ")))
            }
            Err((3, names)) => return Err(format!("{b} {}", stacked_unclear(b, &names, &def))),
            Err(_) => return Ok(()),
        },
        Lineage::None => return Ok(()),
    };
    write(&["config", &format!("branch.{b}.sd-parent"), &parent])?;
    println!("parent: {parent}");
    if old_base.is_empty() {
        let why = no_base_why(state, remote);
        let why = if why.is_empty() { why } else { format!(": {why}") };
        eprintln!("sd act: warning: no usable base for {b}{why}");
    } else {
        write(&["update-ref", &format!("refs/sheepdog/{b}/base"), &old_base])?;
        println!("base pinned at {}", short(&old_base));
    }
    Ok(())
}

pub fn unenroll(b: &str) {
    unset(&["config", "--unset", &format!("branch.{b}.sheepdog")]);
    if !git::branch_exists(b) {
        for f in [crate::config::branch_config_file(".", b), crate::config::describe_file(".", b)].into_iter().flatten() {
            let _ = std::fs::remove_file(f);
        }
    }
}

/// Die unless forget --remove may take <b> and its worktree: the PR merged
/// with the tip in it, no live agent step on it (#270), and a worktree that
/// isn't attached and has nothing uncommitted. The default branch, which a
/// main worktree is switched to.
fn remove_gate(b: &str) -> String {
    let state = state_json(".", b).unwrap_or_else(|_| die(format!("can't read {b}'s state: sd state --json {b} says why")));
    let (pr_state, number, head_sha) = (t(&state, "pr.state"), t(&state, "pr.number"), t(&state, "pr.head_sha"));
    let default = t(&state, "sync.default");
    if pr_state.is_empty() {
        die(format!("no PR found for {b}: can't tell that it merged ({})", sj(at(&state, "errors"), "; ")));
    }
    if pr_state != "MERGED" {
        die(format!("PR #{number} isn't merged (it's {}): not removing {b}", pr_state.to_lowercase()));
    }
    let tip = rev(".", &format!("refs/heads/{b}"));
    if tip != head_sha {
        let merged = short(&head_sha);
        if !git_ok(".", &["cat-file", "-e", &format!("{head_sha}^{{commit}}")]) {
            die(format!("PR #{number} merged at {merged}, which this clone doesn't have: git fetch, then sd act forget --remove"));
        }
        if !is_ancestor(".", &tip, &head_sha) {
            die(format!(
                "{b} is at {}, not in PR #{number} (merged at {merged}): it has commits the PR doesn't; not removing it",
                short(&tip)
            ));
        }
    }
    running::idle(".", b, "removing its worktree").unwrap_or_else(|e| die(e));
    let wt = worktree_for(".", b);
    if wt.is_empty() {
        return default;
    }
    if wt == main_worktree(".") && default.is_empty() {
        die("can't tell the default branch to switch the main worktree to");
    }
    if git_out(".", &["config", &format!("branch.{b}.sd-lock")]).is_some_and(|l| !l.is_empty()) {
        die(format!("{b} is attached (branch.{b}.sd-lock is set): not removing its worktree"));
    }
    // -uall: status.showUntrackedFiles=no hides them, and worktree remove then drops them
    let st = git(&wt, &["status", "--porcelain", "--untracked-files=all"]);
    if !st.ok {
        die(format!("can't read {wt}'s status"));
    }
    if !st.stdout.is_empty() {
        die(format!("{wt} has uncommitted changes: commit or remove them, then sd act forget --remove"));
    }
    default
}

/// Unenroll <b> and drop what sheepdog stores for it; with --remove, first
/// its worktree and the branch itself, so a removal git refuses (a locked
/// worktree) changes nothing. A branch already deleted is just forgotten.
pub fn forget(b: &str, remove: bool) {
    if remove && git::branch_exists(b) {
        let default = remove_gate(b);
        let (main, wt, tip) = (main_worktree("."), worktree_for(".", b), rev(".", &format!("refs/heads/{b}")));
        // The worktree we're in may be the one removed.
        std::env::set_current_dir(&main).unwrap_or_else(|e| die(format!("can't cd to {main}: {e}")));
        if wt == main {
            // Never removed: switched off the branch.
            must(git(".", &["switch", "-q", &default]));
            println!("switched {main} to {default}");
        } else if !wt.is_empty() {
            // worktree remove takes ignored files too (.env, build output): say which.
            let st = git(&wt, &["status", "--porcelain", "--ignored"]).stdout;
            let ignored: Vec<&str> = st.lines().filter_map(|l| l.strip_prefix("!! ")).collect();
            must(git(".", &["worktree", "remove", &wt]));
            let with = if ignored.is_empty() { String::new() } else { format!(" with its ignored files: {}", ignored.join(" ")) };
            println!("removed {wt}{with} (cd {main})");
        }
        must(git(".", &["branch", "-qD", b]));
        println!("deleted {b} (was {})", short(&tip));
    }
    unenroll(b);
    let pins = must(git(".", &["for-each-ref", "--format=delete %(refname)", &format!("refs/sheepdog/{b}/")])).stdout;
    must(git_input(".", &["update-ref", "--stdin"], Some(pins.as_bytes())));
    if let Some(f) = crate::config::describe_file(".", b) {
        let _ = std::fs::remove_file(f);
    }
    println!("forgot {b}");
}

/// <s> as an extended regex, the kind git config's value patterns are, that
/// matches only <s>.
fn ere_escape(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if r"\.[]*^$+?(){}|".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Take <path> (this repo's main worktree when empty) off the watcher's list,
/// unless a branch there is still enrolled; a path that's gone has none.
pub fn unregister(path: &str, force: bool) {
    let path = if path.is_empty() { main_worktree(".") } else { path.to_string() };
    if !force && Path::new(&path).is_dir() {
        let marks = git_out(&path, &["config", "--get-regexp", r"^branch\..*\.sheepdog$"]).unwrap_or_default();
        let enrolled: Vec<&str> = marks
            .lines()
            .filter_map(|l| l.strip_suffix(".sheepdog true")?.strip_prefix("branch."))
            .collect();
        if !enrolled.is_empty() {
            die(format!("not unregistering {path}: still enrolled: {} (sd act unenroll them, or --force)", enrolled.join(" ")));
        }
    }
    unset(&["config", "--global", "--unset-all", "sheepdog.repo", &format!("^{}$", ere_escape(&path))]);
    println!("unregistered {path}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn lineage_is_a_stacked_prs_base_and_its_old_base() {
        let s = json!({"pr": {"number": 2}, "parent": {"name": "A", "source": "pr"}, "sync": {"default": "main", "old_base": "abc"}});
        assert_eq!(lineage(&s), Lineage::Pr { parent: "A".into(), old_base: "abc".into() });
        let s = json!({"pr": {"number": 2}, "parent": {"name": "A", "source": "pr"}, "sync": {"default": "main", "old_base": null}});
        assert_eq!(lineage(&s), Lineage::Pr { parent: "A".into(), old_base: "".into() });
    }

    #[test]
    fn lineage_of_a_pr_on_the_default_branch_or_a_recorded_parent_is_none() {
        let s = json!({"pr": {"number": 2}, "parent": {"name": "main", "source": "pr"}, "sync": {"default": "main"},
                       "threads": {"owed_bot": 0}});
        assert_eq!(lineage(&s), Lineage::None);
        let s = json!({"pr": null, "parent": {"name": "A", "source": "config"}, "sync": {"default": "main"},
                       "threads": {"owed_bot": 0}});
        assert_eq!(lineage(&s), Lineage::None);
    }

    #[test]
    fn lineage_with_no_pr_is_inferred_off_the_default_branch_only_when_gh_answered() {
        let s = json!({"pr": null, "parent": {"name": "main", "source": "default"}, "sync": {"default": "main"},
                       "threads": {"owed_bot": 0}});
        assert_eq!(lineage(&s), Lineage::Infer("main".into()));
        let s = json!({"pr": null, "parent": {"name": "main", "source": "default"}, "sync": {"default": "main"},
                       "threads": {"owed_bot": null}});
        assert_eq!(lineage(&s), Lineage::None);
    }

    #[test]
    fn ere_escape_matches_only_the_path_itself() {
        assert_eq!(ere_escape("/r/a.b"), r"/r/a\.b");
        for p in ["/r/a.b", "/r/c++ (old)/[x]", r"/r/$^*?{1}|\"] {
            let re = regex::Regex::new(&format!("^{}$", ere_escape(p))).unwrap();
            assert!(re.is_match(p), "{p}");
        }
        let re = regex::Regex::new(&format!("^{}$", ere_escape("/r/a.b"))).unwrap();
        assert!(!re.is_match("/r/axb"));
        let re = regex::Regex::new(&format!("^{}$", ere_escape("/r/c+"))).unwrap();
        assert!(!re.is_match("/r/cc"));
    }
}
