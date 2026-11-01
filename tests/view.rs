// sd view (#376): a local server on 127.0.0.1 for the Flock page, every
// branch from the watcher's cache. Each test starts it on a free
// port (--port 0, --no-open) against a fixture $SHEEPDOG_STATE_DIR, reads the
// URL it prints, and asks over plain TCP.

mod common;

use common::{assert_has, spawn, Sd};
use serde_json::{json, Value};
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

/// A running sd view, killed when dropped.
struct Server {
    child: Child,
    port: u16,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Server {
    /// <method> <path> with Host <host>: the status code, headers and body.
    fn ask(&self, method: &str, path: &str, host: &str) -> (u16, String, String) {
        let mut s = TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        write!(s, "{method} {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n").unwrap();
        let mut resp = String::new();
        s.read_to_string(&mut resp).unwrap();
        let (head, body) = resp.split_once("\r\n\r\n").unwrap_or((&resp, ""));
        let code = head.split(' ').nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
        (code, head.to_string(), body.to_string())
    }

    fn get(&self, path: &str) -> (u16, String, String) {
        self.ask("GET", path, &format!("127.0.0.1:{}", self.port))
    }

    fn flock(&self) -> Value {
        let (code, head, body) = self.get("/api/flock");
        assert_eq!(code, 200, "{head}\n{body}");
        assert_has(&head.to_lowercase(), &["content-type: application/json"]);
        serde_json::from_str(&body).unwrap_or_else(|e| panic!("{e}: {body}"))
    }
}

fn sd() -> Sd {
    let mut sd = Sd::for_verb("view");
    let state = sd.path("state");
    sd.env.push(("SHEEPDOG_STATE_DIR".into(), state));
    sd
}

/// The watcher's cache file for <branch> of the repo at /src/<repo>: {"state": <state>}.
fn cache(sd: &Sd, repo: &str, branch: &str, state: Value) {
    let dir = sheepdog::watch::repo_key(&format!("/src/{repo}"));
    let f = sd.tmp.join(format!("state/branches/{dir}/{branch}.json"));
    fs::create_dir_all(f.parent().unwrap()).unwrap();
    fs::write(f, json!({ "glyph": "🤖", "state": state }).to_string()).unwrap();
}

fn state(branch: &str, next: Value) -> Value {
    json!({
        "branch": branch,
        "pr": { "number": 7, "url": "https://github.com/o/r/pull/7", "draft": false },
        "sync": { "ahead": 2, "behind": 0, "conflict": false },
        "push": { "ahead": 0, "dirty": false },
        "ci": { "verdict": "green" },
        "review": { "local": { "status": "covered" }, "bot": { "status": "none" }, "findings": null },
        "bot": { "command": null },
        "threads": { "owed_bot": 0, "owed_human": 0, "blocking": 0, "info": 0, "items": [] },
        "spend": { "usd": 1.5 },
        "next": next,
    })
}

fn serve(sd: &Sd) -> Server {
    let mut c = sd.command(&sd.bin);
    c.args(["--no-open", "--port", "0"]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    let mut child = spawn(&mut c);
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).unwrap();
    let port = regex::Regex::new(r"http://127\.0\.0\.1:(\d+)/")
        .unwrap()
        .captures(&line)
        .unwrap_or_else(|| panic!("no URL in {line:?}"))[1]
        .parse()
        .unwrap();
    Server { child, port }
}

#[test]
fn api_flock_serves_each_cached_branch() {
    let sd = sd();
    let ready = json!([{ "key": "ready", "who": "you", "say": "merge it", "cmd": "gh pr merge 7" }]);
    cache(&sd, "sheepdog", "a-branch", state("a-branch", ready));
    cache(&sd, "sheepdog", "feat/x", state("feat/x", json!([])));
    cache(&sd, "kinsite", "k", state("k", json!([])));
    fs::write(sd.tmp.join("state/last-pass"), "last pass: 2026-10-31 18:43:07\nsheepdog\ta-branch\t🤖\n").unwrap();
    let s = serve(&sd);
    let f = s.flock();
    assert_eq!(f["at"], "2026-10-31 18:43:07");
    let names: Vec<String> = f["branches"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| format!("{} {}", b["repo"].as_str().unwrap(), b["branch"].as_str().unwrap()))
        .collect();
    assert_eq!(names, ["kinsite k", "sheepdog a-branch", "sheepdog feat/x"]);
    let a = &f["branches"][1];
    assert_eq!(a["lead"]["who"], "you");
    assert_eq!(a["lead"]["cmd"], "gh pr merge 7");
    assert_eq!(a["pr"]["number"], 7);
    assert_eq!(a["usd"], 1.5);
}

#[test]
fn no_pass_yet_serves_no_branches() {
    let sd = sd();
    let f = serve(&sd).flock();
    assert_eq!(f, json!({ "at": null, "branches": [] }));
}

#[test]
fn an_unreadable_cache_file_is_listed_as_unreadable() {
    let sd = sd();
    let f = sd.tmp.join(format!("state/branches/{}/torn.json", sheepdog::watch::repo_key("/src/sheepdog")));
    fs::create_dir_all(f.parent().unwrap()).unwrap();
    fs::write(f, "{\"state\":").unwrap();
    let s = serve(&sd);
    let b = &s.flock()["branches"][0];
    assert_eq!(b["branch"], "torn");
    assert_eq!(b["lead"]["who"], "wait");
    assert_has(b["lead"]["say"].as_str().unwrap(), &["unreadable"]);
    let (code, d) = branch(&s, b["id"].as_str().unwrap());
    assert_eq!(code, 200);
    assert_eq!((&d["flow"], &d["owed"], &d["next"]), (&Value::Null, &Value::Null, &Value::Null));
}

#[test]
fn root_serves_the_page() {
    let sd = sd();
    let (code, head, body) = serve(&sd).get("/");
    assert_eq!(code, 200, "{head}");
    assert_has(&head.to_lowercase(), &["content-type: text/html"]);
    assert_has(&body, &["<title>", "/api/flock"]);
}

#[test]
fn an_unknown_path_is_not_found() {
    let sd = sd();
    assert_eq!(serve(&sd).get("/etc/passwd").0, 404);
}

#[test]
fn only_get_is_served() {
    let sd = sd();
    let s = serve(&sd);
    let host = format!("127.0.0.1:{}", s.port);
    assert_eq!(s.ask("POST", "/api/flock", &host).0, 405);
}

// A page on another site can reach 127.0.0.1 by DNS rebinding, under its own
// host name: only our own names are answered.
#[test]
fn another_host_name_is_refused() {
    let sd = sd();
    let s = serve(&sd);
    assert_eq!(s.ask("GET", "/api/flock", &format!("evil.example:{}", s.port)).0, 403);
    assert_eq!(s.ask("GET", "/api/flock", &format!("localhost:{}", s.port)).0, 200);
}

#[test]
fn it_keeps_serving_after_a_request() {
    let sd = sd();
    let s = serve(&sd);
    for _ in 0..3 {
        assert_eq!(s.get("/api/flock").0, 200);
    }
}

// A browser opens sockets it may never send on (preconnect): one mustn't
// hold up the request behind it.
#[test]
fn an_idle_connection_doesnt_hold_up_the_next() {
    let sd = sd();
    let s = serve(&sd);
    let _idle = TcpStream::connect(("127.0.0.1", s.port)).unwrap();
    let t = Instant::now();
    assert_eq!(s.get("/api/flock").0, 200);
    assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
}

#[test]
fn help_and_unknown_options() {
    let sd = sd();
    let r = sd.run(&["--help"]);
    assert_eq!(r.status, 0);
    assert_has(&r.output, &["Usage: sd view", "--port", "--no-open"]);
    let r = sd.run(&["--frob"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["sd view: unknown option: --frob"]);
    let r = sd.run(&["--port", "x"]);
    assert_ne!(r.status, 0);
    assert_has(&r.output, &["sd view: --port takes a number"]);
}

/// GET /api/branch for the card whose id is <id>, percent-encoded as the page sends it.
fn branch(s: &Server, id: &str) -> (u16, Value) {
    let q: String = id.bytes().map(|b| if b.is_ascii_alphanumeric() || b == b'-' { (b as char).to_string() } else { format!("%{b:02X}") }).collect();
    let (code, _, body) = s.get(&format!("/api/branch?id={q}"));
    (code, serde_json::from_str(&body).unwrap_or(Value::Null))
}

#[test]
fn drilling_into_a_branch_serves_its_flow_next_and_owed() {
    let sd = sd();
    let mut st = state("feat/x", json!([
        { "key": "review-fix", "who": "agent", "say": "fix 1 review finding (round 2/3)", "cmd": "sd act implement" },
        { "key": "ci-wait", "who": "wait", "say": "CI running", "cmd": null },
    ]));
    st["review"]["findings"] = json!({ "at_head": true, "n": 1, "round": 2, "items": [
        { "file": "src/a.rs", "line": 3, "severity": "error", "body": "off by one" } ] });
    st["ci"] = json!({ "verdict": "red", "for_head": true, "failing": ["cargo-test"] });
    st["question"] = json!({ "step": "implement", "kind": "ask", "text": "which way?", "at_head": true });
    st["threads"]["items"] = json!([{ "type": "thread", "kind": "bot", "mark": "!", "path": "src/a.rs", "line": 9,
        "author": "bot", "url": "https://github.com/o/r/pull/7#c1", "excerpt": "rename it" }]);
    cache(&sd, "sheepdog", "feat/x", st);
    let rec = |step: &str, b: &str| format!("{{time:2026-10-31T10:00:00Z,repo:\"/src/sheepdog\",branch:\"{b}\",step:\"{step}\",cost_usd:0.5,context:1}}");
    let usage = [rec("implement", "feat/x"), rec("review", "feat/x"), rec("implement", "feat/x"),
                 rec("ci-fix", "feat/x"), rec("review", "feat/x"), rec("implement", "other")].join("\n");
    fs::write(sd.tmp.join("state/usage.sup"), usage + "\n").unwrap();
    let s = serve(&sd);
    let id = s.flock()["branches"][0]["id"].as_str().unwrap().to_string();
    let (code, b) = branch(&s, &id);
    assert_eq!(code, 200, "{b}");
    assert_eq!(b["branch"], "feat/x");
    let counts: Vec<String> = ["implement", "review", "fix", "cifix"].iter().map(|k| b["flow"][k]["count"].to_string()).collect();
    assert_eq!(counts, ["1", "2", "1", "1"]);
    assert_eq!(b["flow"]["fix"]["who"], "agent");
    assert_eq!(b["flow"]["ci"]["who"], "wait");
    assert_eq!(b["next"].as_array().unwrap().len(), 2);
    assert_eq!(b["owed"]["findings"][0]["body"], "off by one");
    assert_eq!(b["owed"]["failing"], json!(["cargo-test"]));
    assert_eq!(b["owed"]["question"]["text"], "which way?");
    assert_eq!(b["owed"]["threads"][0]["excerpt"], "rename it");
}

#[test]
fn an_id_that_isnt_a_cached_branch_is_not_found() {
    let sd = sd();
    cache(&sd, "sheepdog", "a", state("a", json!([])));
    fs::write(sd.tmp.join("secret.json"), "{}").unwrap();
    let s = serve(&sd);
    let dir = sheepdog::watch::repo_key("/src/sheepdog");
    for id in [format!("{dir}/nope"), format!("{dir}/../../secret"), "../secret".into(), String::new()] {
        assert_eq!(branch(&s, &id).0, 404, "{id}");
    }
    assert_eq!(s.get("/api/branch").0, 404);
    assert_eq!(s.get(&format!("/api/branch?_=1&id={dir}%2Fa&x=2")).0, 200);
}

/// GitHub's answer for PR 7: two commits, CI on each, and the merge.
const PR_HISTORY: &str = r#"{"data":{"repository":{"pullRequest":{"createdAt":"2026-10-31T09:05:00Z",
  "mergedAt":"2026-10-31T12:00:00Z","mergedBy":{"login":"chrismo"},"commits":{"nodes":[
  {"commit":{"oid":"aaa1111","committedDate":"2026-10-31T09:00:00Z","messageHeadline":"first",
    "statusCheckRollup":{"contexts":{"nodes":[{"__typename":"StatusContext","context":"RWX","state":"FAILURE","createdAt":"2026-10-31T09:20:00Z"}]}}}},
  {"commit":{"oid":"bbb2222","committedDate":"2026-10-31T11:00:00Z","messageHeadline":"second",
    "statusCheckRollup":{"contexts":{"nodes":[{"__typename":"CheckRun","name":"lint","conclusion":"SUCCESS","status":"COMPLETED",
      "startedAt":"2026-10-31T11:01:00Z","completedAt":"2026-10-31T11:03:00Z"}]}}}}]}}}}}"#;

fn timeline(s: &Server, id: &str) -> (u16, Value) {
    let (code, _, body) = s.get(&format!("/api/timeline?id={}", id.replace('/', "%2F")));
    (code, serde_json::from_str(&body).unwrap_or(Value::Null))
}

#[test]
fn a_branch_s_timeline_has_its_agent_steps_and_its_pr_s_history() {
    let sd = sd();
    cache(&sd, "sheepdog", "feat/x", state("feat/x", json!([])));
    let usage = [
        r#"{time:2026-10-31T08:00:00Z,repo:"/src/sheepdog",branch:"feat/x",step:"implement",status:0,cost_usd:2.,duration_ms:600000}"#,
        r#"{time:2026-10-31T08:30:00Z,repo:"/src/sheepdog",branch:"feat/x",reset:true}"#,
        r#"{time:2026-10-31T09:00:00Z,repo:"/src/sheepdog",branch:"feat/x",step:"implement",status:0,cost_usd:1.5,duration_ms:900000}"#,
        r#"{time:2026-10-31T10:00:00Z,repo:"/src/sheepdog",branch:"feat/x",step:"review",status:75,cost_usd:0.,duration_ms:null}"#,
        r#"{time:2026-10-31T10:00:00Z,repo:"/src/sheepdog",branch:"other",step:"review",status:0,cost_usd:1.,duration_ms:1}"#,
    ];
    fs::write(sd.tmp.join("state/usage.sup"), usage.join("\n") + "\n").unwrap();
    fs::write(sd.tmp.join("history.json"), PR_HISTORY).unwrap();
    sd.fake("gh", &format!("#!/usr/bin/env bash\necho \"$*\" >> {}\ncat {}\n", sd.path("gh.log"), sd.path("history.json")));
    let s = serve(&sd);
    let id = s.flock()["branches"][0]["id"].as_str().unwrap().to_string();
    let (code, t) = timeline(&s, &id);
    assert_eq!(code, 200, "{t}");
    assert_eq!(t["agent"], json!([
        { "step": "implement", "end": "2026-10-31T09:00:00Z", "ms": 900000, "status": 0, "usd": 1.5 },
        { "step": "review", "end": "2026-10-31T10:00:00Z", "ms": null, "status": 75, "usd": 0.0 },
    ]));
    assert_eq!(t["github"]["opened"], "2026-10-31T09:05:00Z");
    assert_eq!(t["github"]["merged"], json!({ "at": "2026-10-31T12:00:00Z", "by": "chrismo" }));
    assert_eq!(t["github"]["commits"], json!([
        { "sha": "aaa1111", "at": "2026-10-31T09:00:00Z", "title": "first",
          "ci": [{ "name": "RWX", "result": "FAILURE", "start": null, "end": "2026-10-31T09:20:00Z" }] },
        { "sha": "bbb2222", "at": "2026-10-31T11:00:00Z", "title": "second",
          "ci": [{ "name": "lint", "result": "SUCCESS", "start": "2026-10-31T11:01:00Z", "end": "2026-10-31T11:03:00Z" }] },
    ]));
    assert_eq!(t["error"], Value::Null);
    let log = fs::read_to_string(sd.tmp.join("gh.log")).unwrap();
    // -F would send an all-digit repo name as a number.
    assert_has(&log, &["api graphql", "-f owner=o", "-f repo=r", "-F number=7"]);
}

#[test]
fn a_timeline_without_github_still_has_its_agent_steps() {
    let sd = sd();
    let mut st = state("a", json!([]));
    cache(&sd, "sheepdog", "b", st.clone());
    st["pr"] = Value::Null;
    cache(&sd, "sheepdog", "a", st);
    let rec = |b: &str| format!("{{time:2026-10-31T09:00:00Z,repo:\"/src/sheepdog\",branch:\"{b}\",step:\"implement\",status:0,cost_usd:1.,duration_ms:5}}");
    fs::write(sd.tmp.join("state/usage.sup"), [rec("a"), rec("b")].join("\n") + "\n").unwrap();
    sd.fake("gh", "#!/usr/bin/env bash\necho 'gh: rate limit exceeded' >&2\nexit 1\n");
    let s = serve(&sd);
    let id = |b: &str| format!("{}/{b}", sheepdog::watch::repo_key("/src/sheepdog"));
    let (_, no_pr) = timeline(&s, &id("a"));
    assert_eq!((no_pr["agent"].as_array().unwrap().len(), &no_pr["github"], &no_pr["error"]), (1, &Value::Null, &Value::Null));
    let (_, gh_down) = timeline(&s, &id("b"));
    assert_eq!((gh_down["agent"].as_array().unwrap().len(), &gh_down["github"]), (1, &Value::Null));
    assert_has(gh_down["error"].as_str().unwrap(), &["rate limit exceeded"]);
    assert_eq!(timeline(&s, "nope/x").0, 404);
}
