// Run NEXT's step from the Flock's drill-in (#396): POST /api/act {id, key}
// runs `sd act <verb> <branch>` in the branch's repo, the watcher's own
// command for that step (watch/pass.rs dispatch_verb), when <key> is still
// NEXT's first step in the watcher's cache. One step at a time, on its own
// thread; when it ends, the branch is checked again (refresh.rs) so the
// page's poll shows the result. The page keeps no state: a card says
// `acting` while its step runs, after a reload too.

use super::branch::find;
use super::refresh;
use crate::proc;
use crate::val::{at, t};
use crate::watch::{act_args, dispatch_verb, gate, may_dispatch, registered, repo_key, sd_verb, step_secs, watch_timeout};
use serde_json::{json, Value};
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

struct Acting {
    id: String,
    key: String,
    branch: String,
    verb: String,
}
static ACTING: Mutex<Option<Acting>> = Mutex::new(None);

fn acting() -> MutexGuard<'static, Option<Acting>> {
    ACTING.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The step slot while no step runs; no step starts while it's held.
pub fn idle() -> Option<impl Sized> {
    let slot = acting();
    slot.is_none().then_some(slot)
}

/// Frees the one slot when the step and its refresh end, even by a panic.
struct Running;

impl Drop for Running {
    fn drop(&mut self) {
        *acting() = None;
    }
}

/// The key of the step running on the branch <id>, else null.
pub fn running(id: &str) -> Value {
    acting().as_ref().filter(|a| a.id == id).map_or(Value::Null, |a| json!(a.key))
}

/// {key, verb} for NEXT's first step when autoRun would run it, else null:
/// the drill-in's button, labelled with the verb.
pub fn offered(s: &Value) -> Value {
    let Some(first) = at(s, "next").get(0) else { return Value::Null };
    match dispatch_verb(first).filter(|_| may_dispatch(s, &t(first, "key"))) {
        Some(step) => json!({"key": at(first, "key"), "verb": act_args(step).join(" ")}),
        None => Value::Null,
    }
}

/// The registered repo whose branches/ dir is <id>'s.
pub(super) fn repo_of(id: &str) -> Option<String> {
    let dir = id.split_once('/')?.0;
    registered().into_iter().find(|r| repo_key(r) == dir)
}

/// The step to run for <key>, NEXT's first step in <s>, or why it isn't run.
fn step_for(s: &Value, key: &str) -> Result<&'static str, String> {
    let first = at(s, "next").get(0).unwrap_or(&Value::Null);
    let now = t(first, "key");
    if now != key {
        let now = if now.is_empty() { "nothing".to_string() } else { now };
        return Err(format!("NEXT's first step is {now} now, not {key}: the page is behind\n"));
    }
    let step = dispatch_verb(first).ok_or_else(|| match t(first, "who").as_str() {
        "you" => format!("{key} is your step: sd act doesn't run it\n"),
        "wait" => format!("{key} is a wait: nothing to run\n"),
        _ => format!("{key} has no sd act verb\n"),
    })?;
    gate(s, key).map_err(|why| format!("{key}: autoRun doesn't run it, {why}: run sd act by hand if you mean to\n"))?;
    Ok(step)
}

/// POST /api/act's body {id, key}: the status line and the body.
pub fn run(state: &Path, body: &str) -> (&'static str, String) {
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let (id, key) = (t(&v, "id"), t(&v, "key"));
    if id.is_empty() || key.is_empty() {
        return ("400 Bad Request", "the body names the branch id and the step key\n".into());
    }
    let Some(c) = find(state, &id) else { return ("404 Not Found", "no such branch in the watcher's cache\n".into()) };
    let s = c.doc.as_ref().map_or(&Value::Null, |d| at(d, "state"));
    let step = match step_for(s, &key) {
        Ok(step) => step,
        Err(why) => return ("409 Conflict", why),
    };
    let Some(repo) = repo_of(&id) else { return ("409 Conflict", format!("{}'s repo isn't registered with sd watch\n", c.branch)) };
    let verb = act_args(step).join(" ");
    let mut slot = acting();
    if let Some(a) = slot.as_ref() {
        return ("409 Conflict", format!("sd act {} is running on {}: one step at a time\n", a.verb, a.branch));
    }
    *slot = Some(Acting { id: id.clone(), key, branch: c.branch.clone(), verb: verb.clone() });
    drop(slot);
    let said = format!("running sd act {verb} {} in {repo}\n", c.branch);
    let (state, branch) = (state.to_path_buf(), c.branch.clone());
    std::thread::spawn(move || launch(&state, &id, &repo, step, &branch));
    ("200 OK", said)
}

/// sd act <step> <branch> in <repo>, then the branch's refresh, holding the
/// slot. A Ctrl-C to sd view ends the step (proc::timeout passes it on to
/// the step's group, as a hand-run sd act does to its agent), and sd view
/// ends once the line saying so is written; the watcher's next pass sees
/// where the branch was left.
fn launch(state: &Path, id: &str, repo: &str, step: &str, branch: &str) {
    let _running = Running;
    let (prog, pre) = sd_verb("act", "SHEEPDOG_ACT_PROGRAM");
    let verb = act_args(step);
    let secs = step_secs(repo, step, branch, watch_timeout());
    proc::deferring_signals(|| {
        let out = proc::with_timeout_merged(secs, &prog, &[&pre[..], &verb, &[branch]].concat(), Some(repo));
        let how = if out.ok {
            "ok".to_string()
        } else if out.timed_out {
            format!("timed out after {secs}s")
        } else if out.stopped {
            "stopped by a signal".to_string()
        } else {
            format!("failed (exit {}): {}", out.code, out.stdout.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or(""))
        };
        eprintln!("sd view: sd act {} {branch} in {repo}: {how}", verb.join(" "));
        if !out.stopped {
            refresh::now(state, id, repo, branch);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn next(key: &str, who: &str) -> Value {
        json!({"viewer": "me", "pr": {"author": "me"}, "next": [{"key": key, "who": who}, {"key": "ci-wait", "who": "wait"}]})
    }

    #[test]
    fn a_pr_someone_else_wrote_gets_no_button_and_is_refused() {
        let mut s = next("push", "run");
        s["pr"]["author"] = json!("them");
        assert_eq!(offered(&s), Value::Null);
        assert_eq!(step_for(&s, "push"), Err("push: autoRun doesn't run it, the PR is them's: run sd act by hand if you mean to\n".into()));
    }

    /// A branch with no PR yet runs its steps (#307).
    #[test]
    fn a_branch_with_no_pr_yet_gets_the_button_307() {
        let mut s = next("implement", "agent");
        s["pr"] = Value::Null;
        assert_eq!(offered(&s), json!({"key": "implement", "verb": "implement"}));
        assert_eq!(step_for(&s, "implement"), Ok("implement"));
    }

    #[test]
    fn the_button_is_offered_for_the_agent_s_and_sheepdog_s_steps_only() {
        assert_eq!(offered(&next("review", "agent")), json!({"key": "review", "verb": "review"}));
        assert_eq!(offered(&next("conflict", "agent")), json!({"key": "conflict", "verb": "restack --resolve"}));
        assert_eq!(offered(&next("push", "run")), json!({"key": "push", "verb": "push"}));
        for (k, w) in [("ready", "you"), ("bot", "you"), ("ci-wait", "wait"), ("open-pr", "run"), ("behind", "wait")] {
            assert_eq!(offered(&next(k, w)), Value::Null, "{k} {w}");
        }
        assert_eq!(offered(&json!({"next": []})), Value::Null);
    }

    #[test]
    fn a_refusal_says_which_step_is_first_now_or_whose_it_is() {
        assert_eq!(step_for(&next("review", "agent"), "review"), Ok("review"));
        assert_eq!(step_for(&next("conflict", "agent"), "conflict"), Ok("resolve"));
        assert_eq!(step_for(&next("push", "run"), "review"), Err("NEXT's first step is push now, not review: the page is behind\n".into()));
        assert_eq!(step_for(&json!({"next": []}), "review"), Err("NEXT's first step is nothing now, not review: the page is behind\n".into()));
        assert_eq!(step_for(&next("ready", "you"), "ready"), Err("ready is your step: sd act doesn't run it\n".into()));
        assert_eq!(step_for(&next("ci-wait", "wait"), "ci-wait"), Err("ci-wait is a wait: nothing to run\n".into()));
        assert_eq!(step_for(&next("open-pr", "run"), "open-pr"), Err("open-pr has no sd act verb\n".into()));
    }
}
