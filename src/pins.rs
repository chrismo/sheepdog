// The pins (#342): refs/sheepdog/<b>/<kind>, what sheepdog keeps for a
// branch in git refs. This module is their only writer
// (only_pins_writes_refs_sheepdog_342); what a record holds is its own
// module's.

use crate::git::{self, git, git_input, strip_nl};
use crate::proc::Out;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pin {
    /// Where the branch left its parent: start, enroll and restack pin it.
    Base,
    /// The review pins, and the base each was taken against (#66).
    Local,
    LocalBase,
    Bot,
    BotBase,
    Accepted,
    AcceptedBase,
    /// A followed PR's head as last fetched, and as fetch just fetched it
    /// (#250).
    PrHead,
    PrFetched,
    /// The records, a blob each.
    Findings,
    Question,
    Answer,
    Decisions,
    Failed,
    VerifyRed,
}

impl Pin {
    pub fn kind(self) -> &'static str {
        match self {
            Pin::Base => "base",
            Pin::Local => "local",
            Pin::LocalBase => "local-base",
            Pin::Bot => "bot",
            Pin::BotBase => "bot-base",
            Pin::Accepted => "accepted",
            Pin::AcceptedBase => "accepted-base",
            Pin::PrHead => "pr-head",
            Pin::PrFetched => "pr-fetched",
            Pin::Findings => "findings",
            Pin::Question => "question",
            Pin::Answer => "answer",
            Pin::Decisions => "decisions",
            Pin::Failed => "failed",
            Pin::VerifyRed => "verify-red",
        }
    }
}

pub fn name(b: &str, pin: Pin) -> String {
    format!("refs/sheepdog/{b}/{}", pin.kind())
}

/// With <old>, only while the ref is still <old> ("": while there's none).
fn update(dir: &str, b: &str, pin: Pin, to: &str, old: Option<&str>) -> Out {
    let (r, msg) = (name(b, pin), format!("sd: {}", pin.kind()));
    let mut args = vec!["update-ref", "-m", &msg, &r, to];
    args.extend(old);
    git(dir, &args)
}

pub fn set(dir: &str, b: &str, pin: Pin, to: &str) -> Out {
    update(dir, b, pin, to, None)
}

/// With <old>, only while the record is still that object ("": while
/// there's none).
pub fn store(dir: &str, b: &str, pin: Pin, blob: &str, old: Option<&str>) -> Out {
    let obj = git_input(dir, &["hash-object", "-w", "--stdin"], Some(blob.as_bytes()));
    if !obj.ok {
        return obj;
    }
    update(dir, b, pin, &strip_nl(obj.stdout), old)
}

/// A review pin at <sha>, and the base it was taken against. sd state
/// compares base..pin with the branch's current base..HEAD, so a restack
/// (which moves the base) doesn't make a reviewed change look changed (#66).
pub fn review(dir: &str, b: &str, pin: Pin, sha: &str, base: &str) -> Result<(), String> {
    let with = match pin {
        Pin::Local => Pin::LocalBase,
        Pin::Bot => Pin::BotBase,
        Pin::Accepted => Pin::AcceptedBase,
        _ => return Err(format!("not a review pin: {} (local, bot, accepted)", pin.kind())),
    };
    for (p, to) in [(pin, sha), (with, base)] {
        let o = set(dir, b, p, to);
        if !o.ok {
            return Err(strip_nl(o.stderr));
        }
    }
    Ok(())
}

/// None there already is no error.
pub fn delete(dir: &str, b: &str, pin: Pin) -> Out {
    git(dir, &["update-ref", "-d", &name(b, pin)])
}

pub fn delete_all(dir: &str, b: &str) -> Out {
    let pins = git(dir, &["for-each-ref", "--format=delete %(refname)", &format!("refs/sheepdog/{b}/")]);
    if !pins.ok {
        return pins;
    }
    git_input(dir, &["update-ref", "--stdin"], Some(pins.stdout.as_bytes()))
}

/// Every pin whose branch no longer exists in <repo> (the pass). Refs
/// outlive git branch -D (config doesn't); a pin's last path component is
/// its kind, the rest the branch name.
pub fn sweep(repo: &str) {
    let refs = git(repo, &["for-each-ref", "--format=%(refname)", "refs/sheepdog/"]).stdout;
    let gone: String = refs
        .lines()
        .filter(|r| {
            let rest = r.strip_prefix("refs/sheepdog/").unwrap_or(r);
            !git::branch_in(repo, rest.rsplit_once('/').map_or(rest, |(b, _)| b))
        })
        .map(|r| format!("delete {r}\n"))
        .collect();
    if !gone.is_empty() {
        git_input(repo, &["update-ref", "--stdin"], Some(gone.as_bytes()));
    }
}

pub fn read(dir: &str, b: &str, pin: Pin) -> Option<String> {
    let o = git(dir, &["cat-file", "blob", &name(b, pin)]);
    o.ok.then_some(o.stdout)
}

/// "" when there's none.
pub fn oid(dir: &str, b: &str, pin: Pin) -> String {
    git::oid(dir, &name(b, pin))
}

/// "" when there's none.
pub fn commit(dir: &str, b: &str, pin: Pin) -> String {
    git::rev(dir, &name(b, pin))
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::Repo;
    use std::path::Path;

    // @test "pin_write records the pin and the base it was taken against"
    #[test]
    fn review_records_the_pin_and_the_base_it_was_taken_against() {
        let r = Repo::new();
        review(&r.dir, "feat", Pin::Local, "HEAD", "main").unwrap();
        assert_eq!(r.git(&["rev-parse", "refs/sheepdog/feat/local"]), r.git(&["rev-parse", "HEAD"]));
        assert_eq!(r.git(&["rev-parse", "refs/sheepdog/feat/local-base"]), r.git(&["rev-parse", "main"]));
    }

    // @test "pin_write overwrites both refs, so a stale base never outlives its pin"
    #[test]
    fn review_overwrites_both_refs_so_a_stale_base_never_outlives_its_pin() {
        let r = Repo::new();
        review(&r.dir, "feat", Pin::Bot, "HEAD", "main").unwrap();
        r.git(&["commit", "-q", "--allow-empty", "-m", "two"]);
        review(&r.dir, "feat", Pin::Bot, "HEAD", "HEAD~1").unwrap();
        assert_eq!(r.git(&["rev-parse", "refs/sheepdog/feat/bot"]), r.git(&["rev-parse", "HEAD"]));
        assert_eq!(r.git(&["rev-parse", "refs/sheepdog/feat/bot-base"]), r.git(&["rev-parse", "HEAD~1"]));
    }

    // @test "pin_write refuses a pin kind it doesn't know"
    #[test]
    fn review_refuses_a_pin_that_isnt_a_review_pin() {
        let r = Repo::new();
        assert_eq!(review(&r.dir, "feat", Pin::Base, "HEAD", "main"), Err("not a review pin: base (local, bot, accepted)".into()));
        assert!(oid(&r.dir, "feat", Pin::Base).is_empty());
    }

    #[test]
    fn store_keeps_a_record_read_reads_it_and_delete_drops_it_342() {
        let r = Repo::new();
        assert!(store(&r.dir, "feat", Pin::Findings, "{\"round\":1}\n", None).ok);
        assert_eq!(read(&r.dir, "feat", Pin::Findings).as_deref(), Some("{\"round\":1}\n"));
        assert!(delete(&r.dir, "feat", Pin::Findings).ok);
        assert_eq!(read(&r.dir, "feat", Pin::Findings), None);
        assert!(delete(&r.dir, "feat", Pin::Findings).ok, "none there is no error");
    }

    #[test]
    fn store_over_an_old_record_fails_once_another_writer_moved_it_342() {
        let r = Repo::new();
        assert!(store(&r.dir, "feat", Pin::Decisions, "a\n", Some("")).ok);
        let old = oid(&r.dir, "feat", Pin::Decisions);
        assert!(!store(&r.dir, "feat", Pin::Decisions, "b\n", Some("")).ok, "there is one now");
        assert!(store(&r.dir, "feat", Pin::Decisions, "a\nb\n", Some(&old)).ok);
        assert!(!store(&r.dir, "feat", Pin::Decisions, "a\nc\n", Some(&old)).ok, "moved since");
        assert_eq!(read(&r.dir, "feat", Pin::Decisions).as_deref(), Some("a\nb\n"));
    }


    /// <dir>'s .rs files, recursively, each with its text before any
    /// #[cfg(test)]: the code a test doesn't own.
    fn code(dir: &Path, out: &mut Vec<(String, String)>) {
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                code(&p, out);
            } else if p.extension().is_some_and(|x| x == "rs") {
                let text = std::fs::read_to_string(&p).unwrap();
                let live = text.split("#[cfg(test)]").next().unwrap_or("").to_string();
                out.push((p.to_string_lossy().into_owned(), live));
            }
        }
    }

    /// The lines in <text> where a git call writes something only this
    /// module may: update-ref that doesn't move a branch alone (no
    /// refs/heads/, or a refs/sheepdog or --stdin beside it), hash-object
    /// -w. A call's args run to its `]`, so one split over lines is read
    /// whole.
    fn writes(text: &str) -> Vec<usize> {
        let calls = text.match_indices("\"update-ref\"").chain(text.match_indices("\"hash-object\""));
        let mut at: Vec<usize> = calls
            .filter(|&(i, verb)| {
                let call = &text[i..];
                let args = &call[..call.find(']').unwrap_or(call.len())];
                if verb == "\"update-ref\"" {
                    !args.contains("refs/heads/") || args.contains("refs/sheepdog") || args.contains("--stdin")
                } else {
                    args.contains("\"-w\"")
                }
            })
            .map(|(i, _)| text[..i].matches('\n').count() + 1)
            .collect();
        at.sort_unstable();
        at
    }

    #[test]
    fn writes_reads_a_call_whole_and_passes_a_branch_move_alone_342() {
        let text = r#"
git(".", &["update-ref", "-m", "sd act fetch", &format!("refs/heads/{b}"), &theirs, &head]);
git(".", &[
    "update-ref",
    &r,
]);
git(".", &["update-ref", "refs/heads/x", "refs/sheepdog/x/local"]);
git(".", &["hash-object", "--stdin"]);
git(".", &["hash-object",
    "-w"]);
"#;
        assert_eq!(writes(text), vec![4, 7, 9]);
    }

    // I8 as a grep (#342): update-ref outside this module only moves a
    // branch, and only this module writes an object (hash-object -w).
    #[test]
    fn only_pins_writes_refs_sheepdog_342() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = vec![];
        code(&src, &mut files);
        let mut bad = vec![];
        for (path, text) in files.iter().filter(|(p, _)| !p.ends_with("/src/pins.rs")) {
            let lines: Vec<&str> = text.lines().collect();
            for n in writes(text) {
                bad.push(format!("{path}:{n}: {}", lines[n - 1].trim()));
            }
        }
        assert!(bad.is_empty(), "pins are written through src/pins.rs only:\n{}", bad.join("\n"));
    }
}
