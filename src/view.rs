// sd view [<branch>] [--port <n>] [--no-open] (src/view_help.txt, #375,
// #376): <branch>, . or a part of a cached branch's name or its PR number,
// opens on its drill-in (view/pick.rs, #458). GET / is
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
use std::path::Path;
use std::process::{Command, ExitCode, Stdio};
use std::time::Duration;

const HELP: &str = include_str!("view_help.txt");
const PAGE: &str = include_str!("view/flock.html");
const MAP_PAGE: &str = include_str!("view/map.html");
const ATLAS_PAGE: &str = include_str!("view/atlas.html");
const GH_JS: &str = include_str!("view/gh.js");
const PORT: u16 = 7377;
const BODY_MAX: usize = 4096;

fn die(msg: impl std::fmt::Display) -> ! {
    eprintln!("sd view: {msg}");
    std::process::exit(1)
}

pub fn run(argv: &[String]) -> ExitCode {
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
        // Most likely our own sd view, on the port it always takes.
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            let url = format!("http://127.0.0.1:{port}/{hash}");
            println!("sd view: already running at {url}");
            if open {
                browse(&url);
            }
            return ExitCode::SUCCESS;
        }
        Err(e) => die(format!("127.0.0.1:{port}: {e}")),
    };
    cache::prune(Path::new(&state));
    let pruning = state.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(3600));
        cache::prune(Path::new(&pruning));
    });
    let port = listener.local_addr().map(|a| a.port()).unwrap_or(port);
    let url = format!("http://127.0.0.1:{port}/{hash}");
    println!("sd view: {url} (Ctrl-C stops it)");
    if open {
        browse(&url);
    }
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
/// saying why it isn't one. . matches its branch's name exactly: a part of
/// it would open some other branch when its own isn't cached.
fn branch_id(state: &Path, target: &str) -> String {
    let (name, parts) = if target == "." {
        let head = crate::git::git(".", &["symbolic-ref", "--short", "HEAD"]);
        if !head.ok {
            die(format!(". names no branch: {}", head.stderr.trim()));
        }
        (head.stdout.trim().to_string(), false)
    } else {
        (target.to_string(), true)
    };
    pick::pick(&flock::cached(state), &name, parts).unwrap_or_else(|e| die(e))
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

fn respond(req: &Request, port: u16, token: &str, state: &Path) -> (&'static str, &'static str, String) {
    let ours = guard::hosts(port);
    if !ours.contains(&req.host) {
        return ("403 Forbidden", "text/plain", format!("sd view answers to {} only\n", ours.join(" or ")));
    }
    let (route, query) = req.path.split_once('?').unwrap_or((&req.path, ""));
    if WRITES.contains(&route) {
        return write(route, req, port, token, state);
    }
    if req.method != "GET" {
        return ("405 Method Not Allowed", "text/plain", "sd view serves GET only\n".into());
    }
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
    let (status, ctype, body) = respond(&req, port, token, state);
    let mut w = &stream;
    let _ = write!(
        w,
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    // Closing with request bytes unread (a POST's body) resets the
    // connection, which can lose the response: drain them first.
    let _ = stream.shutdown(std::net::Shutdown::Write);
    let _ = std::io::copy(&mut r, &mut std::io::sink());
}
