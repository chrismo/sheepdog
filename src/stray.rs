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
    /// Each remote-tracking ref for the branch, by its reflog's length.
    logged: Vec<(usize, String)>,
}

impl Seen {
    /// Before the step, for <b>'s ("" for none).
    pub fn now(b: &str) -> Seen {
        let logged = tracking(b).into_iter().map(|r| (reflog(&r).len(), r)).collect();
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

    /// <r>'s reflog gained an "update by push" since now().
    fn pushed(&self, r: &str) -> bool {
        let had = self.logged.iter().find(|(_, l)| l == r).map_or(0, |(n, _)| *n);
        let log = reflog(r);
        log[..log.len().saturating_sub(had)].iter().any(|m| m.starts_with("update by push"))
    }
}

/// refs/remotes/<remote>/<b> for every configured remote; none without a branch.
fn tracking(b: &str) -> Vec<String> {
    if b.is_empty() {
        return vec![];
    }
    git(".", &["remote"]).stdout.lines().map(|r| format!("refs/remotes/{r}/{b}")).collect()
}

/// <r>'s reflog messages, newest first.
fn reflog(r: &str) -> Vec<String> {
    git(".", &["reflog", "show", "--format=%gs", r, "--"]).stdout.lines().map(str::to_string).collect()
}

fn short(sha: &str) -> &str {
    sha.get(..7).unwrap_or(sha)
}
