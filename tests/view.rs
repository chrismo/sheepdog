// sd view (#376): a local server on 127.0.0.1 for the Flock page, every
// enrolled branch from the watcher's cache. Each test starts it on a free
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

/// The watcher's cache file for <branch> of a repo named <repo>: {"state": <state>}.
fn cache(sd: &Sd, repo: &str, branch: &str, state: Value) {
    let f = sd.tmp.join(format!("state/branches/{repo}-0123abcd/{branch}.json"));
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
        "threads": { "owed_bot": 0, "owed_human": 0 },
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
    let f = sd.tmp.join("state/branches/sheepdog-0123abcd/torn.json");
    fs::create_dir_all(f.parent().unwrap()).unwrap();
    fs::write(f, "{\"state\":").unwrap();
    let b = &serve(&sd).flock()["branches"][0];
    assert_eq!(b["branch"], "torn");
    assert_eq!(b["lead"]["who"], "wait");
    assert_has(b["lead"]["say"].as_str().unwrap(), &["unreadable"]);
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
