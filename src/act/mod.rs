// sd act: the verbs that change a branch's PR (src/act_help.txt).

mod address;
mod answer;
mod attach;
mod bookkeeping;
mod ci_fix;
mod commands;
mod describe;
mod follow;
mod github;
mod hold;
mod implement;
mod push;
mod resolve;
mod restack;
mod review;
mod ship;
mod start;
mod step;
pub(crate) use step::plain_words;

use crate::git::{self, git, git_out};
use crate::proc;
use crate::val::{at, on, sj, t};
use serde_json::Value;
use std::process::ExitCode;

const HELP: &str = include_str!("../act_help.txt");

pub fn die(msg: impl std::fmt::Display) -> ! {
    eprintln!("sd act: {msg}");
    std::process::exit(crate::harness::failed_status())
}

fn or_die<T>(r: Result<T, Vec<String>>) -> T {
    r.unwrap_or_else(|lines| {
        for l in lines {
            eprintln!("sd config: {l}");
        }
        die("sd config failed")
    })
}

/// A config value for branch <b>, or die "sd config failed".
pub fn cfg(path: &str, b: &str) -> String {
    or_die(crate::config::get_text(path, b, false))
}

/// The remote sheepdog fetches, pushes and asks GitHub about:
/// settings.remote, from the repo file inside a repo, else the global file.
pub fn repo_remote() -> String {
    or_die(crate::config::get_global("settings.remote", false))
}

/// The flags every verb shares.
#[derive(Debug, Default, PartialEq)]
pub struct Args {
    pub force: bool,
    pub accept: bool,
    pub remove: bool,
    pub no_publish: bool,
    pub resolve: bool,
    pub no_implement: bool,
    pub question: bool,
    pub harness: String,
    pub parent: String,
    pub ids: Vec<String>,
    /// start's --profile, each one given, in order.
    pub profiles: Vec<String>,
    /// The last positional (start's <name>, answer's <branch>); empty for the
    /// current branch. unregister's <path>.
    pub branch: String,
    pub positionals: Vec<String>,
    pub task: String,
    pub answer: String,
    /// follow's <number>.
    pub pr: String,
}

pub fn parse(argv: &[String]) -> Result<Args, String> {
    let mut a = Args::default();
    let mut it = argv.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--force" => a.force = true,
            "--accept" => a.accept = true,
            "--remove" => a.remove = true,
            "--no-publish" => a.no_publish = true,
            "--resolve" => a.resolve = true,
            "--no-implement" => a.no_implement = true,
            "--question" => a.question = true,
            o @ ("--parent" | "--harness" | "--id" | "--profile") => {
                let v = it.next().ok_or(format!("{o} needs a value"))?.clone();
                match o {
                    "--parent" => a.parent = v,
                    "--harness" => a.harness = v,
                    "--profile" => a.profiles.push(v),
                    _ => a.ids.push(v),
                }
            }
            o if o.starts_with('-') => return Err(format!("unknown option: {o}")),
            p => {
                a.branch = p.to_string();
                a.positionals.push(p.to_string());
            }
        }
    }
    Ok(a)
}

impl Args {
    /// The checks once it's known to be in a repo: start's and answer's
    /// positionals, and the flags that are one verb's.
    pub fn check(&mut self, verb: &str) -> Result<(), String> {
        let n = self.positionals.len();
        let second = self.positionals.get(1).cloned().unwrap_or_default();
        if verb == "start" {
            if !(1..=2).contains(&n) {
                return Err("usage: sd act start [--parent <branch>] [--harness <h>] [--profile <name>]... [--no-implement] <name> [<task>]".into());
            }
            (self.branch, self.task) = (self.positionals[0].clone(), second.clone());
        }
        if verb == "answer" {
            if !(1..=2).contains(&n) || self.positionals[0].trim().is_empty() {
                return Err("usage: sd act answer [--force] <text> [<branch>]".into());
            }
            (self.answer, self.branch) = (self.positionals[0].clone(), second);
        }
        if verb == "follow" {
            if n != 1 || self.positionals[0].is_empty() || !self.positionals[0].bytes().all(|c| c.is_ascii_digit()) {
                return Err("usage: sd act follow <number>".into());
            }
            self.pr = self.positionals[0].clone();
            self.branch = follow::branch(&self.pr);
        }
        let one_verb = [
            (self.accept, "--accept", "review"),
            (self.remove, "--remove", "forget"),
            (self.question, "--question", "ack"),
            (!self.profiles.is_empty(), "--profile", "start"),
        ];
        for (set, flag, only) in one_verb {
            if set && verb != only {
                return Err(format!("{flag} is for sd act {only} only"));
            }
        }
        Ok(())
    }
}

fn hostname() -> String {
    git::strip_nl(proc::run("hostname", &[], None, None).stdout)
}

/// branch.<b>.sd-lock is "<pid>@<host> <time>": the attach holding the
/// branch's session open. Stale once that pid is gone from this host; a lock
/// from another host can't be checked, so it never is.
pub fn lock_stale(lock: &str) -> bool {
    let pid = lock.split('@').next().unwrap_or_default();
    let host = lock.split_once('@').map_or(lock, |(_, h)| h);
    let host = host.split(' ').next().unwrap_or_default();
    !pid.is_empty()
        && pid.bytes().all(|c| c.is_ascii_digit())
        && host == hostname()
        && !proc::run("kill", &["-0", pid], None, None).ok
}

/// Clear a stale lock, so every verb's gate sees only live ones.
pub fn lock_sweep(b: &str) {
    let key = format!("branch.{b}.sd-lock");
    let lock = git_out(".", &["config", "--get", &key]).unwrap_or_default();
    if lock.is_empty() || !lock_stale(&lock) {
        return;
    }
    eprintln!("sd act: clearing a stale lock on {b} ({lock})");
    let o = git(".", &["config", "--unset", &key]);
    if !o.ok {
        eprint!("{}", o.stderr);
        std::process::exit(o.code);
    }
}

/// An agent step on an attached branch is refused; --force runs it anyway.
pub fn attached_check(b: &str, doing: &str, force: bool) {
    let lock = git_out(".", &["config", "--get", &format!("branch.{b}.sd-lock")]).unwrap_or_default();
    if lock.is_empty() {
        return;
    }
    if !force {
        die(format!("not {doing}: attached ({lock}). Exit that session, or --force"));
    }
    eprintln!("sd act: warning: {b} is attached ({lock}); running anyway");
}

pub fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}

/// sd state --json <b> as seen from <dir>, in-process (state::observe): Err
/// is what sd state would have said on stderr. In a debug build (cargo test's)
/// with SHEEPDOG_STATE_FIXTURES set, <b>'s answer is that directory's <b>.json
/// instead: the suites drive a verb from a canned state that way. A release
/// build, the one installed, never reads it.
pub fn state_json(dir: &str, b: &str) -> Result<Value, String> {
    let fixtures = if cfg!(debug_assertions) { std::env::var("SHEEPDOG_STATE_FIXTURES") } else { Err(std::env::VarError::NotPresent) };
    match fixtures {
        Ok(fx) if !fx.is_empty() => {
            let f = format!("{fx}/{b}.json");
            let text = std::fs::read_to_string(&f).map_err(|e| format!("sd act: no state fixture {f}: {e}\n"))?;
            crate::config::one_value(&text).ok_or(format!("sd act: state fixture {f} isn't one JSON value\n"))
        }
        _ => {
            // <dir>'s remote, and a config that can't be read is sd state's
            // failure to report, not a reason to exit here.
            let remote = crate::config::get_global_in(dir, "settings.remote", false).map_err(|lines| {
                lines.iter().map(|l| format!("sd config: {l}\n")).collect::<String>() + "sd config failed\n"
            })?;
            crate::state::observe(dir, b, &remote)
        }
    }
}

/// Why sd state found no base: its errors, else a parent that exists only
/// here (parent.unpushed), which sd state reports as a step, not an error
/// (#105).
pub fn no_base_why(state: &Value, remote: &str) -> String {
    let errors = at(state, "errors");
    if errors.as_array().is_some_and(|e| !e.is_empty()) {
        return sj(errors, "; ");
    }
    if on(state, "parent.unpushed") {
        let p = t(state, "parent.name");
        return format!("parent {p} isn't on {remote} yet: ship it first (sd act ship {p})");
    }
    String::new()
}

/// The mechanical verbs that write the worktree or the remote, in their own
/// words: each refuses, --force or not, before it writes anything, while
/// another process's step holds the branch's running marker (#270), so none
/// rests on NEXT or a pass's snapshot. restack --resolve is refused before
/// its rebase starts, so HEAD never moves under the running step.
const WAIT_FOR_STEP: &[(&str, &str)] = &[
    ("restack", "restacking"),
    ("push", "pushing"),
    ("ship", "shipping"),
    ("autofix", "running autofix"),
    ("verify", "running verify"),
    ("fetch", "fetching"),
];

pub fn run(argv: &[String]) -> ExitCode {
    let Some((verb, rest)) = argv.split_first() else {
        eprint!("{HELP}");
        return ExitCode::from(1);
    };
    if verb == "-h" || verb == "--help" {
        print!("{HELP}");
        return ExitCode::SUCCESS;
    }
    let mut a = parse(rest).unwrap_or_else(|e| die(e));
    if !git::in_repo() {
        die("not a git repository");
    }
    let remote = repo_remote();
    // Unset, gh picks among the remotes, and in a fork's clone that's upstream.
    if let Some(r) = git::gh_repo(".", &remote) {
        std::env::set_var("GH_REPO", r);
    }
    a.check(verb).unwrap_or_else(|e| die(e));
    let b = match a.branch.as_str() {
        "" => git_out(".", &["branch", "--show-current"]).unwrap_or_default(),
        b => b.to_string(),
    };
    if b.is_empty() {
        die("detached HEAD; name a branch");
    }
    lock_sweep(&b);
    if follow::AUTHORS.contains(&verb.as_str()) && follow::reviewing(&b) {
        die(format!("{b} is a PR you're reviewing (sd act follow): sd act {verb} is the author's"));
    }
    if let Some((_, doing)) = WAIT_FOR_STEP.iter().find(|(v, _)| v == verb) {
        crate::running::idle(".", &b, doing).unwrap_or_else(|e| die(e));
    }
    match verb.as_str() {
        "push" => push::push(&b, &remote, a.force),
        "verify" => commands::verify(&b),
        "autofix" => commands::autofix(&b),
        "enroll" => bookkeeping::enroll(&b, &remote),
        "unenroll" => bookkeeping::unenroll(&b),
        "forget" => bookkeeping::forget(&b, a.remove),
        "unregister" => bookkeeping::unregister(&a.branch, a.force),
        "ack" => github::ack(&b, &a.ids, a.question),
        "trigger" => github::trigger(&b, &remote, a.force),
        "answer" => answer::answer(&b, &remote, &a),
        "restack" => restack::restack(&b, &remote, &a),
        "retarget" => restack::retarget(&b),
        "review" => review::review(&b, &remote, a.accept, a.force),
        "attach" => attach::attach(&b, &a.harness, a.force),
        "start" => return ExitCode::from(u8::try_from(start::start(&b, &remote, &a)).unwrap_or(1)),
        "fetch" => follow::fetch(&b, &remote),
        "follow" => return ExitCode::from(u8::try_from(follow::follow(&b, &a.pr, &remote)).unwrap_or(1)),
        "implement" => implement::implement(&b, &a),
        "address" => address::address(&b, &remote, &a),
        "describe" => describe::describe(&b, &remote, &a),
        "ship" => ship::ship(&b, &remote, &a),
        "ci-fix" => ci_fix::ci_fix(&b, &remote, &a),
        _ => die(format!("unknown verb: {verb}")),
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    fn parsed(verb: &str, a: &[&str]) -> Result<Args, String> {
        let mut args = parse(&argv(a))?;
        args.check(verb)?;
        Ok(args)
    }

    #[test]
    fn parse_reads_every_flag_and_the_last_positional_is_the_branch() {
        let a = parse(&argv(&[
            "--force", "--accept", "--remove", "--no-publish", "--resolve", "--no-implement", "--question",
            "--parent", "p", "--harness", "pi", "--id", "1", "--id", "2", "--profile", "x", "--profile", "y", "one", "two",
        ]))
        .unwrap();
        assert!(a.force && a.accept && a.remove && a.no_publish && a.resolve && a.no_implement && a.question);
        assert_eq!((a.parent.as_str(), a.harness.as_str()), ("p", "pi"));
        assert_eq!(a.ids, ["1", "2"]);
        assert_eq!(a.profiles, ["x", "y"]);
        assert_eq!(a.branch, "two");
        assert_eq!(a.positionals, ["one", "two"]);
        assert_eq!(parse(&[]).unwrap(), Args::default());
    }

    #[test]
    fn parse_refuses_an_unknown_option_and_a_value_flag_with_no_value() {
        assert_eq!(parse(&argv(&["--nope"])), Err("unknown option: --nope".into()));
        assert_eq!(parse(&argv(&["-"])), Err("unknown option: -".into()));
        assert_eq!(parse(&argv(&["--"])), Err("unknown option: --".into()));
        for f in ["--parent", "--harness", "--id", "--profile"] {
            assert_eq!(parse(&argv(&["feat", f])), Err(format!("{f} needs a value")));
        }
    }

    #[test]
    fn check_refuses_a_flag_on_a_verb_it_isnt_for() {
        assert_eq!(parsed("push", &["--accept"]), Err("--accept is for sd act review only".into()));
        assert_eq!(parsed("enroll", &["--remove"]), Err("--remove is for sd act forget only".into()));
        assert_eq!(parsed("answer", &["--question", "yes"]), Err("--question is for sd act ack only".into()));
        assert_eq!(parsed("implement", &["--profile", "x"]), Err("--profile is for sd act start only".into()));
        assert!(parsed("start", &["--profile", "x", "feat"]).is_ok());
        assert!(parsed("review", &["--accept"]).is_ok());
        assert!(parsed("forget", &["--remove"]).is_ok());
        assert!(parsed("ack", &["--question"]).is_ok());
    }

    #[test]
    fn check_takes_starts_name_and_task() {
        let a = parsed("start", &["--parent", "p", "fix-login", "Fix the login"]).unwrap();
        assert_eq!((a.branch.as_str(), a.task.as_str()), ("fix-login", "Fix the login"));
        let a = parsed("start", &["fix-login"]).unwrap();
        assert_eq!((a.branch.as_str(), a.task.as_str()), ("fix-login", ""));
        let usage = "usage: sd act start [--parent <branch>] [--harness <h>] [--profile <name>]... [--no-implement] <name> [<task>]";
        assert_eq!(parsed("start", &[]), Err(usage.into()));
        assert_eq!(parsed("start", &["a", "b", "c"]), Err(usage.into()));
    }

    #[test]
    fn check_takes_answers_text_and_branch() {
        let a = parsed("answer", &["revert the test", "feat"]).unwrap();
        assert_eq!((a.answer.as_str(), a.branch.as_str()), ("revert the test", "feat"));
        let a = parsed("answer", &["yes"]).unwrap();
        assert_eq!((a.answer.as_str(), a.branch.as_str()), ("yes", ""));
        let usage = "usage: sd act answer [--force] <text> [<branch>]";
        assert_eq!(parsed("answer", &[]), Err(usage.into()));
        assert_eq!(parsed("answer", &[" \t\n"]), Err(usage.into()));
        assert_eq!(parsed("answer", &["a", "b", "c"]), Err(usage.into()));
    }

    #[test]
    fn the_answer_is_answers_own_positional_not_a_flag() {
        assert_eq!(parse(&argv(&["--answer", "yes"])), Err("unknown option: --answer".into()));
        assert_eq!(parsed("implement", &["yes"]).unwrap().answer, "");
    }

    #[test]
    fn check_leaves_other_verbs_positionals_alone() {
        let a = parsed("verify", &["feat"]).unwrap();
        assert_eq!((a.branch.as_str(), a.task.as_str(), a.answer.as_str()), ("feat", "", ""));
    }

    fn dead_pid() -> u32 {
        let mut c = std::process::Command::new("true").spawn().unwrap();
        let pid = c.id();
        c.wait().unwrap();
        pid
    }

    #[test]
    fn lock_stale_only_when_the_pid_is_gone_from_this_host() {
        let host = hostname();
        assert!(!lock_stale(&format!("{}@{host} 2026-10-21T10:00:00Z", std::process::id())));
        assert!(lock_stale(&format!("{}@{host} 2026-10-21T10:00:00Z", dead_pid())));
        // Another host's pid can't be checked from here.
        assert!(!lock_stale(&format!("{}@elsewhere.example 2026-10-21T10:00:00Z", dead_pid())));
    }

    #[test]
    fn lock_stale_never_on_a_lock_it_cant_read() {
        let host = hostname();
        assert!(!lock_stale(""));
        assert!(!lock_stale(&format!("abc@{host} 2026-10-21T10:00:00Z")));
        assert!(!lock_stale(&format!("@{host} 2026-10-21T10:00:00Z")));
        assert!(!lock_stale(&dead_pid().to_string()));
    }

    #[test]
    fn plural_is_bashs() {
        assert_eq!(plural(1, "file"), "1 file");
        assert_eq!(plural(0, "file"), "0 files");
        assert_eq!(plural(3, "commit"), "3 commits");
    }

    #[test]
    fn no_base_why_is_the_errors_else_an_unpushed_parent() {
        let s = serde_json::json!({"errors": ["gh failed", "no PR"], "parent": {"name": "p", "unpushed": true}});
        assert_eq!(no_base_why(&s, "origin"), "gh failed; no PR");
        let s = serde_json::json!({"errors": [], "parent": {"name": "p", "unpushed": true}});
        assert_eq!(no_base_why(&s, "up"), "parent p isn't on up yet: ship it first (sd act ship p)");
        assert_eq!(no_base_why(&serde_json::json!({"parent": {"name": "p"}}), "origin"), "");
    }
}
