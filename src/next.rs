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
// moot. The restack steps ask gates::restack, and the conflict step
// gates::restack_resolve: one they refuse is chrismo's, saying why, and
// GitHub's BEHIND with nothing new on the target is a wait (#348).
//
// The default branch (or a branch that is its own base) gets one step, to
// start a branch (#159). A branch with nothing ahead of its base and no PR
// has nothing to push or ship: its task is implemented, or with no task it's
// chrismo's (#143). So are uncommitted changes there, which implement refuses.
// Implement and review-fix are offered only when gates::implement says Go
// (#348): with errors, pr may be wrong, so nothing spends on implementing;
// past settings.reviewRounds the findings are chrismo's. Review, the
// branch's own or a followed PR's, only when gates::review says Go; when only
// its Hard rules hold it (no worktree, no base), review-held is chrismo's.
// A remote with commits reset away here isn't empty: the #86 steps handle it.
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
pub mod steps;
pub use derived::{round_key, Derived};
#[cfg(test)]
pub(crate) use derived::arb as derived_arb;
pub(crate) use derived::{no, yes};

use crate::act::Gate;
use steps::{def, StepDef};
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

/// A step of <def>'s key; def! checks the key against STEPS (#335).
fn step(def: &StepDef, who: &str, say: impl Into<String>, cmd: Option<String>) -> Option<Value> {
    Some(json!({"key": def.key, "who": who, "say": say.into(), "cmd": cmd}))
}

fn spends(hold: &Option<Value>, s: Option<Value>) -> Option<Value> {
    if hold.is_some() {
        hold.clone()
    } else {
        s
    }
}

fn agent(hold: &Option<Value>, def: &StepDef, say: impl Into<String>, cmd: &str) -> Option<Value> {
    spends(hold, step(def, "agent", say, Some(cmd.into())))
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
    step(def!("foreign"), "you",
        format!("{remote}/{branch} has {} this copy has never had", plural(at(s, "push.theirs"), "commit")),
        cmd(format!("git log HEAD..{remote}/{branch}")),
    )
}

/// Commits someone else made that the push would remove from the remote,
/// or ones it can't tell who made (#395), when that's all that holds it
/// (#356): chrismo looks, then forces it. Not while attached: --force would
/// push past the sd-lock too.
fn foreign_authors_step(d: &Derived, s: &Value, remote: &str) -> Option<Value> {
    let holds = d.to_push && !d.empty && !push_runs(d) && push_runs(&Derived { foreign_authors: Some(false), ..d.clone() });
    if !holds || yes(d.locked) {
        return None;
    }
    let branch = val::t(s, "branch");
    let look = format!("git log --format='%h %ae %ce %s' {branch}..{remote}/{branch}");
    let say = if yes(d.foreign_authors) {
        format!("the push removes commits from {remote}/{branch} by {}: look ({look}), then force it", sj(at(s, "push.foreign_authors"), ", "))
    } else {
        format!("can't tell who made the commits the push removes from {remote}/{branch} (no user.email, or git failed): set user.email, or look ({look}), then force it")
    };
    step(def!("foreign-authors"), "you", say, cmd("sd act push --force"))
}

/// How to redo by hand a restack that was undone, as one command: NEXT's
/// restack-undone step, and sd act restack's refusal (#395). None with no
/// base to redo it from.
pub(crate) fn redo_restack(d: &Derived, s: &Value, remote: &str) -> Option<String> {
    if d.no_base {
        return None;
    }
    let (target, ob) = (val::t(s, "sync.target"), val::t(s, "sync.old_base"));
    Some(if d.old_base_null {
        format!("git rebase {remote}/{target} && sd act push")
    } else {
        format!("git rebase --onto {remote}/{target} {} && sd act push", &ob[..ob.len().min(12)])
    })
}

/// The restack was undone (#47): chrismo redoes it by hand, before any push.
fn undone_step(d: &Derived, s: &Value, remote: &str) -> Option<Value> {
    let t = |p: &str| val::t(s, p);
    let (target, branch) = (t("sync.target"), t("branch"));
    let redo = redo_restack(d, s, remote);
    let pin = if redo.is_none() {
        format!(
            "; no base to redo it from on {target}: pin one by hand (git update-ref refs/sheepdog/{branch}/base <the commit {branch} left {} at>), then sd act restack",
            t("parent.name")
        )
    } else {
        String::new()
    };
    let copies = if val::n(s, "sync.undone") == 1.0 { "is a copy" } else { "are copies" };
    step(def!("restack-undone"), "you",
        format!(
            "the restack was undone ({} of {target}'s commits {copies} on {branch}; a plain git push after a restack?): redo it, and don't sd act push before that, which would publish the copies{pin}",
            t("sync.undone")
        ),
        redo,
    )
}

/// A restack its gate <g> refuses, as chrismo's <key> step: <say>, then
/// sd act <verb>'s reasons. Hard says how by hand; Soft is forced past.
fn refused_restack(key: &StepDef, say: String, verb: &str, g: Gate, s: &Value, remote: &str) -> Option<Value> {
    let (why, then) = match crate::act::restack_said(g, s, remote) {
        Gate::Soft(why) => (why.join("; "), format!("{verb} --force")),
        Gate::Hard(why) | Gate::Done(why) => (why, verb.to_string()),
        Gate::Warn(_) | Gate::Go => return None,
    };
    step(key, "you", format!("{say}: {verb} refuses: {why}"), cmd(then))
}

/// The restack that would conflict: the agent's, as gates::restack_resolve
/// lets it (#348).
fn conflict_step(d: &Derived, s: &Value, remote: &str, hold: &Option<Value>) -> Option<Value> {
    if !d.open || d.undone || d.parent_closed || !d.conflict {
        return None;
    }
    let target = val::t(s, "sync.target");
    if yes(d.by_tree) {
        return step(def!("conflict"), "you",
            format!("restack onto {target} by tree conflicts, which --resolve can't do: resolve it by hand (sd act restack says how)"),
            cmd("sd act restack"),
        );
    }
    if d.refused || d.asking == "resolve" {
        return None;
    }
    let say = format!("restack onto {target} conflicts");
    match crate::act::gates::restack_resolve(d) {
        Gate::Go | Gate::Warn(_) => agent(hold, def!("conflict"), format!("{say}: resolve it"), "sd act restack --resolve"),
        g => refused_restack(def!("conflict"), say, "sd act restack --resolve", g, s, remote),
    }
}

/// The restack behind a target worth chasing, or the default branch when
/// GitHub requires it: run, as gates::restack lets it (#348).
fn behind_step(d: &Derived, s: &Value, remote: &str) -> Option<Value> {
    let t = |p: &str| val::t(s, p);
    let target = t("sync.target");
    let say = if d.open && !d.undone && !d.parent_closed && d.chase_behind {
        if d.old_base_null {
            return step(def!("behind"), "you",
                format!("restack onto {target} ({} behind): no base to restack from; see sd state's errors", t("sync.behind")),
                cmd("sd state --json"),
            );
        }
        if d.refused {
            return None;
        }
        let (tree, one) = if yes(d.by_tree) { (" by tree", ": its commits become one") } else { ("", "") };
        format!("restack onto {target}{tree} ({} behind){one}", t("sync.behind"))
    } else if d.gh_restack && !d.undone && !d.parent_closed && !d.refused {
        let n = if at(s, "sync.behind").is_null() { "0".into() } else { t("sync.behind") };
        format!("restack onto {} ({n} behind): GitHub requires branches to be up to date", t("sync.default"))
    } else {
        return None;
    };
    match crate::act::gates::restack(d) {
        Gate::Go | Gate::Warn(_) => step(def!("behind"), "run", say, cmd("sd act restack")),
        Gate::Done(_) => step(def!("behind"), "wait",
            format!("GitHub says the PR is behind {}, but {remote}/{target} has nothing {} lacks: wait for GitHub, or fetch", t("sync.default"), t("branch")),
            cmd(format!("git fetch --prune {remote}")),
        ),
        g => refused_restack(def!("behind"), say, "sd act restack", g, s, remote),
    }
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
        step(def!("budget"), "you", say, cmd("sd act attach"))
    } else if d.failed_out {
        step(def!("failed"), "you",
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

/// What every step of the author's reads: the record, and the gates and
/// text worked out once for all of them.
struct Cx<'a> {
    d: &'a Derived,
    s: &'a Value,
    remote: &'a str,
    hold: Option<Value>,
    /// The question's first line, when one stands.
    asked: String,
    last_review: Option<Value>,
    implement: Gate,
    fixes: bool,
    addr: Gate,
    addr_why: String,
    ci_fix: Gate,
    ci_once: &'static str,
    /// Why ci-fix waits, while its own question stands.
    ci_waits: String,
}

impl Cx<'_> {
    fn new<'a>(d: &'a Derived, s: &'a Value, remote: &'a str) -> Cx<'a> {
        let asked = if d.asking.is_empty() {
            String::new()
        } else {
            val::t(s, "question.text")
                .split('\n')
                .find(|l| l.chars().any(|c| !matches!(c, '\t' | '\n' | '\x0c' | '\r' | ' ')))
                .unwrap_or("")
                .to_string()
        };
        let implement = crate::act::gates::implement(d);
        let fixes = crate::act::gates::implement_fixes(d) && implement.runs();
        let ci_rules = crate::act::gates::ci_fix_rules(d);
        let (ci_fix, ci_once) = (crate::act::judge(&ci_rules), crate::act::gates::ci_fix_once(&ci_rules));
        let ci_waits = if let (Gate::Soft(why), "ci-fix") = (&ci_fix, d.asking.as_str()) { why.join("; ") } else { String::new() };
        Cx {
            d,
            s,
            remote,
            hold: hold_step(d, s),
            asked,
            last_review: last_review(s),
            implement,
            fixes,
            addr: crate::act::gates::address(d),
            addr_why: refusal(crate::act::address_said(crate::act::gates::address(d), s)),
            ci_fix,
            ci_once,
            ci_waits,
        }
    }

    fn g(&self, p: &str) -> &Value {
        at(self.s, p)
    }

    fn t(&self, p: &str) -> String {
        val::t(self.s, p)
    }

    fn one(&self, k: &str, a: &str, b: &str) -> String {
        if val::n(self.s, k) == 1.0 { a } else { b }.to_string()
    }

    fn address(&self) -> Option<Value> {
        agent(&self.hold, def!("bot"), format!("address {}", plural(self.g("threads.owed_bot"), "bot item")), "sd act address")
    }
}

/// The author's steps, in the order NEXT lists them: what blocks everything,
/// restack, agent fixes, one push, then review and wait (the header above).
const AUTHOR: &[fn(&Cx) -> Option<Value>] = &[
    merged, closed, errors, stale, gh_wait, attached, foreign, human, question, undone, ship_parent, conflict, behind, dirty,
    ci_red, ci_blocked, bot, read, take, reset_dirty, nothing, verify_red, foreign_authors, ci_config, push, bot_refused,
    open_pr, draft, review, bot_trigger, ci_rerun, bot_wait, bot_silent, ci_wait, ci_answer, parent_closed, retarget,
    review_wait, parent_wait, end,
];

/// The steps for <d>, first is next. Which steps, and whose, come from <d>
/// alone; <s> is the record it was derived from, which some say and cmd
/// text still reads.
pub(crate) fn offer(d: &Derived, s: &Value, remote: &str) -> Vec<Value> {
    let cx = Cx::new(d, s, remote);
    let all: Vec<Option<Value>> = if d.own_base {
        vec![step(def!("default"), "you", format!("on {}: start a branch", cx.t("branch")), cmd("sd act start"))]
    } else if d.running {
        vec![step(def!("running"), "wait",
            format!("{} running since {}", cx.t("session.running.step"), cx.t("session.running.since")),
            None,
        )]
    } else if d.reviewer {
        let again = match &cx.last_review {
            Some(k) => format!("review again: {} since the last review", plural(k, "commit")),
            None if d.reviewed => "review again: the PR was rewritten since the last review".into(),
            None => "review the PR's change".into(),
        };
        reviewer(d, s, &cx.hold, again)
    } else {
        AUTHOR.iter().map(|f| f(&cx)).collect()
    };
    finish(d, all, &cx.addr, &cx.addr_why)
}

fn merged(cx: &Cx) -> Option<Value> {
    if !cx.d.merged {
        return None;
    }
    step(def!("merged"), "run",
        "merged: drop the branch's pins and enrollment, remove its worktree and delete it",
        cmd("sd act forget --remove"),
    )
}

fn closed(cx: &Cx) -> Option<Value> {
    if !cx.d.closed {
        return None;
    }
    step(def!("closed"), "you", "PR closed without merging", None)
}

fn errors(cx: &Cx) -> Option<Value> {
    if !(cx.d.open && cx.d.errors) {
        return None;
    }
    step(def!("errors"), "you",
        format!("{}: the steps below may be wrong", plural(&json!(len(cx.g("errors"))), "error")),
        cmd("sd state --json"),
    )
}

fn stale(cx: &Cx) -> Option<Value> {
    if !(cx.d.open && cx.d.stale) {
        return None;
    }
    step(def!("stale"), "run",
        "fetch: GitHub is ahead of your remote refs, so the steps below may be wrong",
        cmd(format!("git fetch --prune {}", cx.remote)),
    )
}

fn gh_wait(cx: &Cx) -> Option<Value> {
    if !(cx.d.open && cx.d.lagging) {
        return None;
    }
    step(def!("gh-wait"), "wait", "GitHub hasn't seen the push yet, so the steps below may be wrong", None)
}

fn attached(cx: &Cx) -> Option<Value> {
    if !(cx.d.open && yes(cx.d.locked)) {
        return None;
    }
    step(def!("attached"), "wait",
        "attached: the branch's session is open interactively; agent steps wait until it exits",
        None,
    )
}

fn foreign(cx: &Cx) -> Option<Value> {
    foreign_step(cx.d, cx.s, cx.remote)
}

fn human(cx: &Cx) -> Option<Value> {
    if !(cx.d.open && cx.d.owed_human) {
        return None;
    }
    step(def!("human"), "you", format!("reply to {}", plural(cx.g("threads.owed_human"), "human item")), cmd("sd state --long"))
}

fn question(cx: &Cx) -> Option<Value> {
    if !cx.d.open || cx.d.asking.is_empty() || cx.d.answer_pending {
        return None;
    }
    question_step(cx.d, cx.s, &cx.asked, &cx.addr, &cx.addr_why, (&cx.ci_waits, cx.ci_once))
}

fn undone(cx: &Cx) -> Option<Value> {
    if !(cx.d.open && cx.d.undone) {
        return None;
    }
    undone_step(cx.d, cx.s, cx.remote)
}

fn ship_parent(cx: &Cx) -> Option<Value> {
    if !(cx.d.open && cx.d.parent_unpushed) {
        return None;
    }
    let parent = cx.t("parent.name");
    step(def!("ship-parent"), "run",
        format!("ship the parent first: {parent} isn't on {} yet", cx.remote),
        cmd(format!("sd act ship {parent}")),
    )
}

fn conflict(cx: &Cx) -> Option<Value> {
    conflict_step(cx.d, cx.s, cx.remote, &cx.hold)
}

fn behind(cx: &Cx) -> Option<Value> {
    behind_step(cx.d, cx.s, cx.remote)
}

fn dirty(cx: &Cx) -> Option<Value> {
    if !cx.d.dirty || cx.d.dropping {
        return None;
    }
    step(def!("dirty"), "you",
        "the worktree has uncommitted changes, which the agent steps refuse: they're yours: commit and push them (sd act push), or discard them (git reset --hard; git clean for untracked files)",
        cmd("git status"),
    )
}

fn ci_red(cx: &Cx) -> Option<Value> {
    let d = cx.d;
    let failing = sj(cx.g("ci.failing"), ", ");
    if cx.ci_fix.runs() && d.asking != "ci-fix" && !d.refused {
        agent(&cx.hold, def!("ci-red"), format!("fix CI: {failing}"), "sd act ci-fix")
    } else if d.resume_ci && cx.ci_fix.runs() {
        agent(&cx.hold, def!("ci-red"), format!("fix CI: {failing}, going on with your answer to its question"), "sd act ci-fix")
    } else {
        None
    }
}

fn ci_blocked(cx: &Cx) -> Option<Value> {
    let d = cx.d;
    if !(d.has_pr && d.ci_blocked && !d.to_push) {
        return None;
    }
    step(def!("ci-blocked"), "you",
        format!("CI blocked: {} didn't start or want approval; an agent can't fix that", sj(cx.g("ci.blocked"), ", ")),
        cmd("gh pr checks"),
    )
}

/// address, while its gate lets it.
fn bot(cx: &Cx) -> Option<Value> {
    if !cx.addr.runs() {
        None
    } else if cx.d.bot {
        cx.address()
    } else if cx.d.resume {
        let say = format!("address {}, going on with your answer to its question", plural(cx.g("threads.owed_bot"), "bot item"));
        agent(&cx.hold, def!("bot"), say, "sd act address")
    } else {
        None
    }
}

/// Info bot items, only once nothing else moves without chrismo (#168).
fn read(cx: &Cx) -> Option<Value> {
    let d = cx.d;
    let quiet = d.bot_blocking == 0.0
        && !d.to_push
        && !d.need_review
        && !d.fixing
        && !d.bot_due
        && !d.bot_waiting
        && !d.restack_due
        && !d.ci_inconclusive
        && !(d.stacked && yes(d.parent_merged));
    if !(d.open && d.info && quiet) {
        return None;
    }
    step(def!("read"), "you",
        format!("read {} in sd state --long, then ack", plural(cx.g("threads.info"), "bot item")),
        cmd("sd act ack"),
    )
}

fn take(cx: &Cx) -> Option<Value> {
    if !(cx.d.open && cx.d.taken) {
        return None;
    }
    let (remote, branch) = (cx.remote, cx.t("branch"));
    let them = cx.one("push.behind", "it", "them");
    step(def!("take"), "you",
        format!(
            "{remote}/{branch} has {} this branch took but didn't make, reset away here: take {them} back (git merge --ff-only {remote}/{branch}), or drop {them} by hand",
            plural(cx.g("push.behind"), "commit")
        ),
        None,
    )
}

fn reset_dirty(cx: &Cx) -> Option<Value> {
    if !(cx.d.open && cx.d.dropping && yes(cx.d.push_dirty)) {
        return None;
    }
    step(def!("reset-dirty"), "you",
        format!(
            "{}/{} has {} reset away here, and the worktree has uncommitted changes: commit them (then sd act push replaces the remote {}), or remove them (git reset --hard; git clean for untracked files)",
            cx.remote,
            cx.t("branch"),
            plural(cx.g("push.behind"), "commit"),
            cx.one("push.behind", "commit", "commits")
        ),
        None,
    )
}

/// Nothing ahead of the target: implement the task, or chrismo's.
fn nothing(cx: &Cx) -> Option<Value> {
    let d = cx.d;
    if !d.empty {
        None
    } else if yes(d.push_dirty) {
        step(def!("nothing-dirty"), "you",
            format!(
                "nothing to ship, and the worktree has uncommitted changes: commit them{}",
                if d.tasked { ", or remove them and sd act implement" } else { "" }
            ),
            None,
        )
    } else if d.tasked {
        if cx.implement.runs() && d.asking != "implement" {
            agent(&cx.hold, def!("implement"), format!("implement the task: nothing ahead of {} yet", cx.t("sync.target")), "sd act implement")
        } else {
            None
        }
    } else {
        step(def!("nothing"), "you", "nothing to ship: commit, or record a task", None)
    }
}

fn verify_red(cx: &Cx) -> Option<Value> {
    if !cx.d.verify_red {
        return None;
    }
    let sha = cx.t("verify.sha");
    step(def!("verify-red"), "you",
        format!("verify red at {} ({}: {}): fix it, then sd act verify", &sha[..sha.len().min(7)], cx.t("verify.step"), cx.t("verify.cmd")),
        cmd("sd act verify"),
    )
}

fn foreign_authors(cx: &Cx) -> Option<Value> {
    foreign_authors_step(cx.d, cx.s, cx.remote)
}

/// After what holds the push, which its say points up to.
fn ci_config(cx: &Cx) -> Option<Value> {
    if !(cx.d.open && cx.d.ci_held) {
        return None;
    }
    let paths = sj(cx.g("push.held.paths"), ", ");
    if !push_runs(cx.d) {
        step(def!("ci-config"), "you", format!("the agent changed CI config ({paths}): review it; push once the steps above are done"), None)
    } else {
        step(def!("ci-config"), "you", format!("the agent changed CI config ({paths}): review, then sd act push"), cmd("sd act push"))
    }
}

fn push(cx: &Cx) -> Option<Value> {
    if !push_step(cx.d) {
        return None;
    }
    let (s, remote) = (cx.s, cx.remote);
    let ahead = if cx.g("sync.ahead").is_null() { cx.g("push.ahead") } else { cx.g("sync.ahead") };
    let say = if is_null(s, "push.remote_sha") {
        format!("push: not on {remote} yet")
    } else if val::on(s, "push.diverged") {
        format!("force-push {} (rewritten); a plain git push is rejected, and pulling would undo the rewrite", plural(ahead, "commit"))
    } else if val::n(s, "push.ahead") == 0.0 {
        format!("force-push: drop {} the remote has, reset away here", plural(cx.g("push.behind"), "commit"))
    } else {
        format!("push {}", plural(cx.g("push.ahead"), "commit"))
    };
    step(def!("push"), "run", say, cmd("sd act push"))
}

/// address, behind the push, while its gate refuses it: finish says why
/// when it's first.
fn bot_refused(cx: &Cx) -> Option<Value> {
    if cx.addr.runs() {
        None
    } else if cx.d.bot {
        cx.address()
    } else if cx.d.resume {
        // it runs address, and finish's rewrite may offer --force
        spends(&cx.hold, step(def!("answer"), "wait", format!("your answer to the address agent's question waits while {}", cx.addr_why), None))
    } else {
        None
    }
}

fn open_pr(cx: &Cx) -> Option<Value> {
    let d = cx.d;
    if !d.pr_null || d.errors || d.verify_red || d.parent_unpushed || d.ci_held || d.empty || d.refused {
        return None;
    }
    spends(&cx.hold, step(def!("open-pr"), "run", "ship: push, open a draft PR", cmd("sd act ship")))
}

fn draft(cx: &Cx) -> Option<Value> {
    let d = cx.d;
    if !(d.has_pr && d.draft_ready && !d.verify_red && !d.need_review && !d.bot_due && !d.bot_waiting) {
        return None;
    }
    let why = if cx.g("ci.verdict").as_str() == Some("none") { "reviewed, no CI checks" } else { "CI green and reviewed" };
    step(def!("draft"), "run", format!("{why}: mark the PR ready for review"), cmd("gh pr ready"))
}

/// Review findings to fix, or the branch's own review.
fn review(cx: &Cx) -> Option<Value> {
    let d = cx.d;
    let findings = || plural(cx.g("review.findings.n"), "review finding");
    if d.open && d.fixing && (d.asking == "review-fix" || (d.refused && cx.fixes)) {
        None
    } else if cx.fixes {
        let say = format!("fix {} (round {}/{})", findings(), cx.t("review.findings.round"), cx.t("review.findings.cap"));
        agent(&cx.hold, def!("review-fix"), say, "sd act implement")
    } else if d.open && d.fixing {
        let what = if yes(d.findings_declined) {
            format!("the agent declined {}", findings())
        } else {
            format!("{} after {} rounds", findings(), cx.t("review.findings.round"))
        };
        step(def!("review-decide"), "you",
            format!("{what}: fix {}, or accept HEAD as reviewed (sd state --long lists them)", cx.one("review.findings.n", "it", "them")),
            cmd("sd act review --accept"),
        )
    } else if d.need_review {
        review_step(d, &cx.hold, &cx.last_review)
    } else {
        None
    }
}

fn bot_trigger(cx: &Cx) -> Option<Value> {
    let d = cx.d;
    if !(d.has_pr && !d.to_push && !d.held && !d.foreign && !d.verify_red && d.bot_due) {
        return None;
    }
    step(def!("bot-trigger"), "run", format!("trigger the review bot ({})", cx.t("bot.command")), cmd("sd act trigger"))
}

fn ci_rerun(cx: &Cx) -> Option<Value> {
    if !(cx.d.has_pr && cx.d.ci_inconclusive) {
        return None;
    }
    step(def!("ci-rerun"), "run", "rerun cancelled CI", cmd("gh pr checks"))
}

fn bot_wait(cx: &Cx) -> Option<Value> {
    if !(cx.d.has_pr && cx.d.bot_waiting && !cx.d.bot_late) {
        return None;
    }
    step(def!("bot-wait"), "wait", format!("waiting on the review bot ({})", cx.t("bot.command")), None)
}

fn bot_silent(cx: &Cx) -> Option<Value> {
    if !(cx.d.has_pr && cx.d.bot_waiting && cx.d.bot_late) {
        return None;
    }
    step(def!("bot-silent"), "you",
        format!(
            "the review bot hasn't answered in {} min: check it, trigger it again, or accept HEAD as reviewed (sd act review --accept)",
            minutes(cx.g("review.bot.waited"))
        ),
        cmd("sd act trigger --force"),
    )
}

fn ci_wait(cx: &Cx) -> Option<Value> {
    let d = cx.d;
    if !(d.has_pr && !d.to_push && !d.held && (d.ci_pending || no(d.ci_for_head))) {
        return None;
    }
    let missing = cx.g("ci.missing");
    let say = if !d.ci_pending {
        "CI hasn't reported on HEAD yet".into()
    } else if len(cx.g("ci.pending")) == 0 && len(missing) > 0 {
        let draft = if yes(d.draft) {
            format!("; if {} on a draft, undraft by hand (gh pr ready)", if len(missing) == 1 { "it never runs" } else { "they never run" })
        } else {
            String::new()
        };
        format!("waiting on required check{} {} to report{draft}", if len(missing) == 1 { "" } else { "s" }, sj(missing, ", "))
    } else {
        "CI running".into()
    };
    step(def!("ci-wait"), "wait", say, cmd("gh pr checks --watch"))
}

fn ci_answer(cx: &Cx) -> Option<Value> {
    if !cx.d.resume_ci || cx.ci_waits.is_empty() {
        return None;
    }
    step(def!("answer"), "wait", format!("your answer to the ci-fix agent's question waits while {}", cx.ci_waits), None)
}

fn parent_closed(cx: &Cx) -> Option<Value> {
    if !(cx.d.has_pr && cx.d.parent_closed) {
        return None;
    }
    step(def!("parent-closed"), "you",
        format!("{} (#{}) was closed without merging: decide what this PR targets", cx.t("parent.name"), cx.t("parent.pr.number")),
        None,
    )
}

fn retarget(cx: &Cx) -> Option<Value> {
    if !(cx.d.has_pr && cx.d.stacked && yes(cx.d.parent_merged)) {
        return None;
    }
    step(def!("retarget"), "run",
        format!("{} already merged: point this PR at {}", cx.t("pr.base"), cx.t("sync.default")),
        cmd("sd act retarget"),
    )
}

fn review_wait(cx: &Cx) -> Option<Value> {
    if !(cx.d.has_pr && cx.d.review_required) {
        return None;
    }
    let reqs = cx.g("pr.review_requests");
    let say = if len(reqs) > 0 { format!("waiting on review from {}", sj(reqs, ", ")) } else { "waiting on a required review".into() };
    step(def!("review-wait"), "wait", say, None)
}

fn parent_wait(cx: &Cx) -> Option<Value> {
    if !(cx.d.has_pr && cx.d.stacked && !yes(cx.d.parent_merged)) {
        return None;
    }
    step(def!("parent-wait"), "wait", format!("waiting on {} (#{}) to merge first", cx.t("parent.name"), cx.t("parent.pr.number")), None)
}

/// ready, merge-wait or blocked: the end state, listed only alone (finish).
fn end(cx: &Cx) -> Option<Value> {
    let d = cx.d;
    if !d.has_pr || d.review_required || d.stacked || d.parent_closed || d.gh_behind || yes(d.draft) || d.bot_waiting {
        return None;
    }
    match d.merge_state.as_str() {
        "CLEAN" | "UNSTABLE" | "HAS_HOOKS" => step(def!("ready"), "you", "ready to merge", cmd("gh pr merge")),
        "UNKNOWN" => step(def!("merge-wait"), "wait", "GitHub hasn't decided whether it can merge yet", None),
        m => step(def!("blocked"), "you", format!("GitHub says {}: see why", why_blocked(m)), cmd("gh pr view --web")),
    }
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
    step(def!("question"), "you", say, then)
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
            step(def!("merged"), "run",
                "merged: drop the branch's pins and enrollment, remove its worktree and delete it",
                cmd("sd act forget --remove"),
            )
        } else {
            None
        },
        if d.closed {
            step(def!("closed"), "you", "PR closed without merging: forget the branch, and remove its worktree", cmd("sd act forget"))
        } else {
            None
        },
        if !d.merged && !d.closed && d.errors {
            step(def!("errors"), "you",
                format!("{}: the steps below may be wrong", plural(&json!(len(g("errors"))), "error")),
                cmd("sd state --json"),
            )
        } else {
            None
        },
        if d.rv_yours {
            step(def!("yours"), "you",
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
            step(def!("dirty"), "you",
                "the worktree has uncommitted changes, which review and fetch refuse: discard them (git reset --hard; git clean for untracked files)",
                cmd("git status"),
            )
        } else {
            None
        },
        if d.rv_fetch {
            let say = if pushed { "the author pushed: take it" } else { "fetch: GitHub is ahead of your remote refs" };
            step(def!("fetch"), "run", say, cmd("sd act fetch"))
        } else {
            None
        },
        if crate::act::gates::review(d).runs() {
            agent(hold, def!("review"), again, "sd act review")
        } else {
            review_held(d)
        },
        if current && ((!d.rv_submitted && d.rv_local) || d.rv_pending) {
            let say = if d.rv_pending {
                "finish your pending review on GitHub".to_string()
            } else if val::on(s, "review.findings.at_head") && n("review.findings.n") > 0.0 {
                format!("{} at HEAD (sd state --long): put them in your review on GitHub", plural(g("review.findings.n"), "review finding"))
            } else {
                "the reviewer found nothing at HEAD: approve or comment on GitHub".to_string()
            };
            step(def!("submit"), "you", say, cmd(format!("gh pr view {} --web", text(g("pr.number")))))
        } else {
            None
        },
        if current && d.rv_submitted {
            step(def!("author-wait"), "wait",
                format!("you reviewed HEAD ({}): waiting on the author", val::t(s, "review.github.state").to_lowercase().replace('_', " ")),
                None,
            )
        } else {
            None
        },
    ]
}

/// The branch's own review: the agent's when gates::review says Go.
fn review_step(d: &Derived, hold: &Option<Value>, last_review: &Option<Value>) -> Option<Value> {
    if !crate::act::gates::review(d).runs() {
        return review_held(d);
    }
    let say = match last_review {
        Some(k) => format!("review again: {} since the last review", plural(k, "commit")),
        None if d.reviewed => "review again: the branch was rewritten since the last review".into(),
        None => "review the branch's change".into(),
    };
    agent(hold, def!("review"), say, "sd act review")
}

/// chrismo's step when only a Hard rule of review's gate holds it back.
fn review_held(d: &Derived) -> Option<Value> {
    let (why, clears) = crate::act::gates::review_held(d)?;
    step(def!("review-held"), "you", format!("review can't run: {why}: {clears}"), None)
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
