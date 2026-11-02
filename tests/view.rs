// sd view (#376): a local server on 127.0.0.1 for the Flock page, every
// branch from the watcher's cache. Each test starts it on a free
// port (--port 0, --no-open) against a fixture $SHEEPDOG_STATE_DIR, reads the
// URL it prints, and asks over plain TCP.

mod common;

use common::{assert_has, assert_match, spawn, Sd};
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
        self.send(method, path, &[("Host", host)])
    }

    /// <method> <path> with <headers>: the status code, headers and body.
    fn send(&self, method: &str, path: &str, headers: &[(&str, &str)]) -> (u16, String, String) {
        self.send_body(method, path, headers, if method == "POST" { "{}" } else { "" })
    }

    /// send, with <body> (framed by Content-Length) on a POST.
    fn send_body(&self, method: &str, path: &str, headers: &[(&str, &str)], body: &str) -> (u16, String, String) {
        let mut s = TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        let hs: String = headers.iter().map(|(k, v)| format!("{k}: {v}\r\n")).collect();
        let body = if method == "POST" { format!("Content-Length: {}\r\n\r\n{body}", body.len()) } else { "\r\n".into() };
        write!(s, "{method} {path} HTTP/1.1\r\n{hs}Connection: close\r\n{body}").unwrap();
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
        "viewer": "chrismo",
        "pr": { "number": 7, "url": "https://github.com/o/r/pull/7", "draft": false, "author": "chrismo" },
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
    serve_to(sd, Stdio::null())
}

/// serve, its stderr written to <log>.
fn serve_logging(sd: &Sd, log: &str) -> Server {
    serve_to(sd, fs::File::create(log).unwrap().into())
}

fn serve_to(sd: &Sd, stderr: Stdio) -> Server {
    let mut c = sd.command(&sd.bin);
    c.args(["--no-open", "--port", "0"]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(stderr);
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
    assert_eq!(f, json!({ "at": null, "branches": [], "refreshing": false, "held": null }));
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
    assert_has(&body, &["<title>", "data-act", "/api/act", "/api/flock"]);
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
    assert_has(&r.output, &["Usage: sd view", "--port", "--no-open", "button", "sd act", "watcher"]);
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

fn timeline_path(id: &str) -> String {
    format!("/api/timeline?id={}", id.replace('/', "%2F"))
}

/// The timeline once its GitHub fetch, if any, has landed.
fn timeline(s: &Server, id: &str) -> (u16, Value) {
    settled(s, &timeline_path(id))
}

/// GET <path> until its answer isn't "refreshing": the status code and JSON.
fn settled(s: &Server, path: &str) -> (u16, Value) {
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        let (code, _, body) = s.get(path);
        let v: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
        if v["refreshing"] != true {
            return (code, v);
        }
        assert!(Instant::now() < end, "{path} still refreshing after 10s: {body}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The gh calls the fakes logged (the timeline's query spans lines).
fn gh_calls(sd: &Sd) -> usize {
    fs::read_to_string(sd.tmp.join("gh.log")).map_or(0, |l| l.matches("api graphql").count() + l.matches("issue list").count())
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
    assert_eq!(gh_down["error"], "gh: rate limit exceeded");
    assert_eq!(timeline(&s, "nope/x").0, 404);
}

#[test]
fn a_gh_answer_with_no_pr_in_it_says_so() {
    let sd = sd();
    cache(&sd, "sheepdog", "a", state("a", json!([])));
    sd.fake("gh", "#!/usr/bin/env bash\necho '{\"data\":{\"repository\":{\"pullRequest\":null}}}'\n");
    let s = serve(&sd);
    let (_, t) = timeline(&s, &format!("{}/a", sheepdog::watch::repo_key("/src/sheepdog")));
    assert_eq!(t["error"], "gh: no PR in its answer");
}

/// The per-run token the page carries in its sd-token meta tag.
fn page_token(s: &Server) -> String {
    let (_, _, body) = s.get("/");
    let re = regex::Regex::new(r#"<meta name="sd-token" content="([0-9a-f]{32})">"#).unwrap();
    re.captures(&body).unwrap_or_else(|| panic!("no sd-token meta in the page"))[1].to_string()
}

/// A write to the guarded route, from <origin> with <token> (either left out when None).
fn write(s: &Server, method: &str, origin: Option<&str>, token: Option<&str>) -> (u16, String) {
    write_to(s, "/api/act", method, origin, token)
}

fn write_to(s: &Server, path: &str, method: &str, origin: Option<&str>, token: Option<&str>) -> (u16, String) {
    let host = format!("127.0.0.1:{}", s.port);
    let mut hs = vec![("Host", host.as_str())];
    hs.extend(origin.map(|o| ("Origin", o)));
    hs.extend(token.map(|t| ("X-Sd-Token", t)));
    let (code, _, body) = s.send(method, path, &hs);
    (code, body)
}

// Any page open in the browser can POST to 127.0.0.1 (#396): a write needs
// the token only our page carries, from our own origin, by POST.
#[test]
fn a_write_needs_the_page_s_token() {
    let sd = sd();
    let s = serve(&sd);
    let token = page_token(&s);
    let ours = format!("http://127.0.0.1:{}", s.port);
    assert_eq!(write(&s, "POST", Some(&ours), None).0, 403);
    assert_eq!(write(&s, "POST", Some(&ours), Some(&"0".repeat(32))).0, 403);
    assert_eq!(write(&s, "POST", Some(&ours), Some("")).0, 403);
    let (code, body) = write(&s, "POST", Some(&ours), Some(&token));
    assert_ne!(code, 403, "{body}");
}

#[test]
fn each_run_has_its_own_token() {
    let sd = sd();
    assert_ne!(page_token(&serve(&sd)), page_token(&serve(&sd)));
}

#[test]
fn a_write_from_another_origin_is_refused() {
    let sd = sd();
    let s = serve(&sd);
    let token = page_token(&s);
    for origin in [Some("http://evil.example".to_string()), Some(format!("http://127.0.0.1:{}", s.port + 1)),
                   Some(format!("https://127.0.0.1:{}", s.port)), Some("null".into()), None] {
        assert_eq!(write(&s, "POST", origin.as_deref(), Some(&token)).0, 403, "{origin:?}");
    }
    assert_ne!(write(&s, "POST", Some(&format!("http://localhost:{}", s.port)), Some(&token)).0, 403);
}

#[test]
fn a_get_never_writes() {
    let mut sd = sd();
    watched(&mut sd, "repos/r", "feat", 5);
    acting(&mut sd);
    let id = with_next(&sd, "feat", json!([{ "key": "review", "who": "agent", "say": "review it", "cmd": "sd act review" }]));
    let s = serve(&sd);
    let token = page_token(&s);
    let ours = format!("http://127.0.0.1:{}", s.port);
    let (code, body) = write(&s, "GET", Some(&ours), Some(&token));
    assert_eq!(code, 405, "{body}");
    let path = format!("/api/act?id={}&key=review", id.replace('/', "%2F"));
    let (code, body) = write_to(&s, &path, "GET", Some(&ours), Some(&token));
    assert_eq!(code, 405, "{body}");
    std::thread::sleep(Duration::from_millis(200));
    assert!(act_log(&sd).is_empty(), "{:?}", act_log(&sd));
}

// Past the guard, the action reads the body: {} names no branch or step.
#[test]
fn a_guarded_write_reaches_the_action() {
    let sd = sd();
    let s = serve(&sd);
    let token = page_token(&s);
    let (code, body) = write(&s, "POST", Some(&format!("http://127.0.0.1:{}", s.port)), Some(&token));
    assert_eq!(code, 400, "{body}");
    assert_has(&body, &["id", "key"]);
}

/// A fake sd act (SHEEPDOG_ACT_PROGRAM) that logs "start <cwd> <args>" and
/// "end <args>" to act.log, held while $ACT_GATE exists (10s at most).
fn acting(sd: &mut Sd) {
    let log = sd.path("act.log");
    sd.fake("sd-act", &format!(
        "#!/usr/bin/env bash\necho \"start $(pwd -P) $*\" >> {log}\n\
         for _ in $(seq 200); do [ -e \"${{ACT_GATE:-/nonexistent}}\" ] || break; sleep 0.05; done\necho \"end $*\" >> {log}\n"));
    sd.env.push(("SHEEPDOG_ACT_PROGRAM".into(), sd.path("fakebin/sd-act")));
}

fn act_log(sd: &Sd) -> Vec<String> {
    fs::read_to_string(sd.path("act.log")).unwrap_or_default().lines().map(str::to_string).collect()
}

/// <branch>'s cache file in repos/r rewritten, fresh, with <next>: its id.
fn with_next(sd: &Sd, branch: &str, next: Value) -> String {
    let dir = sheepdog::watch::repo_key(&sd.path("repos/r"));
    let f = sd.tmp.join(format!("state/branches/{dir}/{branch}.json"));
    fs::write(&f, json!({ "glyph": "🤖", "state": state(branch, next) }).to_string()).unwrap();
    format!("{dir}/{branch}")
}

/// POST /api/act {id, key} from our page, with the run's token: the status code and body.
fn act(s: &Server, id: &str, key: &str) -> (u16, String) {
    let host = format!("127.0.0.1:{}", s.port);
    let origin = format!("http://{host}");
    let token = page_token(s);
    let hs = [("Host", host.as_str()), ("Origin", origin.as_str()), ("X-Sd-Token", token.as_str()), ("Content-Type", "application/json")];
    let (code, _, body) = s.send_body("POST", "/api/act", &hs, &json!({ "id": id, "key": key }).to_string());
    (code, body)
}

/// GET /api/branch for <id> until no step is running on it; panics after 10s.
fn acted(s: &Server, id: &str) -> Value {
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        let (code, b) = branch(s, id);
        assert_eq!(code, 200, "{b}");
        if b["acting"].is_null() {
            return b;
        }
        assert!(Instant::now() < end, "still acting: {b}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

// The drill-in's button (#396): NEXT's first step, when it's the agent's or
// sheepdog's, runs as the watcher would run it, sd act <verb> <branch> in
// the branch's repo; the branch is checked again when it ends.
#[test]
fn the_drill_in_runs_next_s_step_as_the_watcher_would() {
    let mut sd = sd();
    watched(&mut sd, "repos/r", "feat", 5);
    acting(&mut sd);
    let gate = sd.path("gate");
    fs::write(&gate, "").unwrap();
    sd.env.push(("ACT_GATE".into(), gate.clone()));
    let id = with_next(&sd, "feat", json!([
        { "key": "review", "who": "agent", "say": "review it", "cmd": "sd act review" },
        { "key": "ci-wait", "who": "wait", "say": "CI running", "cmd": null },
    ]));
    let s = serve(&sd);
    let (_, b) = branch(&s, &id);
    assert_eq!(b["act"], json!({ "key": "review", "verb": "review" }));
    assert_eq!(b["acting"], Value::Null);
    let (code, body) = act(&s, &id, "review");
    assert_eq!(code, 200, "{body}");
    assert_has(&body, &["sd act review feat"]);
    assert_eq!(branch(&s, &id).1["acting"], "review");
    assert_eq!(s.flock()["branches"][0]["acting"], "review");
    fs::remove_file(&gate).unwrap();
    let b = acted(&s, &id);
    assert_eq!(b["acting"], Value::Null);
    let repo = fs::canonicalize(sd.path("repos/r")).unwrap().display().to_string();
    assert_eq!(act_log(&sd), [format!("start {repo} review feat"), "end review feat".into()]);
    assert_eq!(watch_log(&sd), [format!("start {repo} refresh feat"), "end feat".into()]);
}

#[test]
fn a_conflict_s_step_runs_as_restack_resolve() {
    let mut sd = sd();
    watched(&mut sd, "repos/r", "feat", 5);
    acting(&mut sd);
    let id = with_next(&sd, "feat", json!([{ "key": "conflict", "who": "agent", "say": "resolve", "cmd": "sd act restack --resolve" }]));
    let s = serve(&sd);
    assert_eq!(branch(&s, &id).1["act"], json!({ "key": "conflict", "verb": "restack --resolve" }));
    assert_eq!(act(&s, &id, "conflict").0, 200);
    acted(&s, &id);
    let repo = fs::canonicalize(sd.path("repos/r")).unwrap().display().to_string();
    assert_eq!(act_log(&sd)[0], format!("start {repo} restack --resolve feat"));
}

// The step's limit is the watcher's: SHEEPDOG_WATCH_TIMEOUT, as a pass's.
#[test]
fn a_step_runs_under_the_watcher_s_timeout() {
    let mut sd = sd();
    watched(&mut sd, "repos/r", "feat", 5);
    acting(&mut sd);
    let gate = sd.path("gate");
    fs::write(&gate, "").unwrap();
    sd.env.push(("ACT_GATE".into(), gate));
    sd.env.push(("SHEEPDOG_WATCH_TIMEOUT".into(), "1".into()));
    let id = with_next(&sd, "feat", json!([{ "key": "push", "who": "run", "say": "push", "cmd": "sd act push" }]));
    let s = serve(&sd);
    assert_eq!(act(&s, &id, "push").0, 200);
    acted(&s, &id);
    let repo = fs::canonicalize(sd.path("repos/r")).unwrap().display().to_string();
    assert_eq!(act_log(&sd), [format!("start {repo} push feat")]);
}

// Ctrl-C to sd view mid-step ends the step as Ctrl-C on a hand-run sd act
// would (proc::timeout passes it on), and sd view's log says so.
#[test]
fn ctrl_c_to_sd_view_stops_its_step_and_says_so() {
    let mut sd = sd();
    watched(&mut sd, "repos/r", "feat", 5);
    acting(&mut sd);
    let gate = sd.path("gate");
    fs::write(&gate, "").unwrap();
    sd.env.push(("ACT_GATE".into(), gate));
    let id = with_next(&sd, "feat", json!([{ "key": "push", "who": "run", "say": "push", "cmd": "sd act push" }]));
    let log = sd.path("view.log");
    let mut s = serve_logging(&sd, &log);
    assert_eq!(act(&s, &id, "push").0, 200);
    let end = Instant::now() + Duration::from_secs(5);
    while act_log(&sd).is_empty() {
        assert!(Instant::now() < end, "the step never started");
        std::thread::sleep(Duration::from_millis(20));
    }
    unsafe { libc::kill(s.child.id() as i32, libc::SIGINT) };
    let end = Instant::now() + Duration::from_secs(5);
    while s.child.try_wait().unwrap().is_none() {
        assert!(Instant::now() < end, "sd view still running");
        std::thread::sleep(Duration::from_millis(20));
    }
    let repo = fs::canonicalize(sd.path("repos/r")).unwrap().display().to_string();
    assert_eq!(act_log(&sd), [format!("start {repo} push feat")]);
    assert_has(&fs::read_to_string(&log).unwrap(), &["sd act push feat", "stopped by a signal"]);
}

#[test]
fn a_step_that_is_no_longer_next_s_first_is_refused() {
    let mut sd = sd();
    watched(&mut sd, "repos/r", "feat", 5);
    acting(&mut sd);
    let id = with_next(&sd, "feat", json!([{ "key": "push", "who": "run", "say": "push", "cmd": "sd act push" }]));
    let s = serve(&sd);
    let (code, body) = act(&s, &id, "review");
    assert_eq!(code, 409, "{body}");
    assert_has(&body, &["push", "review"]);
    assert_eq!(act(&s, "nope/x", "push").0, 404);
    std::thread::sleep(Duration::from_millis(200));
    assert!(act_log(&sd).is_empty(), "{:?}", act_log(&sd));
}

#[test]
fn chrismo_s_and_waiting_steps_have_no_button_and_are_refused() {
    let mut sd = sd();
    watched(&mut sd, "repos/r", "feat", 5);
    acting(&mut sd);
    let s = serve(&sd);
    for (key, who) in [("ready", "you"), ("bot", "you"), ("ci-wait", "wait"), ("open-pr", "run")] {
        let id = with_next(&sd, "feat", json!([{ "key": key, "who": who, "say": "s", "cmd": null }]));
        assert_eq!(branch(&s, &id).1["act"], Value::Null, "{key}");
        let (code, body) = act(&s, &id, key);
        assert_eq!(code, 409, "{key}: {body}");
        assert_has(&body, &[key]);
    }
    std::thread::sleep(Duration::from_millis(200));
    assert!(act_log(&sd).is_empty(), "{:?}", act_log(&sd));
}

// The watcher's dispatch gate (pass.rs may_dispatch) holds for the button
// too: no step on a PR someone else wrote, or whose remote has their commits.
#[test]
fn a_pr_autorun_wouldn_t_touch_has_no_button_and_is_refused() {
    let mut sd = sd();
    watched(&mut sd, "repos/r", "feat", 5);
    acting(&mut sd);
    let dir = sheepdog::watch::repo_key(&sd.path("repos/r"));
    let f = sd.tmp.join(format!("state/branches/{dir}/feat.json"));
    let s = serve(&sd);
    let theirs = |st: &mut Value| st["pr"]["author"] = json!("someone");
    let pushed = |st: &mut Value| st["push"] = json!({ "ahead": 0, "dirty": false, "remote_ours": false, "theirs": 1 });
    for edit in [&theirs as &dyn Fn(&mut Value), &pushed] {
        let mut st = state("feat", json!([{ "key": "push", "who": "run", "say": "push", "cmd": "sd act push" }]));
        edit(&mut st);
        fs::write(&f, json!({ "glyph": "🤖", "state": st }).to_string()).unwrap();
        let id = format!("{dir}/feat");
        assert_eq!(branch(&s, &id).1["act"], Value::Null, "{st}");
        let (code, body) = act(&s, &id, "push");
        assert_eq!(code, 409, "{body}");
        assert_has(&body, &["push", "autoRun"]);
    }
    std::thread::sleep(Duration::from_millis(200));
    assert!(act_log(&sd).is_empty(), "{:?}", act_log(&sd));
}

#[test]
fn one_step_runs_at_a_time() {
    let mut sd = sd();
    watched(&mut sd, "repos/r", "feat", 5);
    watched(&mut sd, "repos/r", "other", 5);
    acting(&mut sd);
    let gate = sd.path("gate");
    fs::write(&gate, "").unwrap();
    sd.env.push(("ACT_GATE".into(), gate.clone()));
    let feat = with_next(&sd, "feat", json!([{ "key": "review", "who": "agent", "say": "s", "cmd": null }]));
    let other = with_next(&sd, "other", json!([{ "key": "push", "who": "run", "say": "s", "cmd": null }]));
    let s = serve(&sd);
    assert_eq!(act(&s, &feat, "review").0, 200);
    let (code, body) = act(&s, &other, "push");
    assert_eq!(code, 409, "{body}");
    assert_has(&body, &["feat"]);
    assert_eq!(act(&s, &feat, "review").0, 409);
    assert_eq!(branch(&s, &other).1["acting"], Value::Null);
    fs::remove_file(&gate).unwrap();
    acted(&s, &feat);
    assert_eq!(act_log(&sd).len(), 2, "{:?}", act_log(&sd));
    assert_eq!(act(&s, &other, "push").0, 200);
    acted(&s, &other);
    assert_eq!(act_log(&sd).len(), 4, "{:?}", act_log(&sd));
}

// The Map (#378) reads the checkout the binary was built in: here, this one.
fn this_repo(path: &str) -> String {
    fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(path)).unwrap()
}

/// The Map once its GitHub fetch has landed.
fn map(s: &Server) -> Value {
    let (_, head, _) = s.get("/api/map");
    assert_has(&head.to_lowercase(), &["200 ok", "content-type: application/json"]);
    let (code, m) = settled(s, "/api/map");
    assert_eq!(code, 200, "{m}");
    m
}

fn map_file<'a>(m: &'a Value, path: &str) -> &'a Value {
    m["groups"].as_array().unwrap().iter().flat_map(|g| g["files"].as_array().unwrap()).find(|f| f["path"] == path)
        .unwrap_or_else(|| panic!("no {path} in {m}"))
}

#[test]
fn api_map_serves_sheepdog_s_source_sized_with_its_ratchet_and_issues() {
    let sd = sd();
    let issues = r#"[{"number":378,"title":"sd view map","url":"https://github.com/o/r/issues/378","body":"see src/view.rs"}]"#;
    sd.fake("gh", &format!("#!/usr/bin/env bash\necho \"$*\" >> {}\necho '{issues}'\n", sd.path("gh.log")));
    let m = map(&serve(&sd));
    let view = map_file(&m, "src/view.rs");
    assert_eq!(view["lines"], this_repo("src/view.rs").lines().count());
    assert_eq!(view["issues"], json!([{ "number": 378, "title": "sd view map", "url": "https://github.com/o/r/issues/378" }]));
    assert_eq!(map_file(&m, "src/view/map.rs")["issues"], json!([]));
    let ratchet = this_repo("tests/lint-ratchet.txt");
    let listed: Vec<Vec<&str>> = ratchet.lines().map(|l| l.split('#').next().unwrap().split_whitespace().collect()).filter(|w: &Vec<&str>| w.len() == 4 && w[0].starts_with("src/")).collect();
    assert!(!listed.is_empty());
    let fns: std::collections::BTreeSet<_> = listed.iter().map(|w| (w[0], w[1])).collect();
    for w in &listed {
        let over = &map_file(&m, w[0])["over"];
        assert!(over.as_array().unwrap().contains(&json!({ "fn": w[1], "lint": w[2], "score": w[3].parse::<u64>().unwrap() })), "{w:?} in {over}");
    }
    assert_eq!((&m["ratchet"], &m["issues"], &m["error"], &m["cut"]), (&json!(fns.len()), &json!(1), &Value::Null, &Value::Null));
    let log = fs::read_to_string(sd.tmp.join("gh.log")).unwrap();
    assert_has(&log, &["issue list", "--state open"]);
}

#[test]
fn the_map_without_github_still_has_its_files() {
    let sd = sd();
    sd.fake("gh", "#!/usr/bin/env bash\necho 'gh: rate limit exceeded' >&2\nexit 1\n");
    let m = map(&serve(&sd));
    assert_eq!(map_file(&m, "src/view.rs")["issues"], json!([]));
    assert_eq!(m["issues"], Value::Null);
    assert_eq!(m["error"], "gh: rate limit exceeded");
}

#[test]
fn the_map_page_and_the_flock_link_to_each_other() {
    let sd = sd();
    let s = serve(&sd);
    let (code, head, body) = s.get("/map");
    assert_eq!(code, 200, "{head}");
    assert_has(&head.to_lowercase(), &["content-type: text/html"]);
    assert_has(&body, &["<title>", "href=\"/\"", "/api/map"]);
    assert_has(&s.get("/").2, &["href=\"/map\""]);
}

// The Atlas (#377): every NEXT step by stage and each verb's gate rules,
// generated from the code; no state is read.

#[test]
fn the_atlas_page_serves_the_stages_and_each_verbs_rules() {
    let sd = sd();
    let s = serve(&sd);
    let (code, head, body) = s.get("/atlas");
    assert_eq!(code, 200, "{head}");
    assert_has(&head.to_lowercase(), &["content-type: text/html"]);
    assert_has(&body, &[&format!(r#"<meta name="sd-token" content="{}">"#, page_token(&s)), "<title>", "/api/atlas"]);
    let (code, head, body) = s.get("/api/atlas");
    assert_eq!(code, 200, "{head}");
    assert_has(&head.to_lowercase(), &["content-type: application/json"]);
    let a: Value = serde_json::from_str(&body).unwrap_or_else(|e| panic!("{e}: {body}"));
    let stages: Vec<&str> = a["stages"].as_array().unwrap().iter().map(|x| x["stage"].as_str().unwrap()).collect();
    assert_eq!(stages, ["build", "push", "ci", "review", "bot", "threads", "merge"]);
    assert!(a["holds"].as_array().unwrap().iter().any(|x| x["key"] == "running"), "{}", a["holds"]);
    let push = a["verbs"].as_array().unwrap().iter().find(|v| v["name"] == "push").unwrap();
    let closed = push["rules"].as_array().unwrap().iter().find(|r| r["name"] == "the PR is merged or closed").unwrap();
    assert_eq!((&closed["level"], &closed["means"]), (&json!("Soft"), &json!("force passes")));
    assert_eq!(push["checks"], json!(["running"]));
}

#[test]
fn the_three_pages_link_to_each_other() {
    let sd = sd();
    let s = serve(&sd);
    assert_has(&s.get("/").2, &["href=\"/atlas\"", "href=\"/map\""]);
    assert_has(&s.get("/map").2, &["href=\"/\"", "href=\"/atlas\""]);
    assert_has(&s.get("/atlas").2, &["href=\"/\"", "href=\"/map\""]);
}

// GitHub data is cached under <state>/github/ (#401): served at once, and
// fetched again in the background once it's a minute old or the page's
// refresh button asks.

#[test]
fn fresh_github_data_isn_t_asked_for_again() {
    let sd = sd();
    cache(&sd, "sheepdog", "a", state("a", json!([])));
    fs::write(sd.tmp.join("history.json"), PR_HISTORY).unwrap();
    sd.fake("gh", &format!("#!/usr/bin/env bash\necho \"$*\" >> {}\ncat {}\n", sd.path("gh.log"), sd.path("history.json")));
    let s = serve(&sd);
    let id = format!("{}/a", sheepdog::watch::repo_key("/src/sheepdog"));
    let (_, _, first) = s.get(&timeline_path(&id));
    let first: Value = serde_json::from_str(&first).unwrap();
    assert_eq!((&first["github"], &first["refreshing"], first["agent"].is_array()), (&Value::Null, &json!(true), true));
    let (_, t) = timeline(&s, &id);
    assert_eq!(t["github"]["opened"], "2026-10-31T09:05:00Z");
    assert!(regex::Regex::new(r"^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ$").unwrap().is_match(t["fetched"].as_str().unwrap_or("")), "{t}");
    let (_, again) = timeline(&s, &id);
    assert_eq!((&again["github"], &again["refreshing"]), (&t["github"], &json!(false)));
    assert_eq!(gh_calls(&sd), 1);
}

/// The Map's cached issue list, as asked <secs_ago> seconds ago.
fn cached_issues(sd: &Sd, secs_ago: u64, issues: Value) {
    let root = env!("CARGO_MANIFEST_DIR");
    let f = sd.tmp.join(format!("state/github/issues-{}.json", sheepdog::watch::repo_key(root)));
    fs::create_dir_all(f.parent().unwrap()).unwrap();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
    let then = now - secs_ago * 1000;
    fs::write(f, json!({ "tried": then, "at": then, "data": issues, "error": null }).to_string()).unwrap();
}

#[test]
fn stale_github_data_is_served_at_once_while_a_fresh_copy_is_fetched() {
    let sd = sd();
    cached_issues(&sd, 61, json!([{ "number": 1, "title": "old", "url": "u1", "body": "src/view.rs" }]));
    let issues = r#"[{"number":2,"title":"new","url":"u2","body":"src/view.rs"}]"#;
    sd.fake("gh", &format!("#!/usr/bin/env bash\necho \"$*\" >> {}\necho '{issues}'\n", sd.path("gh.log")));
    let s = serve(&sd);
    let (_, _, body) = s.get("/api/map");
    let stale: Value = serde_json::from_str(&body).unwrap();
    assert_eq!((&map_file(&stale, "src/view.rs")["issues"][0]["number"], &stale["refreshing"]), (&json!(1), &json!(true)));
    let m = map(&s);
    assert_eq!(map_file(&m, "src/view.rs")["issues"][0]["number"], 2);
    assert_eq!(gh_calls(&sd), 1);
}

#[test]
fn a_failed_fetch_keeps_the_last_data_and_says_why() {
    let sd = sd();
    cached_issues(&sd, 61, json!([{ "number": 1, "title": "old", "url": "u1", "body": "src/view.rs" }]));
    sd.fake("gh", "#!/usr/bin/env bash\necho 'rate limit exceeded' >&2\nexit 1\n");
    let m = map(&serve(&sd));
    assert_eq!((&map_file(&m, "src/view.rs")["issues"][0]["number"], &m["error"]), (&json!(1), &json!("gh: rate limit exceeded")));
}

#[test]
fn a_timeline_s_failed_fetch_keeps_its_last_pr_history() {
    let sd = sd();
    cache(&sd, "sheepdog", "a", state("a", json!([])));
    let f = sd.tmp.join("state/github/pr%2Fgithub.com%2Fo%2Fr%2F7.json");
    fs::create_dir_all(f.parent().unwrap()).unwrap();
    let old = json!({ "opened": "2026-10-31T09:05:00Z", "merged": null, "commits": [], "total": null });
    fs::write(&f, json!({ "tried": 1, "at": 1, "data": old, "error": null }).to_string()).unwrap();
    sd.fake("gh", "#!/usr/bin/env bash\necho 'rate limit exceeded' >&2\nexit 1\n");
    let s = serve(&sd);
    let (_, t) = timeline(&s, &format!("{}/a", sheepdog::watch::repo_key("/src/sheepdog")));
    assert_eq!((&t["github"], &t["error"], &t["fetched"]), (&old, &json!("gh: rate limit exceeded"), &json!("1970-01-01T00:00:00Z")));
}

#[test]
fn a_failed_fetch_of_a_full_list_still_says_it_may_be_cut_off() {
    let sd = sd();
    let full: Vec<Value> = (0..500).map(|i| json!({ "number": i, "title": "t", "url": "u", "body": "" })).collect();
    cached_issues(&sd, 61, json!(full));
    sd.fake("gh", "#!/usr/bin/env bash\necho 'rate limit exceeded' >&2\nexit 1\n");
    let m = map(&serve(&sd));
    assert_eq!((&m["error"], &m["cut"]), (&json!("gh: rate limit exceeded"), &json!("gh lists 500 at most, and gave 500")));
}

#[test]
fn the_refresh_button_makes_cached_github_data_stale() {
    let sd = sd();
    cached_issues(&sd, 0, json!([]));
    sd.fake("gh", &format!("#!/usr/bin/env bash\necho \"$*\" >> {}\necho '[]'\n", sd.path("gh.log")));
    let s = serve(&sd);
    map(&s);
    assert_eq!(gh_calls(&sd), 0);
    let ours = format!("http://127.0.0.1:{}", s.port);
    assert_eq!(write_to(&s, "/api/refresh", "POST", Some(&ours), None).0, 403);
    assert_eq!(write_to(&s, "/api/refresh", "GET", Some(&ours), Some(&page_token(&s))).0, 405);
    let (code, body) = write_to(&s, "/api/refresh", "POST", Some(&ours), Some(&page_token(&s)));
    assert_eq!(code, 200, "{body}");
    map(&s);
    assert_eq!(gh_calls(&sd), 1);
}

#[test]
fn the_map_page_carries_the_run_s_token() {
    let sd = sd();
    let s = serve(&sd);
    let token = page_token(&s);
    assert_has(&s.get("/map").2, &[&format!(r#"<meta name="sd-token" content="{token}">"#), r#"<script src="/gh.js">"#]);
    assert_has(&s.get("/").2, &[r#"<script src="/gh.js">"#]);
    let (code, head, js) = s.get("/gh.js");
    assert_eq!(code, 200, "{head}");
    assert_has(&head.to_lowercase(), &["content-type: text/javascript"]);
    assert_has(&js, &["sd-token", "/api/refresh", "X-Sd-Token"]);
}

/// A registered repo at <dir> (under the temp dir), its watcher cache file for
/// <branch> written <secs_ago> ago, and a fake sd watch, which logs
/// "start <cwd> <args>" and "end <branch>" to watch.log and rewrites the file:
/// after $WATCH_SLEEP, and once $WATCH_GATE is gone (10s at most).
fn watched(sd: &mut Sd, dir: &str, branch: &str, secs_ago: u64) {
    let repo = sd.path(dir);
    fs::create_dir_all(&repo).unwrap();
    if !sd.git_try(&["config", "--global", "--get-all", "sheepdog.repo"]).unwrap_or_default().lines().any(|r| r == repo) {
        sd.git(&["config", "--global", "--add", "sheepdog.repo", &repo]);
    }
    let f = sd.tmp.join(format!("state/branches/{}/{branch}.json", sheepdog::watch::repo_key(&repo)));
    fs::create_dir_all(f.parent().unwrap()).unwrap();
    fs::write(&f, json!({ "glyph": "🤖", "state": state(branch, json!([])) }).to_string()).unwrap();
    let then = std::time::SystemTime::now() - Duration::from_secs(secs_ago);
    fs::File::options().write(true).open(&f).unwrap().set_modified(then).unwrap();
    let log = sd.path("watch.log");
    sd.fake("sd-watch", &format!(
        "#!/usr/bin/env bash\necho \"start $(pwd -P) $*\" >> {log}\nsleep \"${{WATCH_SLEEP:-0}}\"\n\
         for _ in $(seq 200); do [ -e \"${{WATCH_GATE:-/nonexistent}}\" ] || break; sleep 0.05; done\n\
         find \"$SHEEPDOG_STATE_DIR/branches\" -name \"$2.json\" -exec touch {{}} +\necho \"end $2\" >> {log}\n"));
    if !sd.env.iter().any(|(k, _)| k == "SHEEPDOG_WATCH_PROGRAM") {
        sd.env.push(("SHEEPDOG_WATCH_PROGRAM".into(), sd.path("fakebin/sd-watch")));
    }
}

fn watch_log(sd: &Sd) -> Vec<String> {
    fs::read_to_string(sd.path("watch.log")).unwrap_or_default().lines().map(str::to_string).collect()
}

/// /api/flock once nothing is refreshing; panics after 10s.
fn flock_settled(s: &Server) -> Value {
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        let f = s.flock();
        if f["refreshing"] == false {
            return f;
        }
        assert!(Instant::now() < end, "still refreshing: {f}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn a_flock_branch_a_minute_old_is_refreshed_in_its_repo() {
    let mut sd = sd();
    watched(&mut sd, "repos/r", "old", 120);
    let s = serve(&sd);
    assert_eq!(s.flock()["refreshing"], true);
    flock_settled(&s);
    let repo = fs::canonicalize(sd.path("repos/r")).unwrap();
    assert_eq!(watch_log(&sd), [format!("start {} refresh old", repo.display()), "end old".into()]);
}

#[test]
fn a_fresh_flock_branch_isn_t_refreshed() {
    let mut sd = sd();
    watched(&mut sd, "repos/r", "fresh", 5);
    let s = serve(&sd);
    assert_eq!(s.flock()["refreshing"], false);
    std::thread::sleep(Duration::from_millis(200));
    assert!(watch_log(&sd).is_empty(), "{:?}", watch_log(&sd));
}

#[test]
fn flock_branches_are_refreshed_one_at_a_time_stalest_first() {
    let mut sd = sd();
    watched(&mut sd, "repos/r", "older", 300);
    watched(&mut sd, "repos/r", "old", 120);
    sd.env.push(("WATCH_SLEEP".into(), "0.3".into()));
    let s = serve(&sd);
    let end = Instant::now() + Duration::from_secs(10);
    while watch_log(&sd).len() < 4 && Instant::now() < end {
        s.flock();
        std::thread::sleep(Duration::from_millis(50));
    }
    let log: Vec<String> = watch_log(&sd).iter().map(|l| l.split(' ').next_back().unwrap().to_string()).collect();
    assert_eq!(log, ["older", "older", "old", "old"], "{:?}", watch_log(&sd));
}

#[test]
fn a_branch_in_an_unregistered_repo_isn_t_refreshed() {
    let mut sd = sd();
    watched(&mut sd, "repos/r", "fresh", 5);
    cache(&sd, "elsewhere", "old", state("old", json!([])));
    let f = sd.tmp.join(format!("state/branches/{}/old.json", sheepdog::watch::repo_key("/src/elsewhere")));
    fs::File::options().write(true).open(f).unwrap().set_modified(std::time::SystemTime::now() - Duration::from_secs(120)).unwrap();
    let s = serve(&sd);
    assert_eq!(s.flock()["refreshing"], false);
    assert!(watch_log(&sd).is_empty());
}

#[test]
fn a_merged_branch_isn_t_refreshed() {
    let mut sd = sd();
    watched(&mut sd, "repos/r", "done", 120);
    let f = sd.tmp.join(format!("state/branches/{}/done.json", sheepdog::watch::repo_key(&sd.path("repos/r"))));
    let mut st = state("done", json!([]));
    st["pr"]["state"] = json!("MERGED");
    fs::write(&f, json!({ "glyph": "🏁", "state": st }).to_string()).unwrap();
    fs::File::options().write(true).open(&f).unwrap().set_modified(std::time::SystemTime::now() - Duration::from_secs(120)).unwrap();
    let s = serve(&sd);
    assert_eq!(s.flock()["refreshing"], false);
    assert!(watch_log(&sd).is_empty());
}

#[test]
fn the_refresh_button_refreshes_fresh_flock_branches_too() {
    let mut sd = sd();
    watched(&mut sd, "repos/r", "fresh", 5);
    let s = serve(&sd);
    assert_eq!(s.flock()["refreshing"], false);
    let ours = format!("http://127.0.0.1:{}", s.port);
    assert_eq!(write_to(&s, "/api/refresh", "POST", Some(&ours), Some(&page_token(&s))).0, 200);
    assert_eq!(s.flock()["refreshing"], true);
    flock_settled(&s);
    assert_eq!(watch_log(&sd).len(), 2, "{:?}", watch_log(&sd));
}

#[test]
fn each_flock_card_says_when_its_branch_was_checked() {
    let mut sd = sd();
    watched(&mut sd, "repos/r", "fresh", 5);
    let f = serve(&sd).flock();
    let checked = f["branches"][0]["checked"].as_str().unwrap_or_else(|| panic!("{f}"));
    assert_match(checked, r"^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ$");
}

#[test]
fn a_cache_file_written_in_the_future_is_refreshed_once() {
    let mut sd = sd();
    watched(&mut sd, "repos/r", "ahead", 0);
    let f = sd.tmp.join(format!("state/branches/{}/ahead.json", sheepdog::watch::repo_key(&sd.path("repos/r"))));
    fs::File::options().write(true).open(f).unwrap().set_modified(std::time::SystemTime::now() + Duration::from_secs(3600)).unwrap();
    sd.fake("sd-watch", &format!("#!/usr/bin/env bash\necho \"start $*\" >> {}\nexit 1\n", sd.path("watch.log")));
    let s = serve(&sd);
    s.flock();
    flock_settled(&s);
    s.flock();
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(watch_log(&sd), ["start refresh ahead"]);
}

#[test]
fn a_branch_whose_refresh_failed_waits_for_the_watcher_to_rewrite_it() {
    let mut sd = sd();
    watched(&mut sd, "repos/r", "gone", 120);
    sd.fake("sd-watch", &format!("#!/usr/bin/env bash\necho \"start $*\" >> {}\nexit 1\n", sd.path("watch.log")));
    let s = serve(&sd);
    s.flock();
    flock_settled(&s);
    let ours = format!("http://127.0.0.1:{}", s.port);
    assert_eq!(write_to(&s, "/api/refresh", "POST", Some(&ours), Some(&page_token(&s))).0, 200);
    assert_eq!(s.flock()["refreshing"], false);
    let f = sd.tmp.join(format!("state/branches/{}/gone.json", sheepdog::watch::repo_key(&sd.path("repos/r"))));
    fs::File::options().write(true).open(f).unwrap().set_modified(std::time::SystemTime::now() - Duration::from_secs(90)).unwrap();
    assert_eq!(s.flock()["refreshing"], true);
    flock_settled(&s);
    assert_eq!(watch_log(&sd), ["start refresh gone", "start refresh gone"]);
}

/// <branch>'s cache file in repos/r, rewritten with the rate limit sd state saw, <secs_ago> ago.
fn with_rate(sd: &Sd, branch: &str, rate: Value, secs_ago: u64) {
    let f = sd.tmp.join(format!("state/branches/{}/{branch}.json", sheepdog::watch::repo_key(&sd.path("repos/r"))));
    let mut st = state(branch, json!([]));
    st["rate_limit"] = rate;
    fs::write(&f, json!({ "glyph": "🤖", "state": st }).to_string()).unwrap();
    let then = std::time::SystemTime::now() - Duration::from_secs(secs_ago);
    fs::File::options().write(true).open(&f).unwrap().set_modified(then).unwrap();
}

#[test]
fn no_branch_is_refreshed_while_the_last_rate_limit_seen_is_under_the_floor() {
    let mut sd = sd();
    watched(&mut sd, "repos/r", "old", 120);
    watched(&mut sd, "repos/r", "newer", 90);
    with_rate(&sd, "old", json!({ "remaining": 4000, "limited": false }), 120);
    with_rate(&sd, "newer", json!({ "remaining": 100, "limited": false }), 90);
    let s = serve(&sd);
    let f = s.flock();
    assert_eq!(f["refreshing"], false);
    assert_has(f["held"].as_str().unwrap_or_else(|| panic!("{f}")), &["GitHub rate limit", "100 points left", "settings.rateLimitFloor (250)"]);
    std::thread::sleep(Duration::from_millis(200));
    assert!(watch_log(&sd).is_empty(), "{:?}", watch_log(&sd));
    with_rate(&sd, "newer", json!({ "remaining": 0, "limited": true }), 90);
    assert_has(s.flock()["held"].as_str().unwrap(), &["GitHub refused a call for the rate limit"]);
    with_rate(&sd, "newer", json!({ "remaining": 3000, "limited": false }), 90);
    let f = s.flock();
    assert_eq!((&f["refreshing"], &f["held"]), (&json!(true), &Value::Null));
}

#[test]
fn github_data_a_week_unwritten_is_pruned_at_start() {
    let sd = sd();
    let dir = sd.tmp.join("state/github");
    fs::create_dir_all(&dir).unwrap();
    for (name, days) in [("old.json", 8), ("new.json", 6), ("x.json.tmp123", 8)] {
        fs::write(dir.join(name), "{}").unwrap();
        let then = std::time::SystemTime::now() - Duration::from_secs(days * 86400);
        fs::File::options().write(true).open(dir.join(name)).unwrap().set_modified(then).unwrap();
    }
    let _s = serve(&sd);
    let mut left: Vec<String> = fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    left.sort();
    assert_eq!(left, ["new.json"]);
}

#[test]
fn a_refresh_that_started_before_a_hold_still_reports_refreshing() {
    let mut sd = sd();
    watched(&mut sd, "repos/r", "old", 120);
    watched(&mut sd, "repos/r", "other", 5);
    let gate = sd.path("gate");
    fs::write(&gate, "").unwrap();
    sd.env.push(("WATCH_GATE".into(), gate.clone()));
    let s = serve(&sd);
    assert_eq!(s.flock()["refreshing"], true);
    with_rate(&sd, "other", json!({ "remaining": 0, "limited": true }), 0);
    let f = s.flock();
    fs::remove_file(&gate).unwrap();
    assert!(f["held"].is_string(), "{f}");
    assert_eq!(f["refreshing"], true, "{f}");
}

#[test]
fn a_cache_file_written_in_the_future_doesn_t_hold_refreshes() {
    let mut sd = sd();
    watched(&mut sd, "repos/r", "old", 120);
    watched(&mut sd, "repos/r", "ahead", 0);
    with_rate(&sd, "old", json!({ "remaining": 4000, "limited": false }), 120);
    let f = sd.tmp.join(format!("state/branches/{}/ahead.json", sheepdog::watch::repo_key(&sd.path("repos/r"))));
    let mut st = state("ahead", json!([]));
    st["rate_limit"] = json!({ "remaining": 0, "limited": true });
    fs::write(&f, json!({ "glyph": "🤖", "state": st }).to_string()).unwrap();
    fs::File::options().write(true).open(&f).unwrap().set_modified(std::time::SystemTime::now() + Duration::from_secs(3600)).unwrap();
    let f = serve(&sd).flock();
    assert_eq!((&f["held"], &f["refreshing"]), (&Value::Null, &json!(true)), "{f}");
}

// The Map's stages and trend (#378, slice 2).

#[test]
fn api_map_groups_files_by_stage_and_carries_a_trend() {
    let sd = sd();
    sd.fake("gh", "#!/usr/bin/env bash\necho '[]'\n");
    let s = serve(&sd);
    let mut m = map(&s);
    let end = Instant::now() + Duration::from_secs(20);
    while m["trend"]["refreshing"] == true {
        assert!(Instant::now() < end, "trend still refreshing after 20s: {m}");
        std::thread::sleep(Duration::from_millis(50));
        m = map(&s);
    }
    let names: Vec<&str> = m["groups"].as_array().unwrap().iter().map(|g| g["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["watch loop", "observe", "decide", "act", "agent steps", "records", "settings", "pages", "plumbing"]);
    let group = |p: &str| m["groups"].as_array().unwrap().iter().find(|g| g["files"].as_array().unwrap().iter().any(|f| f["path"] == p)).unwrap()["name"].clone();
    assert_eq!([group("src/state.rs"), group("src/act/push.rs"), group("src/act/implement.rs"), group("src/view/map.rs")], ["observe", "act", "agent steps", "pages"]);
    let t = &m["trend"];
    let root = env!("CARGO_MANIFEST_DIR");
    // CI's checkout has no .git: the trend then says so and has no points
    // (the unit tests in src/view/map.rs cover the trend on a built repo).
    if !std::path::Path::new(root).join(".git").exists() {
        assert_eq!(t["points"], serde_json::json!([]), "{t}");
        assert!(t["error"].as_str().unwrap().contains("not a git repository"), "{t}");
        return;
    }
    assert_eq!((&t["error"], t["fetched"].is_string()), (&Value::Null, true), "{t}");
    let points = t["points"].as_array().unwrap();
    assert!(points.len() >= 2, "{t}");
    let head = String::from_utf8(std::process::Command::new("git").args(["-C", root, "rev-parse", "HEAD"]).output().unwrap().stdout).unwrap();
    assert_eq!(points.last().unwrap()["sha"], head.trim());
    let dates: Vec<&str> = points.iter().map(|p| p["date"].as_str().unwrap()).collect();
    let mut sorted = dates.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(dates, sorted, "one point a day, oldest first");
    assert!(dates[0] >= "2026-10-13");
    for p in points {
        assert!(p["src"].is_u64() && p["tests"].is_u64() && p["ratchet"].is_u64(), "{p}");
    }
    assert!(points.last().unwrap()["src"].as_u64().unwrap() > 1000);
    let cached = fs::read_dir(sd.tmp.join("state/github")).unwrap().flatten().any(|e| e.file_name().to_string_lossy().starts_with("trend-"));
    assert!(cached, "the trend is cached under <state>/github/");
}

#[test]
fn the_map_page_draws_the_stages_and_the_trend() {
    let sd = sd();
    let s = serve(&sd);
    assert_has(&s.get("/map").2, &["trend", "<svg"]);
}

// A step's settings (#377): the keys its verb reads, each with this repo's
// value and the layer it comes from (sd config explain) and the sd config
// set command that changes it.

fn atlas_step(a: &Value, key: &str) -> Value {
    a["holds"].as_array().unwrap().iter().chain(a["stages"].as_array().unwrap().iter().flat_map(|s| s["steps"].as_array().unwrap()))
        .find(|s| s["key"] == key).unwrap_or_else(|| panic!("no {key} in {a}")).clone()
}

fn setting(step: &Value, path: &str) -> Value {
    step["settings"].as_array().unwrap_or_else(|| panic!("no settings in {step}")).iter().find(|s| s["path"] == path)
        .unwrap_or_else(|| panic!("no {path} in {step}")).clone()
}

#[test]
fn a_step_s_settings_come_with_this_repo_s_value_its_layer_and_the_set_command() {
    let sd = sd();
    sd.write_global(r#"{"settings": {"reviewRounds": 5, "botCommand": "@bot review"}}"#);
    let s = serve(&sd);
    let (code, _, body) = s.get("/api/atlas");
    assert_eq!(code, 200, "{body}");
    let a: Value = serde_json::from_str(&body).unwrap_or_else(|e| panic!("{e}: {body}"));
    assert_eq!(a["configError"], Value::Null);
    let decide = atlas_step(&a, "review-decide");
    assert_eq!(
        setting(&decide, "settings.reviewRounds"),
        json!({"path": "settings.reviewRounds", "value": 5, "layer": "global", "set": "sd config set --repo settings.reviewRounds 5"})
    );
    let trigger = atlas_step(&a, "bot-trigger");
    assert_eq!(
        setting(&trigger, "settings.botCommand"),
        json!({"path": "settings.botCommand", "value": "@bot review", "layer": "global", "set": "sd config set --repo settings.botCommand '@bot review'"})
    );
    let silent = atlas_step(&a, "bot-silent");
    assert_eq!(setting(&silent, "settings.botTimeout")["layer"], "defaults");
    assert_eq!(setting(&silent, "settings.botTimeout")["value"], 3600);
    assert_eq!(atlas_step(&a, "ready")["settings"], json!([]));
    assert_has(&s.get("/atlas").2, &["data-copy", "Settings that tune it"]);
}

#[test]
fn a_broken_config_leaves_the_settings_named_but_without_values() {
    let sd = sd();
    sd.write_global("not json");
    let s = serve(&sd);
    let (code, _, body) = s.get("/api/atlas");
    assert_eq!(code, 200, "{body}");
    let a: Value = serde_json::from_str(&body).unwrap();
    assert_has(a["configError"].as_str().unwrap(), &["not a JSON object"]);
    let rounds = setting(&atlas_step(&a, "review-decide"), "settings.reviewRounds");
    assert_eq!((&rounds["value"], &rounds["layer"]), (&Value::Null, &Value::Null));
    for key in ["ci-red", "bot", "open-pr", "ship-parent"] {
        let paths: Vec<Value> = atlas_step(&a, key)["settings"].as_array().unwrap().iter().map(|s| s["path"].clone()).collect();
        assert!(paths.contains(&json!("settings.branchBudget")), "{key}: {paths:?}");
    }
    let ship: Vec<Value> = atlas_step(&a, "open-pr")["settings"].as_array().unwrap().iter().map(|s| s["path"].clone()).collect();
    for p in ["describePrompt", "stepTimeout", "stepBudget", "forkLimit", "branchBudget", "autoRun", "remote"] {
        assert!(ship.contains(&json!(format!("settings.{p}"))), "open-pr has no {p}: {ship:?}");
    }
    assert_eq!(rounds["set"], "sd config set --repo settings.reviewRounds <value>");
    let logins = setting(&atlas_step(&a, "read"), "settings.botLogin");
    assert_eq!(logins["set"], "sd config set --repo --json settings.botLogin <value>");
}

// The links between the three pages (#375): a branch's NEXT step opens its
// Atlas step, an Atlas step lists the branches at it now and the files behind
// it in the Map, and a Map file lists the Atlas steps that live in it.

fn atlas(s: &Server) -> Value {
    let (code, _, body) = s.get("/api/atlas");
    assert_eq!(code, 200, "{body}");
    serde_json::from_str(&body).unwrap_or_else(|e| panic!("{e}: {body}"))
}

#[test]
fn an_atlas_step_lists_the_branches_whose_first_step_it_is() {
    let sd = sd();
    cache(&sd, "sheepdog", "feat/x", state("feat/x", json!([
        { "key": "review-fix", "who": "agent", "say": "fix 1 review finding", "cmd": "sd act implement" },
        { "key": "ci-wait", "who": "wait", "say": "CI running", "cmd": null },
    ])));
    cache(&sd, "kinsite", "k", state("k", json!([{ "key": "ready", "who": "you", "say": "merge it", "cmd": null }])));
    let s = serve(&sd);
    let ids: Vec<String> = s.flock()["branches"].as_array().unwrap().iter().map(|b| b["id"].as_str().unwrap().to_string()).collect();
    let a = atlas(&s);
    assert_eq!(atlas_step(&a, "review-fix")["branches"], json!([{ "id": ids[1], "repo": "sheepdog", "branch": "feat/x" }]));
    assert_eq!(atlas_step(&a, "ready")["branches"], json!([{ "id": ids[0], "repo": "kinsite", "branch": "k" }]));
    assert_eq!(atlas_step(&a, "ci-wait")["branches"], json!([]));
    assert_has(&s.get("/atlas").2, &["/#b="]);
}

#[test]
fn an_atlas_step_lists_the_files_behind_it() {
    let sd = sd();
    let s = serve(&sd);
    let a = atlas(&s);
    assert_eq!(atlas_step(&a, "review-fix")["files"], json!(["src/act/implement.rs", "src/act/gates.rs", "src/next/steps.rs"]));
    assert_eq!(atlas_step(&a, "conflict")["files"], json!(["src/act/restack.rs", "src/act/resolve.rs", "src/act/gates.rs", "src/next/steps.rs"]));
    // trigger has no file of its own and no gate
    assert_eq!(atlas_step(&a, "bot-trigger")["files"], json!(["src/next/steps.rs"]));
    assert_eq!(atlas_step(&a, "ready")["files"], json!(["src/next/steps.rs"]));
    assert_has(&s.get("/atlas").2, &["/map#p="]);
}

#[test]
fn a_map_file_lists_the_atlas_steps_that_live_in_it() {
    let sd = sd();
    sd.fake("gh", common::GH_FAILS);
    let s = serve(&sd);
    let m = map(&s);
    let steps = |p: &str| map_file(&m, p)["steps"].as_array().unwrap_or_else(|| panic!("no steps in {p}")).iter().map(|k| k.as_str().unwrap().to_string()).collect::<Vec<_>>();
    assert_eq!(steps("src/act/push.rs"), ["push"]);
    assert!(steps("src/act/gates.rs").contains(&"review-fix".to_string()), "{:?}", steps("src/act/gates.rs"));
    assert!(!steps("src/act/gates.rs").contains(&"ready".to_string()));
    assert!(steps("src/next/steps.rs").contains(&"ready".to_string()));
    assert_eq!(steps("src/view.rs"), Vec::<String>::new());
    assert_has(&s.get("/map").2, &["/atlas#k="]);
}

#[test]
fn the_drill_in_links_each_next_step_to_its_atlas_step() {
    let sd = sd();
    assert_has(&serve(&sd).get("/").2, &["/atlas#k="]);
    let r = sd.run(&["--help"]);
    assert_has(&r.output, &["The three link into each other", "lists the Atlas steps that live in it"]);
}
