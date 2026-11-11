// sd view [<branch>] [--port <n>] [--no-open] (src/view_help.txt, #375,
// #376): <branch>, . or a part of a cached branch's name or its PR number,
// opens on its drill-in (view/pick.rs, #458); one already on the port is
// opened, or replaced when GET /api/build says it's an older build and
// names the pid sd view recorded in the state dir. A running one execs its
// rebuilt binary in place (#496), and the pages reload on the new build
// (theme.js). GET / is
// the Flock page, GET /api/flock its data from the watcher's cache; GET /map
// the Map page (#378), GET /api/map its data from sheepdog's own source; GET
// /atlas the Atlas page (#377), GET /api/atlas its data from the step table
// and the gates; the three link into each other (view/links.rs, #375). std's
// TcpListener, a thread per connection: a browser's idle preconnect socket
// mustn't hold up the page.
//
// Only 127.0.0.1, and only Host 127.0.0.1 or localhost: another site can
// rebind its name to 127.0.0.1. Writes go to POST /api/act (view/act.rs: a
// branch's NEXT step, run as the watcher runs it) and POST /api/config
// (view/settings.rs: one of a branch's settings, set as sd config sets it)
// behind the guard (view/guard.rs, #396): this run's token, which the page
// carries, and our own Origin, as does POST /api/refresh, the pages' refresh
// button (#401).

mod act;
mod atlas;
mod branch;
mod cache;
mod flock;
mod guard;
mod links;
mod map;
mod pick;
mod refresh;
mod settings;
mod timeline;

use crate::config::state_dir;
use guard::Request;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::sync::LazyLock;
use std::time::{Duration, UNIX_EPOCH};

const HELP: &str = include_str!("view_help.txt");
const PAGE: &str = include_str!("view/flock.html");
const MAP_PAGE: &str = include_str!("view/map.html");
const ATLAS_PAGE: &str = include_str!("view/atlas.html");
const GH_JS: &str = include_str!("view/gh.js");
/// The files the pages load as they are: the look (theme.css, its picks in
/// theme.js), the fonts it names (fonts/, each under the SIL Open Font
/// License beside it), and the official art, four-color: the dog beside each
/// page's title, and the tab icon. Each with its Cache-Control: the look
/// changes with a build and must show at once; the fonts and art don't.
const FILES: [(&str, &str, &str, &[u8]); 9] = [
    ("/theme.css", "text/css; charset=utf-8", NO_STORE, include_bytes!("view/theme.css")),
    ("/theme.js", "text/javascript; charset=utf-8", NO_STORE, include_bytes!("view/theme.js")),
    ("/fonts/atkinson-hyperlegible-next.woff2", "font/woff2", A_DAY, include_bytes!("../fonts/atkinson-hyperlegible-next.woff2")),
    ("/fonts/atkinson-hyperlegible-mono.woff2", "font/woff2", A_DAY, include_bytes!("../fonts/atkinson-hyperlegible-mono.woff2")),
    ("/fonts/londrina-solid.woff2", "font/woff2", A_DAY, include_bytes!("../fonts/londrina-solid.woff2")),
    ("/fonts/alfa-slab-one.woff2", "font/woff2", A_DAY, include_bytes!("../fonts/alfa-slab-one.woff2")),
    ("/fonts/big-shoulders-display.woff2", "font/woff2", A_DAY, include_bytes!("../fonts/big-shoulders-display.woff2")),
    ("/fonts/familjen-grotesk.woff2", "font/woff2", A_DAY, include_bytes!("../fonts/familjen-grotesk.woff2")),
    ("/img/sheepdog.png", "image/png", A_DAY, include_bytes!("../img/sheepdog-four-color.png")),
];
const PORT: u16 = 7377;
const BODY_MAX: usize = 4096;

fn die(msg: impl std::fmt::Display) -> ! {
    eprintln!("sd view: {msg}");
    std::process::exit(1)
}

/// This binary's path as it started: on Linux, current_exe() names a
/// rebuilt binary's old, deleted file.
static EXE: LazyLock<Option<PathBuf>> = LazyLock::new(|| std::env::current_exe().ok());

/// This binary's build as it started: its file's mtime, which a rebuild
/// changes. GET /api/build serves it beside the pid.
static BUILD: LazyLock<String> = LazyLock::new(|| built().unwrap_or_default());

/// The mtime of the file at EXE now.
fn built() -> Option<String> {
    let t = std::fs::metadata(EXE.as_ref()?).and_then(|m| m.modified()).ok()?;
    Some(t.duration_since(UNIX_EPOCH).ok()?.as_nanos().to_string())
}

pub fn run(argv: &[String]) -> ExitCode {
    LazyLock::force(&BUILD);
    let (mut port, mut open, mut target) = (PORT, true, None);
    let mut args = argv.iter();
    while let Some(a) = args.next() {
        match a.as_str() {
            "-h" | "--help" => {
                print!("{HELP}");
                return ExitCode::SUCCESS;
            }
            "--no-open" => open = false,
            "--port" => {
                port = args.next().and_then(|p| p.parse().ok()).unwrap_or_else(|| die("--port takes a number"));
            }
            o if o.starts_with('-') || target.is_some() => die(format!("unknown option: {o}")),
            o => target = Some(o.to_string()),
        }
    }
    let state = state_dir();
    let hash = target.map_or_else(String::new, |t| format!("#b={}", branch_id(Path::new(&state), &t)));
    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(l) => l,
        // Most likely our own sd view, on the port it always takes: an
        // older build gives way, the same build is opened.
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => match running(port) {
            Some((pid, build)) if build != *BUILD && recorded(Path::new(&state), port) == Some(pid) => replace(pid, port),
            _ => {
                let url = format!("http://127.0.0.1:{port}/{hash}");
                println!("sd view: already running at {url}");
                if open {
                    browse(&url);
                }
                return ExitCode::SUCCESS;
            }
        },
        Err(e) => die(format!("127.0.0.1:{port}: {e}")),
    };
    cache::prune(Path::new(&state));
    let pruning = state.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(3600));
        cache::prune(Path::new(&pruning));
    });
    let port = listener.local_addr().map(|a| a.port()).unwrap_or(port);
    let _ = std::fs::create_dir_all(&state).and_then(|()| std::fs::write(pid_file(Path::new(&state), port), std::process::id().to_string()));
    let url = format!("http://127.0.0.1:{port}/{hash}");
    // Restarted, its terminal may be gone: println! would panic.
    let _ = writeln!(std::io::stdout(), "sd view: {url} (Ctrl-C stops it)");
    if open {
        browse(&url);
    }
    let argc = argv.len();
    std::thread::spawn(move || restart_on_rebuild(port, argc));
    let token = guard::token();
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let (state, token) = (state.clone(), token.clone());
                std::thread::spawn(move || serve(stream, port, &token, Path::new(&state)));
            }
            Err(e) => {
                eprintln!("sd view: {e}");
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
    ExitCode::SUCCESS
}

/// The cached branch <target> names (. is the current worktree's), or die
/// saying why it isn't one. . matches its branch's name exactly, in its own
/// repo only: anything wider would open some other branch when its own
/// isn't cached.
fn branch_id(state: &Path, target: &str) -> String {
    let cached = flock::cached(state);
    if target != "." {
        return pick::pick(&cached, target, true).unwrap_or_else(|e| die(e));
    }
    // Mid-rebase (a restack's conflict), the branch being rebased.
    let Some(head) = crate::git::current_branch(".") else {
        die(". names no branch: HEAD is detached, or this isn't a git worktree");
    };
    let repo = format!("{}/", crate::watch::repo_key(&crate::git::main_worktree(".")));
    let ours: Vec<_> = cached.into_iter().filter(|c| c.id.starts_with(&repo)).collect();
    pick::pick(&ours, &head, false).unwrap_or_else(|e| die(e))
}

/// The pid and build of the sd view holding <port>; None when what holds
/// it doesn't answer GET /api/build (not sd view, or one built before it).
fn running(port: u16) -> Option<(i32, String)> {
    let mut s = TcpStream::connect(("127.0.0.1", port)).ok()?;
    s.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    write!(s, "GET /api/build HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n").ok()?;
    let mut resp = String::new();
    s.read_to_string(&mut resp).ok()?;
    let v: serde_json::Value = serde_json::from_str(resp.split_once("\r\n\r\n")?.1).ok()?;
    Some((i32::try_from(v["pid"].as_u64()?).ok()?, v["build"].as_str()?.to_string()))
}

/// Where sd view on <port> records its pid: in the user's own state dir,
/// so whatever holds the port can't name a pid for replace to signal.
fn pid_file(state: &Path, port: u16) -> std::path::PathBuf {
    state.join(format!("view-{port}.pid"))
}

fn recorded(state: &Path, port: u16) -> Option<i32> {
    std::fs::read_to_string(pid_file(state, port)).ok()?.trim().parse().ok().filter(|&p| p > 1)
}

/// Stop the older build's sd view, <pid>, and take its port.
fn replace(pid: i32, port: u16) -> TcpListener {
    // SAFETY: kill(2) takes no pointers.
    unsafe { libc::kill(pid, libc::SIGTERM) };
    for _ in 0..50 {
        if let Ok(l) = TcpListener::bind(("127.0.0.1", port)) {
            eprintln!("sd view: stopped an older build's sd view (pid {pid})");
            return l;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    die(format!("127.0.0.1:{port}: an older build's sd view (pid {pid}) didn't stop"))
}

/// Once a rebuild has replaced this binary and it holds still for a look,
/// run the new one in this process (#496): the same pid and port, no
/// browser. Not while a step or refresh it started runs: it would be
/// orphaned, its result never read.
fn restart_on_rebuild(port: u16, argc: usize) {
    let every = Duration::from_millis(recheck_ms());
    let mut last = None;
    loop {
        std::thread::sleep(every);
        let now = built();
        if settled(&BUILD, last.as_deref(), now.as_deref()) {
            if let (Some(_step), Some(_refresh)) = (act::idle(), refresh::idle()) {
                let e = reexec(port, argc);
                let _ = writeln!(std::io::stderr(), "sd view: couldn't restart on the new build: {e}");
                return;
            }
        }
        last = now;
    }
}

fn recheck_ms() -> u64 {
    let v = if cfg!(debug_assertions) { std::env::var("SHEEPDOG_VIEW_RECHECK_MS").ok() } else { None };
    v.and_then(|v| v.parse().ok()).unwrap_or(2000)
}

/// A build other than <build>, the same at the last look as now.
fn settled(build: &str, last: Option<&str>, now: Option<&str>) -> bool {
    now.is_some_and(|n| n != build && last == Some(n))
}

/// Replace this process with sd view from EXE, as it was run but for its
/// own <argc> arguments: those become --no-open and this <port>. Returns
/// only on failure.
fn reexec(port: u16, argc: usize) -> std::io::Error {
    use std::os::unix::process::CommandExt;
    let all: Vec<_> = std::env::args_os().collect();
    let (Some(exe), Some((argv0, verb))) = (EXE.as_ref(), all[..all.len().saturating_sub(argc)].split_first()) else {
        return std::io::Error::other("no path to this binary");
    };
    let _ = writeln!(std::io::stderr(), "sd view: a new build of sd; restarting on port {port}");
    Command::new(exe).arg0(argv0).args(verb).args(["--no-open", "--port", &port.to_string()]).exec()
}

fn browse(url: &str) {
    let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    let _ = Command::new(opener).arg(url).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
}

/// The request line and the headers we read, or None for a request we can't read.
fn read_request(r: &mut impl BufRead) -> Option<Request> {
    let mut line = String::new();
    r.read_line(&mut line).ok()?;
    let mut parts = line.split_whitespace();
    let mut req = Request { method: parts.next()?.into(), path: parts.next()?.into(), ..Default::default() };
    let mut len = 0;
    loop {
        let mut h = String::new();
        if r.read_line(&mut h).ok()? == 0 || h.trim().is_empty() {
            break;
        }
        let Some((k, v)) = h.split_once(':') else { continue };
        let v = v.trim().to_string();
        match k.to_ascii_lowercase().as_str() {
            "host" => req.host = v,
            "origin" => req.origin = Some(v),
            "x-sd-token" => req.token = Some(v),
            "content-length" => len = v.parse().unwrap_or(0),
            _ => {}
        }
    }
    if req.method == "POST" && len > 0 {
        let mut body = vec![0; len.min(BODY_MAX)];
        if r.read_exact(&mut body).is_ok() {
            req.body = String::from_utf8_lossy(&body).into_owned();
        }
    }
    Some(req)
}

const WRITES: [&str; 3] = ["/api/act", "/api/config", "/api/refresh"];

/// The guarded writes: /api/refresh makes GitHub data stale, /api/act runs
/// a branch's NEXT step (view/act.rs), /api/config sets one of its settings
/// (view/settings.rs).
fn write(route: &str, req: &Request, port: u16, token: &str, state: &Path) -> (&'static str, &'static str, String) {
    if req.method != "POST" {
        return ("405 Method Not Allowed", "text/plain", "a write is a POST\n".into());
    }
    if let Some(why) = guard::refusal(req, port, token) {
        return ("403 Forbidden", "text/plain", why.into());
    }
    let (status, body) = match route {
        "/api/refresh" => {
            cache::force();
            ("200 OK", "GitHub data is stale: the next read fetches it again\n".into())
        }
        "/api/config" => settings::set(state, &req.body),
        _ => act::run(state, &req.body),
    };
    (status, "text/plain", body)
}

const NO_STORE: &str = "no-store";
const A_DAY: &str = "max-age=86400";

/// Every route sd view serves, behind the one Host check: the status, the
/// content type, the Cache-Control and the body.
fn respond(req: &Request, port: u16, token: &str, state: &Path) -> (&'static str, &'static str, &'static str, Vec<u8>) {
    let ours = guard::hosts(port);
    if !ours.contains(&req.host) {
        return ("403 Forbidden", "text/plain", NO_STORE, format!("sd view answers to {} only\n", ours.join(" or ")).into());
    }
    let (route, query) = req.path.split_once('?').unwrap_or((&req.path, ""));
    let (status, ctype, body) = if WRITES.contains(&route) {
        write(route, req, port, token, state)
    } else if req.method != "GET" {
        ("405 Method Not Allowed", "text/plain", "sd view serves GET only\n".into())
    } else if let Some((_, ctype, cache, bytes)) = FILES.iter().find(|(p, ..)| *p == route) {
        return ("200 OK", ctype, cache, bytes.to_vec());
    } else {
        get(route, query, token, state)
    };
    (status, ctype, NO_STORE, body.into_bytes())
}

fn get(route: &str, query: &str, token: &str, state: &Path) -> (&'static str, &'static str, String) {
    match route {
        "/" => ("200 OK", "text/html; charset=utf-8", PAGE.replace("__SD_TOKEN__", token)),
        "/api/flock" => ("200 OK", "application/json", flock::flock(state).to_string()),
        "/map" => ("200 OK", "text/html; charset=utf-8", MAP_PAGE.replace("__SD_TOKEN__", token)),
        "/gh.js" => ("200 OK", "text/javascript; charset=utf-8", GH_JS.into()),
        "/api/map" => {
            let mut m = map::map(state);
            links::add_steps(&mut m);
            ("200 OK", "application/json", m.to_string())
        }
        "/atlas" => ("200 OK", "text/html; charset=utf-8", ATLAS_PAGE.replace("__SD_TOKEN__", token)),
        "/api/atlas" => ("200 OK", "application/json", atlas::atlas(state).to_string()),
        "/api/build" => ("200 OK", "application/json", serde_json::json!({"pid": std::process::id(), "build": *BUILD}).to_string()),
        "/api/branch" | "/api/timeline" => {
            let id = query.split('&').find_map(|kv| kv.strip_prefix("id=")).map(unescape).unwrap_or_default();
            let found = match route {
                "/api/branch" => branch::detail(state, &id),
                _ => branch::find(state, &id).map(|c| timeline::timeline(&c, &branch::usage(state), state)),
            };
            match found {
                Some(b) => ("200 OK", "application/json", b.to_string()),
                None => ("404 Not Found", "text/plain", "no such branch in the watcher's cache\n".into()),
            }
        }
        _ => ("404 Not Found", "text/plain", "not found\n".into()),
    }
}

/// %XX escapes decoded, as encodeURIComponent writes them.
fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match (b[i], b.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(std::str::from_utf8(h).ok()?, 16).ok())) {
            (b'%', Some(x)) => {
                out.push(x);
                i += 3;
            }
            (c, _) => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn serve(stream: TcpStream, port: u16, token: &str, state: &Path) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut r = BufReader::new((&stream).take(16 * 1024));
    let Some(req) = read_request(&mut r) else { return };
    let (status, ctype, cache, body) = respond(&req, port, token, state);
    let mut w = &stream;
    let _ = write!(
        w,
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nCache-Control: {cache}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = w.write_all(&body);
    // Closing with request bytes unread (a POST's body) resets the
    // connection, which can lose the response: drain them first.
    let _ = stream.shutdown(std::net::Shutdown::Write);
    let _ = std::io::copy(&mut r, &mut std::io::sink());
}

#[cfg(test)]
mod tests {
    use super::settled;

    #[test]
    fn a_new_build_restarts_once_it_holds_still_for_a_look() {
        assert!(!settled("1", Some("1"), Some("1")), "the build it started with");
        assert!(!settled("1", Some("1"), Some("2")), "just changed: it may still be writing");
        assert!(!settled("1", Some("2"), Some("3")), "still changing");
        assert!(settled("1", Some("2"), Some("2")));
        assert!(!settled("1", Some("2"), None), "no file there now, mid-replace");
        assert!(!settled("1", None, None));
    }
}
