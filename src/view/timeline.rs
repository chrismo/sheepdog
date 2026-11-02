// A branch's timeline (#376): its agent steps from usage.sup, and its PR's
// commits, CI and merge from GitHub, through view/cache.rs.

use super::{branch, cache};
use crate::gh;
use crate::val::{at, t};
use serde_json::{json, Value};
use std::path::Path;

/// A stalled gh mustn't hold the next fetch up for long.
const GH_SECS: u64 = 20;

/// Checks past 50 on a commit aren't drawn, and nothing says so.
const QUERY: &str = "query($owner:String!,$repo:String!,$number:Int!){repository(owner:$owner,name:$repo){pullRequest(number:$number){
  createdAt mergedAt mergedBy{login} commits(last:100){totalCount nodes{commit{oid committedDate messageHeadline
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
    // null unless the list was cut to the last 100.
    let total = at(pr, "commits.totalCount").as_u64().filter(|&n| n > commits.len() as u64);
    json!({"opened": pr["createdAt"], "merged": merged, "commits": commits, "total": total})
}

fn pr_ref(url: &str) -> Option<(&str, &str, &str, &str)> {
    let mut p = url.strip_prefix("https://")?.split('/');
    match (p.next()?, p.next()?, p.next()?, p.next()?, p.next()?) {
        (host, owner, repo, "pull", n) => Some((host, owner, repo, n)),
        _ => None,
    }
}

fn github(host: &str, owner: &str, repo: &str, n: &str) -> Result<Value, String> {
    let (q, o, r, num) = (format!("query={QUERY}"), format!("owner={owner}"), format!("repo={repo}"), format!("number={n}"));
    let out = gh::call(&["api", "graphql", "--hostname", host, "-f", &q, "-f", &o, "-f", &r, "-F", &num], GH_SECS);
    let pr = serde_json::from_str::<Value>(&out.stdout).ok().map(|v| at(&v, "data.repository.pullRequest").clone());
    match pr {
        Some(pr) if out.ok() && pr.is_object() => Ok(history(&pr)),
        _ => Err(cache::gh_error(&out.stderr, "PR")),
    }
}

pub fn timeline(c: &super::flock::Cached, usage: &str, state: &Path) -> Value {
    let repo_dir = c.id.split_once('/').map_or("", |(d, _)| d);
    let url = c.doc.as_ref().map(|d| t(d, "state.pr.url")).unwrap_or_default();
    let gh = match pr_ref(&url) {
        Some((host, owner, repo, n)) => {
            let key = format!("pr/{host}/{owner}/{repo}/{n}");
            let [host, owner, repo, n] = [host, owner, repo, n].map(String::from);
            cache::get(state, &key, move || github(&host, &owner, &repo, &n))
        }
        None => json!({"data": null, "error": null, "fetched": null, "refreshing": false}),
    };
    json!({"agent": agent(usage, repo_dir, &c.branch), "github": gh["data"], "error": gh["error"],
           "fetched": gh["fetched"], "refreshing": gh["refreshing"]})
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
    fn a_history_cut_to_the_last_100_commits_says_how_many_there_were() {
        let pr = |total: u64| json!({"commits": {"totalCount": total, "nodes": [{"commit": {"oid": "a"}}]}});
        assert_eq!(history(&pr(140))["total"], 140);
        assert_eq!(history(&pr(1))["total"], Value::Null);
    }

    #[test]
    fn a_check_run_still_running_has_its_status_for_a_result() {
        let c = json!({"__typename": "CheckRun", "name": "t", "conclusion": null, "status": "IN_PROGRESS", "startedAt": "s", "completedAt": null});
        assert_eq!(check(&c), json!({"name": "t", "result": "IN_PROGRESS", "start": "s", "end": null}));
    }
}
