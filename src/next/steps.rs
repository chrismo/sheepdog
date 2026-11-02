// The NEXT step vocabulary (#335). next.rs names a key only through def!,
// so a key not here doesn't compile; src/watch/pass.rs reads notify, glyph
// and verb here, and src/watch_help.txt's "Notified" block is tested
// against it.

/// Every NEXT key, in next.rs's AUTHOR order; a held agent step's
/// stand-ins and a followed PR's own steps (#250) come last. bot and
/// answer, listed in two places as address's gate lets them, are here at
/// the first. Each sits in a Flock stage, or in none (Hold, saying what it
/// does to the list), with the word a lit stage shows and a line on what
/// it means (#377).
pub const STEPS: &[StepDef] = &[
    s("default", YOU, Stuck, None, None)
        .at(Build, "start", "on the default branch, or a branch that is its own base: start a branch"),
    s("running", WAIT, Turn, Some("🤖"), None)
        .at(Hold, "running", "an agent is working on this branch. sheepdog runs one step per branch at a time, so whatever comes next queues until it finishes")
        .hold(Busy),
    s("merged", RUN, Acts, Some("🏁"), None)
        .at(Merge, "merged", "the PR merged: drop the branch's pins and enrollment, remove its worktree and delete it").path(3),
    s("closed", YOU, Never, Some("🚫"), None).at(Merge, "closed", "the PR was closed without merging"),
    s("errors", YOU, Unsure, None, None)
        .at(Hold, "errors", "sd state hit errors reading the branch, so the steps listed under it may be wrong, and sd watch starts nothing until they clear (sd state --json has them)")
        .hold(HUnsure),
    s("stale", RUN, Unsure, None, None)
        .at(Hold, "stale", "GitHub shows commits our remote refs don't have yet, so the steps under it may be wrong: fetch, then look again")
        .hold(HUnsure),
    s("gh-wait", WAIT, Unsure, None, None)
        .at(Hold, "gh-wait", "the push landed, but GitHub's PR data still shows the old head; it catches up in seconds")
        .hold(HUnsure),
    s("attached", WAIT, Turn, None, None)
        .at(Hold, "attached", "chrismo is working in this branch's session: until it's closed, sd watch starts nothing on the branch and agent steps refuse to run (--force overrides)")
        .hold(Top),
    s("foreign", YOU, Stuck, None, None)
        .at(Hold, "foreign", "the remote has commits this copy never had: look at them before anything pushes")
        .hold(Top),
    s("human", YOU, Stuck, None, None).at(Threads, "reply", "a human's review item is owed a reply, which chrismo writes"),
    s("question", YOU, Stuck, None, None)
        .at(Hold, "question", "an agent step needs chrismo's answer, or gave up; nothing reruns it until chrismo answers, edits or acks")
        .hold(Top),
    s("restack-undone", YOU, Stuck, None, None)
        .at(Push, "restack undone", "the restack was undone: some of the target's commits are copies here, so nothing pushes until it's redone"),
    s("ship-parent", RUN, Turn, None, None).at(Push, "ship parent", "the parent branch isn't on the remote yet: ship it first"),
    s("conflict", &["agent", "you"], Stuck, None, Some("resolve"))
        .at(Push, "conflict", "the restack would conflict: an agent resolves it (restack --resolve), or chrismo by hand"),
    s("behind", &["run", "wait", "you"], Turn, None, Some("restack")).at(Push, "behind", "behind the target: restack onto it").path_if(1, "the target moved"),
    s("dirty", YOU, Stuck, None, None)
        .at(Build, "uncommitted", "uncommitted edits hold up a step that refuses them, and nothing says whose they are"),
    s("ci-red", AGENT, Stuck, None, Some("ci-fix")).at(Ci, "fix", "CI is red at HEAD: an agent diagnoses and fixes it (ci-fix)"),
    s("ci-blocked", YOU, Stuck, None, None).at(Ci, "blocked", "CI's checks wait to be started or approved on GitHub"),
    // chrismo's when address's gate refuses with nothing ahead (#326)
    s("bot", &["agent", "you"], Turn, None, Some("address"))
        .at(Threads, "address", "the review bot's items are owed a reply: an agent addresses them, or chrismo once address's gate refuses").path_if(1, "the bot left items"),
    s("read", YOU, Stuck, None, None)
        .at(Threads, "read", "info-only bot items to read, shown once nothing else moves without chrismo"),
    s("take", YOU, Turn, None, None)
        .at(Push, "take back", "the remote's commits were reset away here and chrismo is mid-change: commit, take them back or drop them"),
    s("reset-dirty", YOU, Turn, None, None)
        .at(Build, "reset away", "pushed commits were reset away and edits sit on top: chrismo's to commit, take back or drop"),
    s("nothing-dirty", YOU, Turn, None, None)
        .at(Build, "nothing to ship", "uncommitted edits on a branch chrismo hasn't committed to yet"),
    s("implement", AGENT, Turn, None, Some("implement")).at(Build, "implement", "an agent builds the branch's recorded task").path(1),
    s("nothing", YOU, Stuck, None, None)
        .at(Build, "nothing to ship", "nothing ahead of the base and no task to implement: commit, or record a task"),
    s("verify-red", YOU, Stuck, None, None)
        .at(Ci, "verify", "settings.verify failed on HEAD, so nothing pushes or ships it until it passes or a new commit replaces HEAD"),
    s("foreign-authors", YOU, Stuck, None, None)
        .at(Push, "others' commits", "the push would remove commits someone else made from the remote: look at them first"),
    s("ci-config", YOU, Stuck, None, None)
        .at(Ci, "set up CI", "an agent step's commit changed CI config, held unpushed until chrismo reviews it and pushes"),
    s("push", RUN, Turn, None, Some("push")).at(Push, "push", "push the branch's commits to the remote").path(2),
    s("answer", &["wait", "you"], Turn, None, None)
        .at(Threads, "answer", "chrismo's stored answer waits for the asking step to run again"),
    s("open-pr", RUN, Turn, None, None).at(Push, "open PR", "open the PR (sd act ship)").path(3),
    s("draft", RUN, Turn, None, None).at(Merge, "mark ready", "the PR is a draft: mark it ready for review").path_if(1, "the PR started as a draft"),
    s("review-fix", AGENT, Turn, None, Some("implement"))
        .at(Review, "fix", "review findings stand at HEAD: an agent fixes them (implement), under settings.reviewRounds").path_if(2, "the review found something"),
    s("review-decide", YOU, Stuck, None, None)
        .at(Review, "decide", "the findings stand after settings.reviewRounds rounds, or the agent declined them: chrismo fixes them or accepts HEAD"),
    s("review", AGENT, Turn, None, Some("review")).at(Review, "queued", "an agent reviews the change since the last review (A8)").path(1),
    s("review-held", YOU, Stuck, None, None)
        .at(Review, "held", "a Hard rule of review's gate holds the review back: no base, or a worktree not known clean"),
    s("bot-trigger", RUN, Turn, None, Some("trigger")).at(Bot, "ask", "ask the review bot to look at HEAD (settings.botCommand)").path(1),
    s("ci-rerun", RUN, Turn, None, None).at(Ci, "rerun", "CI was cancelled: rerun it"),
    s("bot-wait", WAIT, Turn, None, None).at(Bot, "waiting", "waiting for the review bot's answer").path(2),
    s("bot-silent", YOU, Stuck, None, None).at(Bot, "silent", "the review bot hasn't answered within settings.botTimeout"),
    s("ci-wait", WAIT, Turn, None, None).at(Ci, "running", "waiting for CI on HEAD").path(1),
    s("parent-closed", YOU, Stuck, None, None)
        .at(Merge, "parent closed", "the parent PR was closed without merging: point the branch at what it targets now"),
    s("retarget", RUN, Turn, None, Some("retarget")).at(Push, "retarget", "the parent merged: retarget the PR at the parent's target").path_if(4, "the parent merged"),
    s("review-wait", WAIT, Turn, None, None).at(Review, "waiting", "waiting for GitHub's required review"),
    s("parent-wait", WAIT, Turn, None, None).at(Merge, "after parent", "waiting for the parent PR to merge"),
    s("ready", YOU, Acts, Some("✅"), None).at(Merge, "ready", "green, reviewed and in sync: chrismo merges").path(2),
    s("merge-wait", WAIT, Turn, None, None).at(Merge, "merging", "GitHub is recomputing the merge state after the base moved"),
    s("blocked", YOU, Turn, None, None).at(Merge, "blocked", "GitHub's branch rules hold the merge until chrismo sees why"),
    // in place of the agent steps: spend past the budget, or failed in a row
    s("budget", YOU, Stuck, None, None)
        .at(Hold, "budget", "the branch's steps have spent past settings.branchBudget, or a cost is unknown: no agent step runs until chrismo raises it or takes over")
        .hold(Standin),
    s("failed", YOU, Stuck, None, None)
        .at(Hold, "failed", "three agent steps in a row failed (FAIL_LIMIT), so none runs until chrismo fixes what broke and reruns the last one")
        .hold(Standin),
    // a followed PR's (#250)
    s("yours", YOU, Stuck, None, None)
        .at(Review, "yours", "a followed PR's branch has commits of chrismo's the PR doesn't: move them to a branch of your own"),
    s("fetch", RUN, Turn, None, Some("fetch")).at(Push, "fetch", "a followed PR's author pushed: fetch it"),
    s("submit", YOU, Stuck, None, None).at(Review, "submit", "the review of a followed PR's HEAD is chrismo's to submit on GitHub"),
    s("author-wait", WAIT, Turn, None, None)
        .at(Merge, "author", "chrismo's review of a followed PR's HEAD is in: the author's turn"),
];

/// The settings a step reads (#377), for the Atlas, in the order its panel
/// shows them: its own, then its verb's, then what every agent step reads,
/// then what every verb does. Each is checked against config's typed list.
pub fn settings_of(d: &StepDef) -> Vec<&'static str> {
    let listed = |table: &[(&str, &'static [&'static str])], k: Option<&str>| table.iter().find(|(key, _)| Some(*key) == k).map_or(&[][..], |(_, s)| *s);
    let mut out = listed(STEP_SETTINGS, Some(d.key)).to_vec();
    out.extend(listed(VERB_SETTINGS, d.verb));
    if d.who.contains(&"agent") {
        out.extend(AGENT_SETTINGS);
    }
    if d.verb.is_some() {
        out.extend(VERB_SHARED);
    }
    let mut seen = std::collections::BTreeSet::new();
    out.retain(|s| seen.insert(*s));
    out
}

/// By key: what decides a step of chrismo's or a wait (src/state.rs reads
/// them for NEXT), and the prompt a step's verb reads for this step alone.
const STEP_SETTINGS: &[(&str, &[&str])] = &[
    ("implement", &["implementPrompt"]),
    ("review-fix", &["reviewFixPrompt", "reviewRounds"]),
    ("review-decide", &["reviewRounds"]),
    ("verify-red", &["verify"]),
    ("ci-config", &["protectedPaths"]),
    ("bot-wait", &["botTimeout"]),
    ("bot-silent", &["botTimeout"]),
    ("human", &["botLogin"]),
    ("read", &["botLogin", "botIgnore", "botIgnoreBody"]),
    ("budget", &["branchBudget"]),
    ("open-pr", SHIP),
    ("ship-parent", SHIP),
];

/// sd act ship, a run step with no verb in STEPS: it runs describe as an
/// agent step under describe's gate (src/act/ship.rs, gates.rs), and the
/// watcher launches it under autoRun. Not agentSteps: the watcher holds only
/// agent steps to it.
const SHIP: &[&str] = &["describePrompt", "stepTimeout", "stepBudget", "forkLimit", "branchBudget", "autoRun", "remote"];

/// By the verb as STEPS names it: what sd act <verb> reads (src/act/<verb>.rs;
/// editing.rs for implement, address and ci-fix; hold.rs for protectedPaths).
const VERB_SETTINGS: &[(&str, &[&str])] = &[
    ("implement", &["reviewRounds", "verify", "autofix", "implementAllow", "protectedPaths"]),
    ("address", &["addressPrompt", "replyPrefix", "botLogin", "verify", "autofix", "implementAllow", "protectedPaths"]),
    ("ci-fix", &["ciFixPrompt", "verify", "autofix", "implementAllow", "protectedPaths"]),
    ("resolve", &["resolvePrompt", "verify", "implementAllow", "protectedPaths"]),
    ("review", &["reviewPrompt", "localReview", "verify"]),
    // Plain restack: it stops on a conflict without running an agent
    // (src/act/restack.rs), and NEXT offers --resolve as the conflict step,
    // listed under resolve.
    ("restack", &["protectedPaths"]),
    ("trigger", &["botCommand"]),
];

/// Every agent step: its session's timeout and fork limit (src/act/step.rs),
/// a claude step's cap (src/harness.rs), the branch's cap its gate's spend
/// rule holds it to (gates::spend_rule), and how many the watcher runs at
/// once (src/watch/pass.rs).
const AGENT_SETTINGS: &[&str] = &["stepTimeout", "stepBudget", "forkLimit", "branchBudget", "agentSteps"];

/// Every verb: the watcher launches it only under settings.autoRun
/// (src/watch/pass.rs), and act::run points gh at settings.remote.
const VERB_SHARED: &[&str] = &["autoRun", "remote"];

const YOU: &[&str] = &["you"];
const AGENT: &[&str] = &["agent"];
const RUN: &[&str] = &["run"];
const WAIT: &[&str] = &["wait"];
use HoldKind::{Busy, Standin, Top, Unsure as HUnsure};
use Notify::{Acts, Never, Stuck, Turn, Unsure};
use Stage::{Bot, Build, Ci, Hold, Merge, Push, Review, Threads};

/// A step before its place: at() makes the StepDef, so an entry without a
/// stage, label and meaning doesn't compile.
struct Row {
    key: &'static str,
    who: &'static [&'static str],
    notify: Notify,
    glyph: Option<&'static str>,
    verb: Option<&'static str>,
}

const fn s(
    key: &'static str,
    who: &'static [&'static str],
    notify: Notify,
    glyph: Option<&'static str>,
    verb: Option<&'static str>,
) -> Row {
    Row { key, who, notify, glyph, verb }
}

impl Row {
    const fn at(self, stage: Stage, label: &'static str, meaning: &'static str) -> StepDef {
        let Row { key, who, notify, glyph, verb } = self;
        StepDef { key, who, notify, glyph, verb, stage, label, meaning, path: None, when: None, hold: None }
    }
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

/// Where a step sits in a PR's life: the Flock's seven stages (#376), or
/// Hold, a step that stops the whole line and lights none of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Hold,
    Build,
    Push,
    Ci,
    Review,
    Bot,
    Threads,
    Merge,
}

impl Stage {
    /// The seven stages a PR moves through, in order.
    pub const FLOW: [Stage; 7] = [Build, Push, Ci, Review, Bot, Threads, Merge];

    pub const fn name(self) -> &'static str {
        match self {
            Hold => "hold",
            Build => "build",
            Push => "push",
            Ci => "ci",
            Review => "review",
            Bot => "bot",
            Threads => "threads",
            Merge => "merge",
        }
    }
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
    pub stage: Stage,
    /// The word a Flock stage lit by this step shows.
    pub label: &'static str,
    /// One line on what the step means (src/view_help.txt's Atlas, #377).
    pub meaning: &'static str,
    /// Its place on the stage's happy path, or None for an exception (path()).
    pub path: Option<u8>,
    /// For a path step taken only sometimes, the condition (path_if()).
    pub when: Option<&'static str>,
    /// For a Hold step, what it does to NEXT's list (hold()).
    pub hold: Option<HoldKind>,
}

/// What a Hold step does to NEXT's list (src/next.rs offer and finish):
/// four different things the Atlas names, declared on the row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoldKind {
    /// The list is this step alone: running, one step per branch at a time.
    Busy,
    /// Listed above the stage steps as a warning that they may be wrong,
    /// notified ⚠️: errors, stale, gh-wait.
    Unsure,
    /// Listed above the stage steps, which stand: attached, foreign, question.
    Top,
    /// In place of every agent step, at most one shown: budget, failed.
    Standin,
}

impl HoldKind {
    pub const fn name(self) -> &'static str {
        match self {
            HoldKind::Busy => "busy",
            HoldKind::Unsure => "unsure",
            HoldKind::Top => "top",
            HoldKind::Standin => "standin",
        }
    }
}

impl StepDef {
    /// Declares the step's place on its stage's happy path, 1 first. The
    /// path is what the stage does when nothing is wrong, the sentence the
    /// Atlas says for it; a step without one is an exception, shown by the
    /// actor it waits on. NEXT never reads it: its list is a priority
    /// order, and this is the time order, declared beside it (#377).
    pub const fn path(mut self, n: u8) -> Self {
        self.path = Some(n);
        self
    }

    /// A path step taken only when <cond> holds (behind: the target moved),
    /// against one the stage always takes (push). The Atlas colors it and
    /// shows the condition.
    pub const fn path_if(mut self, n: u8, cond: &'static str) -> Self {
        self.path = Some(n);
        self.when = Some(cond);
        self
    }

    /// Declares what a Hold step does to the list; every Hold step has one.
    pub const fn hold(mut self, kind: HoldKind) -> Self {
        self.hold = Some(kind);
        self
    }
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

    /// The steps in no stage (#377): the Atlas draws them as NEXT's list.
    const HOLDS: [&str; 9] = ["attached", "budget", "errors", "failed", "foreign", "gh-wait", "question", "running", "stale"];

    #[test]
    fn every_step_has_a_stage_a_label_and_a_meaning_377() {
        for d in STEPS {
            assert!(!d.meaning.is_empty() && !d.label.is_empty(), "{}", d.key);
            assert!(d.stage == Stage::Hold || Stage::FLOW.contains(&d.stage), "{}", d.key);
        }
        let holds: BTreeSet<&str> = STEPS.iter().filter(|d| d.stage == Stage::Hold).map(|d| d.key).collect();
        assert_eq!(holds, HOLDS.into_iter().collect());
        assert_eq!(Stage::FLOW.map(Stage::name), ["build", "push", "ci", "review", "bot", "threads", "merge"]);
    }

    #[test]
    fn every_setting_a_step_reads_is_in_configs_typed_list_377() {
        let known = crate::config::setting_names();
        for d in STEPS {
            for s in settings_of(d) {
                assert!(known.contains(&s), "{}: settings.{s} isn't in config's typed list", d.key);
            }
        }
        let of = |k: &str| settings_of(find(k).unwrap());
        assert!(of("review-fix").contains(&"reviewRounds") && of("review-fix").contains(&"stepBudget"), "{:?}", of("review-fix"));
        assert!(of("bot-trigger").contains(&"botCommand") && !of("bot-trigger").contains(&"stepBudget"), "{:?}", of("bot-trigger"));
        assert!(of("conflict").contains(&"resolvePrompt"), "{:?}", of("conflict"));
        assert_eq!(of("ready"), Vec::<&str>::new());
    }
}
