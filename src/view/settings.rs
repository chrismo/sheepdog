// A branch's settings on the drill-in (#396): GET /api/branch carries every
// settings.* key with its value and the layer it came from, from `sd config
// explain --json --branch <b>` in the branch's repo, and POST /api/config
// {id, path, value} runs `sd config set --branch <b> -- <path> <value>`
// there. Only the few a branch tunes day to day (EDITABLE) are set; the page
// shows the rest, each with the sd config set that changes it (#476). sd
// config does the typing: its refusal is the page's, as a 400.

use super::act::repo_of;
use super::atlas::quoted;
use super::branch::find;
use crate::proc;
use crate::val::{at, t};
use crate::watch::sd_verb;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Mutex;

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

/// Logs <failure> for <key> unless it's the one last logged for it, as the
/// drill-in reads a branch on every poll; None clears it.
fn log_new(key: &str, failure: Option<String>) {
    static LAST: Mutex<BTreeMap<String, String>> = Mutex::new(BTreeMap::new());
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    match failure {
        None => {
            last.remove(key);
        }
        Some(f) if last.get(key) != Some(&f) => {
            eprintln!("sd view: {f}");
            last.insert(key.to_string(), f);
        }
        Some(_) => {}
    }
}

/// [{path, value, source, edit}] for <branch>'s settings.* keys in <repo>,
/// or null when sd config couldn't explain them.
pub fn list(repo: &str, branch: &str) -> Value {
    let out = sd_config(repo, &["explain", "--json", "--branch", branch]);
    let failure = (!out.ok).then(|| format!("sd config explain --branch {branch} in {repo}: {}", out.stderr.lines().next().unwrap_or("failed")));
    let failed = failure.is_some();
    log_new(&format!("{repo}\n{branch}"), failure);
    if failed {
        return Value::Null;
    }
    let Ok(v) = serde_json::from_str::<Value>(&out.stdout) else { return Value::Null };
    let keys = at(&v, "keys").as_array().cloned().unwrap_or_default();
    let rows = keys.iter().filter(|k| t(k, "path").starts_with("settings.")).map(|k| {
        let path = t(k, "path");
        let editable = allowed(&path);
        let mut row = json!({"path": path, "value": at(k, "value"), "source": at(k, "source"), "editable": editable});
        if !editable {
            row["set"] = json!(set_command(branch, &path, &t(k, "source"), at(k, "value")));
        }
        row
    });
    Value::Array(rows.collect())
}

/// The sd config set that changes <path> in the layer <source> names: a
/// profile's in the repo file's definition of it, a default or unset one in
/// the nearest file that may hold the key.
fn set_command(branch: &str, path: &str, source: &str, value: &Value) -> String {
    let key = path.trim_start_matches("settings.");
    let file = match source {
        "global" | "repo" => source,
        _ => crate::config::nearest_file(key),
    };
    let (scope, path) = match source.strip_prefix("profile ") {
        Some(name) => ("--repo".to_string(), format!("profiles.{name}.{path}")),
        None if file == "branch" => (format!("--branch {}", quoted(branch)), path.to_string()),
        None => (format!("--{file}"), path.to_string()),
    };
    let json = if value.is_array() || value.is_object() || crate::config::takes_json(key) { " --json" } else { "" };
    let v = match value {
        Value::Null => "<value>".to_string(),
        Value::String(s) => quoted(s),
        v => quoted(&v.to_string()),
    };
    format!("sd config set {scope}{json} {path} {v}")
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

    #[test]
    fn a_read_only_setting_s_command_writes_the_layer_it_shows() {
        let cmd = |path, source, value| set_command("feat", path, source, &value);
        assert_eq!(cmd("settings.verify", "repo", json!("cargo test")), "sd config set --repo settings.verify 'cargo test'");
        assert_eq!(cmd("settings.botTimeout", "global", json!(60)), "sd config set --global settings.botTimeout 60");
        assert_eq!(cmd("settings.stepTimeout", "branch", json!(900)), "sd config set --branch feat settings.stepTimeout 900");
        assert_eq!(cmd("settings.botTimeout", "profile fast", json!(60)), "sd config set --repo profiles.fast.settings.botTimeout 60");
    }

    #[test]
    fn a_default_s_command_writes_the_branch_file_unless_the_key_lives_higher() {
        let cmd = |path, value| set_command("fix/it", path, "defaults", &value);
        assert_eq!(cmd("settings.botTimeout", json!(3600)), "sd config set --branch fix/it settings.botTimeout 3600");
        assert_eq!(cmd("settings.interval", json!(300)), "sd config set --global settings.interval 300");
        assert_eq!(cmd("settings.remote", json!("origin")), "sd config set --repo settings.remote origin");
        assert_eq!(set_command("feat", "settings.protectedPaths", "unset", &Value::Null), "sd config set --branch feat --json settings.protectedPaths <value>");
    }

    #[test]
    fn a_list_s_command_takes_it_as_json() {
        assert_eq!(set_command("feat", "settings.flags.skips", "defaults", &json!([r"\.skip\("])),
                   r#"sd config set --branch feat --json settings.flags.skips '["\\.skip\\("]'"#);
        assert_eq!(set_command("feat", "settings.botLogin", "global", &json!(["marge"])), r#"sd config set --global --json settings.botLogin '["marge"]'"#);
    }
}
