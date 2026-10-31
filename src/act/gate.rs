// One gate type for every verb (#348): a flat list of rules over Derived
// that NEXT and the verb both ask. A migrated verb's act fn takes the Go
// token by value; Go's field is private to this module, so only enforce
// makes one.

use crate::next::Derived;

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
}

/// How severe a rule's refusal is, most severe first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Hard,
    Done,
    Soft,
    Warn,
}

/// One line of a verb's gate: when <refuses> holds of the state, <name> is
/// the reason, and <clears> what makes it go away (NEXT's line for it).
pub struct Rule {
    pub name: &'static str,
    pub level: Level,
    pub refuses: fn(&Derived) -> bool,
    pub clears: &'static str,
}

/// The most severe refusal of <rules> on <d>, not the first in source order.
/// Hard and Done report their first rule; Soft and Warn every one.
pub fn judge(rules: &[Rule], d: &Derived) -> Gate {
    let Some(worst) = rules.iter().filter(|r| (r.refuses)(d)).map(|r| r.level).min() else { return Gate::Go };
    let mut why = rules.iter().filter(|r| r.level == worst && (r.refuses)(d)).map(|r| r.name.to_string());
    match worst {
        Level::Hard => Gate::Hard(why.next().unwrap_or_default()),
        Level::Done => Gate::Done(why.next().unwrap_or_default()),
        Level::Soft => Gate::Soft(why.collect()),
        Level::Warn => Gate::Warn(why.collect()),
    }
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
        Gate::Soft(_) | Gate::Go => Enforced::Run(vec![]),
        Gate::Warn(ws) => Enforced::Run(ws),
    }
}

/// Die on a refusal; None when there is nothing to do.
pub fn enforce(gate: Gate, force: bool, doing: &str) -> Option<Go> {
    match decide(gate, force, doing) {
        Enforced::Refuse(lines) => {
            let (first, rest) = lines.split_first().map_or(("", &[][..]), |(f, r)| (f.as_str(), r));
            eprintln!("sd act: {first}");
            for l in rest {
                eprintln!("{l}");
            }
            std::process::exit(crate::harness::failed_status())
        }
        Enforced::Done(line) => {
            println!("{line}");
            None
        }
        Enforced::Run(warn) => {
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

    fn rule(name: &'static str, level: Level, refuses: fn(&Derived) -> bool) -> Rule {
        Rule { name, level, refuses, clears: "" }
    }

    #[test]
    fn judge_reports_the_most_severe_refusal_not_the_first() {
        let rules = [
            rule("locked", Level::Soft, |d| d.locked == Some(true)),
            rule("pending", Level::Warn, |d| d.ci_pending),
            rule("undone", Level::Hard, |d| d.undone),
            rule("closed", Level::Hard, |d| d.closed),
        ];
        let d = Derived { locked: Some(true), ci_pending: true, undone: true, closed: true, ..Derived::default() };
        assert_eq!(judge(&rules, &d), Gate::Hard("undone".into()));
        let d = Derived { locked: Some(true), ci_pending: true, ..Derived::default() };
        assert_eq!(judge(&rules, &d), Gate::Soft(vec!["locked".into()]));
        let d = Derived { ci_pending: true, ..Derived::default() };
        assert_eq!(judge(&rules, &d), Gate::Warn(vec!["pending".into()]));
        assert_eq!(judge(&rules, &Derived::default()), Gate::Go);
        assert_eq!(judge(&[], &d), Gate::Go);
    }

    #[test]
    fn judge_lists_every_soft_reason_in_order_and_done_outranks_soft() {
        let rules = [
            rule("a", Level::Soft, |d| d.locked == Some(true)),
            rule("on it", Level::Done, |d| d.up_to_date == Some(true)),
            rule("b", Level::Soft, |d| d.ci_pending),
        ];
        let d = Derived { locked: Some(true), ci_pending: true, ..Derived::default() };
        assert_eq!(judge(&rules, &d), Gate::Soft(vec!["a".into(), "b".into()]));
        let d = Derived { up_to_date: Some(true), ..d };
        assert_eq!(judge(&rules, &d), Gate::Done("on it".into()));
    }

    #[test]
    fn decide_refuses_hard_and_unforced_soft_runs_go_warn_and_forced_soft() {
        assert_eq!(decide(Gate::Hard("no PR".into()), true, "x"), Enforced::Refuse(vec!["no PR".into()]));
        assert_eq!(decide(Gate::Done("on main".into()), false, "x"), Enforced::Done("on main".into()));
        let soft = || Gate::Soft(vec!["taken over".into(), "not pushed".into()]);
        let want = ["not restacking feat:", "  - taken over", "  - not pushed", "(--force to run anyway)"];
        assert_eq!(decide(soft(), false, "restacking feat"), Enforced::Refuse(want.map(String::from).to_vec()));
        assert_eq!(decide(soft(), true, "restacking feat"), Enforced::Run(vec![]));
        assert_eq!(decide(Gate::Warn(vec!["CI is still running".into()]), false, "x"), Enforced::Run(vec!["CI is still running".into()]));
        assert_eq!(decide(Gate::Go, false, "x"), Enforced::Run(vec![]));
    }

    #[test]
    fn runs_is_go_or_warn() {
        assert!(Gate::Go.runs() && Gate::Warn(vec![]).runs());
        assert!(!Gate::Soft(vec![]).runs() && !Gate::Hard(String::new()).runs() && !Gate::Done(String::new()).runs());
    }
}
