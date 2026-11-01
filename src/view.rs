// sd view [--port <n>] [--no-open] (src/view_help.txt, #375, #376): GET / is
// the Flock page, GET /api/flock its data from the watcher's cache. std's
// TcpListener, a thread per connection: a browser's idle preconnect socket
// mustn't hold up the page.
//
// Only 127.0.0.1, and only Host 127.0.0.1 or localhost: another site can
// rebind its name to 127.0.0.1. Read-only: before anything writes, add a
// per-run token in the page and an Origin check (#376).

mod branch;
mod flock;
mod timeline;

use crate::config::state_dir;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Command, ExitCode, Stdio};
use std::time::Duration;

const HELP: &str = include_str!("view_help.txt");
const PAGE: &str = include_str!("view/flock.html");
const PORT: u16 = 7377;

fn die(msg: impl std::fmt::Display) -> ! {
    eprintln!("sd view: {msg}");
    std::process::exit(1)
}

pub fn run(argv: &[String]) -> ExitCode {
    let (mut port, mut open) = (PORT, true);
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
            o => die(format!("unknown option: {o}")),
        }
    }
    let listener = TcpListener::bind(("127.0.0.1", port)).unwrap_or_else(|e| die(format!("127.0.0.1:{port}: {e}")));
    let port = listener.local_addr().map(|a| a.port()).unwrap_or(port);
    let url = format!("http://127.0.0.1:{port}/");
    println!("sd view: {url} (Ctrl-C stops it)");
    if open {
        let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
        let _ = Command::new(opener).arg(&url).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
    }
    let state = state_dir();
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let state = state.clone();
                std::thread::spawn(move || serve(stream, port, Path::new(&state)));
            }
            Err(e) => {
                eprintln!("sd view: {e}");
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
    ExitCode::SUCCESS
}

/// The request line and Host header, or None for a request we can't read.
fn read_request(r: &mut impl BufRead) -> Option<(String, String, String)> {
    let mut line = String::new();
    r.read_line(&mut line).ok()?;
    let mut parts = line.split_whitespace();
    let (method, path) = (parts.next()?.to_string(), parts.next()?.to_string());
    let mut host = String::new();
    loop {
        let mut h = String::new();
        if r.read_line(&mut h).ok()? == 0 || h.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            if k.eq_ignore_ascii_case("host") {
                host = v.trim().to_string();
            }
        }
    }
    Some((method, path, host))
}

fn respond(method: &str, path: &str, host: &str, port: u16, state: &Path) -> (&'static str, &'static str, String) {
    let ours = [format!("127.0.0.1:{port}"), format!("localhost:{port}")];
    if !ours.iter().any(|h| h == host) {
        return ("403 Forbidden", "text/plain", format!("sd view answers to {} only\n", ours.join(" or ")));
    }
    if method != "GET" {
        return ("405 Method Not Allowed", "text/plain", "sd view serves GET only\n".into());
    }
    let (route, query) = path.split_once('?').unwrap_or((path, ""));
    match route {
        "/" => ("200 OK", "text/html; charset=utf-8", PAGE.into()),
        "/api/flock" => ("200 OK", "application/json", flock::flock(state).to_string()),
        "/api/branch" | "/api/timeline" => {
            let id = query.split('&').find_map(|kv| kv.strip_prefix("id=")).map(unescape).unwrap_or_default();
            let found = match route {
                "/api/branch" => branch::detail(state, &id),
                _ => branch::find(state, &id).map(|c| timeline::timeline(&c, &branch::usage(state))),
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

fn serve(stream: TcpStream, port: u16, state: &Path) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut r = BufReader::new((&stream).take(16 * 1024));
    let Some((method, path, host)) = read_request(&mut r) else { return };
    let (status, ctype, body) = respond(&method, &path, &host, port, state);
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
