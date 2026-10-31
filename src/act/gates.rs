// Each migrated verb's gate over Derived (#348). A verb joins MIGRATED in
// the PR that moves its guard here; parity runs on every tests/next.rs
// fixture and on generated states, whose fields are drawn independently on
// purpose: parity must hold on every state, not only those sd state makes.
// A Migrated's offered reads steps' key and who, which offer takes from
// Derived alone (tested below); cmd and say also read the record. Each gate
// destructures Derived without .., so a new field doesn't compile until
// every gate claims or ignores it.

use super::gate::{judge, Gate, Level, Rule};
use crate::next::Derived;
use serde_json::Value;

/// A verb whose gate NEXT asks.
pub struct Migrated {
    pub verb: &'static str,
    /// Whether NEXT's steps offer the verb, by key and who only.
    pub offered: fn(&[Value]) -> bool,
    pub gate: fn(&Derived) -> Gate,
}

pub const MIGRATED: &[Migrated] = &[Migrated { verb: "push", offered: |steps| steps.iter().any(|s| s["key"] == "push"), gate: push }];

/// sd act push. What's on the remote is push_branch's to judge, after its
/// own fetch: sd state doesn't fetch.
pub fn push(d: &Derived) -> Gate {
    let Derived {
        open, verify_red, undone, parent_unpushed,
        own_base: _, running: _, reviewer: _, pr_null: _, has_pr: _, merged: _, closed: _, errors: _, stale: _,
        lagging: _, locked: _, owed_human: _, owed_bot: _, bot_blocking: _, info: _, draft: _, review_required: _,
        merge_state: _, tasked: _, foreign: _, reset_away: _, dropping: _, taken: _, held: _, unpushed: _,
        to_push: _, ci_held: _, push_dirty: _, empty: _, chase: _, up_to_date: _, behind_target: _, sync_conflict: _,
        conflict: _, chase_behind: _, gh_behind: _, gh_restack: _, by_tree: _, old_base_null: _, no_base: _,
        restack_due: _, stacked: _, parent_closed: _, parent_merged: _, ci_red: _, ci_blocked: _, ci_inconclusive: _,
        ci_pending: _, ci_for_head: _, draft_ready: _, any_cover: _, reviewed: _, need_review: _, fixing: _,
        findings_declined: _, fix_due: _, bot_due: _, bot_late: _, bot_waiting: _, budget_unread: _, over_budget: _,
        failed_out: _, asking: _, stored: _, addr_refuses: _, addr_hard: _, addr_conflict: _, addr_moot: _,
        ci_moot: _, ci_waits: _, ci_runs: _, resume: _, resume_ci: _, answer_pending: _, dirty: _, refused: _,
        bot: _, rv_yours: _, rv_dirty: _, rv_fetch: _, rv_current: _, rv_pending: _, rv_submitted: _, rv_local: _,
    } = d;
    let rule = |name, refuses: bool, clears| Rule { name, level: Level::Soft, refuses, clears };
    judge(&[
        rule("the PR is merged or closed", !open, "reopen it, or start a new branch"),
        rule("verify is red at HEAD", *verify_red, "fix it, then sd act verify"),
        rule("the restack was undone: some of the target's commits are copies here, and pushing would publish them", *undone,
            "redo the restack (sd state's NEXT says how)"),
        rule("the parent branch isn't on the remote yet", *parent_unpushed, "ship it first (sd act ship <parent>)"),
    ])
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
    fn push_refuses_an_undone_restack_a_red_verify_an_unpushed_parent_and_a_closed_pr_unless_forced() {
        let d = Derived { open: true, undone: true, verify_red: true, ..Derived::default() };
        let Gate::Soft(why) = push(&d) else { panic!("{:?}", push(&d)) };
        assert_eq!(why.len(), 2);
        assert!(why[1].starts_with("the restack was undone"), "{why:?}");
        assert!(matches!(push(&Derived { open: true, parent_unpushed: true, ..Derived::default() }), Gate::Soft(_)));
        assert!(matches!(push(&Derived::default()), Gate::Soft(_)));
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
