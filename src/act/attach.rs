// attach (A9, takeover): take branch.<b>.sd-lock, open the branch's session
// in its harness's TUI in its worktree, and release the lock when the TUI
// exits, however it exits. While the lock is held, agent steps refuse to run
// (attached_check); while an agent step holds the branch's running marker,
// attach refuses, --force or not (#270). A step past attached_check that
// hasn't written its marker yet when attach looks still runs alongside, as
// does one run with --force over the lock; either one checks the lock again
// before it commits and before it pushes (attached_during), and stops there.
//
// The session is branch.<b>.sd-session, but for one left by a step that
// ended without its session stored (#270): when the worktree has
// uncommitted or untracked changes and the branch's last usage.sup record is
// an editing step's (implement, address, ci-fix, restack --resolve) that
// failed or was killed, on the harness attach picked, in a session that isn't
// the stored one and has a transcript, attach resumes that session, which
// made the edits, and says so. On exit it's stored, as any resumed session
// is; a rerun of the step still forks the stored session (#69).

use super::commands::status;
use super::{cfg, die, hostname};
use crate::git::{git, git_out, main_worktree, worktree_for};
use crate::running;
use crate::val::at;
use crate::{config, harness, proc, time, usage};
use serde_json::Value;
use std::fs;
use std::io::Write;
use std::path::Path;

fn get(key: &str) -> String {
    git_out(".", &["config", "--get", key]).unwrap_or_default()
}

/// Whether attach may take a branch held by <lock>: free, or taken over with
/// --force, and then Ok is the note that says so.
fn take(b: &str, lock: &str, force: bool) -> Result<Option<String>, String> {
    match (lock, force) {
        ("", _) => Ok(None),
        (_, true) => Ok(Some(format!("taking over {b}'s lock ({lock})"))),
        _ => Err(format!("{b} is already attached ({lock}); exit that session, or --force to take it over")),
    }
}

/// The harness to attach with (--harness, else the stored one, else
/// <default>'s), and whether there's a session to resume. A stored session
/// is its stored harness's, claude's when none is, so --harness can't switch
/// it.
fn pick(b: &str, opt: &str, stored: &str, sid: &str, default: impl FnOnce() -> String) -> Result<(String, bool), String> {
    let h = match (opt, stored) {
        ("", "") => default(),
        ("", s) => s.to_string(),
        (o, _) => o.to_string(),
    };
    let owner = if stored.is_empty() { "claude" } else { stored };
    if sid.is_empty() {
        Ok((h, false))
    } else if h == owner {
        Ok((h, true))
    } else {
        Err(format!("branch.{b}.sd-session belongs to {owner}; unset it to start a {h} session"))
    }
}

/// The steps that edit the worktree, by their usage.sup names.
const EDITING: [&str; 4] = ["implement", "address", "ci-fix", "resolve"];

/// <repo>'s <b>'s last step in usage.sup's <text>, as (step, session), when
/// it was an editing step on <h> that failed in a session other than the
/// stored one, <stored>: its session was never stored.
fn unstored(text: &str, repo: &str, b: &str, h: &str, stored: &str) -> Option<(String, String)> {
    let r = text.lines().rev().filter(|l| usage::shaped(l)).filter_map(usage::sup_record).find(|r| {
        let field = |k: &str| r.get(k).and_then(Value::as_str);
        field("repo") == Some(repo) && field("branch") == Some(b)
    })?;
    let field = |k: &str| r.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
    let (step, session) = (field("step"), field("session"));
    let failed = r.get("status").and_then(Value::as_i64) != Some(0);
    (EDITING.contains(&step.as_str()) && failed && field("harness") == h && !session.is_empty() && session != stored)
        .then_some((step, session))
}

/// The session of <b>'s last step, as unstored finds it, when it left <wt>
/// with uncommitted changes and a transcript to resume: (step, session).
fn left_by_last_step(b: &str, wt: &str, h: &str, stored: &str) -> Option<(String, String)> {
    let text = fs::read_to_string(usage::file()).ok()?;
    let (step, sid) = unstored(&text, &main_worktree("."), b, h, stored)?;
    (harness::session_exists(h, &sid) && !status(wt).is_empty()).then_some((step, sid))
}

/// harnesses.<h>, defaults and all; null when none is set.
fn block(h: &str, b: &str) -> Value {
    match config::get_text("harnesses", b, true) {
        Ok(j) => serde_json::from_str::<Value>(&j).map(|all| at(&all, h).clone()).unwrap_or(Value::Null),
        Err(lines) if lines.is_empty() => Value::Null,
        Err(lines) => {
            for l in lines {
                eprintln!("sd config: {l}");
            }
            die("sd config failed")
        }
    }
}

pub fn attach(b: &str, harness_opt: &str, force: bool) -> ! {
    let wt = worktree_for(".", b);
    if wt.is_empty() {
        die(format!("{b} isn't checked out in any worktree"));
    }
    if !Path::new(&wt).is_dir() {
        die(format!("{b}'s worktree {wt} is gone; git worktree prune forgets it"));
    }
    let key = |k: &str| format!("branch.{b}.sd-{k}");
    let before = get(&key("lock"));
    let note = take(b, &before, force).unwrap_or_else(|e| die(e));
    let stored = get(&key("harness"));
    let read = get(&key("session"));
    let (h, resume) = pick(b, harness_opt, &stored, &read, || cfg("harness", b)).unwrap_or_else(|e| die(e));
    let left = left_by_last_step(b, &wt, &h, &read).map(|(step, sid)| {
        let not = if resume { read.clone() } else { "a new one".into() };
        let said = format!("{wt} has uncommitted changes from {b}'s last step, {step}, which ended without its session stored: resuming its session {sid}, not {not}");
        (said, sid)
    });
    let resume = resume || left.is_some();
    let sid = match &left {
        Some((_, sid)) => sid.clone(),
        None if resume => read.clone(),
        None => harness::new_session(),
    };
    let mut c = harness::tui(&h, &sid, resume, &block(&h, b)).unwrap_or_else(|e| die(e));
    c.current_dir(&wt);

    let mine = format!("{}@{} {}", std::process::id(), hostname(), time::now_utc().0);
    let set = git(".", &["config", &key("lock"), &mine]);
    if !set.ok {
        eprint!("{}", set.stderr);
        std::process::exit(set.code);
    }
    // After the lock, so a step that starts from here refuses (attached_check);
    // --force or not, the lock goes back to what it was.
    if let Err(e) = running::idle(".", b, "attaching") {
        if get(&key("lock")) == mine {
            match before.as_str() {
                "" => git(".", &["config", "--unset", &key("lock")]),
                _ => git(".", &["config", &key("lock"), &before]),
            };
        }
        die(e);
    }
    for note in [note, left.map(|(said, _)| said)].into_iter().flatten() {
        eprintln!("sd act: {note}");
    }
    let mode = if resume { "resume" } else { "new" };
    eprintln!("sd act: attached to {b}: {h} ({mode} session {sid}) in {wt}; agent steps wait until you exit");
    let mut rc = proc::foreground(c);

    // A closed terminal leaves stderr dead, and eprint! would panic there,
    // lock and all: what's left to say is said or dropped.
    let say = |m: &str| {
        let _ = std::io::stderr().write_all(m.as_bytes());
    };
    let now = get(&key("session"));
    if now != read {
        // Someone else, a step's step_store most likely, moved it: theirs stands.
        let moved = if now.is_empty() { "was unset".to_string() } else { format!("moved to {now}") };
        say(&format!("sd act: {b}'s session {moved} during the attach, so {sid} isn't stored\n"));
    } else if resume || harness::session_exists(&h, &sid) {
        for (k, v) in [("session", &sid), ("harness", &h)] {
            let o = git(".", &["config", &key(k), v]);
            if !o.ok {
                say(&o.stderr);
                rc = o.code;
                break;
            }
        }
    } else {
        say(&format!("sd act: {h} wrote no transcript for session {sid}, so nothing is stored; attach again to start one\n"));
    }
    // Only the lock this attach took: a --force may have taken it over since.
    if get(&key("lock")) == mine {
        git(".", &["config", "--unset", &key("lock")]);
    }
    std::process::exit(rc)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn take_a_free_branch_else_only_with_force_saying_so() {
        let lock = "123@host 2026-10-15T00:00:00Z";
        assert_eq!(take("feat", "", false), Ok(None));
        assert_eq!(take("feat", "", true), Ok(None));
        assert_eq!(take("feat", lock, true), Ok(Some(format!("taking over feat's lock ({lock})"))));
        assert_eq!(
            take("feat", lock, false),
            Err(format!("feat is already attached ({lock}); exit that session, or --force to take it over"))
        );
    }

    #[test]
    fn pick_is_the_flag_else_the_stored_harness_else_the_default() {
        let none = || -> String { panic!("the default isn't read when a harness is named") };
        assert_eq!(pick("feat", "", "", "", || "pi".into()), Ok(("pi".into(), false)));
        assert_eq!(pick("feat", "", "pi", "", none), Ok(("pi".into(), false)));
        assert_eq!(pick("feat", "pi", "claude", "", none), Ok(("pi".into(), false)));
    }

    fn row(repo: &str, branch: &str, step: &str, harness: &str, session: &str, status: i32) -> String {
        format!(
            "{{time:2026-10-28T00:00:00Z,repo:\"{repo}\",branch:\"{branch}\",step:\"{step}\",harness:\"{harness}\",session:\"{session}\",mode:\"fork\",status:{status}}}\n"
        )
    }

    #[test]
    fn unstored_is_the_branchs_last_step_when_it_was_a_failed_edit_in_an_unstored_session() {
        let found = |text: &str| unstored(text, "/r", "feat", "claude", "stored");
        let hit = Some(("implement".to_string(), "s".to_string()));
        assert_eq!(found(""), None);
        assert_eq!(found(&row("/r", "feat", "implement", "claude", "s", 1)), hit);
        for step in ["address", "ci-fix", "resolve"] {
            assert_eq!(found(&row("/r", "feat", step, "claude", "s", 143)), Some((step.to_string(), "s".to_string())));
        }
        // A read-only step, one that succeeded, the stored session, another harness's.
        assert_eq!(found(&row("/r", "feat", "review", "claude", "s", 1)), None);
        assert_eq!(found(&row("/r", "feat", "implement", "claude", "s", 0)), None);
        assert_eq!(found(&row("/r", "feat", "implement", "claude", "stored", 1)), None);
        assert_eq!(found(&row("/r", "feat", "implement", "pi", "s", 1)), None);
        // Only the branch's last record counts; other branches' and repos', and lines
        // that aren't records, don't.
        let failed = row("/r", "feat", "implement", "claude", "s", 1);
        assert_eq!(found(&(failed.clone() + &row("/r", "feat", "describe", "claude", "s2", 0))), None);
        let after = row("/r", "other", "review", "claude", "o", 0) + &row("/elsewhere", "feat", "review", "claude", "o", 0);
        assert_eq!(found(&(failed.clone() + &after + "{time:cut off\n")), hit);
    }

    #[test]
    fn pick_resumes_a_stored_session_only_with_its_own_harness() {
        let none = || -> String { panic!("the default isn't read when a harness is named") };
        assert_eq!(pick("feat", "", "pi", "sid", none), Ok(("pi".into(), true)));
        assert_eq!(pick("feat", "claude", "", "sid", none), Ok(("claude".into(), true)));
        // A session with no stored harness is claude's.
        assert_eq!(pick("feat", "", "", "sid", || "claude".into()), Ok(("claude".into(), true)));
        assert_eq!(
            pick("feat", "", "", "sid", || "pi".into()),
            Err("branch.feat.sd-session belongs to claude; unset it to start a pi session".into())
        );
        assert_eq!(
            pick("feat", "pi", "claude", "sid", none),
            Err("branch.feat.sd-session belongs to claude; unset it to start a pi session".into())
        );
    }
}
