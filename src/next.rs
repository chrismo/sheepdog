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
// The default branch (or a branch that is its own base) gets one step, to
// start a branch (#159). A branch with nothing ahead of its base and no PR
// has nothing to push or ship: its task is implemented, or with no task it's
// chrismo's (#143). So are uncommitted changes there, which implement refuses;
// and with errors, pr may be wrong, so nothing spends on implementing. A
// remote with commits reset away here isn't empty: the #86 steps handle it.
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
// when its description is stale, which spends. Spend that can't be read, with
// a budget set, holds the same way: a cost guard fails closed. spends() makes
// that swap, agent() for agent steps; the dedupe at the end keeps the first.
// Not guarded: ship-parent, whose describe spends on the parent; this state
// has only this branch's spend, so the parent's budget isn't checked here.
//
// After FAIL_LIMIT agent steps failed in a row (failed, #242), the agent
// steps go to chrismo the same way.
//
// A followed PR (mode reviewer, sd act follow, #250) is a teammate's: none of
// the author's steps, only its own list (reviewer below). The author's pushes
// are fetched before anything else; the agent reviews HEAD; then the review
// is chrismo's to submit on GitHub, and once it's of HEAD the author's turn.
//
// Field reads follow the SuperDB query this replaced (src/val.rs), and an
// equality holds only between two present values.

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

/// The steps for an sd state record, first is next.
pub fn steps(s: &Value, remote: &str) -> Result<Value, String> {
    if !s.is_object() {
        return Err("sd state JSON must be one record".into());
    }
    let g = |p: &str| at(s, p);
    let on = |p: &str| val::on(s, p);
    let off = |p: &str| g(p) == &Value::Bool(false);
    let n = |p: &str| val::n(s, p);
    let nul = |p: &str| g(p).is_null();
    let is = |p: &str, lit: &str| g(p).as_str() == Some(lit);
    let eq = |a: &str, b: &str| !nul(a) && !nul(b) && g(a) == g(b);
    let ne = |a: &str, b: &str| !nul(a) && !nul(b) && g(a) != g(b);
    let status_in = |p: &str, set: &[&str]| g(p).get("status").and_then(Value::as_str).is_some_and(|x| set.contains(&x));
    let covers = |p: &str| status_in(p, &["covered", "rewritten-same"]);
    let trails = |p: &str| status_in(p, &["uncovered", "rewritten-changed", "unknown"]);
    let t = |p: &str| val::t(s, p);

    let pr_null = is_null(s, "pr");
    let open = pr_null || is("pr.state", "OPEN");
    let has_pr = is("pr.state", "OPEN");
    let foreign = off("push.remote_ours") && n("push.theirs") > 0.0;
    let bot_blocking = n("threads.owed_bot") - n("threads.info");
    let reset_away = on("push.remote_ours") && n("push.behind") > 0.0 && n("push.ahead") == 0.0;
    let dropping = reset_away && on("push.remote_made");
    let taken = reset_away && off("push.remote_made");
    let held = reset_away && (off("push.remote_made") || (on("push.remote_made") && on("push.dirty")));
    let unpushed = is_null(s, "push.remote_sha") || n("push.ahead") > 0.0;
    let to_push = unpushed || (dropping && !on("push.dirty"));
    let ci_held = !nul("push.held");
    let chase = ne("sync.target", "sync.default") || on("parent.merged");
    let undone = n("sync.undone") > 0.0;
    let merge_state = g("pr.merge_state").as_str().unwrap_or("UNKNOWN").to_string();
    let gh_behind = is("pr.merge_state", "BEHIND");
    let ci_ok = is("ci.verdict", "green") || is("ci.verdict", "none");
    // sd state reads the required checks of every draft: null is a failed read
    let draft_ready = on("pr.draft") && !(unpushed || reset_away) && !foreign && ci_ok && !is_null(s, "ci.required");
    let review_required = is("pr.review_decision", "REVIEW_REQUIRED");
    let off_default = ne("pr.base", "sync.default");
    let stacked = off_default && (is("parent.pr.state", "OPEN") || on("parent.merged"));
    let parent_closed = off_default && is("parent.pr.state", "CLOSED");
    let parent_unpushed = on("parent.unpushed");
    let old_base_null = is_null(s, "sync.old_base");
    let no_base = old_base_null && chase;
    let restack = if old_base_null {
        format!("git rebase {remote}/{}", t("sync.target"))
    } else {
        let ob = t("sync.old_base");
        format!("git rebase --onto {remote}/{} {}", t("sync.target"), &ob[..ob.len().min(12)])
    };
    let any_cover = covers("review.local") || covers("review.bot") || covers("review.accepted");
    let reviewed = trails("review.local") || trails("review.bot") || trails("review.accepted");
    let need_review = (is("pr.state", "OPEN") || reviewed || off("review.findings.at_head")) && !any_cover;
    let fixing = on("review.findings.at_head") && !any_cover;
    // an accept stands in for the local review, never for the bot (#181); it ends a silent bot wait
    // only when made after that trigger: it recorded the trigger it followed (no clocks compared, #63)
    let bot_due = !nul("bot.command") && (covers("review.local") || covers("review.accepted")) && !covers("review.bot");
    let bot_late = n("review.bot.waited") > n("bot.timeout");
    let bot_waiting = covers("review.bot")
        && off("review.bot.replied")
        && !(covers("review.accepted") && bot_late && eq("review.accepted.after", "review.bot.triggered"));
    let own_base = eq("branch", "sync.default") || eq("sync.target", "branch");
    let empty = pr_null && g("sync.ahead").as_f64() == Some(0.0) && n("push.behind") == 0.0;
    let tasked = g("task").as_str().is_some_and(|x| !x.is_empty());
    let asking = if on("question.at_head") { t("question.step") } else { String::new() };
    let asked = if on("question.at_head") {
        t("question.text").split('\n').find(|l| l.chars().any(|c| !matches!(c, '\t' | '\n' | '\x0c' | '\r' | ' '))).unwrap_or("").to_string()
    } else {
        String::new()
    };
    let last_review = ["review.local", "review.bot", "review.accepted"]
        .iter()
        .filter(|p| status_in(p, &["uncovered"]))
        .map(|p| at(g(p), "uncovered"))
        .filter(|v| !v.is_null())
        .min_by(|a, b| a.as_f64().partial_cmp(&b.as_f64()).unwrap_or(std::cmp::Ordering::Equal))
        .cloned();
    let budget = if nul("spend") || n("spend.budget") <= 0.0 {
        None
    } else if nul("spend.usd") {
        step("budget", "you",
            "can't tell what this branch has spent (sd state's errors say why), so nothing spends: fix the usage record, or take over",
            cmd("sd act attach"),
        )
    } else if n("spend.usd") > n("spend.budget") {
        step("budget", "you",
            format!(
                "spent {} on this branch, past settings.branchBudget ({}): raise it, or take over",
                usd(n("spend.usd")),
                usd(n("spend.budget"))
            ),
            cmd("sd act attach"),
        )
    } else {
        None
    };
    let hold = budget.or_else(|| {
        if n("failed.count") < FAIL_LIMIT {
            return None;
        }
        step("failed", "you",
            format!(
                "{} agent steps failed in a row, the last {} {} at {}: {}; fix what fails, then rerun it",
                t("failed.count"), t("failed.harness"), t("failed.step"), t("failed.at"), t("failed.error")
            ),
            rerun(&t("failed.step")).or_else(|| cmd(format!("git update-ref -d refs/sheepdog/{}/failed", t("branch")))),
        )
    });
    let hold = &hold;

    let ci_red = has_pr && is("ci.verdict", "red") && on("ci.for_head") && !to_push;
    let fix_due = fixing && !on("review.findings.declined") && n("review.findings.round") < n("review.findings.cap");
    let behind_target = off("sync.up_to_date");
    let conflict = behind_target && on("sync.conflict");
    let chase_behind = behind_target && !on("sync.conflict") && chase;
    let gh_restack = has_pr && !on("sync.conflict") && gh_behind;
    let restack_due = open
        && !undone
        && !parent_closed
        && if conflict {
            !on("sync.by_tree")
        } else if behind_target && chase {
            !old_base_null
        } else {
            gh_restack
        };
    let dirty = open
        && on("push.dirty")
        && asking.is_empty()
        && (ci_red
            || bot_blocking > 0.0
            || fix_due
            || (need_review && !fixing)
            || restack_due
            || (pr_null && !empty && !dropping));
    let refused = dirty || (open && on("push.dirty") && !asking.is_empty());

    let branch = t("branch");
    let target = t("sync.target");
    let parent = t("parent.name");
    let one = |k: &str, a: &str, b: &str| if n(k) == 1.0 { a } else { b }.to_string();

    let all: Vec<Option<Value>> = if own_base {
        vec![step("default", "you", format!("on {branch}: start a branch"), cmd("sd act start"))]
    } else if !nul("session.running") {
        vec![step("running", "wait",
            format!("{} running since {}", t("session.running.step"), t("session.running.since")),
            None,
        )]
    } else if is("mode", "reviewer") {
        let again = match &last_review {
            Some(k) => format!("review again: {} since the last review", plural(k, "commit")),
            None if reviewed => "review again: the PR was rewritten since the last review".into(),
            None => "review the PR's change".into(),
        };
        reviewer(s, hold, again)
    } else {
        vec![
            if is("pr.state", "MERGED") {
                step("merged", "run",
                    "merged: drop the branch's pins and enrollment, remove its worktree and delete it",
                    cmd("sd act forget --remove"),
                )
            } else {
                None
            },
            if is("pr.state", "CLOSED") { step("closed", "you", "PR closed without merging", None) } else { None },
            if open && len(g("errors")) > 0 {
                step("errors", "you",
                    format!("{}: the steps below may be wrong", plural(&json!(len(g("errors"))), "error")),
                    cmd("sd state --json"),
                )
            } else {
                None
            },
            if open && off("fresh") {
                step("stale", "run",
                    "fetch: GitHub is ahead of your remote refs, so the steps below may be wrong",
                    cmd(format!("git fetch --prune {remote}")),
                )
            } else {
                None
            },
            if open && len(g("lag")) > 0 {
                step("gh-wait", "wait", "GitHub hasn't seen the push yet, so the steps below may be wrong", None)
            } else {
                None
            },
            if open && on("session.locked") {
                step("attached", "wait",
                    "attached: the branch's session is open interactively; agent steps wait until it exits",
                    None,
                )
            } else {
                None
            },
            if open && foreign {
                step("foreign", "you",
                    format!("{remote}/{branch} has {} this copy has never had", plural(g("push.theirs"), "commit")),
                    cmd(format!("git log HEAD..{remote}/{branch}")),
                )
            } else {
                None
            },
            if open && n("threads.owed_human") > 0.0 {
                step("human", "you",
                    format!("reply to {}", plural(g("threads.owed_human"), "human item")),
                    cmd("sd state --long"),
                )
            } else {
                None
            },
            if open && !asking.is_empty() {
                let answer = "sd act answer \"<your answer>\"";
                let (say, then) = if asking == "resolve" {
                    (format!("the agent gave up ({asking}): {asked}; resolve it by hand (sd act restack says how), or sd act ack --question to let it try again"), "sd act restack")
                } else if is("question.kind", "gave-up") {
                    (format!("the agent gave up ({asking}): {asked}; guide it with sd act answer, fix it by hand, or sd act ack --question to let it try again"), answer)
                } else {
                    (format!("the agent asks ({asking}): {asked}; answer it with sd act answer; or answer in its session (sd act attach), then sd act ack --question"), answer)
                };
                step("question", "you", say, cmd(then))
            } else {
                None
            },
            if open && undone {
                let pin = if no_base {
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
                    if no_base { None } else { cmd(format!("{restack} && sd act push")) },
                )
            } else {
                None
            },
            if open && parent_unpushed {
                step("ship-parent", "run",
                    format!("ship the parent first: {parent} isn't on {remote} yet"),
                    cmd(format!("sd act ship {parent}")),
                )
            } else {
                None
            },
            if open && !undone && !parent_closed && conflict {
                if on("sync.by_tree") {
                    step("conflict", "you",
                        format!("restack onto {target} by tree conflicts, which --resolve can't do: resolve it by hand (sd act restack says how)"),
                        cmd("sd act restack"),
                    )
                } else if refused || asking == "resolve" {
                    None
                } else {
                    agent(hold, "conflict", format!("restack onto {target} conflicts: resolve it"), "sd act restack --resolve")
                }
            } else {
                None
            },
            if open && !undone && !parent_closed && chase_behind {
                if old_base_null {
                    step("behind", "you",
                        format!("restack onto {target} ({} behind): no base to restack from; see sd state's errors", t("sync.behind")),
                        cmd("sd state --json"),
                    )
                } else if refused {
                    None
                } else if on("sync.by_tree") {
                    step("behind", "run",
                        format!("restack onto {target} by tree ({} behind): its commits become one", t("sync.behind")),
                        cmd("sd act restack"),
                    )
                } else {
                    step("behind", "run", format!("restack onto {target} ({} behind)", t("sync.behind")), cmd("sd act restack"))
                }
            } else if gh_restack && !undone && !parent_closed && !refused {
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
            if dirty && !dropping {
                step("dirty", "you",
                    "the worktree has uncommitted changes, which the agent steps refuse: they're yours: commit and push them (sd act push), or discard them (git reset --hard; git clean for untracked files)",
                    cmd("git status"),
                )
            } else {
                None
            },
            if ci_red && asking != "ci-fix" && !refused {
                agent(hold, "ci-red", format!("fix CI: {}", sj(g("ci.failing"), ", ")), "sd act ci-fix")
            } else {
                None
            },
            if has_pr && is("ci.verdict", "blocked") && on("ci.for_head") && !to_push {
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
            if open && bot_blocking > 0.0 && asking != "address" && !refused {
                agent(hold, "bot", format!("address {}", plural(g("threads.owed_bot"), "bot item")), "sd act address")
            } else {
                None
            },
            // only once nothing else moves without chrismo (#168)
            if open
                && bot_blocking == 0.0
                && n("threads.info") > 0.0
                && !to_push
                && !need_review
                && !fixing
                && !bot_due
                && !bot_waiting
                && !restack_due
                && !is("ci.verdict", "inconclusive")
                && !(stacked && on("parent.merged"))
            {
                step("read", "you",
                    format!("read {} in sd state --long, then ack", plural(g("threads.info"), "bot item")),
                    cmd("sd act ack"),
                )
            } else {
                None
            },
            if open && taken {
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
            if open && dropping && on("push.dirty") {
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
            if open && ci_held {
                let paths = sj(g("push.held.paths"), ", ");
                if foreign || undone || parent_unpushed {
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
            if empty && on("push.dirty") {
                step("nothing-dirty", "you",
                    format!(
                        "nothing to ship, and the worktree has uncommitted changes: commit them{}",
                        if tasked { ", or remove them and sd act implement" } else { "" }
                    ),
                    None,
                )
            } else if empty && tasked {
                if len(g("errors")) == 0 && asking != "implement" {
                    agent(hold, "implement", format!("implement the task: nothing ahead of {target} yet"), "sd act implement")
                } else {
                    None
                }
            } else if empty {
                step("nothing", "you", "nothing to ship: commit, or record a task", None)
            } else {
                None
            },
            if open && !ci_held && !foreign && !undone && !parent_unpushed && !empty && to_push {
                let ahead = if nul("sync.ahead") { g("push.ahead") } else { g("sync.ahead") };
                step("push", "run",
                    if is_null(s, "push.remote_sha") {
                        format!("push: not on {remote} yet")
                    } else if on("push.diverged") {
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
            if pr_null && len(g("errors")) == 0 && !parent_unpushed && !ci_held && !empty && !refused {
                spends(hold, step("open-pr", "run", "ship: push, open a draft PR", cmd("sd act ship")))
            } else {
                None
            },
            if has_pr && draft_ready && !need_review && !bot_due && !bot_waiting {
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
            if open && fixing && (asking == "review-fix" || (refused && fix_due)) {
                None
            } else if open && fix_due {
                agent(hold, "review-fix", format!(
                        "fix {} (round {}/{})",
                        plural(g("review.findings.n"), "review finding"),
                        t("review.findings.round"),
                        t("review.findings.cap")
                    ),
                    "sd act implement",
                )
            } else if open && fixing {
                let findings = plural(g("review.findings.n"), "review finding");
                let what = if on("review.findings.declined") {
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
            } else if open && need_review && !refused {
                let say = match &last_review {
                    Some(k) => format!("review again: {} since the last review", plural(k, "commit")),
                    None if reviewed => "review again: the branch was rewritten since the last review".into(),
                    None => "review the branch's change".into(),
                };
                agent(hold, "review", say, "sd act review")
            } else {
                None
            },
            if has_pr && !to_push && !held && !foreign && bot_due {
                step("bot-trigger", "run",
                    format!("trigger the review bot ({})", t("bot.command")),
                    cmd("sd act trigger"),
                )
            } else {
                None
            },
            if has_pr && is("ci.verdict", "inconclusive") {
                step("ci-rerun", "run", "rerun cancelled CI", cmd("gh pr checks"))
            } else {
                None
            },
            if has_pr && bot_waiting && !bot_late {
                step("bot-wait", "wait", format!("waiting on the review bot ({})", t("bot.command")), None)
            } else {
                None
            },
            if has_pr && bot_waiting && bot_late {
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
            if has_pr && !to_push && !held && (is("ci.verdict", "pending") || off("ci.for_head")) {
                let say = if is("ci.verdict", "pending") {
                    let missing = g("ci.missing");
                    if len(g("ci.pending")) == 0 && len(missing) > 0 {
                        let draft = if on("pr.draft") {
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
            if has_pr && parent_closed {
                step("parent-closed", "you",
                    format!("{parent} (#{}) was closed without merging: decide what this PR targets", t("parent.pr.number")),
                    None,
                )
            } else {
                None
            },
            if has_pr && stacked && on("parent.merged") {
                step("retarget", "run",
                    format!("{} already merged: point this PR at {}", t("pr.base"), t("sync.default")),
                    cmd("sd act retarget"),
                )
            } else {
                None
            },
            if has_pr && review_required {
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
            if has_pr && stacked && !on("parent.merged") {
                step("parent-wait", "wait",
                    format!("waiting on {parent} (#{}) to merge first", t("parent.pr.number")),
                    None,
                )
            } else {
                None
            },
            if has_pr && !review_required && !stacked && !parent_closed && !gh_behind && !on("pr.draft") && !bot_waiting {
                match merge_state.as_str() {
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

    // The first budget or failed step stands in for every agent step; the rest go.
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
    // ready, merge-wait and blocked are the end state, listed only alone.
    if out.len() > 1 && matches!(out[out.len() - 1]["key"].as_str(), Some("ready" | "merge-wait" | "blocked")) {
        out.pop();
    }
    Ok(Value::Array(out))
}

/// A followed PR's steps (#250).
fn reviewer(s: &Value, hold: &Option<Value>, again: String) -> Vec<Option<Value>> {
    let g = |p: &str| at(s, p);
    let on = |p: &str| val::on(s, p);
    let n = |p: &str| val::n(s, p);
    let is = |p: &str, lit: &str| g(p).as_str() == Some(lit);
    let covers = |p: &str| g(p).get("status").and_then(Value::as_str).is_some_and(|x| matches!(x, "covered" | "rewritten-same"));
    let open = is("pr.state", "OPEN");
    // Commits the PR's head doesn't have, with nothing new from the author:
    // chrismo's (sd act fetch refuses them).
    let yours = open && n("push.ahead") > 0.0 && n("push.behind") == 0.0;
    let dirty = open && on("push.dirty");
    let pushed = n("push.behind") > 0.0 || on("push.author_pushed");
    let fetch = open && !yours && !dirty && (g("fresh") == &Value::Bool(false) || pushed);
    let current = open && !yours && !fetch;
    let pending = on("review.github.pending");
    let submitted = on("review.github.at_head") && !pending;
    let local = covers("review.local") || covers("review.accepted") || on("review.findings.at_head");
    vec![
        if is("pr.state", "MERGED") {
            step("merged", "run",
                "merged: drop the branch's pins and enrollment, remove its worktree and delete it",
                cmd("sd act forget --remove"),
            )
        } else {
            None
        },
        if is("pr.state", "CLOSED") {
            step("closed", "you", "PR closed without merging: forget the branch, and remove its worktree", cmd("sd act forget"))
        } else {
            None
        },
        if !is("pr.state", "MERGED") && !is("pr.state", "CLOSED") && len(g("errors")) > 0 {
            step("errors", "you",
                format!("{}: the steps below may be wrong", plural(&json!(len(g("errors"))), "error")),
                cmd("sd state --json"),
            )
        } else {
            None
        },
        if yours {
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
        if dirty {
            step("dirty", "you",
                "the worktree has uncommitted changes, which review and fetch refuse: discard them (git reset --hard; git clean for untracked files)",
                cmd("git status"),
            )
        } else {
            None
        },
        if fetch {
            let say = if pushed { "the author pushed: take it" } else { "fetch: GitHub is ahead of your remote refs" };
            step("fetch", "run", say, cmd("sd act fetch"))
        } else {
            None
        },
        if current && !dirty && !submitted && !local {
            agent(hold, "review", again, "sd act review")
        } else {
            None
        },
        if current && ((!submitted && local) || pending) {
            let say = if pending {
                "finish your pending review on GitHub".to_string()
            } else if on("review.findings.at_head") && n("review.findings.n") > 0.0 {
                format!("{} at HEAD (sd state --long): put them in your review on GitHub", plural(g("review.findings.n"), "review finding"))
            } else {
                "the reviewer found nothing at HEAD: approve or comment on GitHub".to_string()
            };
            step("submit", "you", say, cmd(format!("gh pr view {} --web", text(g("pr.number")))))
        } else {
            None
        },
        if current && submitted {
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
