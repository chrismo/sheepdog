// restack --resolve (J1): the rebase has stopped on a conflict, and the agent
// resolves each stop in the branch's session; the pipeline adds and
// continues. restack.rs runs it under the branch's running marker, across
// every stop and continue (#170), and rolls the branch back when it fails,
// unless the branch was attached during the stop: then the rebase is left
// stopped, with the edits in it (#270).

use super::attached_during;
use super::editing::{unknown, Outcome, Spec};
use super::step::{step_session, step_store};
use crate::git::{git, git_out, rebase_head, strip_nl, Git};
use crate::guidance::{lib_dir, step_guidance};
use crate::proc;
use crate::val::t;
use serde_json::Value;
use std::fs;
use std::path::Path;

const CAP: usize = 3;

// The resolve step's prompt: guidance (settings.resolvePrompt, else
// .sheepdog/resolve.md in the base commit, else lib/prompts/resolve.md), then
// this contract, which the restack's rollback and refusals rely on.
const CONTRACT: &str = r#"Do not run git add, git rebase --continue, or commit: the pipeline does that
after you, and it will stop again if another commit conflicts. If no sensible
resolution exists, answer with outcome "gave-up" and say why; nothing you
changed will be kept.

Before resolving, judge whether the commit being replayed (named below, among
all the commits this rebase replays) is this branch's work, from its subject,
the files it changes and the task. If it isn't, and its change is already on the target
(e.g. a merged parent's commit, which the target holds as a squash), the replay
itself is wrong: don't resolve it, answer with outcome "already-on-target" and
say why. The pipeline rolls back and refuses; it never drops a commit on your
word."#;

const SCHEMA: &str = r#"{"type": "object", "required": ["outcome", "summary"],
 "properties": {"outcome": {"enum": ["done", "gave-up", "already-on-target"]}, "summary": {"type": "string"}}}"#;

/// The stopped rebase: <b> in <wt>, from <old_base>..<old_head> onto <onto>
/// (<remote>/<target>).
pub struct Rebase<'a> {
    pub b: &'a str,
    pub wt: &'a str,
    pub old_base: &'a str,
    pub old_head: &'a str,
    pub onto: &'a str,
}

impl Rebase<'_> {
    fn back(&self) -> String {
        format!("rolled back to {}", self.old_head.get(..7).unwrap_or(self.old_head))
    }
}

/// Why a resolve stopped. <gave_up>, the question's text, marks a stop the
/// agent caused, which a rerun onto the same target would repeat (#257).
/// <keep>: the rebase is left stopped, not rolled back, as the worktree holds
/// edits from an attach (#270).
#[derive(Debug, PartialEq)]
pub struct Halt {
    pub why: String,
    pub gave_up: Option<String>,
    pub keep: bool,
}

impl From<String> for Halt {
    fn from(why: String) -> Self {
        Halt { why, gave_up: None, keep: false }
    }
}

fn gave_up(text: String, why: String) -> Halt {
    Halt { why, gave_up: Some(text), keep: false }
}

struct Stop<'a> {
    task: &'a str,
    replaying: &'a str,
    stat: &'a str,
    replayed: &'a str,
    files: &'a [String],
    hunks: &'a str,
}

fn prompt(r: &Rebase, guidance: &str, stop: &Stop) -> String {
    let task = if stop.task.is_empty() { "(none recorded)" } else { stop.task };
    format!(
        "{guidance}\n\n{CONTRACT}\n\nBranch: {}, rebasing onto {}\nTask: {task}\nReplaying: {}\n{}\n\n\
         Commits being replayed, oldest first:\n{}\n\nConflicting files:\n{}\n\nConflict hunks:\n{}",
        r.b,
        r.onto,
        stop.replaying,
        stop.stat,
        stop.replayed,
        stop.files.join("\n"),
        stop.hunks
    )
}

/// The agent's result for a stop: Ok(its summary) to go on, else why the
/// restack stops. A replayed commit the agent finds already on the target
/// is a wrong base, never dropped on its word.
fn verdict(r: &Rebase, replaying: &str, result: &Value) -> Result<String, Halt> {
    let (b, onto, back, summary) = (r.b, r.onto, r.back(), t(result, "summary"));
    match Outcome::of(result, "done") {
        Outcome::Done => Ok(summary),
        Outcome::Other(w) if w == "already-on-target" => {
            let sha = replaying.split(' ').next().unwrap_or("");
            let why = format!(
                "wrong base: the agent says {replaying} is already on {onto} ({summary}); {back}. If it's a merged parent's \
                 and the stack is linear, rebase past it by hand: git rebase --onto {onto} {sha} {b}, \
                 then git update-ref refs/sheepdog/{b}/base $(git merge-base {onto} {b})"
            );
            Err(gave_up(why.clone(), why))
        }
        Outcome::GaveUp(why) => Err(gave_up(why.clone(), format!("the agent gave up: {why}; {back}"))),
        Outcome::NeedsInput(_) | Outcome::Other(_) => {
            let (text, line) = unknown(&t(result, "outcome"), &summary);
            Err(gave_up(text, format!("{line}; {back}")))
        }
    }
}

/// The <files> in <wt> with a conflict marker left in them.
fn marked(wt: &str, files: &[String]) -> Vec<String> {
    let has_marker = |f: &&String| {
        let text = fs::read(Path::new(wt).join(f)).unwrap_or_default();
        String::from_utf8_lossy(&text).lines().any(|l| l.starts_with("<<<<<<< ") || l.starts_with(">>>>>>> "))
    };
    files.iter().filter(has_marker).cloned().collect()
}

/// The tree of everything in <wt>, tracked or not (ignored files aside),
/// staged or not, through a scratch index; the real index, mid-rebase, is
/// left alone.
fn worktree_tree(wt: &str) -> Option<String> {
    let dir = proc::private_dir("sd-resolve")?;
    let index = dir.join("index");
    let step = |args: &[&str]| Some(Git::new(wt, args).env("GIT_INDEX_FILE", &index).run()).filter(|o| o.ok);
    let tree = step(&["read-tree", "HEAD"])
        .and_then(|_| step(&["add", "-A"]))
        .and_then(|_| step(&["write-tree"]))
        .map(|o| strip_nl(o.stdout));
    let _ = fs::remove_dir_all(&dir);
    tree
}

/// Resolve the stopped rebase, stop by stop, in <harness> (--harness's, else
/// the branch's own) and the branch's session, run from <wt>. Ok(the files
/// the agent changed): the worktree before and after each stop compared
/// whole, so staging or committing them itself hides nothing. Err(why), the
/// rebase left for the caller to roll back.
pub fn resolve_conflicts(r: &Rebase, harness: &str) -> Result<Vec<String>, Halt> {
    let (b, wt) = (r.b, r.wt);
    std::env::set_current_dir(wt).map_err(|e| format!("can't work in {wt}: {e}; {}", r.back()))?;
    let task = git_out(".", &["config", "--get", &format!("branch.{b}.sd-task")]).unwrap_or_default();
    let range = format!("{}..{}", r.old_base, r.old_head);
    let replayed = git_out(".", &["log", "--reverse", "--no-merges", "--topo-order", "--format=%h %s", &range]).unwrap_or_default();
    // implement's settings and session, logged in usage.sup as its own step.
    let mut s = step_session(b, "implement", harness, false);
    let guidance = step_guidance("resolvePrompt", b, r.old_base, &lib_dir().join("prompts").join("resolve.md")).map_err(|e| {
        eprintln!("{e}");
        format!("can't read the resolve guidance; {}", r.back())
    })?;
    let read = |p: &str| crate::config::get_text(p, b, false);
    let verify = match (read("settings.verify"), read("settings.implementAllow")) {
        (Ok(verify), Ok(_)) => verify,
        (Err(lines), _) | (_, Err(lines)) => {
            lines.iter().for_each(|l| eprintln!("sd config: {l}"));
            return Err(format!("sd config failed; {}", r.back()).into());
        }
    };

    let mut touched: Vec<String> = Vec::new();
    for stop in 1.. {
        if stop > CAP {
            let why = format!("the rebase onto {} stopped {stop} times (cap {CAP})", r.onto);
            return Err(gave_up(why.clone(), format!("{why}; {}", r.back())));
        }
        let files: Vec<String> = git(wt, &["diff", "--name-only", "--diff-filter=U"]).stdout.lines().map(str::to_string).collect();
        if files.is_empty() {
            return Err(format!("the rebase stopped without conflicting files; {}", r.back()).into());
        }
        let hunks = strip_nl(git(wt, &["diff"]).stdout);
        let replaying = git_out(wt, &["log", "-1", "--format=%h %s", "REBASE_HEAD"]).unwrap_or_default();
        let stat = git_out(wt, &["show", "--stat", "--format=", "REBASE_HEAD"]).unwrap_or_default();
        let snap = worktree_tree(wt).ok_or_else(|| format!("can't snapshot {wt} before the agent runs; {}", r.back()))?;
        let shown = Stop { task: &task, replaying: &replaying, stat: &stat, replayed: &replayed, files: &files, hunks: &hunks };
        let done = Spec {
            b,
            wt,
            step: "resolve",
            asks_as: "resolve",
            session: &s,
            base: r.old_base,
            prompt: prompt(r, &guidance, &shown),
            schema: SCHEMA,
            done: "done",
            doing: format!("resolving conflicts in: {} (stop {stop})", files.join(", ")),
            verify: &verify,
            answer: "",
            items: "",
        }
        .agent();
        // Before anything that rolls back: a rollback would throw the
        // attached session's edits away along with the agent's.
        if let Some(why) = attached_during(b, "resolve") {
            let why = format!("not committing: {why}; the edits are left in {wt}, mid-rebase (git rebase --continue, or --abort)");
            return Err(Halt { why, gave_up: None, keep: true });
        }
        let Some((result, ran)) = done else {
            let why = format!("the {} step failed or returned no usable result (session {}); {}", s.harness, s.id, r.back());
            return Err(why.into());
        };
        step_store(b, &s, &ran);
        s.fork_of(ran);
        println!("{b} (stop {stop}): {}", verdict(r, &replaying, &result)?);
        let after = worktree_tree(wt).ok_or_else(|| format!("can't read what the agent changed in {wt}; {}", r.back()))?;
        let diff = git(wt, &["diff-tree", "-z", "-r", "--name-only", "--no-renames", &snap, &after]).stdout;
        touched.extend(diff.split('\0').filter(|f| !f.is_empty()).map(str::to_string));
        let left = marked(wt, &files);
        if !left.is_empty() {
            let why = format!("conflict markers left in: {}", left.join(", "));
            return Err(gave_up(why.clone(), format!("{why}; {}", r.back())));
        }
        git(wt, &["add", "-A"]);
        if Git::new(wt, &["rebase", "--continue"]).env("GIT_EDITOR", "true").run().ok {
            break;
        }
        if rebase_head(wt).is_none() {
            return Err(format!("git rebase --continue failed; {}", r.back()).into());
        }
    }
    Ok(touched)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rebase() -> Rebase<'static> {
        Rebase { b: "feat", wt: "/wt", old_base: "base", old_head: "0123456789abcdef", onto: "origin/main" }
    }

    #[test]
    fn verdict_done_goes_on_with_the_agents_summary() {
        let r = json!({"outcome": "done", "summary": "kept both"});
        assert_eq!(verdict(&rebase(), "abc1234 feat", &r), Ok("kept both".into()));
    }

    #[test]
    fn verdict_already_on_target_is_a_wrong_base_with_the_way_past_it() {
        let r = json!({"outcome": "already-on-target", "summary": "main already has this"});
        let why = "wrong base: the agent says abc1234 feat two is already on origin/main (main already has this); rolled back to 0123456. \
                   If it's a merged parent's and the stack is linear, rebase past it by hand: git rebase --onto origin/main abc1234 feat, \
                   then git update-ref refs/sheepdog/feat/base $(git merge-base origin/main feat)";
        assert_eq!(verdict(&rebase(), "abc1234 feat two", &r), Err(gave_up(why.into(), why.into())));
    }

    #[test]
    fn verdict_gave_up_is_giving_up() {
        let r = json!({"outcome": "gave-up", "summary": "no sensible merge"});
        let why = "the agent gave up: no sensible merge; rolled back to 0123456";
        assert_eq!(verdict(&rebase(), "abc1234 feat", &r), Err(gave_up("no sensible merge".into(), why.into())));
    }

    #[test]
    fn verdict_an_outcome_resolve_has_no_word_for_is_given_up_on_naming_it() {
        for o in ["maybe", "needs-input"] {
            let r = json!({"outcome": o, "summary": "no sensible merge"});
            let text = format!("returned outcome \"{o}\": no sensible merge");
            let why = format!("the agent returned outcome \"{o}\"; rolled back to 0123456");
            assert_eq!(verdict(&rebase(), "abc1234 feat", &r), Err(gave_up(text, why)));
        }
        let why = "the agent returned outcome \"\"; rolled back to 0123456";
        assert_eq!(verdict(&rebase(), "abc1234 feat", &json!({})), Err(gave_up("returned outcome \"\": ".into(), why.into())));
    }

    #[test]
    fn prompt_is_guidance_contract_then_the_stop() {
        let files = ["a.txt".to_string(), "b.txt".to_string()];
        let stop = Stop {
            task: "",
            replaying: "abc1234 feat",
            stat: " a.txt | 2 +-\n 1 file changed",
            replayed: "abc1234 feat\ndef5678 feat two",
            files: &files,
            hunks: "<<<<<<< HEAD",
        };
        let p = prompt(&rebase(), "Guide.", &stop);
        assert!(p.starts_with(&format!("Guide.\n\n{CONTRACT}\n\nBranch: feat, rebasing onto origin/main\nTask: (none recorded)\n")));
        assert!(p.contains("\nReplaying: abc1234 feat\n a.txt | 2 +-\n 1 file changed\n\nCommits being replayed, oldest first:\nabc1234 feat\ndef5678 feat two\n\n"));
        assert!(p.ends_with("\n\nConflicting files:\na.txt\nb.txt\n\nConflict hunks:\n<<<<<<< HEAD"));
    }

    #[test]
    fn marked_finds_only_files_with_a_conflict_marker_at_a_line_start() {
        let dir = std::env::temp_dir().join(format!("sd-resolve-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("left"), "a\n<<<<<<< HEAD\nb\n").unwrap();
        fs::write(dir.join("right"), "a\n>>>>>>> feat\n").unwrap();
        fs::write(dir.join("clean"), "a\n x <<<<<<< not at the start\n=======\n").unwrap();
        let files: Vec<String> = ["left", "right", "clean", "gone"].map(String::from).to_vec();
        assert_eq!(marked(&dir.to_string_lossy(), &files), ["left", "right"]);
        fs::remove_dir_all(&dir).unwrap();
    }
}
