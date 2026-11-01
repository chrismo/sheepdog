// A step's agent committing or pushing itself (#346): logged on stderr and
// usage.sup, never refused (#79: log, don't block).
//
// A commit is HEAD moving during the step (a reset or checkout counts too).
// A push is a remote-tracking ref for the branch, at any configured remote,
// gaining an "update by push" reflog entry during the step; a fetch moving
// it isn't one. Unseen: a push to a URL or to another branch's name, which
// moves no remote-tracking ref for the branch.

use crate::git::{git, rev};

/// What a step's agent did itself; None where it can't be told (no HEAD, or
/// for a push, no branch).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Stray {
    pub commit: Option<bool>,
    pub push: Option<bool>,
}

/// The repo as a step found it, in the current directory.
pub struct Seen {
    head: String,
    branch: String,
    /// Each remote-tracking ref for the branch, by its reflog's newest entry
    /// (None for none). Not by its length: a commit's gc --auto can expire
    /// entries during the step.
    logged: Vec<(Option<String>, String)>,
}

impl Seen {
    /// Before the step, for <b>'s ("" for none).
    pub fn now(b: &str) -> Seen {
        let logged = tracking(b).into_iter().map(|r| (reflog(&r).into_iter().next(), r)).collect();
        Seen { head: rev(".", "HEAD"), branch: b.to_string(), logged }
    }

    /// What the agent did since, said on stderr as <label>'s.
    pub fn since(&self, label: &str) -> Stray {
        let head = rev(".", "HEAD");
        let pushed: Vec<String> = tracking(&self.branch).into_iter().filter(|r| self.pushed(r)).collect();
        let commit = (!self.head.is_empty()).then(|| head != self.head);
        if commit == Some(true) {
            let on = if self.branch.is_empty() { String::new() } else { format!(" on {}", self.branch) };
            eprintln!("sd: {label}: the agent committed itself{on} ({} -> {})", short(&self.head), short(&head));
        }
        if !pushed.is_empty() {
            eprintln!("sd: {label}: the agent pushed {} itself ({})", self.branch, pushed.join(", "));
        }
        Stray { commit, push: (!self.branch.is_empty()).then_some(!pushed.is_empty()) }
    }

    /// <r>'s reflog gained an "update by push" since now(): an entry above
    /// the newest it had then, none older than that one. Were that one
    /// expired, the time alone tells.
    fn pushed(&self, r: &str) -> bool {
        let had = self.logged.iter().find(|(_, l)| l == r).and_then(|(e, _)| e.as_deref());
        let floor = had.map_or(i64::MIN, stamp);
        reflog(r)
            .iter()
            .take_while(|e| Some(e.as_str()) != had)
            .filter(|e| stamp(e) >= floor)
            .any(|e| e.splitn(3, '\t').nth(2).is_some_and(|m| m.starts_with("update by push")))
    }
}

/// A reflog entry's time, from its selector (ref@{<unix time>}).
fn stamp(e: &str) -> i64 {
    let gd = e.split('\t').next().unwrap_or_default();
    gd.rsplit_once("@{").and_then(|(_, t)| t.trim_end_matches('}').parse().ok()).unwrap_or(i64::MIN)
}

/// refs/remotes/<remote>/<b> for every configured remote; none without a branch.
fn tracking(b: &str) -> Vec<String> {
    if b.is_empty() {
        return vec![];
    }
    git(".", &["remote"]).stdout.lines().map(|r| format!("refs/remotes/{r}/{b}")).collect()
}

/// <r>'s reflog entries, newest first: selector with its time, SHA and
/// message, tab-separated.
fn reflog(r: &str) -> Vec<String> {
    git(".", &["reflog", "show", "--date=unix", "--format=%gd%x09%H%x09%gs", r, "--"])
        .stdout
        .lines()
        .map(str::to_string)
        .collect()
}

fn short(sha: &str) -> &str {
    sha.get(..7).unwrap_or(sha)
}
