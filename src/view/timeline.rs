// A branch's timeline (#376): its agent steps from usage.sup, and its PR's
// commits, CI and merge from GitHub, asked on demand.

use super::branch;
use crate::proc;
use crate::val::{at, t};
use serde_json::{json, Value};

/// gh runs inside the request: a stalled one mustn't leave the page waiting.
const GH_SECS: u64 = 20;

const QUERY: &str = "query($owner:String!,$repo:String!,$number:Int!){repository(owner:$owner,name:$repo){pullRequest(number:$number){
  createdAt mergedAt mergedBy{login} commits(last:100){nodes{commit{oid committedDate messageHeadline
  statusCheckRollup{contexts(first:50){nodes{__typename
    ...on StatusContext{context state createdAt} ...on CheckRun{name conclusion status startedAt completedAt}}}}}}}}}}";

fn agent(usage: &str, repo_dir: &str, b: &str) -> Vec<Value> {
    branch::records(usage, repo_dir, b)
        .iter()
        .map(|r| {
            let f = |k: &str| r.get(k).cloned().unwrap_or(Value::Null);
            json!({"step": f("step"), "end": f("time"), "ms": f("duration_ms"), "status": f("status"), "usd": f("cost_usd")})
        })
        .collect()
}

fn check(c: &Value) -> Value {
    if t(c, "__typename") == "StatusContext" {
        return json!({"name": c["context"], "result": c["state"], "start": null, "end": c["createdAt"]});
    }
    let result = if c["conclusion"].is_null() { &c["status"] } else { &c["conclusion"] };
    json!({"name": c["name"], "result": result, "start": c["startedAt"], "end": c["completedAt"]})
}

fn history(pr: &Value) -> Value {
    let nodes = |v: &Value| v.as_array().cloned().unwrap_or_default();
    let commits: Vec<Value> = nodes(at(pr, "commits.nodes"))
        .iter()
        .map(|n| {
            let c = &n["commit"];
            let ci: Vec<Value> = nodes(at(c, "statusCheckRollup.contexts.nodes")).iter().map(check).collect();
            json!({"sha": c["oid"], "at": c["committedDate"], "title": c["messageHeadline"], "ci": ci})
        })
        .collect();
    let merged = if pr["mergedAt"].is_null() { Value::Null } else { json!({"at": pr["mergedAt"], "by": at(pr, "mergedBy.login")}) };
    json!({"opened": pr["createdAt"], "merged": merged, "commits": commits})
}

fn pr_ref(url: &str) -> Option<(&str, &str, &str, &str)> {
    let mut p = url.strip_prefix("https://")?.split('/');
    match (p.next()?, p.next()?, p.next()?, p.next()?, p.next()?) {
        (host, owner, repo, "pull", n) => Some((host, owner, repo, n)),
        _ => None,
    }
}

/// (history, error): both null without a PR.
fn github(url: &str) -> (Value, Value) {
    let Some((host, owner, repo, n)) = pr_ref(url) else { return (Value::Null, Value::Null) };
    let (q, o, r, num) = (format!("query={QUERY}"), format!("owner={owner}"), format!("repo={repo}"), format!("number={n}"));
    let out = proc::with_timeout(GH_SECS, "gh", &["api", "graphql", "--hostname", host, "-f", &q, "-F", &o, "-F", &r, "-F", &num], None);
    let pr = serde_json::from_str::<Value>(&out.stdout).ok().map(|v| at(&v, "data.repository.pullRequest").clone());
    match pr {
        Some(pr) if out.ok && pr.is_object() => (history(&pr), Value::Null),
        _ => (Value::Null, json!(format!("gh: {}", out.stderr.trim()))),
    }
}

pub fn timeline(c: &super::flock::Cached, usage: &str) -> Value {
    let repo_dir = c.id.split_once('/').map_or("", |(d, _)| d);
    let url = c.doc.as_ref().map(|d| t(d, "state.pr.url")).unwrap_or_default();
    let (gh, error) = github(&url);
    json!({"agent": agent(usage, repo_dir, &c.branch), "github": gh, "error": error})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pr_url_names_its_host_repo_and_number() {
        assert_eq!(pr_ref("https://github.com/o/r/pull/7"), Some(("github.com", "o", "r", "7")));
        assert_eq!(pr_ref("https://ghe.example/o/r/issues/7"), None);
        assert_eq!(pr_ref(""), None);
    }

    #[test]
    fn a_check_run_still_running_has_its_status_for_a_result() {
        let c = json!({"__typename": "CheckRun", "name": "t", "conclusion": null, "status": "IN_PROGRESS", "startedAt": "s", "completedAt": null});
        assert_eq!(check(&c), json!({"name": "t", "result": "IN_PROGRESS", "start": "s", "end": null}));
    }
}
