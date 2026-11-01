// The NEXT step vocabulary (#335). next.rs names a key only through def!,
// so a key not here doesn't compile; src/watch/pass.rs reads notify, glyph
// and verb here, and src/watch_help.txt's "Notified" block is tested
// against it.

/// Every NEXT key, in next.rs's AUTHOR order; a held agent step's
/// stand-ins and a followed PR's own steps (#250) come last. bot and
/// answer, listed in two places as address's gate lets them, are here at
/// the first.
pub const STEPS: &[StepDef] = &[
    s("default", YOU, Stuck, None, None),
    s("running", WAIT, Turn, Some("🤖"), None),
    s("merged", RUN, Acts, Some("🏁"), None),
    s("closed", YOU, Never, Some("🚫"), None),
    s("errors", YOU, Unsure, None, None),
    s("stale", RUN, Unsure, None, None),
    s("gh-wait", WAIT, Unsure, None, None),
    s("attached", WAIT, Turn, None, None),
    s("foreign", YOU, Stuck, None, None),
    s("human", YOU, Stuck, None, None),
    s("question", YOU, Stuck, None, None),
    s("restack-undone", YOU, Stuck, None, None),
    s("ship-parent", RUN, Turn, None, None),
    s("conflict", &["agent", "you"], Stuck, None, Some("resolve")),
    s("behind", &["run", "wait", "you"], Turn, None, Some("restack")),
    s("dirty", YOU, Stuck, None, None),
    s("ci-red", AGENT, Stuck, None, Some("ci-fix")),
    s("ci-blocked", YOU, Stuck, None, None),
    // chrismo's when address's gate refuses with nothing ahead (#326)
    s("bot", &["agent", "you"], Turn, None, Some("address")),
    s("read", YOU, Stuck, None, None),
    s("take", YOU, Turn, None, None),
    s("reset-dirty", YOU, Turn, None, None),
    s("nothing-dirty", YOU, Turn, None, None),
    s("implement", AGENT, Turn, None, Some("implement")),
    s("nothing", YOU, Stuck, None, None),
    s("verify-red", YOU, Stuck, None, None),
    s("foreign-authors", YOU, Stuck, None, None),
    s("ci-config", YOU, Stuck, None, None),
    s("push", RUN, Turn, None, Some("push")),
    s("answer", &["wait", "you"], Turn, None, None),
    s("open-pr", RUN, Turn, None, None),
    s("draft", RUN, Turn, None, None),
    s("review-fix", AGENT, Turn, None, Some("implement")),
    s("review-decide", YOU, Stuck, None, None),
    s("review", AGENT, Turn, None, Some("review")),
    s("review-held", YOU, Stuck, None, None),
    s("bot-trigger", RUN, Turn, None, Some("trigger")),
    s("ci-rerun", RUN, Turn, None, None),
    s("bot-wait", WAIT, Turn, None, None),
    s("bot-silent", YOU, Stuck, None, None),
    s("ci-wait", WAIT, Turn, None, None),
    s("parent-closed", YOU, Stuck, None, None),
    s("retarget", RUN, Turn, None, Some("retarget")),
    s("review-wait", WAIT, Turn, None, None),
    s("parent-wait", WAIT, Turn, None, None),
    s("ready", YOU, Acts, Some("✅"), None),
    s("merge-wait", WAIT, Turn, None, None),
    s("blocked", YOU, Turn, None, None),
    // in place of the agent steps: spend past the budget, or failed in a row
    s("budget", YOU, Stuck, None, None),
    s("failed", YOU, Stuck, None, None),
    // a followed PR's (#250)
    s("yours", YOU, Stuck, None, None),
    s("fetch", RUN, Turn, None, Some("fetch")),
    s("submit", YOU, Stuck, None, None),
    s("author-wait", WAIT, Turn, None, None),
];

const YOU: &[&str] = &["you"];
const AGENT: &[&str] = &["agent"];
const RUN: &[&str] = &["run"];
const WAIT: &[&str] = &["wait"];
use Notify::{Acts, Never, Stuck, Turn, Unsure};

const fn s(
    key: &'static str,
    who: &'static [&'static str],
    notify: Notify,
    glyph: Option<&'static str>,
    verb: Option<&'static str>,
) -> StepDef {
    StepDef { key, who, notify, glyph, verb }
}

/// How sd watch notifies a step (src/watch_help.txt, "Notified").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Notify {
    /// ❌ wherever NEXT lists it: stuck until chrismo, or an agent chrismo
    /// starts, deals with it.
    Stuck,
    /// ❌ when it's first in NEXT and chrismo's (who "you"), so a step
    /// handed to chrismo is never a silent stall (#326).
    Turn,
    /// Alone in NEXT, an end state chrismo acts on: notified, and its glyph.
    Acts,
    /// sd state isn't sure yet: ⚠️, and what was notified carries over.
    Unsure,
    /// Never: an end state chrismo made, its glyph shows it.
    Never,
}

pub struct StepDef {
    pub key: &'static str,
    /// Who NEXT can give it to: you, agent, run or wait.
    pub who: &'static [&'static str],
    pub notify: Notify,
    /// The glyph a NEXT of this step alone shows, where who doesn't say it.
    pub glyph: Option<&'static str>,
    /// The step an autoRun pass launches when it's first, the agent's or
    /// run's: sd act <verb>, resolve as restack --resolve. None for stale
    /// (the pass has just fetched), merged (forget --remove deletes the
    /// worktree), draft, ci-rerun, and ship-parent and open-pr, which may
    /// spend tokens describing.
    pub verb: Option<&'static str>,
}

/// const, so def! can check a key when it compiles.
pub const fn find(key: &str) -> Option<&'static StepDef> {
    let mut i = 0;
    while i < STEPS.len() {
        if same(STEPS[i].key.as_bytes(), key.as_bytes()) {
            return Some(&STEPS[i]);
        }
        i += 1;
    }
    None
}

const fn same(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

macro_rules! def {
    ($k:literal) => {
        const {
            match $crate::next::steps::find($k) {
                Some(d) => d,
                None => panic!(concat!("not a NEXT key in STEPS: ", $k)),
            }
        }
    };
}
pub(crate) use def;

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// (notified, not notified)
    fn help_keys() -> (Vec<String>, Vec<String>) {
        let help = include_str!("../watch_help.txt");
        let block = help.split_once("\nNotified,").expect("a Notified block").1;
        let block = block.split_once("\nA notification that fails").expect("its end").0;
        let (on, off) = block.split_once("\nNot notified:").expect("a Not notified part");
        let entry = regex::Regex::new(r"(?m)^  ([a-z][a-z-]*(?: [a-z][a-z-]*)*):").unwrap();
        let keys = |part: &str| entry.captures_iter(part).flat_map(|c| c[1].split(' ').map(String::from).collect::<Vec<_>>()).collect();
        (keys(on), keys(off))
    }

    fn rendered() -> (BTreeSet<&'static str>, BTreeSet<&'static str>) {
        let notified = |d: &StepDef| match d.notify {
            Notify::Stuck | Notify::Acts => true,
            Notify::Turn => d.who.contains(&"you"),
            Notify::Unsure | Notify::Never => false,
        };
        let (on, off): (Vec<_>, Vec<_>) = STEPS.iter().partition(|d| notified(d));
        (on.iter().map(|d| d.key).collect(), off.iter().map(|d| d.key).collect())
    }

    #[test]
    fn the_watch_helps_notified_block_lists_every_key_as_the_table_notifies_it_335() {
        let (on, off) = help_keys();
        let all: Vec<&String> = on.iter().chain(&off).collect();
        let once: BTreeSet<&String> = all.iter().copied().collect();
        assert_eq!(once.len(), all.len(), "a key listed twice: {all:?}");
        let (want_on, want_off) = rendered();
        fn got(v: &[String]) -> BTreeSet<&str> {
            v.iter().map(String::as_str).collect()
        }
        assert_eq!(got(&on), want_on, "Notified");
        assert_eq!(got(&off), want_off, "Not notified");
    }

    #[test]
    fn next_gives_each_step_only_to_who_its_entry_names_on_generated_states() {
        let rec = serde_json::json!({});
        crate::next::derived_arb::check("who", |d| {
            for st in crate::next::offer(d, &rec, "origin") {
                let (key, who) = (st["key"].as_str().unwrap_or(""), st["who"].as_str().unwrap_or(""));
                match find(key) {
                    Some(def) if def.who.contains(&who) => {}
                    Some(_) => return Err(format!("{key} given to {who}")),
                    None => return Err(format!("{key} isn't in STEPS")),
                }
            }
            Ok(())
        });
    }

    #[test]
    fn each_key_is_listed_once_with_who_it_can_be() {
        let keys: BTreeSet<&str> = STEPS.iter().map(|d| d.key).collect();
        assert_eq!(keys.len(), STEPS.len());
        for d in STEPS {
            assert!(!d.who.is_empty() && d.who.iter().all(|w| ["you", "agent", "run", "wait"].contains(w)), "{}", d.key);
        }
        assert!(find("ci-red").is_some_and(|d| d.key == "ci-red"));
        assert!(find("ci-re").is_none() && find("nope").is_none());
    }
}
