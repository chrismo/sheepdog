// Each migrated verb's gate over Derived (#348). A verb joins MIGRATED in
// the PR that moves its guard here; parity runs on every tests/next.rs
// fixture and on generated states, whose fields are drawn independently on
// purpose: parity must hold on every state, not only those sd state makes.
// A Migrated's offered reads steps' key and who, which offer takes from
// Derived alone (tested below); cmd and say also read the record. Each gate
// destructures Derived without .., so a new field doesn't compile until
// every gate claims or ignores it.

use super::gate::{judge, reported, warnings, Gate, Level, Rule};
use crate::next::{no, yes, Derived};
use serde_json::Value;

/// A verb whose gate NEXT asks.
pub struct Migrated {
    pub verb: &'static str,
    /// Whether NEXT's steps offer the verb, by key and who only.
    pub offered: fn(&[Value]) -> bool,
    pub gate: fn(&Derived) -> Gate,
}

pub const MIGRATED: &[Migrated] = &[
    Migrated { verb: "push", offered: |steps| steps.iter().any(|s| s["key"] == "push"), gate: push },
    // NEXT lists address behind the steps that clear its gate (#326): only a first step runs
    Migrated { verb: "address", offered: |steps| steps.first().is_some_and(|s| s["key"] == "bot" && s["who"] == "agent"), gate: address },
    Migrated {
        verb: "implement",
        offered: |steps| steps.iter().any(|s| (s["key"] == "implement" || s["key"] == "review-fix") && s["who"] == "agent"),
        gate: implement,
    },
    Migrated { verb: "ci-fix", offered: |steps| steps.iter().any(|s| s["key"] == "ci-red"), gate: ci_fix },
];

/// sd act push. What's on the remote is push_branch's to judge, after its
/// own fetch: sd state doesn't fetch. Only sd act push asks this gate:
/// ship, address and ci-fix call push_branch directly, so FOREIGN_AUTHORS
/// (#356) doesn't hold their pushes yet (#386).
pub fn push(d: &Derived) -> Gate {
    let Derived {
        open, verify_red, undone, parent_unpushed, foreign_authors,
        unpushed: _, behind_remote: _, dropping: _,
        own_base: _, running: _, reviewer: _, pr_null: _, has_pr: _, merged: _, closed: _, errors: _, stale: _,
        lagging: _, locked: _, owed_human: _, owed_bot: _, bot_blocking: _, info: _, unposted: _, draft: _, review_required: _,
        merge_state: _, tasked: _, foreign: _, reset_away: _, taken: _, held: _,
        to_push: _, ci_held: _, push_dirty: _, empty: _, chase: _, up_to_date: _, behind_target: _, sync_conflict: _,
        conflict: _, chase_behind: _, gh_behind: _, gh_restack: _, by_tree: _, old_base_null: _, no_base: _,
        ahead_target: _, restack_due: _, stacked: _, parent_closed: _, parent_merged: _, ci_verdict: _, ci_blocked: _, ci_inconclusive: _,
        ci_pending: _, ci_for_head: _, draft_ready: _, any_cover: _, reviewed: _, need_review: _, fixing: _,
        findings_declined: _, fix_due: _, bot_due: _, bot_late: _, bot_waiting: _, budget_unread: _, over_budget: _,
        failed_out: _, asking: _, stored: _, addr_moot: _, ci_moot: _, resume: _, resume_ci: _, answer_pending: _, dirty: _, refused: _,
        bot: _, rv_yours: _, rv_dirty: _, rv_fetch: _, rv_current: _, rv_pending: _, rv_submitted: _, rv_local: _,
    } = d;
    // NEXT offers no push while one of these refuses: each is chrismo's
    let rule = |name, level, refuses: bool, clears| Rule { name, level, refuses, clears, yours: true };
    judge(&[
        // Hard, so a --force meant for another refusal can't publish the
        // copies; if the guess is wrong, git push by hand still works.
        rule("not pushing: the restack was undone: some of the target's commits are copies here, and pushing would publish them \
              (if they aren't, git push by hand)", Level::Hard, *undone, "redo the restack (sd state's NEXT says how)"),
        rule("the PR is merged or closed", Level::Soft, !open, "reopen it, or start a new branch"),
        rule("verify is red at HEAD", Level::Soft, *verify_red, "fix it, then sd act verify"),
        rule("the parent branch isn't on the remote yet", Level::Soft, *parent_unpushed, "ship it first (sd act ship <parent>)"),
        rule(FOREIGN_AUTHORS, Level::Soft, *foreign_authors && rewrites(d), "look at them, then sd act push --force"),
    ])
}

/// Whether sd act push replaces what's on the remote: a diverged push or a
/// drop of commits reset away here.
pub fn rewrites(d: &Derived) -> bool {
    (d.unpushed && d.behind_remote) || d.dropping
}

pub const FOREIGN_AUTHORS: &str =
    "the push would rewrite commits someone else made on the branch (sd state --json: sync.foreign_authors): look at them first";

/// address's reasons; address::said puts each in the record's words.
pub mod addr {
    pub const NO_PR: &str = "no PR";
    pub const CLOSED: &str = "the PR is merged or closed";
    pub const UNREAD: &str = "can't read the PR from GitHub";
    pub const NOTHING: &str = "nothing to address";
    pub const LOCKED: &str = "taken over (the branch's sd-lock is set)";
    pub const UNPOSTED: &str = "its last round's replies didn't post, and nothing new is owed";
    pub const CONFLICT: &str = "restack due, and it would conflict";
    pub const SYNC_UNKNOWN: &str = "sync with the target unknown";
    pub const DIVERGED: &str = "diverged from the remote branch";
    pub const REMOTE_MORE: &str = "the remote branch has commits you don't";
    pub const UNPUSHED: &str = "unpushed commits";
    pub const BEHIND: &str = "behind the target";
    pub const CI_RUNNING: &str = "CI is still running";
}

/// sd act address. Its push carries whatever else is unpushed, so the
/// remote must match first.
pub fn address(d: &Derived) -> Gate {
    judge(&address_rules(d))
}

/// address's Warn reasons, which a Soft verdict leaves out: --force still
/// prints them.
pub fn address_warnings(d: &Derived) -> Vec<String> {
    warnings(&address_rules(d))
}

fn address_rules(d: &Derived) -> [Rule; 11] {
    let Derived {
        pr_null, merged, closed, owed_bot, locked, sync_conflict, up_to_date, unpushed, behind_remote, ci_pending,
        unposted, own_base: _, running: _, reviewer: _, open: _, has_pr: _, errors: _, stale: _, lagging: _,
        owed_human: _, bot_blocking: _, info: _, draft: _, review_required: _, merge_state: _, tasked: _, verify_red: _, foreign: _, foreign_authors: _,
        reset_away: _, dropping: _, taken: _, held: _, to_push: _, ci_held: _, push_dirty: _, parent_unpushed: _,
        empty: _, chase: _, undone: _, behind_target: _, conflict: _, chase_behind: _, gh_behind: _, gh_restack: _,
        by_tree: _, old_base_null: _, no_base: _, ahead_target: _, restack_due: _, stacked: _, parent_closed: _, parent_merged: _,
        ci_verdict: _, ci_blocked: _, ci_inconclusive: _, ci_for_head: _, draft_ready: _, any_cover: _, reviewed: _,
        need_review: _, fixing: _, findings_declined: _, fix_due: _, bot_due: _, bot_late: _, bot_waiting: _,
        budget_unread: _, over_budget: _, failed_out: _, asking: _, stored: _, addr_moot: _, ci_moot: _,
        resume: _, resume_ci: _, answer_pending: _, dirty: _, refused: _, bot: _, rv_yours: _, rv_dirty: _,
        rv_fetch: _, rv_current: _, rv_pending: _, rv_submitted: _, rv_local: _,
    } = d;
    let conflict = yes(*sync_conflict);
    let push = match (unpushed, behind_remote) {
        (true, true) => Some(addr::DIVERGED),
        (false, true) => Some(addr::REMOTE_MORE),
        (true, false) => Some(addr::UNPUSHED),
        (false, false) => None,
    };
    // only ci-fix's NEXT reads yours (#355)
    let rule = |name, level, refuses: bool, clears| Rule { name, level, refuses, clears, yours: true };
    [
        rule(addr::NO_PR, Level::Hard, *pr_null, "sd act ship"),
        rule(addr::CLOSED, Level::Hard, *merged || *closed, "reopen it, or start a new branch"),
        rule(addr::UNREAD, Level::Hard, owed_bot.is_none(), "sd state's errors say why"),
        rule(addr::NOTHING, Level::Done, no(*owed_bot), "a bot item owed a reply"),
        rule(addr::LOCKED, Level::Soft, yes(*locked), "the branch's session exits"),
        rule(addr::CONFLICT, Level::Soft, conflict, "sd act restack --resolve"),
        rule(addr::SYNC_UNKNOWN, Level::Soft, !conflict && up_to_date.is_none(), "sd state's errors say why"),
        rule(addr::UNPOSTED, Level::Soft, *unposted, "a new bot item, or sd act address --force"),
        // last: address::yours drops it when a push clears it
        rule(push.unwrap_or(addr::UNPUSHED), Level::Soft, push.is_some(), "sd act push"),
        rule(addr::BEHIND, Level::Warn, no(*up_to_date) && !conflict, "sd act restack"),
        rule(addr::CI_RUNNING, Level::Warn, *ci_pending, "CI reports"),
    ]
}

/// Whether sd act push would clear address's push reason, when it has one:
/// commits of ours to push, or ones reset away here to drop.
pub fn address_pushes(d: &Derived) -> bool {
    (d.unpushed && !d.foreign) || d.dropping
}

/// implement's reasons; implement::worded puts each in the record's words.
pub mod imp {
    pub const NO_TASK: &str = "no task recorded";
    pub const CAPPED: &str = "the review findings had settings.reviewRounds rounds";
    pub const CLOSED: &str = "the PR is merged or closed";
    pub const ERRORS: &str = "sd state has errors, so the PR may be wrong";
    pub const AHEAD_UNKNOWN: &str = "how far ahead of the target is unknown";
    pub const AHEAD: &str = "nothing to implement: already ahead of the target";
    pub const REMOTE_MORE: &str = "the remote branch has commits you don't";
}

/// Whether sd act implement runs a review-fix round, not the task: review
/// findings stand at HEAD, uncovered, and the agent hasn't declined them all.
pub fn implement_fixes(d: &Derived) -> bool {
    d.fixing && !yes(d.findings_declined)
}

/// sd act implement: the task, before the branch's first commit (#241), or
/// a review-fix round below settings.reviewRounds. Its question and a dirty
/// worktree are step.rs's, as for every agent step.
pub fn implement(d: &Derived) -> Gate {
    let Derived {
        open, errors, tasked, behind_remote, ahead_target, fix_due, asking,
        fixing: _, findings_declined: _,
        own_base: _, running: _, reviewer: _, pr_null: _, has_pr: _, merged: _, closed: _, stale: _, lagging: _, locked: _,
        owed_human: _, owed_bot: _, bot_blocking: _, info: _, unposted: _, draft: _, review_required: _, merge_state: _,
        verify_red: _, foreign: _, foreign_authors: _, reset_away: _, dropping: _, taken: _, held: _, unpushed: _, to_push: _, ci_held: _,
        push_dirty: _, parent_unpushed: _, empty: _, chase: _, undone: _, up_to_date: _, behind_target: _, sync_conflict: _,
        conflict: _, chase_behind: _, gh_behind: _, gh_restack: _, by_tree: _, old_base_null: _, no_base: _,
        restack_due: _, stacked: _, parent_closed: _, parent_merged: _, ci_verdict: _, ci_blocked: _, ci_inconclusive: _,
        ci_pending: _, ci_for_head: _, draft_ready: _, any_cover: _, reviewed: _, need_review: _, bot_due: _, bot_late: _,
        bot_waiting: _, budget_unread: _, over_budget: _, failed_out: _, stored: _, addr_moot: _, ci_moot: _,
        resume: _, resume_ci: _, answer_pending: _, dirty: _, refused: _, bot: _, rv_yours: _, rv_dirty: _,
        rv_fetch: _, rv_current: _, rv_pending: _, rv_submitted: _, rv_local: _,
    } = d;
    let task = !implement_fixes(d);
    // with its own question standing, what's ahead is its work in progress
    let unasked = task && asking != "implement";
    // only ci-fix's NEXT reads yours (#355)
    let rule = |name, level, refuses: bool, clears| Rule { name, level, refuses, clears, yours: true };
    judge(&[
        rule(imp::NO_TASK, Level::Hard, task && !tasked, "git config branch.<b>.sd-task \"<task>\""),
        rule(imp::CAPPED, Level::Hard, !task && !fix_due, "raise settings.reviewRounds, fix them by hand, or sd act review --accept"),
        rule(imp::CLOSED, Level::Soft, !open, "reopen it, or start a new branch"),
        rule(imp::ERRORS, Level::Soft, task && *errors, "sd state --json says why"),
        rule(imp::AHEAD_UNKNOWN, Level::Soft, unasked && ahead_target.is_none(), "sd state's errors say why"),
        rule(imp::AHEAD, Level::Soft, unasked && yes(*ahead_target), "sd act implement --force"),
        rule(imp::REMOTE_MORE, Level::Soft, unasked && *behind_remote, "take or drop them, as sd state's NEXT says"),
    ])
}

/// sd act ci-fix: CI red at HEAD, with nothing to push first.
pub fn ci_fix(d: &Derived) -> Gate {
    judge(&ci_fix_rules(d))
}

/// ci-fix's rules. Done (a question is moot) once CI passed at HEAD or none
/// ran there, for_head null included (#355). Soft while CI may yet be red at
/// HEAD: an answer is stored meanwhile (#326), and --force runs anyway.
pub fn ci_fix_rules(d: &Derived) -> Vec<Rule> {
    let Derived {
        pr_null, has_pr, to_push, unpushed, ci_held, verify_red, ci_verdict, ci_for_head, undone, parent_unpushed,
        own_base: _, running: _, reviewer: _, open: _, merged: _, closed: _, errors: _, stale: _, lagging: _, locked: _,
        owed_human: _, owed_bot: _, bot_blocking: _, info: _, unposted: _, draft: _, review_required: _, merge_state: _, tasked: _,
        foreign: _, foreign_authors: _, behind_remote: _, reset_away: _, dropping: _, taken: _, held: _, push_dirty: _,
        empty: _, chase: _, up_to_date: _, behind_target: _, sync_conflict: _, conflict: _, chase_behind: _,
        gh_behind: _, gh_restack: _, by_tree: _, old_base_null: _, no_base: _, ahead_target: _, restack_due: _, stacked: _,
        parent_closed: _, parent_merged: _, ci_blocked: _, ci_inconclusive: _, ci_pending: _, draft_ready: _,
        any_cover: _, reviewed: _, need_review: _, fixing: _, findings_declined: _, fix_due: _, bot_due: _,
        bot_late: _, bot_waiting: _, budget_unread: _, over_budget: _, failed_out: _, asking: _, stored: _,
        addr_moot: _, ci_moot: _, resume: _, resume_ci: _,
        answer_pending: _, dirty: _, refused: _, bot: _, rv_yours: _, rv_dirty: _, rv_fetch: _, rv_current: _,
        rv_pending: _, rv_submitted: _, rv_local: _,
    } = d;
    let pushes = crate::next::push_step(d);
    let (v, at_head, settled) = (ci_verdict.as_str(), yes(*ci_for_head), !*to_push);
    let known = matches!(v, "red" | "green" | "none" | "pending" | "blocked" | "inconclusive");
    let rule = |name, level, refuses: bool, clears, yours| Rule { name, level, refuses, clears, yours };
    let wait = |name, refuses: bool, clears, yours| rule(name, Level::Soft, refuses, clears, yours);
    // to_push without unpushed is a drop: the remote's commits reset away here
    let (unoffered, drop) = (*unpushed && !pushes, *to_push && !unpushed);
    vec![
        rule("no PR", Level::Hard, *pr_null, "open one", true),
        rule("the PR is merged or closed; nothing to fix", Level::Hard, !pr_null && !has_pr, "reopen it", true),
        rule("CI is green at HEAD", Level::Done, v == "green" && at_head && settled, "", true),
        rule("no CI ran at HEAD", Level::Done, v == "none" && !no(*ci_for_head) && settled, "", true),
        wait("CI is pending", v == "pending", "wait for it", false),
        wait("CI is blocked", v == "blocked" && at_head, "start or approve its checks: gh pr checks", true),
        wait("CI was cancelled", v == "inconclusive" && at_head, "rerun it: gh run rerun, or rwx retry", true),
        wait("CI's verdict is unknown", !known, "sd state says why", true),
        wait("CI's red isn't for HEAD", v == "red" && !at_head, "push if there's anything to push, then wait for CI on HEAD", false),
        wait("CI is green, but not on HEAD", v == "green" && !at_head, "wait for CI on HEAD", false),
        wait("CI is blocked, but not on HEAD", v == "blocked" && !at_head, "wait for CI on HEAD", false),
        wait("CI was cancelled, but not on HEAD", v == "inconclusive" && !at_head, "wait for CI on HEAD", false),
        wait("no CI has reported on HEAD yet", v == "none" && no(*ci_for_head), "wait for CI on HEAD", false),
        wait("there are unpushed commits, which may be the fix already", *unpushed && pushes, "sd act push, then wait for CI on HEAD", false),
        wait("there are unpushed commits, held with the agent's CI config change", unoffered && *ci_held, "review it, then sd act push", true),
        wait("there are unpushed commits, held by the verify red on HEAD", unoffered && !ci_held && *verify_red, "fix it, then sd act verify", true),
        wait("there are unpushed commits NEXT won't push", unoffered && !ci_held && !verify_red && !undone && !parent_unpushed, "sd state says why", true),
        wait("the remote has commits reset away here, which a push drops", drop,
             if pushes { "sd act push, then wait for CI on HEAD" } else { "sd state says why" }, !pushes),
        // What push's gate refuses after the fix, asked before the agent runs.
        // Soft: the agent only costs tokens, and push's Hard undone rule
        // still keeps the copies off the remote.
        wait(CI_FIX_PUSH_WAITS[0], *undone, "redo the restack (sd state's NEXT says how)", true),
        wait(CI_FIX_PUSH_WAITS[1], *parent_unpushed, "ship it first (sd act ship <parent>)", true),
    ]
}

/// ci-fix's rules that wait on push's gate, not on CI: with only these
/// refusing, CI is red at HEAD.
const CI_FIX_PUSH_WAITS: [&str; 2] = ["the restack was undone, so a fix can't be pushed", "the parent branch isn't on the remote yet"];

/// The CI waits among ci-fix's reported <rules>: none when CI is red at HEAD.
pub fn ci_fix_ci_waits(rules: &[Rule]) -> Vec<&Rule> {
    reported(rules).into_iter().filter(|r| !CI_FIX_PUSH_WAITS.contains(&r.name)).collect()
}

/// What ci-fix waits for while its Soft <rules> refuse: it runs once <this>.
pub fn ci_fix_once(rules: &[Rule]) -> &'static str {
    let push = reported(rules).iter().any(|r| CI_FIX_PUSH_WAITS.contains(&r.name));
    match (ci_fix_ci_waits(rules).is_empty(), push) {
        (true, true) => "a fix can be pushed",
        (false, true) => "CI is red at HEAD and a fix can be pushed",
        _ => "CI is red at HEAD",
    }
}

/// Each verb NEXT's <steps> offer while its gate refuses (runs() false).
pub fn parity(d: &Derived, steps: &[Value]) -> Vec<String> {
    MIGRATED
        .iter()
        .filter(|m| (m.offered)(steps))
        .filter_map(|m| match (m.gate)(d) {
            g if g.runs() => None,
            g => Some(format!("NEXT offers {} while its gate says {g:?}", m.verb)),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::next::derived_arb::check;
    use serde_json::json;

    #[test]
    fn push_goes_on_an_open_pr_or_none_whatever_the_remote_says() {
        let open = Derived { open: true, ..Derived::default() };
        assert_eq!(push(&open), Gate::Go);
        assert_eq!(push(&Derived { foreign: true, to_push: false, ..open }), Gate::Go);
    }

    #[test]
    fn push_refuses_a_red_verify_an_unpushed_parent_and_a_closed_pr_unless_forced() {
        let d = Derived { open: true, parent_unpushed: true, verify_red: true, ..Derived::default() };
        assert_eq!(push(&d), Gate::Soft(vec!["verify is red at HEAD".into(), "the parent branch isn't on the remote yet".into()]));
        assert!(matches!(push(&Derived::default()), Gate::Soft(_)));
    }

    #[test]
    fn push_refuses_to_rewrite_commits_someone_else_made_on_the_branch_unless_forced_356() {
        let theirs = Derived { open: true, foreign_authors: true, ..Derived::default() };
        let diverged = Derived { unpushed: true, behind_remote: true, ..theirs.clone() };
        assert_eq!(push(&diverged), Gate::Soft(vec![FOREIGN_AUTHORS.into()]));
        let dropping = Derived { behind_remote: true, reset_away: true, dropping: true, ..theirs.clone() };
        assert_eq!(push(&dropping), Gate::Soft(vec![FOREIGN_AUTHORS.into()]));
        assert_eq!(push(&Derived { unpushed: true, ..theirs }), Gate::Go);
        assert_eq!(push(&Derived { foreign_authors: false, ..diverged }), Gate::Go);
    }

    #[test]
    fn push_never_rewrites_someone_elses_commits_unforced_and_holds_no_fast_forward_on_generated_states() {
        check("push over foreign authors", |d| match (d.foreign_authors && rewrites(d), push(d).runs()) {
            (true, true) => Err("push runs over their commits".into()),
            _ => Ok(()),
        });
        check("a fast-forward held for foreign authors", |d| {
            let held = push(d) != push(&Derived { foreign_authors: false, ..d.clone() });
            if held && !rewrites(d) { Err("a push that rewrites nothing is held".into()) } else { Ok(()) }
        });
    }

    #[test]
    fn push_refuses_an_undone_restack_even_forced() {
        let d = Derived { open: true, undone: true, verify_red: true, ..Derived::default() };
        assert!(matches!(push(&d), Gate::Hard(why) if why.starts_with("not pushing: the restack was undone")));
    }

    /// An open PR with bot items owed, in sync and pushed.
    fn owed() -> Derived {
        Derived { open: true, has_pr: true, owed_bot: Some(true), up_to_date: Some(true), sync_conflict: Some(false), ..Derived::default() }
    }

    fn soft(why: &[&str]) -> Gate {
        Gate::Soft(why.iter().map(|w| w.to_string()).collect())
    }

    #[test]
    fn address_goes_with_bot_items_owed_in_sync_and_pushed() {
        assert_eq!(address(&owed()), Gate::Go);
    }

    #[test]
    fn address_no_pr_a_closed_one_or_github_unread_is_hard_and_outranks_the_rest() {
        let d = Derived { pr_null: true, owed_bot: None, locked: Some(true), ..owed() };
        assert_eq!(address(&d), Gate::Hard(addr::NO_PR.into()));
        assert_eq!(address(&Derived { merged: true, ..owed() }), Gate::Hard(addr::CLOSED.into()));
        assert_eq!(address(&Derived { closed: true, ..owed() }), Gate::Hard(addr::CLOSED.into()));
        assert_eq!(address(&Derived { owed_bot: None, unpushed: true, ..owed() }), Gate::Hard(addr::UNREAD.into()));
    }

    #[test]
    fn address_with_nothing_owed_is_done_before_any_soft_reason() {
        assert_eq!(address(&Derived { owed_bot: Some(false), unpushed: true, ..owed() }), Gate::Done(addr::NOTHING.into()));
    }

    #[test]
    fn address_soft_lists_every_reason_in_order() {
        let d = Derived { locked: Some(true), sync_conflict: Some(true), unpushed: true, behind_remote: true, ..owed() };
        assert_eq!(address(&d), soft(&[addr::LOCKED, addr::CONFLICT, addr::DIVERGED]));
        assert_eq!(address(&Derived { up_to_date: None, ..owed() }), soft(&[addr::SYNC_UNKNOWN]));
        assert_eq!(address(&Derived { up_to_date: None, sync_conflict: Some(true), ..owed() }), soft(&[addr::CONFLICT]));
    }

    #[test]
    fn address_names_the_remote_relation_a_push_or_the_remote_must_settle() {
        assert_eq!(address(&Derived { unpushed: true, ..owed() }), soft(&[addr::UNPUSHED]));
        assert_eq!(address(&Derived { behind_remote: true, ..owed() }), soft(&[addr::REMOTE_MORE]));
        assert!(address_pushes(&Derived { unpushed: true, ..owed() }));
        assert!(!address_pushes(&Derived { unpushed: true, foreign: true, ..owed() }));
        assert!(address_pushes(&Derived { behind_remote: true, dropping: true, ..owed() }));
        assert!(!address_pushes(&Derived { behind_remote: true, ..owed() }));
    }

    #[test]
    fn address_holds_a_round_whose_replies_didnt_post_till_a_new_item_or_force_357() {
        assert_eq!(address(&Derived { unposted: true, ..owed() }), soft(&[addr::UNPOSTED]));
        assert_eq!(address(&Derived { unposted: true, unpushed: true, ..owed() }), soft(&[addr::UNPOSTED, addr::UNPUSHED]));
    }

    #[test]
    fn address_behind_with_no_conflict_and_ci_running_only_warn() {
        let d = Derived { up_to_date: Some(false), ci_pending: true, ..owed() };
        assert_eq!(address(&d), Gate::Warn(vec![addr::BEHIND.into(), addr::CI_RUNNING.into()]));
        assert_eq!(address(&Derived { up_to_date: Some(false), sync_conflict: Some(true), ..owed() }), soft(&[addr::CONFLICT]));
    }

    /// A branch with a task and nothing ahead of its target, no PR yet.
    fn fresh() -> Derived {
        Derived { open: true, pr_null: true, tasked: true, ahead_target: Some(false), ..Derived::default() }
    }

    /// Review findings standing at HEAD, below the round cap.
    fn findings() -> Derived {
        Derived { open: true, has_pr: true, fixing: true, findings_declined: Some(false), fix_due: true, ahead_target: Some(true), ..Derived::default() }
    }

    #[test]
    fn implement_goes_on_a_tasked_branch_with_nothing_ahead_and_a_fix_round_below_the_cap() {
        assert_eq!(implement(&fresh()), Gate::Go);
        assert_eq!(implement(&findings()), Gate::Go);
        assert!(implement_fixes(&findings()) && !implement_fixes(&fresh()));
    }

    #[test]
    fn implement_with_no_task_and_no_findings_to_fix_is_hard() {
        assert_eq!(implement(&Derived { tasked: false, ..fresh() }), Gate::Hard(imp::NO_TASK.into()));
        assert_eq!(implement(&Derived { tasked: false, ..findings() }), Gate::Go);
        let declined = Derived { tasked: false, findings_declined: Some(true), ..findings() };
        assert!(!implement_fixes(&declined));
        assert_eq!(implement(&declined), Gate::Hard(imp::NO_TASK.into()));
    }

    #[test]
    fn implement_refuses_a_branch_already_ahead_of_its_target_or_unknown_unless_forced_241() {
        assert_eq!(implement(&Derived { ahead_target: Some(true), ..fresh() }), soft(&[imp::AHEAD]));
        assert_eq!(implement(&Derived { ahead_target: None, ..fresh() }), soft(&[imp::AHEAD_UNKNOWN]));
        assert_eq!(implement(&Derived { behind_remote: true, ..fresh() }), soft(&[imp::REMOTE_MORE]));
    }

    #[test]
    fn implement_its_own_question_standing_makes_whats_ahead_its_work_in_progress() {
        let asked = Derived { asking: "implement".into(), ahead_target: Some(true), behind_remote: true, ..fresh() };
        assert_eq!(implement(&asked), Gate::Go);
        assert_eq!(implement(&Derived { asking: "address".into(), ..asked }), soft(&[imp::AHEAD, imp::REMOTE_MORE]));
    }

    #[test]
    fn implement_refuses_the_task_with_sd_state_errors_and_either_round_on_a_closed_pr() {
        assert_eq!(implement(&Derived { errors: true, ..fresh() }), soft(&[imp::ERRORS]));
        assert_eq!(implement(&Derived { errors: true, ..findings() }), Gate::Go);
        assert_eq!(implement(&Derived { open: false, merged: true, ..findings() }), soft(&[imp::CLOSED]));
        assert_eq!(implement(&Derived { open: false, closed: true, ahead_target: Some(true), ..fresh() }), soft(&[imp::CLOSED, imp::AHEAD]));
    }

    #[test]
    fn implement_a_fix_round_past_settings_review_rounds_is_hard_even_forced() {
        let capped = Derived { fix_due: false, ..findings() };
        assert_eq!(implement(&capped), Gate::Hard(imp::CAPPED.into()));
        assert_eq!(implement(&Derived { open: false, ..capped }), Gate::Hard(imp::CAPPED.into()));
    }

    #[test]
    fn implement_never_runs_a_fix_round_past_the_cap_or_the_task_untasked_on_generated_states() {
        check("implement past the cap or with no task", |d| {
            let runs = implement(d).runs();
            match (implement_fixes(d), runs) {
                (true, true) if !d.fix_due => Err("a fix round past the cap runs".into()),
                (false, true) if !d.tasked => Err("the task runs with none recorded".into()),
                (false, true) if d.ahead_target.is_none() && d.asking != "implement" => Err("the task runs with ahead unknown".into()),
                _ => Ok(()),
            }
        });
    }

    #[test]
    fn address_never_runs_unless_github_says_bot_items_are_owed_on_generated_states() {
        check("address on unknown or none", |d| if d.owed_bot != Some(true) && address(d).runs() { Err("address runs".into()) } else { Ok(()) });
    }

    #[test]
    fn push_never_runs_with_an_undone_restack_on_generated_states() {
        check("push with undone", |d| if d.undone && push(d).runs() { Err("push runs".into()) } else { Ok(()) });
    }

    /// An open PR with CI red at HEAD and nothing to push.
    fn red() -> Derived {
        Derived { has_pr: true, open: true, ci_verdict: "red".into(), ci_for_head: Some(true), ..Derived::default() }
    }

    /// The names and whether each is chrismo's, of what ci-fix's gate reports.
    fn ci_fix_said(d: &Derived) -> Vec<(&'static str, bool)> {
        crate::act::gate::reported(&ci_fix_rules(d)).iter().map(|r| (r.name, r.yours)).collect()
    }

    #[test]
    fn ci_fix_goes_on_red_at_head_with_nothing_to_push_and_refuses_hard_with_no_open_pr() {
        assert_eq!(ci_fix(&red()), Gate::Go);
        assert_eq!(ci_fix(&Derived { has_pr: false, pr_null: true, ..red() }), Gate::Hard("no PR".into()));
        assert!(matches!(ci_fix(&Derived { has_pr: false, merged: true, ..red() }), Gate::Hard(why) if why.contains("merged")));
    }

    #[test]
    fn ci_fix_is_done_once_ci_passed_or_none_ran_at_head_with_no_ci_commit_too_355() {
        assert_eq!(ci_fix(&Derived { ci_verdict: "green".into(), ..red() }), Gate::Done("CI is green at HEAD".into()));
        assert_eq!(ci_fix(&Derived { ci_verdict: "none".into(), ..red() }), Gate::Done("no CI ran at HEAD".into()));
        // no CI commit: for_head null, none at HEAD all the same
        assert_eq!(ci_fix(&Derived { ci_verdict: "none".into(), ci_for_head: None, ..red() }), Gate::Done("no CI ran at HEAD".into()));
        // on another SHA, CI on HEAD may yet be red
        let none_elsewhere = Derived { ci_verdict: "none".into(), ci_for_head: Some(false), ..red() };
        assert_eq!(ci_fix(&none_elsewhere), Gate::Soft(vec!["no CI has reported on HEAD yet".into()]));
    }

    #[test]
    fn ci_fix_waits_on_ci_and_says_blocked_and_cancelled_are_chrismos_to_clear_355() {
        let on = |v: &str| ci_fix_said(&Derived { ci_verdict: v.into(), ..red() });
        assert_eq!(on("pending"), [("CI is pending", false)]);
        assert_eq!(on("blocked"), [("CI is blocked", true)]);
        assert_eq!(on("inconclusive"), [("CI was cancelled", true)]);
        assert_eq!(on("odd"), [("CI's verdict is unknown", true)]);
        assert_eq!(ci_fix_said(&Derived { ci_for_head: Some(false), ..red() }), [("CI's red isn't for HEAD", false)]);
        assert_eq!(ci_fix_said(&Derived { ci_for_head: None, ..red() }), [("CI's red isn't for HEAD", false)]);
    }

    #[test]
    fn ci_fix_waits_on_unpushed_commits_chrismos_when_next_offers_no_push() {
        let ahead = Derived { to_push: true, unpushed: true, ..red() };
        assert_eq!(ci_fix_said(&ahead), [("there are unpushed commits, which may be the fix already", false)]);
        let held = ci_fix_said(&Derived { ci_held: true, ..ahead.clone() });
        assert_eq!(held, [("there are unpushed commits, held with the agent's CI config change", true)]);
        let red_verify = ci_fix_said(&Derived { verify_red: true, ..ahead.clone() });
        assert_eq!(red_verify, [("there are unpushed commits, held by the verify red on HEAD", true)]);
        let parent = ci_fix_said(&Derived { parent_unpushed: true, ..ahead.clone() });
        assert_eq!(parent, [("the parent branch isn't on the remote yet", true)]);
        let other = ci_fix_said(&Derived { foreign: true, ..ahead });
        assert_eq!(other, [("there are unpushed commits NEXT won't push", true)]);
    }

    // What push's gate would refuse after the fix, refused before the agent
    // runs. Soft: the agent only costs tokens; push's own Hard undone rule
    // still keeps the copies off the remote.
    #[test]
    fn ci_fix_waits_on_what_would_refuse_its_push_an_undone_restack_and_an_unpushed_parent() {
        assert_eq!(ci_fix_said(&Derived { undone: true, ..red() }), [("the restack was undone, so a fix can't be pushed", true)]);
        assert_eq!(ci_fix_said(&Derived { parent_unpushed: true, ..red() }), [("the parent branch isn't on the remote yet", true)]);
    }

    // Only the CI waits say CI may not be red at HEAD (#348).
    #[test]
    fn ci_fix_waits_for_ci_red_at_head_or_for_a_fix_to_be_pushable_by_which_rules_refuse_348() {
        let said = |d: &Derived| {
            let rules = ci_fix_rules(d);
            let waits: Vec<&str> = ci_fix_ci_waits(&rules).iter().map(|r| r.name).collect();
            (waits, ci_fix_once(&rules))
        };
        let pending = Derived { ci_verdict: "pending".into(), ..red() };
        assert_eq!(said(&pending), (vec!["CI is pending"], "CI is red at HEAD"));
        assert_eq!(said(&Derived { undone: true, ..red() }), (vec![], "a fix can be pushed"));
        assert_eq!(said(&Derived { parent_unpushed: true, ..red() }), (vec![], "a fix can be pushed"));
        assert_eq!(said(&Derived { undone: true, ..pending }), (vec!["CI is pending"], "CI is red at HEAD and a fix can be pushed"));
        assert_eq!(said(&red()), (vec![], "CI is red at HEAD"));
    }

    // CI on HEAD hasn't run yet: a push starts it.
    #[test]
    fn ci_fix_says_blocked_and_cancelled_ci_are_chrismos_only_at_head() {
        let old = |v: &str| Derived { ci_verdict: v.into(), ci_for_head: Some(false), ..red() };
        assert_eq!(ci_fix_said(&old("blocked")), [("CI is blocked, but not on HEAD", false)]);
        assert_eq!(ci_fix_said(&old("inconclusive")), [("CI was cancelled, but not on HEAD", false)]);
        let ahead = ci_fix_said(&Derived { to_push: true, unpushed: true, ..old("blocked") });
        assert_eq!(ahead, [("CI is blocked, but not on HEAD", false), ("there are unpushed commits, which may be the fix already", false)]);
    }

    #[test]
    fn ci_fix_names_a_drop_not_unpushed_commits() {
        let drop = Derived { to_push: true, reset_away: true, dropping: true, ..red() };
        assert_eq!(ci_fix_said(&drop), [("the remote has commits reset away here, which a push drops", false)]);
        let held = ci_fix_said(&Derived { verify_red: true, ..drop });
        assert_eq!(held, [("the remote has commits reset away here, which a push drops", true)]);
    }

    #[test]
    fn ci_fix_runs_only_on_red_known_at_head_with_an_open_pr_and_nothing_to_push_on_generated_states() {
        check("ci-fix runs", |d| {
            let fits = d.has_pr && d.ci_verdict == "red" && d.ci_for_head == Some(true) && !d.to_push && !d.undone && !d.parent_unpushed;
            if ci_fix(d).runs() && !fits {
                Err("ci-fix runs".into())
            } else {
                Ok(())
            }
        });
    }

    /// The record from tests/next.rs's READY, which disagrees with most
    /// generated values: the steps' keys and who must not change with it.
    #[test]
    fn offer_takes_each_steps_key_and_who_from_derived_alone() {
        let ready = json!({"branch": "feat", "pr": {"number": 7, "state": "OPEN", "base": "main"},
            "sync": {"target": "main", "default": "main", "ahead": 1, "old_base": "b0b0b0b0b0b0"},
            "push": {"remote_sha": "h1", "ahead": 0}, "threads": {"owed_bot": 2},
            "question": {"at_head": true, "step": "address", "kind": "gave-up", "text": "why?"},
            "ci": {"verdict": "red", "for_head": true}});
        let shape = |steps: Vec<Value>| steps.iter().map(|s| format!("{}/{}", s["key"], s["who"])).collect::<Vec<_>>();
        check("offer's shape", |d| {
            let (a, b) = (shape(crate::next::offer(d, &json!({}), "origin")), shape(crate::next::offer(d, &ready, "origin")));
            if a == b {
                Ok(())
            } else {
                Err(format!("{a:?} vs {b:?}"))
            }
        });
    }

    #[test]
    fn next_offers_no_migrated_verb_its_gate_refuses_on_generated_states() {
        let s = json!({});
        check("parity", |d| {
            let steps = crate::next::offer(d, &s, "origin");
            let off = parity(d, &steps);
            if off.is_empty() {
                Ok(())
            } else {
                Err(off.join("; "))
            }
        });
    }
}
