// What to do next on a branch, from its sd state JSON. sd state runs it.
//
// A step is {key, who, say, cmd}. who is who does it once it's automated:
//   you    chrismo's judgment     agent  a model step
//   run    mechanical             wait   nothing to do yet
// cmd is what to run by hand meanwhile (null: no command yet).
//
// Ordering of the loop's own steps, after what blocks everything (stale refs,
// human items): restack, agent fixes, one push, then review and wait; the push
// leads because every push restarts CI (#79 Design record). GitHub behind the
// remote refs, a push it hasn't seen yet (sd state's lag), is a wait where
// stale refs are a fetch (#246). It has no time limit: a PR GitHub never
// updates waits until chrismo sees sd watch's ⚠️ glyph.
//
// While address's own gate (src/act/gates.rs) would refuse it, address
// comes after the steps that clear the gate, or is chrismo's when it would be
// first; an address question's step says what clears it (#326). An answer
// stored for the question stands in for its question step: the bot step goes
// on with it once the gate clears, an answer step waits behind what clears it
// till then, and the steps that would move HEAD, dropping the question
// (restacks, agent fixes, review), wait for it. The ci-red step, and a
// ci-fix question, follow gates::ci_fix (src/act/gates.rs, #348): a stored
// answer runs as the ci-red step once CI is red at HEAD and a fix can be
// pushed, and CI passing at HEAD, or none running there, makes the question
// moot.
//
// The default branch (or a branch that is its own base) gets one step, to
// start a branch (#159). A branch with nothing ahead of its base and no PR
// has nothing to push or ship: its task is implemented, or with no task it's
// chrismo's (#143). So are uncommitted changes there, which implement refuses.
// Implement and review-fix are offered only when gates::implement says Go
// (#348): with errors, pr may be wrong, so nothing spends on implementing;
// past settings.reviewRounds the findings are chrismo's. A remote with
// commits reset away here isn't empty: the #86 steps handle it.
// While a step runs (session.running), its one step is to wait: the
// worktree is the step's, so an agent's edits aren't chrismo's to commit,
// and no other step should touch it (#170, #270).
//
// An agent step's question standing at HEAD (sd state's question, #167; a
// give-up is one too, #254) is chrismo's question step, with the human
// items; the step that asked (asking: implement, review-fix, address,
// ci-fix) goes until it's answered. restack --resolve's give-up holds the
// conflict step until the target moves; it can't be answered, as its rebase
// rolled back (#257).
// Once it no longer stands, uncommitted edits are chrismo's (#178): the
// steps that refuse them (ci-red, bot, review-fix, review, open-pr, and
// restack's behind and conflict) give way to one dirty step, as the empty
// branch's nothing-dirty does for implement; once commits were reset away,
// reset-dirty is that step. While it stands the edits are the asker's, so
// those steps wait behind the question instead.
//
// Behind the default branch alone is no step: it moves too often to chase.
// A conflict with it is, and so is a restack after the parent merged.
// Only bot items are ever marked i, so owed_bot - info is the blocking bot count.
//
// Info bot items are read once nothing else moves without chrismo (#168): a
// restack too, whose push and re-review would supersede them (#247).
//
// Past settings.branchBudget (spend.budget; 0 is none), nothing spends tokens
// on the branch: the first agent step becomes chrismo's, the rest go, and the
// run steps stay (#121). So does open-pr: sd act ship describes the branch
// when its description is stale, which spends. Spend that can't be read, or a
// step whose cost is unknown (spend.unknown, #349), with a budget set, holds
// the same way: a cost guard fails closed. spends() makes
// that swap, agent() for agent steps; the dedupe at the end keeps the first.
// The verbs refuse the same way (budget_hold, sd act's spend_gate), so
// ship-parent, whose describe spends on the parent, is held there, against
// the parent's own budget: this state has only this branch's spend (#148).
//
// After FAIL_LIMIT agent steps failed in a row (failed, #242), the agent
// steps go to chrismo the same way.
//
// A verify that failed on HEAD (verify.at_head, #325) is chrismo's step in
// place of push, open-pr, draft, review and bot-trigger: nothing publishes or
// reviews a commit verify rejected (review would only rerun verify and fail
// again), and ready, as an end state, is listed only alone.
//
// A followed PR (mode reviewer, sd act follow, #250) is a teammate's: none of
// the author's steps, only its own list (reviewer below). The author's pushes
// are fetched before anything else; the agent reviews HEAD; then the review
// is chrismo's to submit on GitHub, and once it's of HEAD the author's turn.
//
// Field reads follow the SuperDB query this replaced (src/val.rs), and an
// equality holds only between two present values.

mod derived;
pub use derived::{round_key, Derived};
#[cfg(test)]
pub(crate) use derived::arb as derived_arb;
pub(crate) use derived::{no, yes};

use crate::act::Gate;
use crate::val::{self, at, sj, text};
use serde_json::{json, Value};

/// "x is null" in the query: present and null. A missing field there was an
/// error value, which no condition took.
fn is_null(v: &Value, path: &str) -> bool {
    path.split('.').try_fold(v, |cur, k| cur.as_object()?.get(k)).is_some_and(Value::is_null)
}

fn plural(k: &Value, w: &str) -> String {
    let one = k.as_f64() == Some(1.0);
    format!("{} {w}{}", text(k), if one { "" } else { "s" })
}

fn step(key: &str, who: &str, say: impl Into<String>, cmd: Option<String>) -> Option<Value> {
    Some(json!({"key": key, "who": who, "say": say.into(), "cmd": cmd}))
}

fn spends(hold: &Option<Value>, s: Option<Value>) -> Option<Value> {
    if hold.is_some() {
        hold.clone()
    } else {
        s
    }
}

fn agent(hold: &Option<Value>, key: &str, say: impl Into<String>, cmd: &str) -> Option<Value> {
    spends(hold, step(key, "agent", say, Some(cmd.into())))
}

pub const FAIL_LIMIT: f64 = 3.0;

fn rerun(step: &str) -> Option<String> {
    match step {
        "resolve" => cmd("sd act restack --resolve"),
        "implement" | "review" | "address" | "ci-fix" | "describe" => cmd(format!("sd act {step}")),
        _ => None,
    }
}

fn cmd(s: impl Into<String>) -> Option<String> {
    Some(s.into())
}

fn usd(x: f64) -> String {
    let c = (x * 100.0).round() as i64;
    format!("${}.{}{}", c / 100, if c % 100 < 10 { "0" } else { "" }, c % 100)
}

fn why_blocked(m: &str) -> String {
    match m {
        "BLOCKED" => "the merge is blocked".into(),
        "DIRTY" => "there are conflicts".into(),
        "DRAFT" => "it's a draft".into(),
        _ => m.to_lowercase(),
    }
}

fn len(v: &Value) -> usize {
    v.as_array().map_or(0, Vec::len)
}

/// Whether NEXT offers sd act push for <s>; sd act answer reads it too (#326).
pub fn push_offered(s: &Value) -> bool {
    push_step(&Derived::new(s))
}

/// Whether sd act push would push: its gate, and NEXT's view of the remote.
fn push_runs(d: &Derived) -> bool {
    !d.foreign && crate::act::gates::push(d).runs()
}

/// A held CI config change isn't offered: chrismo's own push approves it.
pub(crate) fn push_step(d: &Derived) -> bool {
    d.to_push && !d.empty && !d.ci_held && push_runs(d)
}

/// The remote has commits this copy never had: chrismo looks.
fn foreign_step(d: &Derived, s: &Value, remote: &str) -> Option<Value> {
    if !(d.open && d.foreign) {
        return None;
    }
    let branch = val::t(s, "branch");
    step("foreign", "you",
        format!("{remote}/{branch} has {} this copy has never had", plural(at(s, "push.theirs"), "commit")),
        cmd(format!("git log HEAD..{remote}/{branch}")),
    )
}

/// Commits someone else made on the branch, when they're all that holds a
/// push (#356): chrismo looks, then forces it. Not while attached: --force
/// would push past the sd-lock too.
fn foreign_authors_step(d: &Derived, s: &Value, remote: &str) -> Option<Value> {
    let holds = d.to_push && !d.empty && !push_runs(d) && push_runs(&Derived { foreign_authors: false, ..d.clone() });
    if !(d.foreign_authors && holds && !yes(d.locked)) {
        return None;
    }
    let step_say = format!(
        "commits on {} by {}: look (git log --format='%h %ae %ce %s' {}), then push them",
        val::t(s, "branch"),
        sj(at(s, "sync.foreign_authors"), ", "),
        authors_range(s, remote)
    );
    step("foreign-authors", "you", step_say, cmd("sd act push --force"))
}

/// The range sync.foreign_authors is read from: the branch past its target,
/// base, the default branch and the head last forced past them.
fn authors_range(s: &Value, remote: &str) -> String {
    let (target, default) = (val::t(s, "sync.target"), val::t(s, "sync.default"));
    let short = |p: &str| val::t(s, p).chars().take(7).collect::<String>();
    let mut not = vec![format!("{remote}/{target}"), short("sync.old_base")];
    if !default.is_empty() && default != target {
        not.push(format!("{remote}/{default}"));
    }
    not.push(short("sync.authors_seen"));
    not.retain(|p| !p.is_empty());
    format!("{} --not {}", val::t(s, "branch"), not.join(" "))
}

/// The steps for an sd state record, first is next.
pub fn steps(s: &Value, remote: &str) -> Result<Value, String> {
    if !s.is_object() {
        return Err("sd state JSON must be one record".into());
    }
    Ok(Value::Array(offer(&Derived::new(s), s, remote)))
}

/// Why the agent steps are held for <s>'s spend (spend.usd, .unknown and
/// .budget), if they are; sd act's spend gate asks it too (#349).
pub fn budget_hold(s: &Value) -> Option<String> {
    budget_say(&Derived::new(s), s)
}

fn budget_say(d: &Derived, s: &Value) -> Option<String> {
    if d.budget_unread {
        Some(unread_say(at(s, "spend.unknown")))
    } else if d.over_budget {
        let n = |p: &str| val::n(s, p);
        Some(format!("spent {} on this branch, past settings.branchBudget ({}): raise it, or take over", usd(n("spend.usd")), usd(n("spend.budget"))))
    } else {
        None
    }
}

fn unread_say(unknown: &Value) -> String {
    let unknown = unknown.as_array().map_or(&[][..], Vec::as_slice);
    if unknown.is_empty() {
        return "can't tell what this branch has spent (sd state's errors say why), so nothing spends: fix the usage record, or take over".into();
    }
    let named: Vec<String> = unknown.iter().map(|u| format!("{} at {}, session {}", val::t(u, "step"), val::t(u, "time"), val::t(u, "session"))).collect();
    let (steps, its) = if unknown.len() == 1 { ("1 step".to_string(), "its") } else { (format!("{} steps", unknown.len()), "their") };
    format!("can't tell what {steps} on this branch cost ({}), so nothing spends: set {its} cost_usd in usage.sup, or take over", named.join("; "))
}

/// The budget or failed step that stands in for every agent step, if any.
fn hold_step(d: &Derived, s: &Value) -> Option<Value> {
    let t = |p: &str| val::t(s, p);
    if let Some(say) = budget_say(d, s) {
        step("budget", "you", say, cmd("sd act attach"))
    } else if d.failed_out {
        step("failed", "you",
            format!(
                "{} agent steps failed in a row, the last {} {} at {}: {}; fix what fails, then rerun it",
                t("failed.count"), t("failed.harness"), t("failed.step"), t("failed.at"), t("failed.error")
            ),
            rerun(&t("failed.step")).or_else(|| cmd(format!("git update-ref -d refs/sheepdog/{}/failed", t("branch")))),
        )
    } else {
        None
    }
}

/// The commits since the oldest review that trails HEAD, if counted.
fn last_review(s: &Value) -> Option<Value> {
    ["review.local", "review.bot", "review.accepted"]
        .iter()
        .filter(|p| at(s, p).get("status").and_then(Value::as_str) == Some("uncovered"))
        .map(|p| at(at(s, p), "uncovered"))
        .filter(|v| !v.is_null())
        .min_by(|a, b| a.as_f64().partial_cmp(&b.as_f64()).unwrap_or(std::cmp::Ordering::Equal))
        .cloned()
}

/// The steps for <d>, first is next. Which steps, and whose, come from <d>
/// alone; <s> is the record it was derived from, which some say and cmd
/// text still reads.
pub(crate) fn offer(d: &Derived, s: &Value, remote: &str) -> Vec<Value> {
    let g = |p: &str| at(s, p);
    let n = |p: &str| val::n(s, p);
    let nul = |p: &str| g(p).is_null();
    let is = |p: &str, lit: &str| g(p).as_str() == Some(lit);
    let t = |p: &str| val::t(s, p);
    let restack = if d.old_base_null {
        format!("git rebase {remote}/{}", t("sync.target"))
    } else {
        let ob = t("sync.old_base");
        format!("git rebase --onto {remote}/{} {}", t("sync.target"), &ob[..ob.len().min(12)])
    };
    let asking = d.asking.as_str();
    let asked = if asking.is_empty() {
        String::new()
    } else {
        t("question.text").split('\n').find(|l| l.chars().any(|c| !matches!(c, '\t' | '\n' | '\x0c' | '\r' | ' '))).unwrap_or("").to_string()
    };
    let last_review = last_review(s);
    let hold = &hold_step(d, s);
    let implement = crate::act::gates::implement(d);
    let fixes = crate::act::gates::implement_fixes(d) && implement.runs();
    let addr = crate::act::gates::address(d);
    let addr_why = refusal(crate::act::address_said(crate::act::gates::address(d), s));
    let ci_rules = crate::act::gates::ci_fix_rules(d);
    let (ci_fix, ci_once) = (crate::act::judge(&ci_rules), crate::act::gates::ci_fix_once(&ci_rules));
    let ci_waits = if let (crate::act::Gate::Soft(why), "ci-fix") = (&ci_fix, asking) { why.join("; ") } else { String::new() };
    let address = || agent(hold, "bot", format!("address {}", plural(g("threads.owed_bot"), "bot item")), "sd act address");

    let branch = t("branch");
    let target = t("sync.target");
    let parent = t("parent.name");
    let one = |k: &str, a: &str, b: &str| if n(k) == 1.0 { a } else { b }.to_string();
    let open = d.open;

    let all: Vec<Option<Value>> = if d.own_base {
        vec![step("default", "you", format!("on {branch}: start a branch"), cmd("sd act start"))]
    } else if d.running {
        vec![step("running", "wait",
            format!("{} running since {}", t("session.running.step"), t("session.running.since")),
            None,
        )]
    } else if d.reviewer {
        let again = match &last_review {
            Some(k) => format!("review again: {} since the last review", plural(k, "commit")),
            None if d.reviewed => "review again: the PR was rewritten since the last review".into(),
            None => "review the PR's change".into(),
        };
        reviewer(d, s, hold, again)
    } else {
        vec![
            if d.merged {
                step("merged", "run",
                    "merged: drop the branch's pins and enrollment, remove its worktree and delete it",
                    cmd("sd act forget --remove"),
                )
            } else {
                None
            },
            if d.closed { step("closed", "you", "PR closed without merging", None) } else { None },
            if open && d.errors {
                step("errors", "you",
                    format!("{}: the steps below may be wrong", plural(&json!(len(g("errors"))), "error")),
                    cmd("sd state --json"),
                )
            } else {
                None
            },
            if open && d.stale {
                step("stale", "run",
                    "fetch: GitHub is ahead of your remote refs, so the steps below may be wrong",
                    cmd(format!("git fetch --prune {remote}")),
                )
            } else {
                None
            },
            if open && d.lagging {
                step("gh-wait", "wait", "GitHub hasn't seen the push yet, so the steps below may be wrong", None)
            } else {
                None
            },
            if open && yes(d.locked) {
                step("attached", "wait",
                    "attached: the branch's session is open interactively; agent steps wait until it exits",
                    None,
                )
            } else {
                None
            },
            foreign_step(d, s, remote),
            if open && d.owed_human {
                step("human", "you",
                    format!("reply to {}", plural(g("threads.owed_human"), "human item")),
                    cmd("sd state --long"),
                )
            } else {
                None
            },
            if open && !asking.is_empty() && !d.answer_pending {
                question_step(d, s, &asked, &addr, &addr_why, (&ci_waits, ci_once))
            } else {
                None
            },
            if open && d.undone {
                let pin = if d.no_base {
                    format!(
                        "; no base to redo it from on {target}: pin one by hand (git update-ref refs/sheepdog/{branch}/base <the commit {branch} left {parent} at>), then sd act restack"
                    )
                } else {
                    String::new()
                };
                step("restack-undone", "you",
                    format!(
                        "the restack was undone ({} of {target}'s commits {} on {branch}; a plain git push after a restack?): redo it, and don't sd act push before that, which would publish the copies{pin}",
                        t("sync.undone"),
                        one("sync.undone", "is a copy", "are copies")
                    ),
                    if d.no_base { None } else { cmd(format!("{restack} && sd act push")) },
                )
            } else {
                None
            },
            if open && d.parent_unpushed {
                step("ship-parent", "run",
                    format!("ship the parent first: {parent} isn't on {remote} yet"),
                    cmd(format!("sd act ship {parent}")),
                )
            } else {
                None
            },
            if open && !d.undone && !d.parent_closed && d.conflict {
                if yes(d.by_tree) {
                    step("conflict", "you",
                        format!("restack onto {target} by tree conflicts, which --resolve can't do: resolve it by hand (sd act restack says how)"),
                        cmd("sd act restack"),
                    )
                } else if d.refused || asking == "resolve" {
                    None
                } else {
                    agent(hold, "conflict", format!("restack onto {target} conflicts: resolve it"), "sd act restack --resolve")
                }
            } else {
                None
            },
            if open && !d.undone && !d.parent_closed && d.chase_behind {
                if d.old_base_null {
                    step("behind", "you",
                        format!("restack onto {target} ({} behind): no base to restack from; see sd state's errors", t("sync.behind")),
                        cmd("sd state --json"),
                    )
                } else if d.refused {
                    None
                } else if yes(d.by_tree) {
                    step("behind", "run",
                        format!("restack onto {target} by tree ({} behind): its commits become one", t("sync.behind")),
                        cmd("sd act restack"),
                    )
                } else {
                    step("behind", "run", format!("restack onto {target} ({} behind)", t("sync.behind")), cmd("sd act restack"))
                }
            } else if d.gh_restack && !d.undone && !d.parent_closed && !d.refused {
                step("behind", "run",
                    format!(
                        "restack onto {} ({} behind): GitHub requires branches to be up to date",
                        t("sync.default"),
                        if nul("sync.behind") { "0".into() } else { t("sync.behind") }
                    ),
                    cmd("sd act restack"),
                )
            } else {
                None
            },
            if d.dirty && !d.dropping {
                step("dirty", "you",
                    "the worktree has uncommitted changes, which the agent steps refuse: they're yours: commit and push them (sd act push), or discard them (git reset --hard; git clean for untracked files)",
                    cmd("git status"),
                )
            } else {
                None
            },
            if ci_fix.runs() && asking != "ci-fix" && !d.refused {
                agent(hold, "ci-red", format!("fix CI: {}", sj(g("ci.failing"), ", ")), "sd act ci-fix")
            } else if d.resume_ci && ci_fix.runs() {
                agent(hold, "ci-red", format!("fix CI: {}, going on with your answer to its question", sj(g("ci.failing"), ", ")), "sd act ci-fix")
            } else {
                None
            },
            if d.has_pr && d.ci_blocked && !d.to_push {
                step("ci-blocked", "you",
                    format!(
                        "CI blocked: {} didn't start or want approval; an agent can't fix that",
                        sj(g("ci.blocked"), ", ")
                    ),
                    cmd("gh pr checks"),
                )
            } else {
                None
            },
            if d.bot && addr.runs() {
                address()
            } else if d.resume && addr.runs() {
                agent(hold, "bot", format!("address {}, going on with your answer to its question", plural(g("threads.owed_bot"), "bot item")), "sd act address")
            } else {
                None
            },
            // only once nothing else moves without chrismo (#168)
            if open
                && d.bot_blocking == 0.0
                && d.info
                && !d.to_push
                && !d.need_review
                && !d.fixing
                && !d.bot_due
                && !d.bot_waiting
                && !d.restack_due
                && !d.ci_inconclusive
                && !(d.stacked && yes(d.parent_merged))
            {
                step("read", "you",
                    format!("read {} in sd state --long, then ack", plural(g("threads.info"), "bot item")),
                    cmd("sd act ack"),
                )
            } else {
                None
            },
            if open && d.taken {
                let them = one("push.behind", "it", "them");
                step("take", "you",
                    format!(
                        "{remote}/{branch} has {} this branch took but didn't make, reset away here: take {them} back (git merge --ff-only {remote}/{branch}), or drop {them} by hand",
                        plural(g("push.behind"), "commit")
                    ),
                    None,
                )
            } else {
                None
            },
            if open && d.dropping && yes(d.push_dirty) {
                step("reset-dirty", "you",
                    format!(
                        "{remote}/{branch} has {} reset away here, and the worktree has uncommitted changes: commit them (then sd act push replaces the remote {}), or remove them (git reset --hard; git clean for untracked files)",
                        plural(g("push.behind"), "commit"),
                        one("push.behind", "commit", "commits")
                    ),
                    None,
                )
            } else {
                None
            },
            if d.empty && yes(d.push_dirty) {
                step("nothing-dirty", "you",
                    format!(
                        "nothing to ship, and the worktree has uncommitted changes: commit them{}",
                        if d.tasked { ", or remove them and sd act implement" } else { "" }
                    ),
                    None,
                )
            } else if d.empty && d.tasked {
                if implement.runs() && asking != "implement" {
                    agent(hold, "implement", format!("implement the task: nothing ahead of {target} yet"), "sd act implement")
                } else {
                    None
                }
            } else if d.empty {
                step("nothing", "you", "nothing to ship: commit, or record a task", None)
            } else {
                None
            },
            if d.verify_red {
                let sha = t("verify.sha");
                step("verify-red", "you",
                    format!(
                        "verify red at {} ({}: {}): fix it, then sd act verify",
                        &sha[..sha.len().min(7)],
                        t("verify.step"),
                        t("verify.cmd")
                    ),
                    cmd("sd act verify"),
                )
            } else {
                None
            },
            foreign_authors_step(d, s, remote),
            // after what holds the push, which its say points up to
            if open && d.ci_held {
                let paths = sj(g("push.held.paths"), ", ");
                if !push_runs(d) {
                    step("ci-config", "you",
                        format!("the agent changed CI config ({paths}): review it; push once the steps above are done"),
                        None,
                    )
                } else {
                    step("ci-config", "you",
                        format!("the agent changed CI config ({paths}): review, then sd act push"),
                        cmd("sd act push"),
                    )
                }
            } else {
                None
            },
            if push_step(d) {
                let ahead = if nul("sync.ahead") { g("push.ahead") } else { g("sync.ahead") };
                step("push", "run",
                    if is_null(s, "push.remote_sha") {
                        format!("push: not on {remote} yet")
                    } else if val::on(s, "push.diverged") {
                        format!(
                            "force-push {} (rewritten); a plain git push is rejected, and pulling would undo the rewrite",
                            plural(ahead, "commit")
                        )
                    } else if n("push.ahead") == 0.0 {
                        format!("force-push: drop {} the remote has, reset away here", plural(g("push.behind"), "commit"))
                    } else {
                        format!("push {}", plural(g("push.ahead"), "commit"))
                    },
                    cmd("sd act push"),
                )
            } else {
                None
            },
            if d.bot && !addr.runs() {
                address()
            } else if d.resume && !addr.runs() {
                // it runs address, and the rewrite below may offer --force
                spends(hold, step("answer", "wait", format!("your answer to the address agent's question waits while {addr_why}"), None))
            } else {
                None
            },
            if d.pr_null && !d.errors && !d.verify_red && !d.parent_unpushed && !d.ci_held && !d.empty && !d.refused {
                spends(hold, step("open-pr", "run", "ship: push, open a draft PR", cmd("sd act ship")))
            } else {
                None
            },
            if d.has_pr && d.draft_ready && !d.verify_red && !d.need_review && !d.bot_due && !d.bot_waiting {
                step("draft", "run",
                    format!(
                        "{}: mark the PR ready for review",
                        if is("ci.verdict", "none") { "reviewed, no CI checks" } else { "CI green and reviewed" }
                    ),
                    cmd("gh pr ready"),
                )
            } else {
                None
            },
            if open && d.fixing && (asking == "review-fix" || (d.refused && fixes)) {
                None
            } else if fixes {
                agent(hold, "review-fix", format!(
                        "fix {} (round {}/{})",
                        plural(g("review.findings.n"), "review finding"),
                        t("review.findings.round"),
                        t("review.findings.cap")
                    ),
                    "sd act implement",
                )
            } else if open && d.fixing {
                let findings = plural(g("review.findings.n"), "review finding");
                let what = if yes(d.findings_declined) {
                    format!("the agent declined {findings}")
                } else {
                    format!("{findings} after {} rounds", t("review.findings.round"))
                };
                step("review-decide", "you",
                    format!(
                        "{what}: fix {}, or accept HEAD as reviewed (sd state --long lists them)",
                        one("review.findings.n", "it", "them")
                    ),
                    cmd("sd act review --accept"),
                )
            } else if open && d.need_review && !d.refused && !d.verify_red {
                let say = match &last_review {
                    Some(k) => format!("review again: {} since the last review", plural(k, "commit")),
                    None if d.reviewed => "review again: the branch was rewritten since the last review".into(),
                    None => "review the branch's change".into(),
                };
                agent(hold, "review", say, "sd act review")
            } else {
                None
            },
            if d.has_pr && !d.to_push && !d.held && !d.foreign && !d.verify_red && d.bot_due {
                step("bot-trigger", "run",
                    format!("trigger the review bot ({})", t("bot.command")),
                    cmd("sd act trigger"),
                )
            } else {
                None
            },
            if d.has_pr && d.ci_inconclusive {
                step("ci-rerun", "run", "rerun cancelled CI", cmd("gh pr checks"))
            } else {
                None
            },
            if d.has_pr && d.bot_waiting && !d.bot_late {
                step("bot-wait", "wait", format!("waiting on the review bot ({})", t("bot.command")), None)
            } else {
                None
            },
            if d.has_pr && d.bot_waiting && d.bot_late {
                step("bot-silent", "you",
                    format!(
                        "the review bot hasn't answered in {} min: check it, trigger it again, or accept HEAD as reviewed (sd act review --accept)",
                        minutes(g("review.bot.waited"))
                    ),
                    cmd("sd act trigger --force"),
                )
            } else {
                None
            },
            if d.has_pr && !d.to_push && !d.held && (d.ci_pending || no(d.ci_for_head)) {
                let say = if d.ci_pending {
                    let missing = g("ci.missing");
                    if len(g("ci.pending")) == 0 && len(missing) > 0 {
                        let draft = if yes(d.draft) {
                            format!(
                                "; if {} on a draft, undraft by hand (gh pr ready)",
                                if len(missing) == 1 { "it never runs" } else { "they never run" }
                            )
                        } else {
                            String::new()
                        };
                        format!(
                            "waiting on required check{} {} to report{draft}",
                            if len(missing) == 1 { "" } else { "s" },
                            sj(missing, ", ")
                        )
                    } else {
                        "CI running".into()
                    }
                } else {
                    "CI hasn't reported on HEAD yet".into()
                };
                step("ci-wait", "wait", say, cmd("gh pr checks --watch"))
            } else {
                None
            },
            if d.resume_ci && !ci_waits.is_empty() {
                step("answer", "wait", format!("your answer to the ci-fix agent's question waits while {ci_waits}"), None)
            } else {
                None
            },
            if d.has_pr && d.parent_closed {
                step("parent-closed", "you",
                    format!("{parent} (#{}) was closed without merging: decide what this PR targets", t("parent.pr.number")),
                    None,
                )
            } else {
                None
            },
            if d.has_pr && d.stacked && yes(d.parent_merged) {
                step("retarget", "run",
                    format!("{} already merged: point this PR at {}", t("pr.base"), t("sync.default")),
                    cmd("sd act retarget"),
                )
            } else {
                None
            },
            if d.has_pr && d.review_required {
                let reqs = g("pr.review_requests");
                step("review-wait", "wait",
                    if len(reqs) > 0 {
                        format!("waiting on review from {}", sj(reqs, ", "))
                    } else {
                        "waiting on a required review".into()
                    },
                    None,
                )
            } else {
                None
            },
            if d.has_pr && d.stacked && !yes(d.parent_merged) {
                step("parent-wait", "wait",
                    format!("waiting on {parent} (#{}) to merge first", t("parent.pr.number")),
                    None,
                )
            } else {
                None
            },
            if d.has_pr && !d.review_required && !d.stacked && !d.parent_closed && !d.gh_behind && !yes(d.draft) && !d.bot_waiting {
                match d.merge_state.as_str() {
                    "CLEAN" | "UNSTABLE" | "HAS_HOOKS" => step("ready", "you", "ready to merge", cmd("gh pr merge")),
                    "UNKNOWN" => step("merge-wait", "wait", "GitHub hasn't decided whether it can merge yet", None),
                    m => step("blocked", "you",
                        format!("GitHub says {}: see why", why_blocked(m)),
                        cmd("gh pr view --web"),
                    ),
                }
            } else {
                None
            },
        ]
    };
    finish(d, all, &addr, &addr_why)
}

/// The question step (#167, #326); <asked> is the question's first line, and
/// ci-fix runs once <ci_once> while <ci_waits>.
fn question_step(d: &Derived, s: &Value, asked: &str, addr: &Gate, addr_why: &str, (ci_waits, ci_once): (&str, &str)) -> Option<Value> {
    let t = |p: &str| val::t(s, p);
    let is = |p: &str, lit: &str| at(s, p).as_str() == Some(lit);
    let asking = d.asking.as_str();
    let answer = "sd act answer \"<your answer>\"";
    let did = if is("question.kind", "gave-up") { "gave up" } else { "asks" };
    let and = if d.stored { ", and your answer" } else { "" };
    let (say, then) = if d.addr_moot {
        (format!("the agent {did} (address): {asked}; nothing is owed now, so it's moot: sd act ack --question drops it{and}"), cmd("sd act ack --question"))
    } else if d.ci_moot {
        let ci = if t("ci.verdict") == "green" { "CI is green at HEAD now" } else { "no CI ran at HEAD" };
        (format!("the agent {did} (ci-fix): {asked}; {ci}, so it's moot: sd act ack --question drops it{and}"), cmd("sd act ack --question"))
    } else if !ci_waits.is_empty() {
        (format!("the agent {did} (ci-fix): {asked}; ci-fix can't go on while {ci_waits}: {}", waits_say(s, &format!("once {ci_once}"))), cmd(answer))
    } else if asking == "address" && !addr.runs() {
        answer_refused(s, d, addr, addr_why, asked, push_step(d) || (d.ci_held && push_runs(d)))
    } else if asking == "resolve" {
        (format!("the agent gave up ({asking}): {asked}; resolve it by hand (sd act restack says how), or sd act ack --question to let it try again"), cmd("sd act restack"))
    } else if is("question.kind", "gave-up") {
        (format!("the agent gave up ({asking}): {asked}; guide it with sd act answer, fix it by hand, or sd act ack --question to let it try again"), cmd(answer))
    } else {
        (format!("the agent asks ({asking}): {asked}; answer it with sd act answer; or answer in its session (sd act attach), then sd act ack --question"), cmd(answer))
    };
    step("question", "you", say, then)
}

/// The steps as listed: the first budget or failed step stands in for every
/// agent step, address refused with nothing ahead of it is chrismo's, and an
/// end state is listed only alone.
fn finish(d: &Derived, all: Vec<Option<Value>>, addr: &Gate, addr_why: &str) -> Vec<Value> {
    let mut seen_hold = false;
    let mut out: Vec<Value> = Vec::new();
    for st in all.into_iter().flatten() {
        if st["key"] == "budget" || st["key"] == "failed" {
            if seen_hold {
                continue;
            }
            seen_hold = true;
        }
        out.push(st);
    }
    // Address refused, with nothing ahead of it: chrismo's, with why. Behind a
    // step that doesn't clear the gate (attached, take), it stays an agent
    // step: that step blocks first.
    let first = out.first().map(|st| text(&st["key"])).unwrap_or_default();
    if (first == "bot" || (first == "answer" && d.asking == "address")) && !addr.runs() {
        let force = Some("sd act address --force".to_string());
        let (say, cmd) = if !matches!(addr, Gate::Soft(_)) {
            ("clear that first", None)
        } else if first == "answer" && conflicts(addr) {
            ("a restack would drop the question, so sd act address --force runs it now, or sd act ack --question drops both", force)
        } else {
            ("clear that first, or sd act address --force", force)
        };
        let why = if first == "bot" { format!(": sd act address refuses while {addr_why}") } else { String::new() };
        let said = format!("{}{why}: {say}", text(&out[0]["say"]));
        out[0]["who"] = json!("you");
        out[0]["say"] = json!(said);
        out[0]["cmd"] = json!(cmd);
    }
    // ready, merge-wait and blocked are the end state, listed only alone.
    if out.len() > 1 && matches!(out[out.len() - 1]["key"].as_str(), Some("ready" | "merge-wait" | "blocked")) {
        out.pop();
    }
    out
}

/// A worded address refusal, as one line.
fn refusal(g: Gate) -> String {
    match g {
        Gate::Hard(why) | Gate::Done(why) => why,
        Gate::Soft(why) => why.join("; "),
        Gate::Warn(_) | Gate::Go => String::new(),
    }
}

fn conflicts(g: &Gate) -> bool {
    matches!(g, Gate::Soft(why) if why.iter().any(|w| w == crate::act::gates::addr::CONFLICT))
}

/// The question step for an address question that address's gate <g>
/// refuses. sd act answer stores an answer to run once soft reasons clear;
/// a restack would drop the question, so a conflict is answered with
/// --force, after the push when NEXT offers one (<can_push>).
fn answer_refused(s: &Value, d: &Derived, g: &Gate, why: &str, asked: &str, can_push: bool) -> (String, Option<String>) {
    let did = if val::t(s, "question.kind") == "gave-up" { "gave up" } else { "asks" };
    let (soft, hard, conflict): (&[String], _, _) = match g {
        Gate::Soft(why) => (why, false, conflicts(g)),
        _ => (&[], true, false),
    };
    if !hard && !conflict {
        return (
            format!("the agent {did} (address): {asked}; address refuses while {why}: {}", waits_say(s, "once that clears")),
            cmd("sd act answer \"<your answer>\""),
        );
    }
    let head = format!("the agent {did} (address): {asked}; sd act answer refuses while {why}");
    let answer = "answer it with sd act answer --force, since a restack first would drop the question";
    let pushes = crate::act::gates::address_pushes(d) && can_push;
    let others = soft.len().saturating_sub(usize::from(pushes) + usize::from(conflict));
    if hard || others > 0 {
        (format!("{head}: clear that first, or answer in its session (sd act attach), then sd act ack --question"), None)
    } else if pushes {
        let push = if at(s, "push.held").is_null() {
            "push first,".to_string()
        } else {
            format!("review the agent's CI config change ({}), then sd act push;", sj(at(s, "push.held.paths"), ", "))
        };
        (format!("{head}: {push} then {answer}"), cmd("sd act push"))
    } else {
        (format!("{head}: {answer}"), cmd("sd act answer --force \"<your answer>\""))
    }
}

/// How a question whose step's gate refuses is answered, to run <once>; a
/// give-up keeps its other ways on.
fn waits_say(s: &Value, once: &str) -> String {
    if val::t(s, "question.kind") == "gave-up" {
        format!("guide it with sd act answer, and it runs {once}; or fix it by hand, or sd act ack --question to let it try again")
    } else {
        format!("answer it with sd act answer, and it runs {once}")
    }
}


/// A followed PR's steps (#250).
fn reviewer(d: &Derived, s: &Value, hold: &Option<Value>, again: String) -> Vec<Option<Value>> {
    let g = |p: &str| at(s, p);
    let n = |p: &str| val::n(s, p);
    let pushed = n("push.behind") > 0.0 || val::on(s, "push.author_pushed");
    let current = d.rv_current;
    vec![
        if d.merged {
            step("merged", "run",
                "merged: drop the branch's pins and enrollment, remove its worktree and delete it",
                cmd("sd act forget --remove"),
            )
        } else {
            None
        },
        if d.closed {
            step("closed", "you", "PR closed without merging: forget the branch, and remove its worktree", cmd("sd act forget"))
        } else {
            None
        },
        if !d.merged && !d.closed && d.errors {
            step("errors", "you",
                format!("{}: the steps below may be wrong", plural(&json!(len(g("errors"))), "error")),
                cmd("sd state --json"),
            )
        } else {
            None
        },
        if d.rv_yours {
            step("yours", "you",
                format!(
                    "{} has {} the PR doesn't: move {} to a branch of your own; review and fetch wait",
                    val::t(s, "branch"),
                    plural(g("push.ahead"), "commit"),
                    if n("push.ahead") == 1.0 { "it" } else { "them" }
                ),
                None,
            )
        } else {
            None
        },
        if d.rv_dirty {
            step("dirty", "you",
                "the worktree has uncommitted changes, which review and fetch refuse: discard them (git reset --hard; git clean for untracked files)",
                cmd("git status"),
            )
        } else {
            None
        },
        if d.rv_fetch {
            let say = if pushed { "the author pushed: take it" } else { "fetch: GitHub is ahead of your remote refs" };
            step("fetch", "run", say, cmd("sd act fetch"))
        } else {
            None
        },
        if current && !d.rv_dirty && !d.rv_submitted && !d.rv_local {
            agent(hold, "review", again, "sd act review")
        } else {
            None
        },
        if current && ((!d.rv_submitted && d.rv_local) || d.rv_pending) {
            let say = if d.rv_pending {
                "finish your pending review on GitHub".to_string()
            } else if val::on(s, "review.findings.at_head") && n("review.findings.n") > 0.0 {
                format!("{} at HEAD (sd state --long): put them in your review on GitHub", plural(g("review.findings.n"), "review finding"))
            } else {
                "the reviewer found nothing at HEAD: approve or comment on GitHub".to_string()
            };
            step("submit", "you", say, cmd(format!("gh pr view {} --web", text(g("pr.number")))))
        } else {
            None
        },
        if current && d.rv_submitted {
            step("author-wait", "wait",
                format!("you reviewed HEAD ({}): waiting on the author", val::t(s, "review.github.state").to_lowercase().replace('_', " ")),
                None,
            )
        } else {
            None
        },
    ]
}

fn minutes(w: &Value) -> String {
    match w.as_i64() {
        Some(i) => (i / 60).to_string(),
        None => crate::json::float(w.as_f64().unwrap_or(0.0) / 60.0),
    }
}

/// The NEXT line: the first step and a count of the rest; with all, every
/// step, the rest as "then".
pub fn lines(steps: &Value, all: bool) -> String {
    let ls: Vec<String> = steps
        .as_array()
        .map(|a| {
            a.iter()
                .map(|st| {
                    let who = text(&st["who"]);
                    let pad = " ".repeat(5usize.saturating_sub(who.len()));
                    let arrow = if st["cmd"].is_null() { String::new() } else { format!("  → {}", text(&st["cmd"])) };
                    format!("{who}{pad}  {}{arrow}", text(&st["say"]))
                })
                .collect()
        })
        .unwrap_or_default();
    let Some(first) = ls.first() else { return String::new() };
    if all {
        let mut out = format!("NEXT  {first}\n");
        for l in &ls[1..] {
            out.push_str(&format!("then  {l}\n"));
        }
        out
    } else {
        let more = if ls.len() > 1 { format!("  (+{} more)", ls.len() - 1) } else { String::new() };
        format!("NEXT  {first}{more}\n")
    }
}
