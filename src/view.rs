// sd view [--port <n>] [--no-open] (src/view_help.txt, #375, #376): GET / is
// the Flock page, GET /api/flock its data from the watcher's cache. std's
// TcpListener, one request at a time.
//
// Only 127.0.0.1, and only Host 127.0.0.1 or localhost: another site can
// rebind its name to 127.0.0.1. Read-only: before anything writes, add a
// per-run token in the page and an Origin check (#376).

mod flock;

use crate::config::state_dir;
use std::io::{BufRead, BufReader, Write};
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
    for stream in listener.incoming().flatten() {
        serve(stream, port, Path::new(&state));
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
    match path.split('?').next().unwrap_or("") {
        "/" => ("200 OK", "text/html; charset=utf-8", PAGE.into()),
        "/api/flock" => ("200 OK", "application/json", flock::flock(state).to_string()),
        _ => ("404 Not Found", "text/plain", "not found\n".into()),
    }
}

fn serve(stream: TcpStream, port: u16, state: &Path) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut r = BufReader::new(&stream);
    let Some((method, path, host)) = read_request(&mut r) else { return };
    let (status, ctype, body) = respond(&method, &path, &host, port, state);
    let mut w = &stream;
    let _ = write!(
        w,
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}
