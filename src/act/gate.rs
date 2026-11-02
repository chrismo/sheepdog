// One gate type for every verb (#348): a flat list of rules over Derived
// that NEXT and the verb both ask. A migrated verb's act fn takes the Go
// token by value; Go's field is private to this module, so only enforce
// makes one.

use std::sync::Mutex;

/// A verb's verdict on the state. Hard can't be forced; Done means nothing
/// to do; Soft refuses unless --force; Warn runs with a warning.
#[must_use]
#[derive(Debug, PartialEq)]
pub enum Gate {
    Hard(String),
    Done(String),
    Soft(Vec<String>),
    Warn(Vec<String>),
    Go,
}

impl Gate {
    /// Whether the verb runs without --force: Go, or Go with warnings.
    pub fn runs(&self) -> bool {
        matches!(self, Gate::Go | Gate::Warn(_))
    }

    /// The same verdict with each reason put in <word>'s words.
    pub fn worded(self, word: impl Fn(&str) -> String) -> Gate {
        let all = |ws: Vec<String>| ws.iter().map(|w| word(w)).collect();
        match self {
            Gate::Hard(why) => Gate::Hard(word(&why)),
            Gate::Done(why) => Gate::Done(word(&why)),
            Gate::Soft(ws) => Gate::Soft(all(ws)),
            Gate::Warn(ws) => Gate::Warn(all(ws)),
            Gate::Go => Gate::Go,
        }
    }
}

/// How severe a rule's refusal is, most severe first. Limit refuses like
/// Hard but after Done: a settings limit binds only a verb with work to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Hard,
    Done,
    Limit,
    Soft,
    Warn,
}

/// One line of a verb's gate: when it <refuses>, <name> is the reason, and
/// <clears> what makes it go away (NEXT's line for it). <yours>: nothing sd
/// watch runs clears it (#355); false promises that sd watch will.
pub struct Rule {
    pub name: &'static str,
    pub level: Level,
    pub refuses: bool,
    pub clears: &'static str,
    pub yours: bool,
}

/// The rules judge reports: the most severe refusals, in source order.
pub fn reported(rules: &[Rule]) -> Vec<&Rule> {
    let Some(worst) = rules.iter().filter(|r| r.refuses).map(|r| r.level).min() else { return vec![] };
    rules.iter().filter(|r| r.level == worst && r.refuses).collect()
}

/// The most severe of <rules>' refusals, not the first in source order. Hard
/// and Done report their first rule; Soft and Warn every one.
pub fn judge(rules: &[Rule]) -> Gate {
    let shown = reported(rules);
    let Some(first) = shown.first() else { return Gate::Go };
    let why = || shown.iter().map(|r| r.name.to_string()).collect();
    match first.level {
        Level::Hard | Level::Limit => Gate::Hard(first.name.into()),
        Level::Done => Gate::Done(first.name.into()),
        Level::Soft => Gate::Soft(why()),
        Level::Warn => Gate::Warn(why()),
    }
}

/// <rules>' Warn reasons that refuse, which judge drops from a worse verdict.
pub fn warnings(rules: &[Rule]) -> Vec<String> {
    rules.iter().filter(|r| r.level == Level::Warn && r.refuses).map(|r| r.name.to_string()).collect()
}

/// The check act::run arms for an agent verb (#341): enforce runs it on Go,
/// so a verb with nothing to do reports Done, attached or not.
static ARMED: Mutex<Option<Box<dyn FnOnce() + Send>>> = Mutex::new(None);

pub fn arm(check: impl FnOnce() + Send + 'static) {
    *ARMED.lock().unwrap_or_else(|e| e.into_inner()) = Some(Box::new(check));
}

/// Proof the verb's gate let it run: only enforce makes one.
#[must_use]
#[derive(Debug)]
pub struct Go(());

/// What enforce does with a verdict, before any of it is printed.
#[derive(Debug, PartialEq)]
pub enum Enforced {
    /// Die with these lines (Hard, or Soft without --force).
    Refuse(Vec<String>),
    /// Print the line and stop: nothing to do.
    Done(String),
    /// Run, after printing these warnings.
    Run(Vec<String>),
}

/// What enforce does for a verb <doing> something ("restacking feat").
pub fn decide(gate: Gate, force: bool, doing: &str) -> Enforced {
    match gate {
        Gate::Hard(why) => Enforced::Refuse(vec![why]),
        Gate::Done(line) => Enforced::Done(line),
        Gate::Soft(why) if !force => {
            let mut out = vec![format!("not {doing}:")];
            out.extend(why.iter().map(|w| format!("  - {w}")));
            out.push("(--force to run anyway)".into());
            Enforced::Refuse(out)
        }
        Gate::Go => Enforced::Run(vec![]),
        Gate::Soft(ws) | Gate::Warn(ws) => Enforced::Run(ws),
    }
}

/// Die on a refusal; None when there is nothing to do.
pub fn enforce(gate: Gate, force: bool, doing: &str) -> Option<Go> {
    enforce_leaving(gate, force, doing, "")
}

/// enforce, ending a refusal with <left>, what it leaves behind, if any.
pub fn enforce_leaving(gate: Gate, force: bool, doing: &str, left: &str) -> Option<Go> {
    match decide(gate, force, doing) {
        Enforced::Refuse(lines) => {
            let (first, rest) = lines.split_first().map_or(("", &[][..]), |(f, r)| (f.as_str(), r));
            eprintln!("sd act: {first}");
            for l in rest {
                eprintln!("{l}");
            }
            if !left.is_empty() {
                eprintln!("sd act: {left}");
            }
            std::process::exit(crate::harness::failed_status())
        }
        Enforced::Done(line) => {
            println!("{line}");
            None
        }
        Enforced::Run(warn) => {
            if let Some(check) = ARMED.lock().unwrap_or_else(|e| e.into_inner()).take() {
                check();
            }
            for w in warn {
                eprintln!("sd act: warning: {w}");
            }
            Some(Go(()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(name: &'static str, level: Level, refuses: bool) -> Rule {
        Rule { name, level, refuses, clears: "", yours: true }
    }

    /// Soft "locked", Warn "pending", Hard "undone" and Hard "closed", each
    /// refusing as given.
    fn four(locked: bool, pending: bool, undone: bool, closed: bool) -> [Rule; 4] {
        [
            rule("locked", Level::Soft, locked),
            rule("pending", Level::Warn, pending),
            rule("undone", Level::Hard, undone),
            rule("closed", Level::Hard, closed),
        ]
    }

    #[test]
    fn judge_reports_the_most_severe_refusal_not_the_first() {
        assert_eq!(judge(&four(true, true, true, true)), Gate::Hard("undone".into()));
        assert_eq!(judge(&four(true, true, false, false)), Gate::Soft(vec!["locked".into()]));
        assert_eq!(judge(&four(false, true, false, false)), Gate::Warn(vec!["pending".into()]));
        assert_eq!(judge(&four(false, false, false, false)), Gate::Go);
        assert_eq!(judge(&[]), Gate::Go);
    }

    #[test]
    fn judge_lists_every_soft_reason_in_order_and_done_outranks_soft() {
        let rules = |done| [rule("a", Level::Soft, true), rule("on it", Level::Done, done), rule("b", Level::Soft, true)];
        assert_eq!(judge(&rules(false)), Gate::Soft(vec!["a".into(), "b".into()]));
        assert_eq!(judge(&rules(true)), Gate::Done("on it".into()));
    }

    #[test]
    fn decide_refuses_hard_and_unforced_soft_and_warns_what_force_overrode() {
        assert_eq!(decide(Gate::Hard("no PR".into()), true, "x"), Enforced::Refuse(vec!["no PR".into()]));
        assert_eq!(decide(Gate::Done("on main".into()), false, "x"), Enforced::Done("on main".into()));
        let soft = || Gate::Soft(vec!["taken over".into(), "not pushed".into()]);
        let want = ["not restacking feat:", "  - taken over", "  - not pushed", "(--force to run anyway)"];
        assert_eq!(decide(soft(), false, "restacking feat"), Enforced::Refuse(want.map(String::from).to_vec()));
        assert_eq!(decide(soft(), true, "restacking feat"), Enforced::Run(vec!["taken over".into(), "not pushed".into()]));
        assert_eq!(decide(Gate::Warn(vec!["CI is still running".into()]), false, "x"), Enforced::Run(vec!["CI is still running".into()]));
        assert_eq!(decide(Gate::Go, false, "x"), Enforced::Run(vec![]));
    }

    #[test]
    fn worded_words_every_reason_and_keeps_the_verdict() {
        let up = |w: &str| w.to_uppercase();
        assert_eq!(Gate::Soft(vec!["a".into(), "b".into()]).worded(up), Gate::Soft(vec!["A".into(), "B".into()]));
        assert_eq!(Gate::Hard("no".into()).worded(up), Gate::Hard("NO".into()));
        assert_eq!(Gate::Go.worded(up), Gate::Go);
    }

    #[test]
    fn runs_is_go_or_warn() {
        assert!(Gate::Go.runs() && Gate::Warn(vec![]).runs());
        assert!(!Gate::Soft(vec![]).runs() && !Gate::Hard(String::new()).runs() && !Gate::Done(String::new()).runs());
    }
}
