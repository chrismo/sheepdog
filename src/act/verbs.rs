// Each sd act verb's declaration (#341), read once by act::run for its
// interlocks, and the registry gates::parity checks NEXT against; an agent
// verb's prompt files, which its code reads through prompt() and the Atlas
// shows (#377).
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
    /// The prompts its agent reads, one per step it runs.
    pub prompts: &'static [Prompt],
}

/// A step's shipped prompt, lib/prompts/<file>, which settings.<setting>
/// overrides, then .sheepdog/<file> in the base commit (guidance.rs).
pub struct Prompt {
    /// The step it's for: the verb's, or review-fix for implement's other.
    pub step: &'static str,
    pub setting: &'static str,
    pub file: &'static str,
}

const fn v(name: &'static str, scope: Scope, agent: bool, spends: bool, doing: &'static str) -> Verb {
    Verb { name, scope, agent, spends, doing, gate: None, prompts: &[] }
}

const fn p(step: &'static str, setting: &'static str, file: &'static str) -> Prompt {
    Prompt { step, setting, file }
}

impl Prompt {
    /// The step's guidance for <b>, by guidance::step_guidance from <base>.
    pub fn read(&self, b: &str, base: &str) -> Result<String, String> {
        crate::guidance::step_guidance(self.setting, b, base, &crate::guidance::lib_dir().join("prompts").join(self.file))
    }
}

impl Verb {
    const fn reads(self, prompts: &'static [Prompt]) -> Verb {
        Verb { prompts, ..self }
    }
}

const fn gated(v: Verb, offered: fn(&[serde_json::Value]) -> bool, rules: fn(&crate::next::Derived) -> Vec<super::gate::Rule>) -> Verb {
    Verb { gate: Some(Gated { offered, rules }), ..v }
}

use Scope::{None as Nothing, Remote, Worktree};

pub const VERBS: &[Verb] = &[
    // its sd act implement is a verb of its own
    v("start", Nothing, false, true, ""),
    gated(v("implement", Worktree, true, true, "").reads(&[p("implement", "implementPrompt", "implement.md"), p("review-fix", "reviewFixPrompt", "review-fix.md"), p("verify-fix", "verifyFixPrompt", "verify-fix.md")]), |steps| steps.iter().any(|s| ["implement", "review-fix", "verify-red"].iter().any(|k| s["key"] == *k) && s["who"] == "agent"), gates::implement_rules),
    v("follow", Nothing, false, false, ""),
    v("fetch", Worktree, false, false, "fetching"),
    v("enroll", Nothing, false, false, ""),
    v("unenroll", Nothing, false, false, ""),
    v("forget", Nothing, false, false, ""),
    v("unregister", Nothing, false, false, ""),
    // NEXT lists address behind the steps that clear its gate (#326): only a first step runs
    gated(v("address", Worktree, true, true, "").reads(&[p("address", "addressPrompt", "address.md")]), |steps| steps.first().is_some_and(|s| s["key"] == "bot" && s["who"] == "agent"), gates::address_rules),
    v("ack", Nothing, false, false, ""),
    // act::run takes the declaration of the verb it resumes
    v("answer", Worktree, true, true, ""),
    // writes only the decisions record; refuses, unless --force, while a
    // step runs (its own check: the marker's refusal can't be forced)
    v("decision", Nothing, false, false, ""),
    gated(v("restack", Worktree, false, false, "restacking"), |steps| steps.iter().any(|s| s["key"] == "behind" && s["who"] == "run"), |d| gates::restack_rules(d, false)),
    gated(v("restack --resolve", Worktree, true, true, "restacking").reads(&[p("resolve", "resolvePrompt", "resolve.md")]), |steps| steps.iter().any(|s| s["key"] == "conflict" && s["who"] == "agent"), |d| gates::restack_rules(d, true)),
    gated(v("retarget", Nothing, false, false, ""), |steps| steps.iter().any(|s| s["key"] == "retarget"), gates::retarget_rules),
    // gh pr ready: writes only GitHub (#500)
    gated(v("mark-ready", Nothing, false, false, ""), |steps| steps.iter().any(|s| s["key"] == "draft"), gates::mark_ready_rules),
    gated(v("push", Remote, false, false, "pushing"), |steps| steps.iter().any(|s| s["key"] == "push"), gates::push_rules),
    v("verify", Worktree, false, false, "running verify"),
    v("autofix", Worktree, false, false, "running autofix"),
    gated(v("ci-fix", Worktree, true, true, "").reads(&[p("ci-fix", "ciFixPrompt", "ci-fix.md")]), |steps| steps.iter().any(|s| s["key"] == "ci-red"), gates::ci_fix_rules),
    gated(v("review", Worktree, true, true, "").reads(&[p("review", "reviewPrompt", "review.md")]), |steps| steps.iter().any(|s| s["key"] == "review" && s["who"] == "agent"), gates::review_rules),
    // reads no worktree; refused while attached, as review is. NEXT's
    // review-decide step offers it, but to chrismo, so parity doesn't ask it
    gated(v("review --accept", Nothing, true, false, ""), |_| false, gates::accept_rules),
    // NEXT never offers it: ship runs it
    gated(v("describe", Nothing, true, true, "").reads(&[p("describe", "describePrompt", "describe.md")]), |_| false, gates::describe_rules),
    // describes unless a description is stored (its gate is describe's)
    v("ship", Remote, true, true, "shipping"),
    v("trigger", Nothing, false, false, ""),
    v("attach", Nothing, false, false, ""),
    v("hold", Nothing, false, false, ""),
    v("release", Nothing, false, false, ""),
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

/// The prompt <step> reads, as its verb declares it.
pub fn prompt(step: &str) -> &'static Prompt {
    VERBS.iter().flat_map(|v| v.prompts).find(|p| p.step == step).unwrap_or_else(|| panic!("no verb declares {step}'s prompt"))
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

    /// mark-ready (#500): gh pr ready behind the draft step's gate, which
    /// writes only GitHub and spends nothing.
    #[test]
    fn mark_ready_is_a_gated_zero_token_verb_that_writes_only_github_500() {
        let v = find("mark-ready").expect("mark-ready is declared");
        assert!(!v.spends && !v.agent);
        assert_eq!(v.scope, Scope::None);
        let g = v.gate.as_ref().expect("mark-ready is gated");
        assert!((g.offered)(&[serde_json::json!({"key": "draft", "who": "run"})]));
        assert!(!(g.offered)(&[serde_json::json!({"key": "ready", "who": "you"})]));
    }

    #[test]
    fn the_markers_step_is_the_verbs_and_resolve_for_restack_resolve() {
        assert_eq!(find("restack --resolve").unwrap().step(), "resolve");
        assert_eq!(find("ci-fix").unwrap().step(), "ci-fix");
    }

    /// Every prompt a verb declares is shipped, and its setting is one
    /// sd config knows and whose .sheepdog/ convention file (guidance.rs) is
    /// named as the shipped one is.
    #[test]
    fn every_declared_prompt_is_a_file_under_lib_prompts_with_a_known_setting() {
        let lib = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("lib/prompts");
        let names = crate::config::setting_names();
        let declared: Vec<&Prompt> = VERBS.iter().flat_map(|v| v.prompts).collect();
        assert!(!declared.is_empty());
        for p in &declared {
            assert!(lib.join(p.file).is_file(), "{}: no lib/prompts/{}", p.step, p.file);
            assert!(names.contains(&p.setting), "{}: settings.{} isn't a setting", p.step, p.setting);
            assert_eq!(crate::guidance::convention(p.setting), format!(".sheepdog/{}", p.file), "{}", p.step);
            assert_eq!(declared.iter().filter(|q| q.step == p.step).count(), 1, "{}: declared twice", p.step);
        }
    }

    /// The verbs that run an agent on a prompt of their own declare it;
    /// answer resumes the asking step's session, review --accept runs no
    /// agent, and ship runs describe's (and start, which runs none of its
    /// own, implement's).
    #[test]
    fn every_agent_verb_but_answer_review_accept_and_ship_declares_its_prompt() {
        for v in VERBS {
            let borrows = ["answer", "review --accept", "ship"].contains(&v.name);
            assert_eq!(!v.prompts.is_empty(), v.agent && !borrows, "{}", v.name);
        }
        let steps = |n: &str| find(n).unwrap().prompts.iter().map(|p| p.step).collect::<Vec<_>>();
        assert_eq!(steps("implement"), ["implement", "review-fix", "verify-fix"]);
        assert_eq!(steps("restack --resolve"), ["resolve"]);
        assert_eq!(prompt("review-fix").file, "review-fix.md");
        assert_eq!(prompt("review-fix").setting, "reviewFixPrompt");
    }

    #[test]
    fn the_agent_verbs_with_no_marker_of_their_own_are_review_accept_and_describe() {
        for v in VERBS.iter().filter(|v| v.agent && v.scope == Scope::None) {
            assert!(["review --accept", "describe"].contains(&v.name), "{}", v.name);
        }
    }
}
