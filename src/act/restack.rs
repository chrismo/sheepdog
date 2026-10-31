// restack (A2), its --resolve (J1) in resolve.rs, and retarget (A4). One
// difference from the bash: a failed git status in the worktree stops the
// restack, where the bash read it as a clean tree.
//
// Field reads follow the SuperDB gates this replaced (src/val.rs): a missing
// field reads as null, and an equality holds only between two present values.

use super::commands::verify_run;
use super::hold::hold_after;
use super::resolve::{resolve_conflicts, Halt, Rebase};
use super::step::{question_gate, resolve_gave_up};
use super::{die, no_base_why, plural, state_json, Args};
use crate::git::{count, git, git_input, git_out, is_ancestor, patch_id, rev, show_ref, worktree_for};
use crate::{proc, running};
use crate::val::{at, sj, t, text};
use serde_json::Value;
use std::process::Command;

#[derive(Debug, PartialEq)]
enum Gate {
    Hard(String),
    Done(String),
    Soft(String),
    Go,
}

fn short(s: &str, n: usize) -> &str {
    s.get(..n).unwrap_or(s)
}

fn eq(s: &Value, a: &str, b: &str) -> bool {
    !at(s, a).is_null() && at(s, a) == at(s, b)
}

fn ne(s: &Value, a: &str, b: &str) -> bool {
    !at(s, a).is_null() && !at(s, b).is_null() && at(s, a) != at(s, b)
}

fn is(s: &Value, p: &str, lit: &str) -> bool {
    at(s, p).as_str() == Some(lit)
}

/// RESTACK_GATE. A predicted conflict is refused, unless <resolve> hands it
/// to the agent.
fn restack_gate(s: &Value, remote: &str, resolve: bool) -> Gate {
    let on = |p: &str| crate::val::on(s, p);
    let nul = |p: &str| at(s, p).is_null();
    let (b, parent) = (t(s, "branch"), t(s, "parent.name"));
    let onto = format!("{remote}/{}", t(s, "sync.target"));
    let why = no_base_why(s, remote);
    let (old_base, head) = (t(s, "sync.old_base"), t(s, "head"));
    let (old_base, head) = (short(&old_base, 12), short(&head, 12));

    if is(s, "parent.pr.state", "CLOSED") && ne(s, "parent.name", "sync.default") {
        return Gate::Hard(format!(
            "{parent}'s PR #{} was closed without merging; decide what {b} targets",
            t(s, "parent.pr.number")
        ));
    }
    if nul("parent.merged") && ne(s, "parent.name", "sync.default") {
        return Gate::Hard(format!("can't tell whether {parent} merged: {why}"));
    }
    let behind = at(s, "sync.up_to_date") == &Value::Bool(false);
    if nul("sync.up_to_date") || (behind && nul("sync.old_base")) {
        return Gate::Hard(format!("can't restack {b}: {why}"));
    }
    if on("sync.up_to_date") {
        return Gate::Done(format!("{b} is already on {onto}"));
    }
    if on("parent.merged") {
        let Some(from_parent) = at(s, "sync.replays_parent").as_array() else {
            return Gate::Hard(format!("can't list {}..{b}", t(s, "sync.old_base")));
        };
        if let Some(newest) = from_parent.first().filter(|_| !on("sync.by_tree")) {
            return Gate::Hard(format!(
                "wrong base: this would replay {} of #{}'s commits onto {onto}; if the stack is linear, rebase past them by hand: \
                 git rebase --onto {onto} {} {b} && git update-ref refs/sheepdog/{b}/base {onto}",
                from_parent.len(),
                t(s, "parent.pr.number"),
                short(&text(newest), 12)
            ));
        }
    }
    if nul("sync.conflict") {
        return Gate::Hard(format!("can't tell whether restacking {b} would conflict: {why}"));
    }
    if on("sync.by_tree") && on("sync.conflict") {
        return Gate::Hard(format!(
            "restacking {b} onto {onto} by tree would conflict; --resolve can't resolve a tree restack. \
             By hand: git switch {b} && git reset --hard {onto} && git diff --binary {old_base} {head} | git apply -3, \
             resolve and commit, then git update-ref refs/sheepdog/{b}/base {onto} (the old head stays at {head})"
        ));
    }
    if on("sync.conflict") && !resolve {
        return Gate::Hard(format!(
            "restacking {b} onto {onto} would conflict; resolve it by hand: git rebase --onto {onto} {old_base} {b}, \
             then git update-ref refs/sheepdog/{b}/base $(git merge-base {onto} {b}); or let the agent: sd act restack --resolve"
        ));
    }
    if on("session.locked") {
        return Gate::Soft(format!("taken over (branch.{b}.sd-lock is set)"));
    }
    Gate::Go
}

/// KEEP_PARENT: why sd-parent stays on a merged parent, else None. Until the
/// PR targets the default branch, sd-parent is what keeps retarget owed:
/// deleting the merged parent's branch would close a PR still on it.
fn keep_parent(s: &Value) -> Option<String> {
    if !crate::val::on(s, "parent.merged") {
        return None;
    }
    let b = t(s, "branch");
    if at(s, "pr").is_null() {
        let gh = format!("gh ({b}):");
        let unread = at(s, "errors").as_array().is_some_and(|e| e.iter().any(|e| e.as_str().is_some_and(|e| e.starts_with(&gh))));
        return unread.then(|| format!("sd-parent of {b} stays {}: can't read its PR", t(s, "parent.name")));
    }
    (is(s, "pr.state", "OPEN") && ne(s, "pr.base", "sync.default"))
        .then(|| format!("PR #{} still targets {}: sd act retarget", t(s, "pr.number"), t(s, "pr.base")))
}

/// The default branch, once the branch is on it: a sd-parent naming the
/// merged parent then only makes every later pass look up a dead PR.
fn onto_default(s: &Value) -> Option<String> {
    eq(s, "sync.target", "sync.default").then(|| t(s, "sync.default"))
}

pub(super) fn parent_to_default(b: &str, s: &Value) {
    let Some(default) = onto_default(s) else { return };
    let key = format!("branch.{b}.sd-parent");
    if git_out(".", &["config", "--get", &key]).unwrap_or_default().is_empty() {
        return;
    }
    must(git(".", &["config", &key, &default]));
    println!("sd-parent of {b} is now {default}");
}

/// A git step the bash ran under set -e: its failure ends the run with git's
/// own message and status.
fn must(o: proc::Out) {
    if !o.ok {
        eprint!("{}", o.stderr);
        std::process::exit(o.code);
    }
}

pub(super) fn state(dir: &str, b: &str) -> Value {
    state_json(dir, b).unwrap_or_else(|e| {
        eprint!("{e}");
        die("sd state failed")
    })
}

pub fn restack(b: &str, remote: &str, a: &Args) {
    let wt = worktree_for(".", b);
    if wt.is_empty() {
        die(format!("{b} isn't checked out in any worktree"));
    }
    let st = git(&wt, &["status", "--porcelain"]);
    if !st.ok {
        die(format!("git status failed in {wt}: {}", st.stderr.trim()));
    }
    if !st.stdout.is_empty() {
        die(format!("{wt} has uncommitted changes"));
    }
    let f = git(&wt, &["fetch", "--quiet", "--prune", remote]);
    eprint!("{}", f.stderr);
    if !f.ok {
        die(format!("fetch from {remote} failed"));
    }
    let s = state(&wt, b);
    let old_base = t(&s, "sync.old_base");
    match restack_gate(&s, remote, a.resolve) {
        Gate::Hard(why) => die(why),
        Gate::Done(line) => {
            println!("{line}");
            return;
        }
        Gate::Soft(why) if !a.force => {
            eprintln!("sd act: not restacking {b}:\n  - {why}\n(--force to run anyway)");
            std::process::exit(1)
        }
        _ => {}
    }
    if a.resolve {
        question_gate(b, "resolve", a.force, false);
    }

    let target = t(&s, "sync.target");
    let onto = format!("{remote}/{target}");
    let new_base = rev(".", &format!("refs/remotes/{onto}"));
    if new_base.is_empty() {
        die(format!("no {onto} ref"));
    }
    let old_head = rev(".", &format!("refs/heads/{b}"));
    let before = patch_id(".", &old_base, &old_head);
    let by_tree = crate::val::on(&s, "sync.by_tree");
    let mut resolved = false;
    if by_tree {
        restack_by_tree(&wt, b, &old_base, &old_head, &new_base, &onto);
    } else if !git(&wt, &["rebase", "--quiet", "--onto", &new_base, &old_base, b]).ok {
        if !a.resolve {
            rollback(&wt, b, &old_head);
            die(format!(
                "the rebase onto {onto} stopped on a conflict partway through; rolled back to {}. \
                 Resolve it by hand: git rebase --onto {} {} {b}, then git update-ref refs/sheepdog/{b}/base {}; \
                 or let the agent: sd act restack --resolve",
                short(&old_head, 7),
                short(&new_base, 12),
                short(&old_base, 12),
                short(&new_base, 12)
            ));
        }
        let r = Rebase { b, wt: &wt, old_base: &old_base, old_head: &old_head, onto: &onto };
        // Marked running across every stop and continue, not only each agent
        // run (#170). By hand and from a pass, the marker is already held
        // (act::run, sd watch step); this holds it for any other caller.
        let run = || {
            resolve_conflicts(&r, &a.harness, a.force).inspect_err(|h| {
                if !h.keep {
                    rollback(&wt, b, &old_head)
                }
            })
        };
        match running::holding(b, "resolve", run) {
            None => {
                rollback(&wt, b, &old_head);
                std::process::exit(1)
            }
            Some(Err(Halt { why, gave_up: Some(text), .. })) => resolve_gave_up(b, &wt, &text, why, &onto, &new_base),
            Some(Err(Halt { why, .. })) => die(why),
            // Before anything below can die: the resolution is on the branch now.
            Some(Ok(touched)) => hold_after(b, &old_base, &touched),
        }
        resolved = true;
    }
    let new_head = rev(".", &format!("refs/heads/{b}"));
    // A resolution changes the branch's diff by definition, so the check only
    // applies to a clean rebase; a resolved one goes through verify and review.
    if !resolved && patch_id(".", &new_base, &new_head) != before {
        must(git(&wt, &["reset", "--quiet", "--hard", &old_head]));
        die(format!(
            "{b}'s change came out different on {onto} (part of it may be there already); rolled back to {}. \
             Compare: git range-diff {}..{} {}..{}",
            short(&old_head, 7),
            short(&old_base, 12),
            short(&old_head, 12),
            short(&new_base, 12),
            short(&new_head, 12)
        ));
    }
    must(git(".", &["update-ref", "-m", "sd act restack", &format!("refs/sheepdog/{b}/base"), &new_base]));
    let mut how = "sd act push";
    let remote_b = format!("refs/remotes/{remote}/{b}");
    if show_ref(".", &remote_b) && !is_ancestor(".", &remote_b, &new_head) {
        how = "a plain git push is rejected now, and git's pull advice would undo the restack; use sd act push";
    }
    let n = count(".", &[&format!("{new_base}..{new_head}")]);
    let via = if by_tree { " by tree" } else { "" };
    println!("restacked {b} onto {onto}{via} ({}); not pushed: {how}", plural(n as usize, "commit"));
    if by_tree {
        println!("{b}'s commits are now one, committed by you; the old ones are at {}", short(&old_head, 7));
    }
    match keep_parent(&s) {
        Some(line) => println!("{line}"),
        None => parent_to_default(b, &s),
    }
    if resolved {
        println!("conflicts resolved by the agent, so {b}'s change is unreviewed");
        if let Err(cmd) = verify_run(&wt, b, "resolve") {
            die(format!(
                "verify failed ({cmd}) on the resolved branch; it is kept at {}, unpushed. Fix, then sd act verify",
                short(&new_head, 7)
            ));
        }
    }
}

/// <old_base>..<old_head>'s change as one commit on <new_base>, for an old
/// base that isn't an ancestor (sd state's sync.by_tree). The message leaves
/// out exact copies of the parent's commits: their change is on the target.
fn restack_by_tree(wt: &str, b: &str, old_base: &str, old_head: &str, new_base: &str, onto: &str) {
    let tree = git_out(".", &["merge-tree", "--write-tree", &format!("--merge-base={old_base}"), new_base, old_head])
        .unwrap_or_else(|| die(format!("restacking {b} onto {onto} by tree hit a conflict; nothing moved")));
    let log = git_out(".", &["log", "--reverse", "--format=%s", "--right-only", "--cherry-pick", &format!("{old_base}...{old_head}")])
        .unwrap_or_default();
    let msg = format!("{b} (restacked by tree onto {onto})\n\nReplaces:\n{log}\n");
    let c = git_input(".", &["commit-tree", &tree, "-p", new_base], Some(msg.as_bytes()));
    if !c.ok {
        eprint!("{}", c.stderr);
        die("git commit-tree failed; nothing moved");
    }
    let new = crate::git::strip_nl(c.stdout);
    // Not "reset: moving to" in the reflog: stacked_on counts this commit as the branch's own.
    let st = Command::new("git")
        .args(["-C", wt, "reset", "--quiet", "--hard", &new])
        .env("GIT_REFLOG_ACTION", "sd act restack by tree")
        .status();
    if !st.is_ok_and(|s| s.success()) {
        die(format!("git reset --hard {} failed in {wt}", short(&new, 7)));
    }
}

fn rollback(wt: &str, b: &str, old_head: &str) {
    git(wt, &["rebase", "--abort"]);
    if rev(".", &format!("refs/heads/{b}")) != old_head {
        must(git(wt, &["reset", "--quiet", "--hard", old_head]));
    }
}

fn retarget_gate(s: &Value) -> Gate {
    let nul = |p: &str| at(s, p).is_null();
    let why = sj(at(s, "errors"), "; ");
    let (b, parent, default) = (t(s, "branch"), t(s, "parent.name"), t(s, "sync.default"));
    let number = t(s, "pr.number");
    if nul("pr") {
        return Gate::Hard(format!("no PR for {b}{}", if why.is_empty() { String::new() } else { format!(": {why}") }));
    }
    if !nul("pr.state") && !is(s, "pr.state", "OPEN") {
        return Gate::Hard(format!("PR #{number} is {}", t(s, "pr.state").to_lowercase()));
    }
    if eq(s, "parent.name", "sync.default") && eq(s, "pr.base", "sync.default") {
        return Gate::Done(format!("PR #{number} already targets {default}"));
    }
    if is(s, "parent.pr.state", "CLOSED") {
        return Gate::Hard(format!(
            "{parent}'s PR #{} was closed without merging; decide what {b} targets",
            t(s, "parent.pr.number")
        ));
    }
    if nul("parent.merged") {
        return Gate::Hard(format!("can't tell whether {parent} merged: {why}"));
    }
    if at(s, "parent.merged") == &Value::Bool(false) {
        return Gate::Hard(format!("{parent} hasn't merged; PR #{number} stays on it"));
    }
    if eq(s, "pr.base", "sync.default") {
        return Gate::Done(format!("PR #{number} already targets {default} (GitHub retargeted it)"));
    }
    Gate::Go
}

pub fn retarget(b: &str) {
    let s = state(".", b);
    match retarget_gate(&s) {
        Gate::Hard(why) => die(why),
        Gate::Done(line) => println!("{line}"),
        _ => {
            let (number, default) = (t(&s, "pr.number"), t(&s, "sync.default"));
            // GH_REPO, which act::run set, points gh at settings.remote's repo.
            let o = proc::run("gh", &["pr", "edit", &number, "--base", &default], None, None);
            eprint!("{}", o.stderr);
            if !o.ok {
                die(format!("gh pr edit --base {default} failed for PR #{number}"));
            }
            println!("PR #{number} now targets {default} (was {})", t(&s, "pr.base"));
        }
    }
    parent_to_default(b, &s);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// child, stacked on A, behind main; A merged as PR #1.
    fn behind() -> Value {
        json!({
            "branch": "child", "head": "ccccccccccccccccc", "errors": [],
            "parent": {"name": "A", "merged": true, "pr": {"number": 1, "state": "MERGED", "commits": ["a1a1a1a1a1a1a1a1", "a2a2a2a2a2a2a2a2"]}},
            "sync": {"target": "main", "default": "main", "up_to_date": false, "old_base": "bbbbbbbbbbbbbbbbb", "by_tree": false, "conflict": false,
                     "replays_parent": []},
            "pr": {"number": 2, "state": "OPEN", "base": "A"},
            "session": {"locked": false},
        })
    }

    fn with(mut s: Value, path: &str, v: Value) -> Value {
        let (parent, key) = path.rsplit_once('.').map_or(("", path), |(p, k)| (p, k));
        let mut cur = &mut s;
        for k in parent.split('.').filter(|k| !k.is_empty()) {
            cur = &mut cur[k];
        }
        cur[key] = v;
        s
    }

    fn hard(g: Gate) -> String {
        match g {
            Gate::Hard(why) => why,
            other => panic!("not hard: {other:?}"),
        }
    }

    #[test]
    fn restack_gate_goes_when_behind_with_a_base_and_no_conflict() {
        assert_eq!(restack_gate(&behind(), "origin", false), Gate::Go);
    }

    #[test]
    fn restack_gate_is_done_when_up_to_date() {
        let s = with(behind(), "sync.up_to_date", json!(true));
        assert_eq!(restack_gate(&s, "origin", false), Gate::Done("child is already on origin/main".into()));
    }

    #[test]
    fn restack_gate_refuses_a_parent_closed_without_merging_unless_its_the_default() {
        let s = with(behind(), "parent.pr.state", json!("CLOSED"));
        assert_eq!(hard(restack_gate(&s, "origin", false)), "A's PR #1 was closed without merging; decide what child targets");
        let s = with(with(s, "parent.name", json!("main")), "sync.up_to_date", json!(true));
        assert!(matches!(restack_gate(&s, "origin", false), Gate::Done(_)));
    }

    #[test]
    fn restack_gate_refuses_an_unknown_merge_with_sd_states_reason() {
        let s = with(with(behind(), "parent.merged", Value::Null), "errors", json!(["gh (A): boom"]));
        assert_eq!(hard(restack_gate(&s, "origin", false)), "can't tell whether A merged: gh (A): boom");
    }

    #[test]
    fn restack_gate_refuses_no_base_with_the_step_for_an_unpushed_parent() {
        let s = with(with(behind(), "sync.old_base", Value::Null), "parent.unpushed", json!(true));
        assert_eq!(hard(restack_gate(&s, "up", false)), "can't restack child: parent A isn't on up yet: ship it first (sd act ship A)");
        let s = with(behind(), "sync.up_to_date", Value::Null);
        assert_eq!(hard(restack_gate(&s, "origin", false)), "can't restack child: ");
    }

    #[test]
    fn restack_gate_refuses_replaying_a_merged_parents_commits_newest_first() {
        let s = with(behind(), "sync.replays_parent", json!(["a2a2a2a2a2a2a2a2", "a1a1a1a1a1a1a1a1"]));
        assert_eq!(
            hard(restack_gate(&s, "origin", false)),
            "wrong base: this would replay 2 of #1's commits onto origin/main; if the stack is linear, rebase past them by hand: \
             git rebase --onto origin/main a2a2a2a2a2a2 child && git update-ref refs/sheepdog/child/base origin/main"
        );
        // A tree restack leaves the parent's commits out by itself.
        let s = with(s, "sync.by_tree", json!(true));
        assert_eq!(restack_gate(&s, "origin", false), Gate::Go);
    }

    #[test]
    fn restack_gate_refuses_a_replay_sd_state_couldnt_list_after_a_merged_parent() {
        let s = with(behind(), "sync.replays_parent", Value::Null);
        assert_eq!(hard(restack_gate(&s, "origin", false)), "can't list bbbbbbbbbbbbbbbbb..child");
        let s = with(with(s, "parent.merged", json!(false)), "parent.pr.state", json!("OPEN"));
        assert_eq!(restack_gate(&s, "origin", false), Gate::Go);
    }

    #[test]
    fn restack_gate_refuses_a_conflict_and_says_how_by_hand() {
        let s = with(behind(), "sync.conflict", json!(true));
        assert_eq!(
            hard(restack_gate(&s, "origin", false)),
            "restacking child onto origin/main would conflict; resolve it by hand: git rebase --onto origin/main bbbbbbbbbbbb child, \
             then git update-ref refs/sheepdog/child/base $(git merge-base origin/main child); or let the agent: sd act restack --resolve"
        );
        let s = with(s, "sync.by_tree", json!(true));
        assert!(hard(restack_gate(&s, "origin", false)).starts_with("restacking child onto origin/main by tree would conflict; --resolve can't"));
        let s = with(behind(), "sync.conflict", Value::Null);
        assert_eq!(hard(restack_gate(&s, "origin", false)), "can't tell whether restacking child would conflict: ");
    }

    #[test]
    fn restack_gate_with_resolve_hands_a_conflict_to_the_agent_but_not_a_tree_restacks() {
        let s = with(behind(), "sync.conflict", json!(true));
        assert_eq!(restack_gate(&s, "origin", true), Gate::Go);
        let s = with(with(s, "session.locked", json!(true)), "sync.by_tree", json!(true));
        assert!(hard(restack_gate(&s, "origin", true)).contains("--resolve can't resolve a tree restack"));
        let s = with(behind(), "sync.conflict", Value::Null);
        assert!(matches!(restack_gate(&s, "origin", true), Gate::Hard(_)));
    }

    #[test]
    fn restack_gate_is_soft_when_taken_over_and_hard_refusals_come_first() {
        let s = with(behind(), "session.locked", json!(true));
        assert_eq!(restack_gate(&s, "origin", false), Gate::Soft("taken over (branch.child.sd-lock is set)".into()));
        let s = with(s, "sync.conflict", json!(true));
        assert!(matches!(restack_gate(&s, "origin", false), Gate::Hard(_)));
    }

    #[test]
    fn keep_parent_while_the_pr_still_targets_the_merged_parent() {
        assert_eq!(keep_parent(&behind()).as_deref(), Some("PR #2 still targets A: sd act retarget"));
        assert_eq!(keep_parent(&with(behind(), "pr.base", json!("main"))), None);
        assert_eq!(keep_parent(&with(behind(), "pr.state", json!("CLOSED"))), None);
        assert_eq!(keep_parent(&with(behind(), "parent.merged", json!(false))), None);
    }

    #[test]
    fn keep_parent_when_the_branchs_own_pr_cant_be_read() {
        let s = with(with(behind(), "pr", Value::Null), "errors", json!(["gh (child): boom"]));
        assert_eq!(keep_parent(&s).as_deref(), Some("sd-parent of child stays A: can't read its PR"));
        // Only the branch's own lookup: no PR at all, or the parent's failing, isn't it.
        let s = with(s, "errors", json!(["gh (A): boom"]));
        assert_eq!(keep_parent(&s), None);
    }

    #[test]
    fn onto_default_only_when_the_target_is_the_default_branch() {
        assert_eq!(onto_default(&behind()).as_deref(), Some("main"));
        assert_eq!(onto_default(&with(behind(), "sync.target", json!("A"))), None);
        assert_eq!(onto_default(&with(with(behind(), "sync.target", Value::Null), "sync.default", Value::Null)), None);
    }

    #[test]
    fn retarget_gate_edits_a_pr_still_on_the_merged_parent() {
        assert_eq!(retarget_gate(&behind()), Gate::Go);
    }

    #[test]
    fn retarget_gate_is_done_when_the_pr_already_targets_the_default() {
        let s = with(behind(), "pr.base", json!("main"));
        assert_eq!(retarget_gate(&s), Gate::Done("PR #2 already targets main (GitHub retargeted it)".into()));
        let s = with(s, "parent.name", json!("main"));
        assert_eq!(retarget_gate(&s), Gate::Done("PR #2 already targets main".into()));
    }

    #[test]
    fn retarget_gate_refusals() {
        let s = with(with(behind(), "pr", Value::Null), "errors", json!(["gh (child): boom"]));
        assert_eq!(hard(retarget_gate(&s)), "no PR for child: gh (child): boom");
        assert_eq!(hard(retarget_gate(&with(behind(), "pr", Value::Null))), "no PR for child");
        assert_eq!(hard(retarget_gate(&with(behind(), "pr.state", json!("MERGED")))), "PR #2 is merged");
        let s = with(behind(), "parent.pr.state", json!("CLOSED"));
        assert_eq!(hard(retarget_gate(&s)), "A's PR #1 was closed without merging; decide what child targets");
        assert_eq!(hard(retarget_gate(&with(behind(), "parent.merged", Value::Null))), "can't tell whether A merged: ");
        assert_eq!(hard(retarget_gate(&with(behind(), "parent.merged", json!(false)))), "A hasn't merged; PR #2 stays on it");
    }
}
