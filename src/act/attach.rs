// attach (A9, takeover): take branch.<b>.sd-lock, open the branch's session
// in its harness's TUI in its worktree, and release the lock when the TUI
// exits, however it exits. While the lock is held, agent steps refuse to run
// (attached_check).

use super::{cfg, die, hostname};
use crate::git::{git, git_out, worktree_for};
use crate::val::at;
use crate::{config, harness, proc, time};
use serde_json::Value;
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
    if let Some(note) = take(b, &get(&key("lock")), force).unwrap_or_else(|e| die(e)) {
        eprintln!("sd act: {note}");
    }
    let stored = get(&key("harness"));
    let sid = get(&key("session"));
    let (h, resume) = pick(b, harness_opt, &stored, &sid, || cfg("harness", b)).unwrap_or_else(|e| die(e));
    let sid = if resume { sid } else { harness::new_session() };
    let mut c = harness::tui(&h, &sid, resume, &block(&h, b)).unwrap_or_else(|e| die(e));
    c.current_dir(&wt);

    let mine = format!("{}@{} {}", std::process::id(), hostname(), time::now_utc().0);
    let set = git(".", &["config", &key("lock"), &mine]);
    if !set.ok {
        eprint!("{}", set.stderr);
        std::process::exit(set.code);
    }
    let mode = if resume { "resume" } else { "new" };
    eprintln!("sd act: attached to {b}: {h} ({mode} session {sid}) in {wt}; agent steps wait until you exit");
    let mut rc = proc::foreground(c);

    // A closed terminal leaves stderr dead, and eprint! would panic there,
    // lock and all: what's left to say is said or dropped.
    let say = |m: &str| {
        let _ = std::io::stderr().write_all(m.as_bytes());
    };
    if resume || harness::session_exists(&h, &sid) {
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
