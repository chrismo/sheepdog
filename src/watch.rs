// sd watch's subcommands that need no pass (src/watch_help.txt): glyph,
// status, install and uninstall. The pass, refresh and vacuum are still
// libexec/sd-watch's, which hands these to the binary (#201).

use crate::config::{self, state_dir};
use crate::git::{git_out, rebase_head};
use std::fs;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Component, Path};
use std::process::{Command, ExitCode, Stdio};

const HELP: &str = include_str!("watch_help.txt");

const LABEL: &str = "com.sheepdog.watch";
// Earlier labels: a machine still running one would run two watchers.
const OLD_LABELS: [&str; 2] = ["com.agent-rig.pr-watch", "com.pr-tend.pr-watch"];

fn die(msg: impl std::fmt::Display) -> ! {
    eprintln!("sd watch: {msg}");
    std::process::exit(1)
}

fn env_or(var: &str, default: impl FnOnce() -> String) -> String {
    std::env::var(var).ok().filter(|v| !v.is_empty()).unwrap_or_else(default)
}

fn home() -> String {
    std::env::var("HOME").unwrap_or_default()
}

fn launch_agents() -> String {
    env_or("SHEEPDOG_WATCH_LAUNCH_AGENTS", || format!("{}/Library/LaunchAgents", home()))
}

fn plist_file() -> String {
    format!("{}/{LABEL}.plist", launch_agents())
}

/// The glyph a glyphs index (<worktree> TAB <branch> TAB <glyph> rows) holds
/// for <top> on <branch>.
fn glyph_in(index: &str, top: &str, branch: &str) -> Option<String> {
    index.lines().find_map(|l| {
        let mut f = l.split('\t');
        (f.next() == Some(top) && f.next() == Some(branch)).then(|| f.next().unwrap_or("").to_string())
    })
}

/// The glyph the last check left for the worktree holding <path> (default
/// .), while it's still on the branch checked. A shell prompt runs it, so it
/// asks neither sd state nor GitHub.
fn glyph_of(path: &str) -> Option<String> {
    let mut dir = if path.is_empty() { "." } else { path };
    if !Path::new(dir).is_dir() {
        dir = Path::new(dir).parent().and_then(Path::to_str).filter(|d| !d.is_empty()).unwrap_or(".");
    }
    let top = git_out(dir, &["rev-parse", "--show-toplevel"])?;
    let branch = match git_out(&top, &["symbolic-ref", "-q", "HEAD"]) {
        Some(r) => r.strip_prefix("refs/heads/").unwrap_or(&r).to_string(),
        None => rebase_head(&top)?,
    };
    glyph_in(&fs::read_to_string(format!("{}/glyphs", state_dir())).ok()?, &top, &branch)
}

fn xml(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn plist(prog: &str, interval: &str, path: &str, log: &str) -> String {
    let (prog, path, log) = (xml(prog), xml(path), xml(log));
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{LABEL}</string>
  <key>ProgramArguments</key><array><string>{prog}</string><string>watch</string></array>
  <key>StartInterval</key><integer>{interval}</integer>
  <key>RunAtLoad</key><true/>
  <key>AbandonProcessGroup</key><true/>
  <key>EnvironmentVariables</key>
  <dict>
    <key>PATH</key><string>{path}</string>
  </dict>
  <key>StandardOutPath</key><string>{log}</string>
  <key>StandardErrorPath</key><string>{log}</string>
</dict>
</plist>
"#
    )
}

/// The job's PATH: <bindir> first unless it's ~/.local/bin, which stays on
/// PATH anyway (bash5 and claude are linked there), and cargo's directory
/// last unless it's already on it.
fn job_path(home: &str, bindir: &str, cargo: Option<&str>) -> String {
    let mut path = format!("{home}/.local/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin");
    if bindir != format!("{home}/.local/bin") {
        path = format!("{bindir}:{path}");
    }
    if let Some(dir) = cargo.map(|c| c.rsplit_once('/').map_or(c, |(d, _)| d)) {
        if !format!(":{path}:").contains(&format!(":{dir}:")) {
            path = format!("{path}:{dir}");
        }
    }
    path
}

fn whole_seconds(s: &str) -> bool {
    s.starts_with(|c: char| ('1'..='9').contains(&c)) && s.chars().all(|c| c.is_ascii_digit())
}

fn executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// command -v <cmd>: <cmd> itself when it names a path, else the first
/// executable of that name on PATH.
fn command_v(cmd: &str) -> Option<String> {
    if cmd.contains('/') {
        return executable(Path::new(cmd)).then(|| cmd.to_string());
    }
    let path = std::env::var("PATH").unwrap_or_default();
    path.split(':').map(|d| format!("{}/{cmd}", if d.is_empty() { "." } else { d })).find(|p| executable(Path::new(p)))
}

/// cd <dir> && pwd: <dir> made absolute, with its . and .. resolved.
fn absolute(dir: &str) -> String {
    let mut out = std::env::current_dir().unwrap_or_default();
    for c in Path::new(dir).components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(n) => out.push(n),
            _ => {}
        }
    }
    out.to_string_lossy().into_owned()
}

/// The repo's bin/sd, from the binary (target/release/sd).
fn repo_sd() -> String {
    let exe = std::env::current_exe().and_then(fs::canonicalize).unwrap_or_default();
    let sd = exe.ancestors().nth(3).unwrap_or(Path::new("/")).join("bin/sd");
    fs::canonicalize(&sd).unwrap_or(sd).to_string_lossy().into_owned()
}

fn bootout(uid: &str, label: &str) {
    let _ = Command::new("launchctl").args(["bootout", &format!("gui/{uid}/{label}")]).stderr(Stdio::null()).status();
}

fn uid() -> String {
    crate::proc::run("id", &["-u"], None, None).stdout.trim_end().to_string()
}

fn retire_old_labels(uid: &str) {
    for l in OLD_LABELS {
        bootout(uid, l);
        let _ = fs::remove_file(format!("{}/{l}.plist", launch_agents()));
    }
}

fn install() -> ExitCode {
    let plist_file = plist_file();
    let mut bindir = env_or("LOCAL_BIN", || format!("{}/.local/bin", home()));
    // launchd runs the job from /, so a relative LOCAL_BIN is resolved here.
    if !bindir.starts_with('/') && Path::new(&bindir).is_dir() {
        bindir = absolute(&bindir);
    }
    let mut prog = format!("{bindir}/sd");
    // sd rebuilds its Rust half after a pull (bin/sd), and the job has to find cargo.
    let cargo = command_v(&env_or("CARGO", || "cargo".into()));
    if cargo.is_none() {
        eprintln!("sd watch: no cargo on PATH, so the watcher can't rebuild sd after a pull; run ./install.sh by hand");
    }
    let path = job_path(&home(), &bindir, cargo.as_deref());
    if !executable(Path::new(&prog)) {
        prog = repo_sd();
    }
    let interval = config::get_global("settings.interval", false).unwrap_or_else(|lines| {
        for l in lines {
            eprintln!("sd config: {l}");
        }
        die("sd config failed")
    });
    if !whole_seconds(&interval) {
        die(format!("settings.interval must be whole seconds: {interval}"));
    }
    let state = state_dir();
    for d in [launch_agents(), state.clone()] {
        if let Err(e) = fs::create_dir_all(&d) {
            die(format!("can't create {d}: {e}"));
        }
    }
    // 0600 whatever the umask, as the bash's mktemp made it.
    let tmp = format!("{plist_file}.{}.tmp", std::process::id());
    let text = plist(&prog, &interval, &path, &format!("{state}/launchd.log"));
    let wrote = fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp);
    if let Err(e) = wrote.and_then(|mut f| f.write_all(text.as_bytes())) {
        let _ = fs::remove_file(&tmp);
        die(format!("can't write {tmp}: {e}"));
    }
    if !Command::new("plutil").args(["-lint", "-s", &tmp]).status().is_ok_and(|s| s.success()) {
        let _ = fs::remove_file(&tmp);
        die("generated plist is invalid");
    }
    if let Err(e) = fs::rename(&tmp, &plist_file) {
        die(format!("can't write {plist_file}: {e}"));
    }
    let uid = uid();
    retire_old_labels(&uid);
    bootout(&uid, LABEL);
    match Command::new("launchctl").args(["bootstrap", &format!("gui/{uid}"), &plist_file]).status() {
        Ok(s) if s.success() => {}
        Ok(s) => return ExitCode::from(s.code().unwrap_or(1) as u8),
        Err(e) => die(format!("launchctl: {e}")),
    }
    println!("installed {plist_file} (every {interval}s)");
    ExitCode::SUCCESS
}

fn uninstall() -> ExitCode {
    let uid = uid();
    retire_old_labels(&uid);
    bootout(&uid, LABEL);
    let _ = fs::remove_file(plist_file());
    println!("uninstalled {LABEL}");
    ExitCode::SUCCESS
}

pub fn run(argv: &[String]) -> ExitCode {
    let sub = argv.first().map_or("", String::as_str);
    match sub {
        "-h" | "--help" => print!("{HELP}"),
        "glyph" => {
            if argv.len() > 2 {
                die("usage: sd watch glyph [<path>]");
            }
            match argv.get(1).map_or("", String::as_str) {
                "-h" | "--help" => print!("{HELP}"),
                o if o.starts_with('-') => die(format!("unknown option: {o}")),
                p => {
                    if let Some(g) = glyph_of(p) {
                        println!("{g}");
                    }
                }
            }
        }
        "status" => match fs::read_to_string(format!("{}/last-pass", state_dir())) {
            Ok(text) => print!("{}", text.replace('\t', "  ")),
            Err(_) => println!("no pass yet"),
        },
        "install" => return install(),
        "uninstall" => return uninstall(),
        "" | "refresh" | "vacuum" => die("the pass, refresh and vacuum are still in bash: run them through bin/sd"),
        _ => die(format!("unknown command: {sub}")),
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xml_escapes_ampersands_first_then_angle_brackets() {
        assert_eq!(xml("/a&b/<c>"), "/a&amp;b/&lt;c&gt;");
        assert_eq!(xml("&lt;"), "&amp;lt;");
        assert_eq!(xml("plain \"quotes\" 'stay'"), "plain \"quotes\" 'stay'");
    }

    #[test]
    fn plist_is_byte_for_byte_what_the_bash_wrote() {
        let want = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>com.sheepdog.watch</string>
  <key>ProgramArguments</key><array><string>/h/.local/bin/sd</string><string>watch</string></array>
  <key>StartInterval</key><integer>300</integer>
  <key>RunAtLoad</key><true/>
  <key>AbandonProcessGroup</key><true/>
  <key>EnvironmentVariables</key>
  <dict>
    <key>PATH</key><string>/h/.local/bin:/bin</string>
  </dict>
  <key>StandardOutPath</key><string>/s/a&amp;b/launchd.log</string>
  <key>StandardErrorPath</key><string>/s/a&amp;b/launchd.log</string>
</dict>
</plist>
"#;
        assert_eq!(plist("/h/.local/bin/sd", "300", "/h/.local/bin:/bin", "/s/a&b/launchd.log"), want);
    }

    #[test]
    fn job_path_leads_with_a_local_bin_that_isnt_the_default_and_ends_with_cargos_dir() {
        let base = "/h/.local/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin";
        assert_eq!(job_path("/h", "/h/.local/bin", None), base);
        assert_eq!(job_path("/h", "/lb", None), format!("/lb:{base}"));
        assert_eq!(job_path("/h", "/h/.local/bin", Some("/c/bin/cargo")), format!("{base}:/c/bin"));
        assert_eq!(job_path("/h", "/lb", Some("/lb/cargo")), format!("/lb:{base}"));
        assert_eq!(job_path("/h", "/h/.local/bin", Some("/usr/bin/cargo")), base);
    }

    #[test]
    fn whole_seconds_takes_positive_integers_only() {
        assert!(whole_seconds("300"));
        assert!(whole_seconds("1"));
        for s in ["", "0", "05", "5m", "-5", "1.5", " 5"] {
            assert!(!whole_seconds(s), "{s:?}");
        }
    }

    #[test]
    fn glyph_in_matches_worktree_and_branch_both() {
        let index = "/r\tfeat\t✅\n/r-wt\tother\t❌\n/x/wt\tfeat\t⏳\n";
        assert_eq!(glyph_in(index, "/r", "feat").as_deref(), Some("✅"));
        assert_eq!(glyph_in(index, "/r-wt", "other").as_deref(), Some("❌"));
        assert_eq!(glyph_in(index, "/r", "other"), None);
        assert_eq!(glyph_in(index, "/r-w", "other"), None);
        assert_eq!(glyph_in("", "/r", "feat"), None);
    }

    #[test]
    fn glyph_in_takes_the_first_row_that_matches() {
        assert_eq!(glyph_in("/r\tfeat\t✅\n/r\tfeat\t❌\n", "/r", "feat").as_deref(), Some("✅"));
    }
}
