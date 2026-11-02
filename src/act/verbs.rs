// Each sd act verb's declaration (#341), read once by act::run for its
// interlocks, and the registry gates::parity checks NEXT against.
// Not here: attach and forget --remove ask running::idle mid-verb, and
// harness::run_step marks every agent run, describe's included.

use super::gates::{self, Gated};
use super::Args;

/// What a verb writes of the branch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// Nothing the running marker guards: config, refs, GitHub.
    None,
    /// Its worktree or local branch.
    Worktree,
    /// Its pushed head.
    Remote,
}

pub struct Verb {
    /// The verb, or "<verb> --<flag>" for a flag that changes what it does.
    pub name: &'static str,
    pub scope: Scope,
    /// Runs an agent, or decides for one (review --accept): refused while
    /// attached, unless --force.
    pub agent: bool,
    pub spends: bool,
    /// What running::idle's refusal says it wasn't doing. Empty for an agent
    /// step: the marker's own refusal says "not starting another agent step".
    pub doing: &'static str,
    pub gate: Option<Gated>,
}

const fn v(name: &'static str, scope: Scope, agent: bool, spends: bool, doing: &'static str) -> Verb {
    Verb { name, scope, agent, spends, doing, gate: None }
}

const fn gated(v: Verb, offered: fn(&[serde_json::Value]) -> bool, gate: fn(&crate::next::Derived) -> super::Gate) -> Verb {
    Verb { gate: Some(Gated { offered, gate }), ..v }
}

use Scope::{None as Nothing, Remote, Worktree};

pub const VERBS: &[Verb] = &[
    // its sd act implement is a verb of its own
    v("start", Nothing, false, true, ""),
    gated(v("implement", Worktree, true, true, ""), |steps| steps.iter().any(|s| (s["key"] == "implement" || s["key"] == "review-fix") && s["who"] == "agent"), gates::implement),
    v("follow", Nothing, false, false, ""),
    v("fetch", Worktree, false, false, "fetching"),
    v("enroll", Nothing, false, false, ""),
    v("unenroll", Nothing, false, false, ""),
    v("forget", Nothing, false, false, ""),
    v("unregister", Nothing, false, false, ""),
    // NEXT lists address behind the steps that clear its gate (#326): only a first step runs
    gated(v("address", Worktree, true, true, ""), |steps| steps.first().is_some_and(|s| s["key"] == "bot" && s["who"] == "agent"), gates::address),
    v("ack", Nothing, false, false, ""),
    // act::run takes the declaration of the verb it resumes
    v("answer", Worktree, true, true, ""),
    gated(v("restack", Worktree, false, false, "restacking"), |steps| steps.iter().any(|s| s["key"] == "behind" && s["who"] == "run"), gates::restack),
    gated(v("restack --resolve", Worktree, true, true, "restacking"), |steps| steps.iter().any(|s| s["key"] == "conflict" && s["who"] == "agent"), gates::restack_resolve),
    v("retarget", Nothing, false, false, ""),
    gated(v("push", Remote, false, false, "pushing"), |steps| steps.iter().any(|s| s["key"] == "push"), gates::push),
    v("verify", Worktree, false, false, "running verify"),
    v("autofix", Worktree, false, false, "running autofix"),
    gated(v("ci-fix", Worktree, true, true, ""), |steps| steps.iter().any(|s| s["key"] == "ci-red"), gates::ci_fix),
    gated(v("review", Worktree, true, true, ""), |steps| steps.iter().any(|s| s["key"] == "review" && s["who"] == "agent"), gates::review),
    // reads no worktree; refused while attached, as review is
    v("review --accept", Nothing, true, false, ""),
    // NEXT never offers it: ship runs it
    gated(v("describe", Nothing, true, true, ""), |_| false, gates::describe),
    // describes unless a description is stored (its gate is describe's)
    v("ship", Remote, true, true, "shipping"),
    v("trigger", Nothing, false, false, ""),
    v("attach", Nothing, false, false, ""),
];

pub fn find(name: &str) -> Option<&'static Verb> {
    VERBS.iter().find(|v| v.name == name)
}

/// <verb>'s declaration as <a> runs it: restack --resolve and review
/// --accept are their own.
pub fn of(verb: &str, a: &Args) -> Option<&'static Verb> {
    let flag = match verb {
        "restack" if a.resolve => "--resolve",
        "review" if a.accept => "--accept",
        _ => "",
    };
    find(&format!("{verb} {flag}")).or_else(|| find(verb))
}

/// <cmd> is what follows "sd act ".
pub fn of_cmd(cmd: &str) -> Option<&'static Verb> {
    let mut words = cmd.split_whitespace();
    let verb = words.next()?;
    words.find_map(|w| find(&format!("{verb} {w}"))).or_else(|| find(verb))
}

impl Verb {
    /// The running marker's step.
    pub fn step(&self) -> &'static str {
        match self.name {
            "restack --resolve" => "resolve",
            n => n,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flag_that_changes_the_verb_picks_its_own_declaration() {
        let a = Args { resolve: true, ..Args::default() };
        assert_eq!(of("restack", &a).map(|v| v.name), Some("restack --resolve"));
        assert_eq!(of("restack", &Args::default()).map(|v| v.name), Some("restack"));
        assert_eq!(of("review", &Args { accept: true, ..Args::default() }).map(|v| v.name), Some("review --accept"));
        assert_eq!(of("push", &a).map(|v| v.name), Some("push"));
        assert!(of("frobnicate", &a).is_none());
        assert_eq!(of_cmd("restack --resolve").map(|v| v.name), Some("restack --resolve"));
        assert_eq!(of_cmd("answer \"<your answer>\" feat").map(|v| v.name), Some("answer"));
        assert_eq!(of_cmd("ship feat").map(|v| v.name), Some("ship"));
    }

    #[test]
    fn the_markers_step_is_the_verbs_and_resolve_for_restack_resolve() {
        assert_eq!(find("restack --resolve").unwrap().step(), "resolve");
        assert_eq!(find("ci-fix").unwrap().step(), "ci-fix");
    }

    #[test]
    fn the_agent_verbs_with_no_marker_of_their_own_are_review_accept_and_describe() {
        for v in VERBS.iter().filter(|v| v.agent && v.scope == Scope::None) {
            assert!(["review --accept", "describe"].contains(&v.name), "{}", v.name);
        }
    }
}
