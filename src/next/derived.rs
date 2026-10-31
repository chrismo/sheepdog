// Every predicate NEXT decides by, read once from the sd state JSON (#348,
// #335). next::offer reads its conditions only from here, and a verb's gate
// (src/act/gates.rs) asks the same fields. A JSON flag lifted as it is stays
// Option<bool>: unknown is not false. derived!'s random() names every field
// with no ..Default, so a new field can't escape the generator.

use crate::val::{self, at};
use serde_json::Value;

macro_rules! derived {
    (@gen $r:ident $t:ty) => { <$t as crate::next::derived::arb::Arb>::arb($r) };
    (@gen $r:ident $t:ty, $gen:expr) => { $gen($r) };
    ($( $(#[doc = $doc:literal])* $f:ident : $t:ty $(=> $gen:expr)? ),* $(,)?) => {
        /// What NEXT decides by, from one sd state record.
        #[derive(Clone, Debug, Default, PartialEq)]
        pub struct Derived {
            $( $(#[doc = $doc])* pub $f: $t, )*
        }

        #[cfg(test)]
        impl Derived {
            pub const FIELDS: &'static [&'static str] = &[$(stringify!($f)),*];

            /// A value with every field drawn from <r>.
            pub fn random(r: &mut crate::next::derived::arb::Rng) -> Derived {
                Derived { $( $f: derived!(@gen r $t $(, $gen)?), )* }
            }

            /// Field <i> (of FIELDS) back to its default.
            pub fn reset(&mut self, i: usize) {
                let fresh = Derived::default();
                let mut k = 0;
                $( if k == i { self.$f = fresh.$f.clone(); } k += 1; )*
                let _ = k;
            }

            /// Rust that rebuilds this value: its fields off the default.
            pub fn builder_call(&self) -> String {
                use crate::next::derived::arb::Lit;
                let fresh = Derived::default();
                let mut set: Vec<String> = Vec::new();
                $( if self.$f != fresh.$f { set.push(format!("{}: {}", stringify!($f), self.$f.lit())); } )*
                if set.is_empty() {
                    "Derived::default()".into()
                } else {
                    format!("Derived {{ {}, ..Derived::default() }}", set.join(", "))
                }
            }
        }
    };
}

derived! {
    // The branch and its PR.
    /// On the default branch, or a branch that is its own base (#159).
    own_base: bool,
    /// A step is running on the branch (session.running, #170).
    running: bool,
    /// A followed PR, a teammate's (mode reviewer, #250).
    reviewer: bool,
    /// pr is present and null: no PR yet.
    pr_null: bool,
    /// No PR yet, or the PR is open.
    open: bool,
    /// The PR is open.
    has_pr: bool,
    /// The PR merged.
    merged: bool,
    /// The PR closed without merging.
    closed: bool,
    /// sd state reported errors.
    errors: bool,
    /// GitHub is ahead of the remote refs (fresh false).
    stale: bool,
    /// GitHub hasn't seen a push yet (lag, #246).
    lagging: bool,
    /// The branch's session is open interactively (session.locked).
    locked: Option<bool>,
    /// Human items are owed a reply.
    owed_human: bool,
    /// Bot items are owed a reply.
    owed_bot: bool,
    /// Bot items owed less the info ones: the blocking bot count.
    bot_blocking: f64 => |r: &mut arb::Rng| r.pick(&[0.0, 1.0, 2.0]),
    /// Info bot items are owed a read.
    info: bool,
    /// The PR is a draft.
    draft: Option<bool>,
    /// GitHub requires a review.
    review_required: bool,
    /// GitHub's merge state; UNKNOWN when null.
    merge_state: String => |r: &mut arb::Rng| r.pick(&["CLEAN", "UNSTABLE", "HAS_HOOKS", "UNKNOWN", "BLOCKED", "DIRTY", "DRAFT", "BEHIND"]).to_string(),
    /// The branch has a task recorded.
    tasked: bool,
    /// verify failed on HEAD, with the PR open or not made yet (#325).
    verify_red: bool,

    // Local vs remote (O4).
    /// The remote has commits this copy never had.
    foreign: bool,
    /// Our remote has commits reset away here.
    reset_away: bool,
    /// Reset away, and this branch made them: a force-push drops them.
    dropping: bool,
    /// Reset away, and this branch took them without making them.
    taken: bool,
    /// Reset away, and nothing pushes it yet: taken, or dropping with edits.
    held: bool,
    /// Commits not on the remote, or no remote branch.
    unpushed: bool,
    /// Something for sd act push to publish.
    to_push: bool,
    /// The agent changed CI config, held for chrismo's review (push.held).
    ci_held: bool,
    /// The worktree has uncommitted changes.
    push_dirty: Option<bool>,
    /// The parent isn't on the remote yet.
    parent_unpushed: bool,
    /// No PR, nothing ahead of the base and nothing behind the remote.
    empty: bool,

    // Sync with the parent (O2, O3).
    /// The target isn't the default branch, or the parent merged: restack after it.
    chase: bool,
    /// A restack was undone: some of the target's commits are copies here (#47).
    undone: bool,
    /// Up to date with the target (sync.up_to_date).
    up_to_date: Option<bool>,
    /// Behind the target.
    behind_target: bool,
    /// A restack would conflict (sync.conflict).
    sync_conflict: Option<bool>,
    /// Behind the target, and a restack would conflict.
    conflict: bool,
    /// Behind a target worth chasing, with no conflict.
    chase_behind: bool,
    /// GitHub says the PR is behind its base (merge state BEHIND).
    gh_behind: bool,
    /// GitHub requires the PR up to date, and a restack wouldn't conflict.
    gh_restack: bool,
    /// The restack goes by tree (sync.by_tree).
    by_tree: Option<bool>,
    /// No pinned base (sync.old_base is null).
    old_base_null: bool,
    /// No base to restack from, with a target worth chasing.
    no_base: bool,
    /// A restack NEXT offers to someone: the agent, the pipeline or chrismo.
    restack_due: bool,
    /// The PR targets an open parent, or one that merged.
    stacked: bool,
    /// The parent's PR closed without merging.
    parent_closed: bool,
    /// The parent merged (parent.merged).
    parent_merged: Option<bool>,

    // CI (O5).
    /// CI is red on HEAD, with nothing to push.
    ci_red: bool,
    /// CI is blocked on HEAD: a check didn't start, or wants approval.
    ci_blocked: bool,
    /// CI was cancelled.
    ci_inconclusive: bool,
    /// CI is running.
    ci_pending: bool,
    /// CI's verdict is for HEAD (ci.for_head).
    ci_for_head: Option<bool>,
    /// A draft with nothing to push, CI green or none, and its required checks read.
    draft_ready: bool,

    // Review (O6).
    /// A local, bot or accepted review covers HEAD.
    any_cover: bool,
    /// A review trails HEAD: uncovered, rewritten or unknown.
    reviewed: bool,
    /// HEAD needs a review.
    need_review: bool,
    /// Review findings stand at HEAD, with no review covering it.
    fixing: bool,
    /// The agent declined the findings.
    findings_declined: Option<bool>,
    /// The findings are owed another fix round.
    fix_due: bool,
    /// The bot is owed a trigger: a local or accepted review covers HEAD, the bot's doesn't.
    bot_due: bool,
    /// The bot has waited past its timeout.
    bot_late: bool,
    /// Waiting on the bot's reply to a trigger at HEAD.
    bot_waiting: bool,

    // Spend and failures (#121, #242).
    /// A budget is set and the spend can't be read: a cost guard fails closed.
    budget_unread: bool,
    /// The spend is past settings.branchBudget.
    over_budget: bool,
    /// FAIL_LIMIT agent steps failed in a row.
    failed_out: bool,

    // An agent's question (#167, #326).
    /// The step whose question stands at HEAD, else empty.
    asking: String => |r: &mut arb::Rng| r.pick(&["", "address", "ci-fix", "implement", "review-fix", "review", "resolve"]).to_string(),
    /// chrismo's answer is stored for it.
    stored: bool,
    /// address's gate refuses (hard or soft).
    addr_refuses: bool,
    /// address's gate refuses hard.
    addr_hard: bool,
    /// One of address's soft reasons is a restack that would conflict.
    addr_conflict: bool,
    /// An address question with nothing owed: moot.
    addr_moot: bool,
    /// A ci-fix question CI passing at HEAD made moot.
    ci_moot: bool,
    /// ci-fix waits on CI (a ci-fix question only).
    ci_waits: bool,
    /// ci-fix's gate runs, or no ci-fix question stands.
    ci_runs: bool,
    /// A stored answer to address's question goes on.
    resume: bool,
    /// A stored answer to ci-fix's question goes on.
    resume_ci: bool,
    /// resume or resume_ci.
    answer_pending: bool,
    /// The worktree's edits are chrismo's and an agent step refuses them.
    dirty: bool,
    /// What would move HEAD waits: dirty, or a question stands.
    refused: bool,
    /// Bot items to address now.
    bot: bool,

    // A followed PR (#250).
    /// Commits the PR's head doesn't have, with nothing new from the author.
    rv_yours: bool,
    /// A followed PR's worktree has uncommitted changes.
    rv_dirty: bool,
    /// The author pushed, or GitHub is ahead: fetch.
    rv_fetch: bool,
    /// Caught up with the PR.
    rv_current: bool,
    /// chrismo has a pending review on GitHub.
    rv_pending: bool,
    /// chrismo's review on GitHub is of HEAD.
    rv_submitted: bool,
    /// The local review (or an accept, or findings) covers HEAD.
    rv_local: bool,
}

/// An Option<bool> flag that is true.
pub fn yes(o: Option<bool>) -> bool {
    o == Some(true)
}

/// An Option<bool> flag that is false: known, not unknown.
pub fn no(o: Option<bool>) -> bool {
    o == Some(false)
}

/// Reads of one record: an equality holds only between two present values.
struct J<'a>(&'a Value);

impl J<'_> {
    fn g(&self, p: &str) -> &Value {
        at(self.0, p)
    }
    fn on(&self, p: &str) -> bool {
        val::on(self.0, p)
    }
    fn off(&self, p: &str) -> bool {
        self.g(p) == &Value::Bool(false)
    }
    fn flag(&self, p: &str) -> Option<bool> {
        self.g(p).as_bool()
    }
    fn n(&self, p: &str) -> f64 {
        val::n(self.0, p)
    }
    fn nul(&self, p: &str) -> bool {
        self.g(p).is_null()
    }
    fn null(&self, p: &str) -> bool {
        super::is_null(self.0, p)
    }
    fn is(&self, p: &str, lit: &str) -> bool {
        self.g(p).as_str() == Some(lit)
    }
    fn eq(&self, a: &str, b: &str) -> bool {
        !self.nul(a) && !self.nul(b) && self.g(a) == self.g(b)
    }
    fn ne(&self, a: &str, b: &str) -> bool {
        !self.nul(a) && !self.nul(b) && self.g(a) != self.g(b)
    }
    fn status_in(&self, p: &str, set: &[&str]) -> bool {
        self.g(p).get("status").and_then(Value::as_str).is_some_and(|x| set.contains(&x))
    }
    fn covers(&self, p: &str) -> bool {
        self.status_in(p, &["covered", "rewritten-same"])
    }
    fn trails(&self, p: &str) -> bool {
        self.status_in(p, &["uncovered", "rewritten-changed", "unknown"])
    }
    fn len(&self, p: &str) -> usize {
        self.g(p).as_array().map_or(0, Vec::len)
    }
}

impl Derived {
    /// The predicates of sd state record <s>.
    pub fn new(s: &Value) -> Derived {
        let j = J(s);
        let mut d = Derived::default();
        d.pr(&j);
        d.push(&j);
        d.sync(&j);
        d.ci(&j);
        d.review(&j);
        d.spend(&j);
        d.question(s, &j);
        d.followed(&j);
        d
    }

    fn pr(&mut self, j: &J) {
        self.own_base = j.eq("branch", "sync.default") || j.eq("sync.target", "branch");
        self.running = !j.nul("session.running");
        self.reviewer = j.is("mode", "reviewer");
        self.pr_null = j.null("pr");
        self.open = self.pr_null || j.is("pr.state", "OPEN");
        self.has_pr = j.is("pr.state", "OPEN");
        self.merged = j.is("pr.state", "MERGED");
        self.closed = j.is("pr.state", "CLOSED");
        self.errors = j.len("errors") > 0;
        self.stale = j.off("fresh");
        self.lagging = j.len("lag") > 0;
        self.locked = j.flag("session.locked");
        self.owed_human = j.n("threads.owed_human") > 0.0;
        self.owed_bot = j.n("threads.owed_bot") > 0.0;
        self.bot_blocking = j.n("threads.owed_bot") - j.n("threads.info");
        self.info = j.n("threads.info") > 0.0;
        self.draft = j.flag("pr.draft");
        self.review_required = j.is("pr.review_decision", "REVIEW_REQUIRED");
        self.merge_state = j.g("pr.merge_state").as_str().unwrap_or("UNKNOWN").to_string();
        self.tasked = j.g("task").as_str().is_some_and(|x| !x.is_empty());
        self.verify_red = self.open && j.on("verify.at_head");
    }

    fn push(&mut self, j: &J) {
        self.foreign = j.off("push.remote_ours") && j.n("push.theirs") > 0.0;
        self.reset_away = j.on("push.remote_ours") && j.n("push.behind") > 0.0 && j.n("push.ahead") == 0.0;
        self.push_dirty = j.flag("push.dirty");
        let dirty = yes(self.push_dirty);
        self.dropping = self.reset_away && j.on("push.remote_made");
        self.taken = self.reset_away && j.off("push.remote_made");
        self.held = self.reset_away && (j.off("push.remote_made") || (j.on("push.remote_made") && dirty));
        self.unpushed = j.null("push.remote_sha") || j.n("push.ahead") > 0.0;
        self.to_push = self.unpushed || (self.dropping && !dirty);
        self.ci_held = !j.nul("push.held");
        self.parent_unpushed = j.on("parent.unpushed");
        self.undone = j.n("sync.undone") > 0.0;
        self.empty = self.pr_null && j.g("sync.ahead").as_f64() == Some(0.0) && j.n("push.behind") == 0.0;
    }

    fn sync(&mut self, j: &J) {
        self.chase = j.ne("sync.target", "sync.default") || j.on("parent.merged");
        self.up_to_date = j.flag("sync.up_to_date");
        self.behind_target = no(self.up_to_date);
        self.sync_conflict = j.flag("sync.conflict");
        let conflicts = yes(self.sync_conflict);
        self.conflict = self.behind_target && conflicts;
        self.chase_behind = self.behind_target && !conflicts && self.chase;
        self.gh_behind = j.is("pr.merge_state", "BEHIND");
        self.gh_restack = self.has_pr && !conflicts && self.gh_behind;
        self.by_tree = j.flag("sync.by_tree");
        self.old_base_null = j.null("sync.old_base");
        self.no_base = self.old_base_null && self.chase;
        let off_default = j.ne("pr.base", "sync.default");
        self.stacked = off_default && (j.is("parent.pr.state", "OPEN") || j.on("parent.merged"));
        self.parent_closed = off_default && j.is("parent.pr.state", "CLOSED");
        self.parent_merged = j.flag("parent.merged");
        let offered = if self.conflict {
            !yes(self.by_tree)
        } else if self.behind_target && self.chase {
            !self.old_base_null
        } else {
            self.gh_restack
        };
        self.restack_due = self.open && !self.undone && !self.parent_closed && offered;
    }

    fn ci(&mut self, j: &J) {
        self.ci_for_head = j.flag("ci.for_head");
        let at_head = yes(self.ci_for_head);
        self.ci_red = self.has_pr && j.is("ci.verdict", "red") && at_head && !self.to_push;
        self.ci_blocked = j.is("ci.verdict", "blocked") && at_head;
        self.ci_inconclusive = j.is("ci.verdict", "inconclusive");
        self.ci_pending = j.is("ci.verdict", "pending");
        let ci_ok = j.is("ci.verdict", "green") || j.is("ci.verdict", "none");
        // sd state reads the required checks of every draft: null is a failed read
        self.draft_ready =
            yes(self.draft) && !(self.unpushed || self.reset_away) && !self.foreign && ci_ok && !j.null("ci.required");
    }

    fn review(&mut self, j: &J) {
        self.any_cover = j.covers("review.local") || j.covers("review.bot") || j.covers("review.accepted");
        self.reviewed = j.trails("review.local") || j.trails("review.bot") || j.trails("review.accepted");
        self.need_review = (j.is("pr.state", "OPEN") || self.reviewed || j.off("review.findings.at_head")) && !self.any_cover;
        self.fixing = j.on("review.findings.at_head") && !self.any_cover;
        self.findings_declined = j.flag("review.findings.declined");
        self.fix_due =
            self.fixing && !yes(self.findings_declined) && j.n("review.findings.round") < j.n("review.findings.cap");
        // an accept stands in for the local review, never for the bot (#181); it ends a silent bot wait
        // only when made after that trigger: it recorded the trigger it followed (no clocks compared, #63)
        self.bot_due = !j.nul("bot.command")
            && (j.covers("review.local") || j.covers("review.accepted"))
            && !j.covers("review.bot");
        self.bot_late = j.n("review.bot.waited") > j.n("bot.timeout");
        self.bot_waiting = j.covers("review.bot")
            && j.off("review.bot.replied")
            && !(j.covers("review.accepted") && self.bot_late && j.eq("review.accepted.after", "review.bot.triggered"));
    }

    fn spend(&mut self, j: &J) {
        let budgeted = !j.nul("spend") && j.n("spend.budget") > 0.0;
        self.budget_unread = budgeted && j.nul("spend.usd");
        self.over_budget = budgeted && !j.nul("spend.usd") && j.n("spend.usd") > j.n("spend.budget");
        self.failed_out = j.n("failed.count") >= super::FAIL_LIMIT;
    }

    fn question(&mut self, s: &Value, j: &J) {
        let at_head = j.on("question.at_head");
        self.asking = if at_head { val::t(s, "question.step") } else { String::new() };
        // chrismo's answer, stored while its step's gate refused (#326)
        self.stored = at_head && !j.nul("question.answer");
        let addr = crate::act::address_gate(s);
        self.addr_refuses = addr.hard.is_some() || !addr.soft.is_empty();
        self.addr_hard = addr.hard.is_some();
        self.addr_conflict = addr.conflict;
        self.addr_moot = self.asking == "address" && addr.hard.is_none() && !self.owed_bot;
        let ci = if self.asking == "ci-fix" { crate::act::ci_fix_gate(s) } else { crate::act::CiFixGate::Runs };
        self.ci_moot = matches!(ci, crate::act::CiFixGate::Moot(_));
        self.ci_waits = matches!(ci, crate::act::CiFixGate::Waits { .. });
        self.ci_runs = ci == crate::act::CiFixGate::Runs;
        // a stored answer to a moot question has nothing to go on with
        self.resume = self.open && self.stored && self.asking == "address" && !self.addr_moot;
        self.resume_ci = self.open && self.stored && self.asking == "ci-fix" && !self.ci_moot;
        self.answer_pending = self.resume || self.resume_ci;
        let dirty = yes(self.push_dirty);
        self.dirty = self.open
            && dirty
            && self.asking.is_empty()
            && (self.ci_red
                || self.bot_blocking > 0.0
                || self.fix_due
                || (self.need_review && !self.fixing)
                || self.restack_due
                || (self.pr_null && !self.empty && !self.dropping));
        // while it stands, what would move HEAD waits: that drops the question
        self.refused = self.dirty || (self.open && !self.asking.is_empty() && (dirty || self.answer_pending));
        self.bot = self.open && self.bot_blocking > 0.0 && self.asking != "address" && !self.refused;
    }

    fn followed(&mut self, j: &J) {
        let open = self.has_pr;
        // Commits the PR's head doesn't have, with nothing new from the author:
        // chrismo's (sd act fetch refuses them).
        self.rv_yours = open && j.n("push.ahead") > 0.0 && j.n("push.behind") == 0.0;
        self.rv_dirty = open && j.on("push.dirty");
        let pushed = j.n("push.behind") > 0.0 || j.on("push.author_pushed");
        self.rv_fetch = open && !self.rv_yours && !self.rv_dirty && (j.off("fresh") || pushed);
        self.rv_current = open && !self.rv_yours && !self.rv_fetch;
        self.rv_pending = j.on("review.github.pending");
        self.rv_submitted = j.on("review.github.at_head") && !self.rv_pending;
        self.rv_local = j.covers("review.local") || j.covers("review.accepted") || j.on("review.findings.at_head");
    }
}

/// xorshift64, not proptest, while Derived is flat (#348).
#[cfg(test)]
pub mod arb {
    use super::Derived;

    pub struct Rng(u64);

    impl Rng {
        pub fn new(seed: u64) -> Rng {
            Rng(seed.max(1))
        }
        pub fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0 = x;
            x
        }
        pub fn pick<T: Clone>(&mut self, from: &[T]) -> T {
            from[(self.next() % from.len() as u64) as usize].clone()
        }
    }

    /// A field type the generator draws.
    pub trait Arb {
        fn arb(r: &mut Rng) -> Self;
    }
    impl Arb for bool {
        fn arb(r: &mut Rng) -> bool {
            r.next() & 1 == 1
        }
    }
    impl Arb for Option<bool> {
        fn arb(r: &mut Rng) -> Option<bool> {
            r.pick(&[None, Some(false), Some(true)])
        }
    }

    /// A field's value as Rust source.
    pub trait Lit {
        fn lit(&self) -> String;
    }
    impl Lit for bool {
        fn lit(&self) -> String {
            self.to_string()
        }
    }
    impl Lit for f64 {
        fn lit(&self) -> String {
            format!("{self:?}")
        }
    }
    impl Lit for String {
        fn lit(&self) -> String {
            format!("{self:?}.into()")
        }
    }
    impl Lit for Option<bool> {
        fn lit(&self) -> String {
            format!("{self:?}")
        }
    }

    /// The seeds CI runs; SD_GEN_CASES sets the cases per seed (default 500).
    pub const SEEDS: [u64; 4] = [1, 0x5eed, 0xdead_beef, 348];

    pub fn cases() -> usize {
        std::env::var("SD_GEN_CASES").ok().and_then(|v| v.parse().ok()).unwrap_or(500).max(1)
    }

    /// <d> with every field reset to its default that <fails> doesn't need.
    pub fn shrink(mut d: Derived, fails: impl Fn(&Derived) -> bool) -> Derived {
        loop {
            let before = d.clone();
            for i in 0..Derived::FIELDS.len() {
                let mut t = d.clone();
                t.reset(i);
                if t != d && fails(&t) {
                    d = t;
                }
            }
            if d == before {
                return d;
            }
        }
    }

    /// Panics with the shrunk builder call, to paste as a regression test.
    pub fn check(what: &str, holds: impl Fn(&Derived) -> Result<(), String>) {
        check_n(what, cases(), holds);
    }

    pub fn check_n(what: &str, cases: usize, holds: impl Fn(&Derived) -> Result<(), String>) {
        for seed in SEEDS {
            let mut r = Rng::new(seed);
            for _ in 0..cases {
                let d = Derived::random(&mut r);
                if holds(&d).is_err() {
                    let small = shrink(d, |x| holds(x).is_err());
                    let why = holds(&small).err().unwrap_or_default();
                    panic!("{what} fails (seed {seed}): {why}\n  {}", small.builder_call());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::arb::{check_n, shrink, Rng};
    use super::*;
    use serde_json::json;

    #[test]
    fn unknown_flags_stay_unknown_not_false() {
        let d = Derived::new(&json!({"sync": {"up_to_date": null, "conflict": false}, "push": {}}));
        assert_eq!((d.up_to_date, d.sync_conflict, d.push_dirty), (None, Some(false), None));
        assert!(!d.behind_target, "unknown sync is not behind");
        let d = Derived::new(&json!({"sync": {"up_to_date": false}}));
        assert!(d.behind_target);
    }

    #[test]
    fn the_generator_draws_every_field_and_reset_restores_each() {
        let mut r = Rng::new(7);
        let mut seen = vec![false; Derived::FIELDS.len()];
        for _ in 0..200 {
            let d = Derived::random(&mut r);
            for (i, s) in seen.iter_mut().enumerate() {
                let mut t = d.clone();
                t.reset(i);
                *s |= t != d;
            }
        }
        let never: Vec<_> = Derived::FIELDS.iter().zip(&seen).filter(|(_, s)| !**s).map(|(f, _)| *f).collect();
        assert!(never.is_empty(), "never drawn off its default: {never:?}");
    }

    #[test]
    fn shrink_keeps_only_the_fields_the_failure_needs_and_prints_a_builder_call() {
        let fails = |d: &Derived| d.undone && d.asking == "ci-fix" && yes(d.locked);
        let mut r = Rng::new(1);
        let d = std::iter::repeat_with(|| Derived::random(&mut r)).find(fails).unwrap();
        let small = shrink(d, fails);
        assert_eq!(
            small.builder_call(),
            r#"Derived { locked: Some(true), undone: true, asking: "ci-fix".into(), ..Derived::default() }"#
        );
        assert_eq!(Derived::default().builder_call(), "Derived::default()");
    }

    #[test]
    #[should_panic(expected = "Derived { foreign: true, ..Derived::default() }")]
    fn check_panics_with_the_shrunk_value() {
        check_n("no foreign", 50, |d| if d.foreign { Err("foreign".into()) } else { Ok(()) });
    }
}
