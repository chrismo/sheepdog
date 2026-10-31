// Each migrated verb's gate over Derived (#348). A verb joins MIGRATED in
// the PR that moves its guard here; parity runs on every tests/next.rs
// fixture and on generated states, whose fields are drawn independently on
// purpose: parity must hold on every state, not only those sd state makes.
// A Migrated's offered reads steps' key and who, which offer takes from
// Derived alone (tested below); cmd and say also read the record. Each gate
// destructures Derived without .., so a new field doesn't compile until
// every gate claims or ignores it.

use super::gate::{judge, warnings, Gate, Level, Rule};
use crate::next::{no, yes};
use crate::next::Derived;
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
];

/// sd act push. What's on the remote is push_branch's to judge, after its
/// own fetch: sd state doesn't fetch.
pub fn push(d: &Derived) -> Gate {
    let Derived {
        open, verify_red, undone, parent_unpushed,
        own_base: _, running: _, reviewer: _, pr_null: _, has_pr: _, merged: _, closed: _, errors: _, stale: _,
        lagging: _, locked: _, owed_human: _, owed_bot: _, bot_blocking: _, info: _, unposted: _, draft: _, review_required: _,
        merge_state: _, tasked: _, foreign: _, behind_remote: _, reset_away: _, dropping: _, taken: _, held: _, unpushed: _,
        to_push: _, ci_held: _, push_dirty: _, empty: _, chase: _, up_to_date: _, behind_target: _, sync_conflict: _,
        conflict: _, chase_behind: _, gh_behind: _, gh_restack: _, by_tree: _, old_base_null: _, no_base: _,
        restack_due: _, stacked: _, parent_closed: _, parent_merged: _, ci_red: _, ci_blocked: _, ci_inconclusive: _,
        ci_pending: _, ci_for_head: _, draft_ready: _, any_cover: _, reviewed: _, need_review: _, fixing: _,
        findings_declined: _, fix_due: _, bot_due: _, bot_late: _, bot_waiting: _, budget_unread: _, over_budget: _,
        failed_out: _, asking: _, stored: _, addr_moot: _, ci_moot: _, ci_waits: _, ci_runs: _, resume: _, resume_ci: _, answer_pending: _, dirty: _, refused: _,
        bot: _, rv_yours: _, rv_dirty: _, rv_fetch: _, rv_current: _, rv_pending: _, rv_submitted: _, rv_local: _,
    } = d;
    let rule = |name, level, refuses: bool, clears| Rule { name, level, refuses, clears };
    judge(&[
        // Hard, so a --force meant for another refusal can't publish the
        // copies; if the guess is wrong, git push by hand still works.
        rule("not pushing: the restack was undone: some of the target's commits are copies here, and pushing would publish them \
              (if they aren't, git push by hand)", Level::Hard, *undone, "redo the restack (sd state's NEXT says how)"),
        rule("the PR is merged or closed", Level::Soft, !open, "reopen it, or start a new branch"),
        rule("verify is red at HEAD", Level::Soft, *verify_red, "fix it, then sd act verify"),
        rule("the parent branch isn't on the remote yet", Level::Soft, *parent_unpushed, "ship it first (sd act ship <parent>)"),
    ])
}

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
        owed_human: _, bot_blocking: _, info: _, draft: _, review_required: _, merge_state: _, tasked: _, verify_red: _, foreign: _,
        reset_away: _, dropping: _, taken: _, held: _, to_push: _, ci_held: _, push_dirty: _, parent_unpushed: _,
        empty: _, chase: _, undone: _, behind_target: _, conflict: _, chase_behind: _, gh_behind: _, gh_restack: _,
        by_tree: _, old_base_null: _, no_base: _, restack_due: _, stacked: _, parent_closed: _, parent_merged: _,
        ci_red: _, ci_blocked: _, ci_inconclusive: _, ci_for_head: _, draft_ready: _, any_cover: _, reviewed: _,
        need_review: _, fixing: _, findings_declined: _, fix_due: _, bot_due: _, bot_late: _, bot_waiting: _,
        budget_unread: _, over_budget: _, failed_out: _, asking: _, stored: _, addr_moot: _, ci_moot: _, ci_waits: _,
        ci_runs: _, resume: _, resume_ci: _, answer_pending: _, dirty: _, refused: _, bot: _, rv_yours: _, rv_dirty: _,
        rv_fetch: _, rv_current: _, rv_pending: _, rv_submitted: _, rv_local: _,
    } = d;
    let conflict = yes(*sync_conflict);
    let push = match (unpushed, behind_remote) {
        (true, true) => Some(addr::DIVERGED),
        (false, true) => Some(addr::REMOTE_MORE),
        (true, false) => Some(addr::UNPUSHED),
        (false, false) => None,
    };
    let rule = |name, level, refuses: bool, clears| Rule { name, level, refuses, clears };
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

    #[test]
    fn address_never_runs_unless_github_says_bot_items_are_owed_on_generated_states() {
        check("address on unknown or none", |d| if d.owed_bot != Some(true) && address(d).runs() { Err("address runs".into()) } else { Ok(()) });
    }

    #[test]
    fn push_never_runs_with_an_undone_restack_on_generated_states() {
        check("push with undone", |d| if d.undone && push(d).runs() { Err("push runs".into()) } else { Ok(()) });
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
