// A branch's settings on the drill-in (#396): GET /api/branch carries every
// settings.* key with its value and the layer it came from, from `sd config
// explain --json --branch <b>` in the branch's repo, and POST /api/config
// {id, path, value} runs `sd config set --branch <b> -- <path> <value>`
// there. Only the few a branch tunes day to day (EDITABLE) are set; the page
// shows the rest. sd config does the typing: its refusal is the page's, as a 400.

use super::act::repo_of;
use super::branch::find;
use crate::proc;
use crate::val::{at, t};
use crate::watch::sd_verb;
use serde_json::{json, Value};
use std::path::Path;

const SECS: u64 = 30;

/// What the page sets; interval is global-file only, so the page shows it.
pub const EDITABLE: [&str; 3] = ["settings.autoRun", "settings.reviewRounds", "settings.stepBudget"];

/// <path> is one the page sets. The API reaches no further than the page:
/// settings.verify is a command the pipeline runs, so it stays sd config's.
pub fn allowed(path: &str) -> bool {
    EDITABLE.contains(&path)
}

fn sd_config(repo: &str, args: &[&str]) -> proc::Out {
    let (prog, pre) = sd_verb("config", "SHEEPDOG_CONFIG_PROGRAM");
    proc::with_timeout(SECS, &prog, &[&pre[..], args].concat(), Some(repo))
}

/// [{path, value, source, edit}] for <branch>'s settings.* keys in <repo>,
/// or null when sd config couldn't explain them.
pub fn list(repo: &str, branch: &str) -> Value {
    let out = sd_config(repo, &["explain", "--json", "--branch", branch]);
    if !out.ok {
        eprintln!("sd view: sd config explain --branch {branch} in {repo}: {}", out.stderr.lines().next().unwrap_or("failed"));
        return Value::Null;
    }
    let Ok(v) = serde_json::from_str::<Value>(&out.stdout) else { return Value::Null };
    let keys = at(&v, "keys").as_array().cloned().unwrap_or_default();
    let rows = keys.iter().filter(|k| t(k, "path").starts_with("settings.")).map(|k| {
        let path = t(k, "path");
        json!({"path": path, "value": at(k, "value"), "source": at(k, "source"), "editable": EDITABLE.contains(&path.as_str())})
    });
    Value::Array(rows.collect())
}

/// POST /api/config's body {id, path, value}: the status line and the body.
pub fn set(state: &Path, body: &str) -> (&'static str, String) {
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let (id, path, value) = (t(&v, "id"), t(&v, "path"), t(&v, "value"));
    if id.is_empty() || path.is_empty() {
        return ("400 Bad Request", "the body names the branch id, the setting's path and its value\n".into());
    }
    if !allowed(&path) {
        return ("400 Bad Request", format!("{path} isn't one sd view sets; use sd config set\n"));
    }
    let Some(c) = find(state, &id) else { return ("404 Not Found", "no such branch in the watcher's cache\n".into()) };
    let Some(repo) = repo_of(&id) else { return ("409 Conflict", format!("{}'s repo isn't registered with sd watch\n", c.branch)) };
    let out = sd_config(&repo, &["set", "--branch", &c.branch, "--", &path, &value]);
    if out.ok {
        ("200 OK", format!("set {path} {value} on {}\n", c.branch))
    } else if out.timed_out {
        ("500 Internal Server Error", format!("sd config set timed out after {SECS}s\n"))
    } else {
        ("400 Bad Request", if out.stderr.trim().is_empty() { out.stdout } else { out.stderr })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config;

    #[test]
    fn only_a_key_the_page_sets_may_be_set() {
        let typed = config::settings_keys();
        for k in EDITABLE {
            assert!(allowed(k), "{k}");
            assert!(typed.iter().any(|t| t == k), "{k} isn't a typed settings key");
        }
        for k in ["settings.interval", "settings.verify", "settings.flags.skips", "settings.botLogin",
                  "", "settings", "settings.", "settings.nope", "settings.autoRun.x", "harness", "steps.review.harness",
                  "profile", "profiles.x.settings.autoRun", "Settings.autoRun", "settings.autorun"] {
            assert!(!allowed(k), "{k}");
        }
    }
}
