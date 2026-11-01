// git, run in a directory (git -C), and what the verbs ask of it. The ones
// with no directory run here.

use crate::proc::{self, Out};
use regex::Regex;
use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::OnceLock;

/// git <args> in <dir> with env vars, or with its output somewhere other
/// than an Out.
pub struct Git(Command);

impl Git {
    pub fn new(dir: &str, args: &[&str]) -> Git {
        let mut c = Command::new("git");
        c.arg("-C").arg(dir).args(args);
        Git(c)
    }

    pub fn env(mut self, k: &str, v: impl AsRef<OsStr>) -> Git {
        self.0.env(k, v);
        self
    }

    pub fn run(self) -> Out {
        proc::capture(self.0, None)
    }

    /// Both streams on sd's stderr, so in git's order.
    pub fn on_stderr(self) -> bool {
        proc::on_stderr(self.0).is_ok_and(|s| s.success())
    }

    pub fn inherit(mut self) -> bool {
        self.0.status().is_ok_and(|s| s.success())
    }
}

pub fn git(dir: &str, args: &[&str]) -> Out {
    git_input(dir, args, None)
}

pub fn git_input(dir: &str, args: &[&str], input: Option<&[u8]>) -> Out {
    let mut a = vec!["-C", dir];
    a.extend_from_slice(args);
    proc::run("git", &a, None, input)
}

/// git <args> in <dir> that talks to a remote: never prompts, and given up
/// on after <secs> (proc::timeout).
pub fn git_timed(dir: &str, secs: u64, args: &[&str]) -> Out {
    let mut c = Command::new("git");
    c.arg("-C").arg(dir).args(args).env("GIT_TERMINAL_PROMPT", "0");
    proc::timeout(secs, c, false)
}

pub fn git_out(dir: &str, args: &[&str]) -> Option<String> {
    let o = git(dir, args);
    o.ok.then(|| strip_nl(o.stdout))
}

pub fn strip_nl(mut s: String) -> String {
    while s.ends_with('\n') {
        s.pop();
    }
    s
}

pub fn git_ok(dir: &str, args: &[&str]) -> bool {
    git(dir, args).ok
}

pub fn in_repo() -> bool {
    git_ok(".", &["rev-parse", "--git-dir"])
}

pub fn branch_exists(b: &str) -> bool {
    branch_in(".", b)
}

pub fn branch_in(dir: &str, b: &str) -> bool {
    git_ok(dir, &["rev-parse", "-q", "--verify", &format!("refs/heads/{b}")])
}

pub fn common_dir(dir: &str) -> Option<String> {
    git_out(dir, &["rev-parse", "--path-format=absolute", "--git-common-dir"])
}

/// Mid-rebase, the branch being rebased (restack --resolve runs detached, #179).
pub fn current_branch(dir: &str) -> Option<String> {
    match git_out(dir, &["branch", "--show-current"]) {
        Some(b) if !b.is_empty() => Some(b),
        _ => rebase_head(dir),
    }
}

/// The branch a rebase paused in <dir> is rebasing.
pub fn rebase_head(dir: &str) -> Option<String> {
    let gd = git_out(dir, &["rev-parse", "--absolute-git-dir"])?;
    let head = ["rebase-merge", "rebase-apply"]
        .iter()
        .find_map(|d| fs::read_to_string(format!("{gd}/{d}/head-name")).ok())?;
    head.trim_end().strip_prefix("refs/heads/").map(str::to_string)
}

pub fn rev(dir: &str, r: &str) -> String {
    git_out(dir, &["rev-parse", "-q", "--verify", &format!("{r}^{{commit}}")]).unwrap_or_default()
}

/// The object <r> names, whatever its type (a record's blob), else "".
pub fn oid(dir: &str, r: &str) -> String {
    git_out(dir, &["rev-parse", "-q", "--verify", r]).unwrap_or_default()
}

pub fn is_ancestor(dir: &str, a: &str, b: &str) -> bool {
    git_ok(dir, &["merge-base", "--is-ancestor", a, b])
}

pub fn count(dir: &str, args: &[&str]) -> i64 {
    let mut a = vec!["rev-list", "--count"];
    a.extend_from_slice(args);
    git_out(dir, &a).and_then(|s| s.trim().parse().ok()).unwrap_or(0)
}

/// -U0: an upstream edit next to our lines must not read as a changed patch.
pub fn patch_id(dir: &str, a: &str, b: &str) -> String {
    let d = git(dir, &["diff", "-U0", a, b]);
    let p = git_input(dir, &["patch-id", "--stable"], Some(d.stdout.as_bytes()));
    p.stdout.split(' ').next().unwrap_or("").trim_end_matches('\n').to_string()
}

pub fn show_ref(dir: &str, r: &str) -> bool {
    git_ok(dir, &["show-ref", "-q", "--verify", r])
}

/// One entry of git worktree list --porcelain. branch is the branch checked
/// out there; a detached worktree paused mid-rebase has the one being rebased
/// and rebasing set, which worktrees fills in and parse_worktrees doesn't.
#[derive(Debug, Default, PartialEq)]
pub struct Worktree {
    pub path: String,
    pub head: String,
    pub branch: Option<String>,
    pub detached: bool,
    pub rebasing: bool,
}

pub fn parse_worktrees(porcelain: &str) -> Vec<Worktree> {
    let mut out: Vec<Worktree> = Vec::new();
    for l in porcelain.lines() {
        if let Some(p) = l.strip_prefix("worktree ") {
            out.push(Worktree { path: p.to_string(), ..Worktree::default() });
            continue;
        }
        let Some(w) = out.last_mut() else { continue };
        if let Some(h) = l.strip_prefix("HEAD ") {
            w.head = h.to_string();
        } else if let Some(b) = l.strip_prefix("branch ") {
            w.branch = Some(b.strip_prefix("refs/heads/").unwrap_or(b).to_string());
        } else if l == "detached" {
            w.detached = true;
        }
    }
    out
}

/// <dir>'s repo's worktrees, the main one first.
pub fn worktrees(dir: &str) -> Vec<Worktree> {
    let mut ws = parse_worktrees(&git(dir, &["worktree", "list", "--porcelain"]).stdout);
    for w in ws.iter_mut().filter(|w| w.detached) {
        w.branch = rebase_head(&w.path);
        w.rebasing = w.branch.is_some();
    }
    ws
}

/// The worktree that has refs/heads/<b> checked out, if any; not one paused
/// mid-rebase of it.
pub fn worktree_for(dir: &str, b: &str) -> String {
    let list = git(dir, &["worktree", "list", "--porcelain"]).stdout;
    parse_worktrees(&list).into_iter().find(|w| w.branch.as_deref() == Some(b)).map_or(String::new(), |w| w.path)
}

pub fn main_worktree(dir: &str) -> String {
    let list = git(dir, &["worktree", "list", "--porcelain"]).stdout;
    parse_worktrees(&list).into_iter().next().map_or(String::new(), |w| w.path)
}

/// A hash of what's uncommitted in <wt>, untracked files' contents included,
/// made without writing objects or refreshing the index: sd act records it
/// with a question, and sd state compares it.
pub fn worktree_mark(wt: &str) -> Option<String> {
    let mut buf: Vec<u8> = Vec::new();
    let st = Command::new("git")
        .args(["-C", wt, "--no-optional-locks", "status", "--porcelain", "-z", "--untracked-files=all"])
        .stderr(Stdio::null())
        .output()
        .ok()?;
    buf.extend_from_slice(&st.stdout);
    if st.status.success() {
        let d = Command::new("git")
            .args(["-C", wt, "--no-optional-locks", "diff", "--binary", "--no-color", "--no-ext-diff", "--no-textconv", "HEAD"])
            .stderr(Stdio::null())
            .output()
            .ok()?;
        buf.extend_from_slice(&d.stdout);
        if d.status.success() {
            let o = Command::new("git")
                .args(["-C", wt, "ls-files", "-o", "-z", "--exclude-standard"])
                .stderr(Stdio::null())
                .output()
                .ok()?;
            let paths: String = o
                .stdout
                .split(|b| *b == 0)
                .filter(|f| !f.is_empty())
                .map(|f| String::from_utf8_lossy(f).into_owned())
                .filter(|f| !f.contains('\n') && Path::new(wt).join(f).is_file())
                .map(|f| f + "\n")
                .collect();
            let h = git_input(wt, &["hash-object", "--stdin-paths"], Some(paths.as_bytes()));
            buf.extend_from_slice(h.stdout.as_bytes());
        }
    }
    let h = git_input(wt, &["hash-object", "--stdin"], Some(&buf));
    h.ok.then(|| strip_nl(h.stdout))
}

/// What the remote says its HEAD is, else refs/remotes/<remote>/HEAD, else main.
pub fn remote_default(dir: &str, remote: &str) -> String {
    let o = git_timed(dir, 10, &["ls-remote", "--symref", remote, "HEAD"]);
    let out = if o.timed_out { String::new() } else { o.stdout };
    let from_remote = out
        .lines()
        .find_map(|l| l.strip_prefix("ref: refs/heads/").and_then(|r| r.strip_suffix("\tHEAD")))
        .map(str::to_string);
    let r = from_remote.unwrap_or_else(|| {
        let r = git_out(dir, &["symbolic-ref", "-q", "--short", &format!("refs/remotes/{remote}/HEAD")]).unwrap_or_default();
        r.strip_prefix(&format!("{remote}/")).unwrap_or(&r).to_string()
    });
    if r.is_empty() {
        "main".into()
    } else {
        r
    }
}

/// host/owner/repo, the form GH_REPO takes,
/// from a remote's URL (scp-style, ssh://, https://); None for a local path
/// or file:// URL.
pub fn gh_repo_of(url: &str) -> Option<String> {
    static SCHEMED: OnceLock<Regex> = OnceLock::new();
    static SCP: OnceLock<Regex> = OnceLock::new();
    if url.starts_with("file:") {
        return None;
    }
    let u = url.strip_suffix('/').unwrap_or(url);
    let u = u.strip_suffix(".git").unwrap_or(u);
    let schemed = SCHEMED.get_or_init(|| Regex::new(r"^[a-z+]+://([^@/]+@)?([^/:]+)(:[0-9]+)?/([^/]+)/([^/]+)$").unwrap());
    let scp = SCP.get_or_init(|| Regex::new(r"^([^@/:]+@)?([^/:]+):([^/]+)/([^/]+)$").unwrap());
    if let Some(c) = schemed.captures(u) {
        return Some(format!("{}/{}/{}", &c[2], &c[4], &c[5]));
    }
    scp.captures(u).map(|c| format!("{}/{}/{}", &c[2], &c[3], &c[4]))
}

/// gh_repo_of <remote>'s URL, for the caller to export as GH_REPO.
pub fn gh_repo(dir: &str, remote: &str) -> Option<String> {
    gh_repo_of(&git_out(dir, &["remote", "get-url", remote])?)
}

/// How many commits of <sha> no reflog entry reaches.
pub fn commits_not_ours(dir: &str, reflog: &str, sha: &str) -> Option<i64> {
    let input: String = reflog.lines().filter(|l| !l.is_empty()).map(|l| format!("^{l}\n")).collect();
    let o = git_input(dir, &["rev-list", "--count", "--stdin", sha], Some(input.as_bytes()));
    o.stdout.trim().parse().ok()
}

/// Every commit in <range> was authored and committed by this repo's user.
pub fn made_here(dir: &str, range: &str) -> bool {
    let Some(me) = git_out(dir, &["config", "user.email"]).filter(|m| !m.is_empty()) else { return false };
    let Some(people) = git_out(dir, &["log", "--format=%ae%n%ce", range]) else { return false };
    people.lines().all(|p| p == me)
}

/// The branch <b> was made from when no one recorded it (a plain git branch
/// from another's tip), found by the commits they share that aren't on
/// <default>, local or remote. Ok((name, base)), base being where they
/// forked. Err((1, "")): none; Err((2, names)): a tie; Err((3, names)): the
/// ones found have commits of their own past the fork too, so git can't say
/// which was made from which (a parent that moved on looks like a child).
/// Git only, and bounded for sd state: candidates are local branches and
/// their remote-tracking refs that hold <b>'s oldest commit past the default,
/// two rev-lists and a merge-base each, plus one read of <b>'s reflog. A
/// branch holding all of <b>'s commits is its child, not its parent, and so
/// is one whose sd-parent names <b>, and one forked where <b>'s reflog has it
/// arrive other than by being made, renamed, reset or fast-forwarded there (a
/// child or a backup). At equal shared commits, one with none of its own
/// beats one with some: a sibling shares the parent's commits too.
pub fn stacked_on(dir: &str, b: &str, remote: &str, default: &str) -> Result<(String, String), (i32, String)> {
    let def = format!("refs/remotes/{remote}/{default}");
    if b == default {
        return Err((1, String::new()));
    }
    let mut not = vec![format!("^{def}")];
    if show_ref(dir, &format!("refs/heads/{default}")) {
        not.push(format!("^refs/heads/{default}"));
    }
    let head = format!("refs/heads/{b}");
    let mut a: Vec<&str> = vec!["rev-list", "--topo-order", "--reverse", &head];
    a.extend(not.iter().map(String::as_str));
    let Some(oldest) = git_out(dir, &a) else { return Err((1, String::new())) };
    let oldest = oldest.lines().next().unwrap_or("").to_string();
    if oldest.is_empty() {
        return Err((1, String::new()));
    }
    let mut a: Vec<&str> = vec![&head];
    a.extend(not.iter().map(String::as_str));
    let total = count(dir, &a);
    let locals: HashSet<String> = git(dir, &["for-each-ref", "--format=%(refname:short)", "refs/heads"])
        .stdout
        .lines()
        .map(str::to_string)
        .collect();
    let mut reached: HashSet<String> = HashSet::new();
    for l in git(dir, &["reflog", "show", "--format=%H %gs", &head, "--"]).stdout.lines() {
        let (sha, msg) = l.split_once(' ').unwrap_or((l, ""));
        let made = msg.starts_with("branch: Created from ")
            || msg.starts_with("Branch: renamed ")
            || msg.starts_with("reset: moving to ")
            || msg.starts_with("branch: Reset to ")
            || msg.ends_with(": Fast-forward");
        if !made {
            reached.insert(sha.to_string());
        }
    }
    let (mut best, mut best_base, mut best_score, mut best_theirs) = (String::new(), String::new(), 0i64, 0i64);
    let mut tie: Vec<String> = Vec::new();
    let refs = git(dir, &["for-each-ref", "--contains", &oldest, "--format=%(refname)", "refs/heads", &format!("refs/remotes/{remote}")]).stdout;
    for r in refs.lines() {
        let name = r.strip_prefix("refs/heads/").unwrap_or(r);
        let name = name.strip_prefix(&format!("refs/remotes/{remote}/")).unwrap_or(name).to_string();
        if name == b || name == default || !locals.contains(&name) {
            continue;
        }
        let excl = format!("^{r}");
        let mut a: Vec<&str> = vec![&head];
        a.extend(not.iter().map(String::as_str));
        a.push(&excl);
        let own = count(dir, &a);
        if own <= 0 {
            continue;
        }
        if git_out(dir, &["config", "--get", &format!("branch.{name}.sd-parent")]).as_deref() == Some(b) {
            continue;
        }
        let excl = format!("^refs/heads/{b}");
        let mut a: Vec<&str> = vec![r];
        a.extend(not.iter().map(String::as_str));
        a.push(&excl);
        let theirs = count(dir, &a);
        let fork = git_out(dir, &["merge-base", &head, r]).unwrap_or_default();
        if reached.contains(&fork) {
            continue;
        }
        let score = (total - own) * 2 + i64::from(theirs == 0);
        if score > best_score {
            (best, best_base, best_score, best_theirs) = (name, fork, score, theirs);
            tie.clear();
        } else if score == best_score && name != best && !tie.contains(&name) {
            tie.push(name);
        }
    }
    if best.is_empty() {
        return Err((1, String::new()));
    }
    if best_theirs > 0 || !tie.is_empty() {
        let mut names = vec![best];
        names.extend(tie);
        names.sort();
        return Err((if best_theirs == 0 { 2 } else { 3 }, names.join(" ")));
    }
    Ok((best, best_base))
}

/// Why stacked_on found <names> unclear (3), and the git config either way.
pub fn stacked_unclear(b: &str, names: &str, default: &str) -> String {
    let name = if names.contains(' ') { "<name>" } else { names };
    format!(
        "shares commits with {}, and each has its own since, so git can't tell which, if either, was made from the other: git config branch.{b}.sd-parent {name} if {b} was, or git config branch.{b}.sd-parent {default} if not",
        names.replace(' ', " or ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{production_lines, Repo};

    impl Repo {
        fn sha(&self, r: &str) -> String {
            self.git(&["rev-parse", r])
        }

        /// c <branch> <msg>: a commit on <branch>, left checked out.
        fn c(&self, b: &str, msg: &str) {
            self.git(&["switch", "-q", b]);
            self.git(&["commit", "-q", "--allow-empty", "-m", msg]);
        }

        fn stack_setup(&self) {
            self.git(&["switch", "-q", "main"]);
            self.git(&["update-ref", "refs/remotes/origin/main", "main"]);
            self.git(&["branch", "-qD", "feat"]);
            self.git(&["switch", "-qc", "par"]);
            self.git(&["commit", "-q", "--allow-empty", "-m", "p1"]);
            self.git(&["commit", "-q", "--allow-empty", "-m", "p2"]);
            self.git(&["switch", "-qc", "kid"]);
            self.git(&["commit", "-q", "--allow-empty", "-m", "k1"]);
        }

        /// stacked_on <b> origin main, as bats' run sees it: (status, output).
        fn stacked_on(&self, b: &str) -> (i32, String) {
            match stacked_on(&self.dir, b, "origin", "main") {
                Ok((name, base)) => (0, format!("{name} {base}")),
                Err(e) => e,
            }
        }
    }

    // stacked_on's tests were lib/sheepdog.bats's, title for title.

    // @test "stacked_on: a branch made from another's tip names it and its tip"
    #[test]
    fn stacked_on_a_branch_made_from_anothers_tip_names_it_and_its_tip() {
        let r = Repo::new();
        r.stack_setup();
        assert_eq!(r.stacked_on("kid"), (0, format!("par {}", r.sha("par"))));
    }

    // @test "stacked_on: the parent moved on since; named, but either way round"
    #[test]
    fn stacked_on_the_parent_moved_on_since_named_but_either_way_round() {
        let r = Repo::new();
        r.stack_setup();
        r.c("par", "p3");
        assert_eq!(r.stacked_on("kid"), (3, "par".into()));
    }

    // @test "stacked_on: a moved parent made the fork commit itself, so its child isn't its parent"
    #[test]
    fn stacked_on_a_moved_parent_made_the_fork_commit_itself_so_its_child_isnt_its_parent() {
        let r = Repo::new();
        r.stack_setup();
        r.c("par", "p3");
        assert_eq!(r.stacked_on("par"), (1, String::new()));
    }

    // @test "stacked_on: a backup of the branch taken before its last commit isn't its parent"
    #[test]
    fn stacked_on_a_backup_of_the_branch_taken_before_its_last_commit_isnt_its_parent() {
        let r = Repo::new();
        r.stack_setup();
        r.git(&["branch", "kid-bak", "kid"]);
        r.c("kid", "k2");
        assert_eq!(r.stacked_on("kid"), (0, format!("par {}", r.sha("par"))));
    }

    // @test "stacked_on: a branch renamed right after it was made still names its parent"
    #[test]
    fn stacked_on_a_branch_renamed_right_after_it_was_made_still_names_its_parent() {
        let r = Repo::new();
        r.stack_setup();
        r.git(&["switch", "-q", "par"]);
        r.git(&["switch", "-qc", "tmp"]);
        r.git(&["branch", "-m", "tmp", "kid2"]);
        r.git(&["commit", "-q", "--allow-empty", "-m", "k2"]);
        assert_eq!(r.stacked_on("kid2"), (0, format!("par {}", r.sha("par"))));
    }

    // @test "stacked_on: a backup taken after the branch was rebased isn't its parent"
    #[test]
    fn stacked_on_a_backup_taken_after_the_branch_was_rebased_isnt_its_parent() {
        let r = Repo::new();
        r.stack_setup();
        r.c("par", "p3");
        r.git(&["switch", "-q", "kid"]);
        r.git(&["rebase", "-q", "par"]);
        r.git(&["branch", "kid-bak", "kid"]);
        r.c("kid", "k2");
        assert_eq!(r.stacked_on("kid"), (0, format!("par {}", r.sha("par"))));
    }

    // @test "stacked_on: siblings that each moved on tie without a direction"
    #[test]
    fn stacked_on_siblings_that_each_moved_on_tie_without_a_direction() {
        let r = Repo::new();
        r.stack_setup();
        r.git(&["switch", "-q", "par"]);
        r.git(&["switch", "-qc", "sib1"]);
        r.git(&["commit", "-q", "--allow-empty", "-m", "s1"]);
        r.git(&["switch", "-q", "par"]);
        r.git(&["switch", "-qc", "sib2"]);
        r.git(&["commit", "-q", "--allow-empty", "-m", "s2"]);
        r.git(&["switch", "-q", "kid"]);
        r.git(&["branch", "-qD", "par"]);
        assert_eq!(r.stacked_on("kid"), (3, "sib1 sib2".into()));
    }

    // @test "stacked_on: a branch reset onto its parent's tip still names the parent"
    #[test]
    fn stacked_on_a_branch_reset_onto_its_parents_tip_still_names_the_parent() {
        let r = Repo::new();
        r.stack_setup();
        r.git(&["switch", "-q", "main"]);
        r.git(&["switch", "-qc", "kid2"]);
        r.git(&["reset", "-q", "--hard", "par"]);
        r.git(&["commit", "-q", "--allow-empty", "-m", "k2"]);
        assert_eq!(r.stacked_on("kid2"), (0, format!("par {}", r.sha("par"))));
    }

    // @test "stacked_on: a recorded child with no commits of its own isn't its parent's parent, reflog or not"
    #[test]
    fn stacked_on_a_recorded_child_with_no_commits_of_its_own_isnt_its_parents_parent_reflog_or_not() {
        let r = Repo::new();
        r.stack_setup();
        r.git(&["switch", "-q", "par"]);
        r.git(&["branch", "-qD", "kid"]);
        r.git(&["branch", "fresh", "par"]);
        r.git(&["config", "branch.fresh.sd-parent", "par"]);
        r.c("par", "p3");
        r.git(&["reflog", "expire", "--expire=all", "--all"]);
        assert_eq!(r.stacked_on("par"), (1, String::new()));
    }

    // @test "stacked_on: a moved parent whose commits came by fast-forward can't tell its child from a parent"
    #[test]
    fn stacked_on_a_moved_parent_whose_commits_came_by_fast_forward_cant_tell_its_child_from_a_parent() {
        let r = Repo::new();
        r.git(&["update-ref", "refs/remotes/origin/main", "main"]);
        r.git(&["switch", "-qc", "up"]);
        r.git(&["commit", "-q", "--allow-empty", "-m", "u1"]);
        r.git(&["switch", "-q", "main"]);
        r.git(&["switch", "-qc", "mine"]);
        r.git(&["merge", "-q", "--ff-only", "up"]);
        r.git(&["branch", "-qD", "up"]);
        r.git(&["switch", "-qc", "kid"]);
        r.git(&["commit", "-q", "--allow-empty", "-m", "k1"]);
        r.c("mine", "m2");
        assert_eq!(r.stacked_on("mine"), (3, "kid".into()));
    }

    // @test "stacked_on: without a reflog, a moved parent still can't tell its child from a parent"
    #[test]
    fn stacked_on_without_a_reflog_a_moved_parent_still_cant_tell_its_child_from_a_parent() {
        let r = Repo::new();
        r.stack_setup();
        r.c("par", "p3");
        r.git(&["reflog", "expire", "--expire=all", "--all"]);
        assert_eq!(r.stacked_on("par"), (3, "kid".into()));
    }

    // @test "stacked_on: a moved parent skips a child that recorded it as its parent"
    #[test]
    fn stacked_on_a_moved_parent_skips_a_child_that_recorded_it_as_its_parent() {
        let r = Repo::new();
        r.stack_setup();
        r.c("par", "p3");
        r.git(&["config", "branch.kid.sd-parent", "par"]);
        assert_eq!(r.stacked_on("par"), (1, String::new()));
    }

    // @test "stacked_on: a moved parent whose remote copy is still at the fork is the parent"
    #[test]
    fn stacked_on_a_moved_parent_whose_remote_copy_is_still_at_the_fork_is_the_parent() {
        let r = Repo::new();
        r.stack_setup();
        let fork = r.sha("par");
        r.git(&["update-ref", "refs/remotes/origin/par", "par"]);
        r.c("par", "p3");
        assert_eq!(r.stacked_on("kid"), (0, format!("par {fork}")));
    }

    // @test "stacked_on: a sibling made from the same tip loses to the parent"
    #[test]
    fn stacked_on_a_sibling_made_from_the_same_tip_loses_to_the_parent() {
        let r = Repo::new();
        r.stack_setup();
        r.git(&["switch", "-q", "par"]);
        r.git(&["switch", "-qc", "sib"]);
        r.git(&["commit", "-q", "--allow-empty", "-m", "s1"]);
        assert_eq!(r.stacked_on("kid"), (0, format!("par {}", r.sha("par"))));
    }

    // @test "stacked_on: the default branch is stacked on nothing, unpushed commits or not"
    #[test]
    fn stacked_on_the_default_branch_is_stacked_on_nothing_unpushed_commits_or_not() {
        let r = Repo::new();
        r.git(&["update-ref", "refs/remotes/origin/main", "main"]);
        r.c("main", "m1");
        r.git(&["switch", "-qc", "topic"]);
        r.git(&["commit", "-q", "--allow-empty", "-m", "t1"]);
        r.c("main", "m2");
        assert_eq!(r.stacked_on("main"), (1, String::new()));
    }

    // @test "stacked_on: a local parent rewritten since is found by its remote-tracking ref"
    #[test]
    fn stacked_on_a_local_parent_rewritten_since_is_found_by_its_remote_tracking_ref() {
        let r = Repo::new();
        r.stack_setup();
        r.git(&["update-ref", "refs/remotes/origin/par", "par"]);
        r.git(&["switch", "-q", "par"]);
        r.git(&["commit", "-q", "--amend", "--allow-empty", "-m", "p2b"]);
        assert_eq!(r.stacked_on("kid"), (0, format!("par {}", r.sha("origin/par"))));
    }

    // @test "stacked_on: only local branches and their remote-tracking refs are candidates"
    #[test]
    fn stacked_on_only_local_branches_and_their_remote_tracking_refs_are_candidates() {
        let r = Repo::new();
        r.stack_setup();
        r.git(&["update-ref", "refs/remotes/origin/par", "par"]);
        r.git(&["branch", "-qD", "par"]);
        assert_eq!(r.stacked_on("kid").0, 1);
    }

    // @test "stacked_on: a parent squash-merged but still local is still found"
    #[test]
    fn stacked_on_a_parent_squash_merged_but_still_local_is_still_found() {
        let r = Repo::new();
        r.stack_setup();
        r.git(&["switch", "-q", "main"]);
        r.git(&["merge", "-q", "--squash", "par"]);
        r.git(&["commit", "-q", "--allow-empty", "-m", "par (#1)"]);
        r.git(&["update-ref", "refs/remotes/origin/main", "main"]);
        assert_eq!(r.stacked_on("kid"), (0, format!("par {}", r.sha("par"))));
    }

    // @test "stacked_on: a branch that merged the default branch in isn't stacked"
    #[test]
    fn stacked_on_a_branch_that_merged_the_default_branch_in_isnt_stacked() {
        let r = Repo::new();
        r.git(&["update-ref", "refs/remotes/origin/main", "main"]);
        r.c("main", "m1");
        r.git(&["update-ref", "refs/remotes/origin/main", "main"]);
        r.git(&["branch", "other", "main"]);
        r.git(&["switch", "-q", "feat"]);
        r.git(&["merge", "-q", "--no-edit", "main"]);
        assert_eq!(r.stacked_on("feat").0, 1);
    }

    // @test "stacked_on: sharing only commits already on the default branch isn't stacked"
    #[test]
    fn stacked_on_sharing_only_commits_already_on_the_default_branch_isnt_stacked() {
        let r = Repo::new();
        r.git(&["update-ref", "refs/remotes/origin/main", "main"]);
        r.git(&["branch", "older", "main"]);
        r.c("main", "m1");
        r.git(&["update-ref", "refs/remotes/origin/main", "main"]);
        r.git(&["switch", "-qc", "newer"]);
        r.git(&["commit", "-q", "--allow-empty", "-m", "n1"]);
        r.c("main", "m2");
        assert_eq!(r.stacked_on("newer").0, 1);
        assert_eq!(r.stacked_on("feat").0, 1);
    }

    // @test "stacked_on: unpushed commits on the local default branch aren't a parent"
    #[test]
    fn stacked_on_unpushed_commits_on_the_local_default_branch_arent_a_parent() {
        let r = Repo::new();
        r.git(&["update-ref", "refs/remotes/origin/main", "main"]);
        r.c("main", "m1");
        r.git(&["switch", "-qc", "topic"]);
        r.git(&["commit", "-q", "--allow-empty", "-m", "t1"]);
        assert_eq!(r.stacked_on("topic").0, 1);
    }

    // @test "stacked_on: two topics made from unpushed commits on the local default branch aren't stacked"
    #[test]
    fn stacked_on_two_topics_made_from_unpushed_commits_on_the_local_default_branch_arent_stacked() {
        let r = Repo::new();
        r.git(&["update-ref", "refs/remotes/origin/main", "main"]);
        r.c("main", "m1");
        r.git(&["switch", "-qc", "t1"]);
        r.git(&["commit", "-q", "--allow-empty", "-m", "t1"]);
        r.git(&["switch", "-q", "main"]);
        r.git(&["switch", "-qc", "t2"]);
        r.git(&["commit", "-q", "--allow-empty", "-m", "t2"]);
        assert_eq!(r.stacked_on("t1"), (1, String::new()));
    }

    // @test "stacked_on: a branch's own child isn't its parent"
    #[test]
    fn stacked_on_a_branchs_own_child_isnt_its_parent() {
        let r = Repo::new();
        r.stack_setup();
        assert_eq!(r.stacked_on("par"), (1, String::new()));
    }

    // @test "stacked_on: a branch on the default branch alone has none"
    #[test]
    fn stacked_on_a_branch_on_the_default_branch_alone_has_none() {
        let r = Repo::new();
        r.git(&["update-ref", "refs/remotes/origin/main", "main"]);
        assert_eq!(r.stacked_on("feat").0, 1);
    }

    // @test "stacked_on: the nearest of a chain wins"
    #[test]
    fn stacked_on_the_nearest_of_a_chain_wins() {
        let r = Repo::new();
        r.stack_setup();
        r.git(&["switch", "-qc", "grandkid"]);
        r.git(&["commit", "-q", "--allow-empty", "-m", "g1"]);
        assert_eq!(r.stacked_on("grandkid"), (0, format!("kid {}", r.sha("kid"))));
    }

    // @test "stacked_on: two branches sharing as much is ambiguous, and both are named"
    #[test]
    fn stacked_on_two_branches_sharing_as_much_is_ambiguous_and_both_are_named() {
        let r = Repo::new();
        r.stack_setup();
        r.git(&["branch", "twin", "par"]);
        assert_eq!(r.stacked_on("kid"), (2, "par twin".into()));
    }

    // @test "stacked_on: the branch's own remote copy doesn't count"
    #[test]
    fn stacked_on_the_branchs_own_remote_copy_doesnt_count() {
        let r = Repo::new();
        r.stack_setup();
        r.git(&["update-ref", "refs/remotes/origin/kid", "kid"]);
        assert_eq!(r.stacked_on("kid"), (0, format!("par {}", r.sha("par"))));
    }

    #[test]
    fn stacked_unclear_names_one_branch_or_a_placeholder_for_several() {
        assert_eq!(
            stacked_unclear("kid", "par", "main"),
            "shares commits with par, and each has its own since, so git can't tell which, if either, was made from the other: git config branch.kid.sd-parent par if kid was, or git config branch.kid.sd-parent main if not"
        );
        assert_eq!(
            stacked_unclear("kid", "sib1 sib2", "main"),
            "shares commits with sib1 or sib2, and each has its own since, so git can't tell which, if either, was made from the other: git config branch.kid.sd-parent <name> if kid was, or git config branch.kid.sd-parent main if not"
        );
    }

    #[test]
    fn parse_worktrees_reads_branches_detached_heads_and_bare_repos() {
        let text = "worktree /r\nbare\n\nworktree /r/a\nHEAD 1111\nbranch refs/heads/feat/x\n\nworktree /r/b\nHEAD 2222\ndetached\nprunable gitdir file points to non-existent location\n\n";
        let ws = parse_worktrees(text);
        let w = |path: &str, head: &str, branch: Option<&str>, detached: bool| Worktree {
            path: path.into(),
            head: head.into(),
            branch: branch.map(str::to_string),
            detached,
            rebasing: false,
        };
        assert_eq!(ws, vec![w("/r", "", None, false), w("/r/a", "1111", Some("feat/x"), false), w("/r/b", "2222", None, true)]);
        assert!(parse_worktrees("").is_empty());
    }

    #[test]
    fn worktrees_name_the_branch_a_detached_rebase_is_rebasing() {
        let r = Repo::new();
        let wt = format!("{}/../wt", r.dir);
        r.git(&["worktree", "add", "-q", "-b", "side", &wt, "main"]);
        fs::write(format!("{wt}/f"), "side\n").unwrap();
        r.git(&["-C", &wt, "add", "f"]);
        r.git(&["-C", &wt, "commit", "-q", "-m", "side"]);
        r.git(&["switch", "-q", "main"]);
        fs::write(format!("{}/f", r.dir), "main\n").unwrap();
        r.git(&["add", "f"]);
        r.git(&["commit", "-q", "-m", "main"]);
        assert!(!git(&wt, &["rebase", "-q", "main"]).ok, "the rebase stops on its conflict");
        let ws = worktrees(&r.dir);
        let side = ws.iter().find(|w| w.path.ends_with("/wt")).expect("listed");
        assert!(side.detached && side.rebasing);
        assert_eq!(side.branch.as_deref(), Some("side"));
        assert_eq!(ws[0].branch.as_deref(), Some("main"));
        assert!(!ws[0].rebasing);
        assert_eq!(worktree_for(&r.dir, "side"), "");
        assert_eq!(worktree_for(&r.dir, "main"), r.dir);
        assert_eq!(main_worktree(&wt), r.dir);
    }

    // @test "gh_repo_of: [host/]owner/repo from each URL form; nothing for a local path"
    #[test]
    fn gh_repo_of_host_owner_repo_from_each_url_form_nothing_for_a_local_path() {
        let of = |u: &str| gh_repo_of(u).unwrap_or_default();
        assert_eq!(of("git@github.com:me/r.git"), "github.com/me/r");
        assert_eq!(of("git@github.com:me/r"), "github.com/me/r");
        assert_eq!(of("github.com:me/r"), "github.com/me/r");
        assert_eq!(of("ssh://git@github.com/me/r.git"), "github.com/me/r");
        assert_eq!(of("ssh://git@github.com/me/r"), "github.com/me/r");
        assert_eq!(of("ssh://git@ghe.example.com:2222/org/r.git"), "ghe.example.com/org/r");
        assert_eq!(of("https://github.com/me/r.git"), "github.com/me/r");
        assert_eq!(of("https://github.com/me/r"), "github.com/me/r");
        assert_eq!(of("https://user@github.com/me/r/"), "github.com/me/r");
        assert_eq!(gh_repo_of("/tmp/origin.git"), None);
        assert_eq!(gh_repo_of("../up/origin.git"), None);
        assert_eq!(gh_repo_of("file:///tmp/origin.git"), None);
        assert_eq!(gh_repo_of("file://host/up/origin.git"), None);
    }

    #[test]
    fn gh_repo_the_github_repo_a_remotes_url_names_none_for_a_local_path_or_no_such_remote() {
        let r = Repo::new();
        r.git(&["remote", "add", "origin", "https://github.com/upstream/r.git"]);
        r.git(&["remote", "add", "fork", "git@github.com:me/r.git"]);
        r.git(&["remote", "add", "local", "/tmp/origin.git"]);
        assert_eq!(gh_repo(&r.dir, "fork").as_deref(), Some("github.com/me/r"));
        assert_eq!(gh_repo(&r.dir, "origin").as_deref(), Some("github.com/upstream/r"));
        assert_eq!(gh_repo(&r.dir, "local"), None);
        assert_eq!(gh_repo(&r.dir, "nosuch"), None);
    }

    // @test "worktree_mark: an untracked file's contents change the mark"
    #[test]
    fn worktree_mark_an_untracked_files_contents_change_the_mark() {
        let r = Repo::new();
        let u = Path::new(&r.dir).join("u.txt");
        fs::write(&u, "a\n").unwrap();
        let m = worktree_mark(&r.dir).unwrap();
        fs::write(&u, "b\n").unwrap();
        assert_ne!(worktree_mark(&r.dir).unwrap(), m);
    }

    // @test "worktree_mark: marks a worktree with a nested repo and names git quotes, under pipefail"
    #[test]
    fn worktree_mark_marks_a_worktree_with_a_nested_repo_and_names_git_quotes() {
        let r = Repo::new();
        let sub = format!("{}/sub", r.dir);
        r.git(&["init", "-q", &sub]);
        r.git(&["-C", &sub, "commit", "-q", "--allow-empty", "-m", "s"]);
        let q = Path::new(&r.dir).join("q\"t\tb\\x.txt");
        fs::write(&q, "a").unwrap();
        let m = worktree_mark(&r.dir).expect("a mark");
        assert!(Regex::new("^[0-9a-f]{40}$").unwrap().is_match(&m), "{m}");
        fs::write(&q, "b").unwrap();
        assert_ne!(worktree_mark(&r.dir).unwrap(), m);
    }

    /// Test modules may run their own.
    #[test]
    fn git_runs_only_from_git_rs() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut lines = Vec::new();
        production_lines(&src, &mut lines);
        assert!(lines.iter().any(|(f, _, _)| f.ends_with("src/act/start.rs")), "read no src/act/start.rs");
        let runs: Vec<String> = lines
            .iter()
            .filter(|(f, _, l)| !f.ends_with("src/git.rs") && l.contains("\"git\""))
            .map(|(f, n, l)| format!("{}:{n}: {}", f.strip_prefix(&src).unwrap().display(), l.trim()))
            .collect();
        assert!(runs.is_empty(), "git run outside git.rs; run it through git::Git:\n{}", runs.join("\n"));
    }
}
