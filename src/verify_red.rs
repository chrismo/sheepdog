// The verify record (#325): refs/sheepdog/<b>/verify-red, one line of JSON,
// {head, step, cmd, at}: settings.verify failed with <head> checked out, run
// by <step> (implement, address, ci-fix, review, resolve, or verify by hand).
// Every run on a clean tree writes it: a failure records the commit it ran
// on, a pass removes it. Keyed on the commit, so sd state calls it at_head
// only while HEAD is still that commit: a new commit leaves it standing on
// an old one, which NEXT and ship ignore. A restack is no fix, so it carries
// the record to the commit it lands on (carry).

use crate::git::git_out;
use crate::pins::{self, Pin};
use crate::{json, time};
use serde_json::json;

pub fn name(b: &str) -> String {
    pins::name(b, Pin::VerifyRed)
}

/// verify failed in <wt>: record its HEAD; a failure to record it only warns.
pub fn record(wt: &str, b: &str, step: &str, cmd: &str) {
    let Some(head) = git_out(wt, &["rev-parse", "HEAD"]) else {
        eprintln!("sd: warning: couldn't read HEAD in {wt}, so the failed verify isn't recorded");
        return;
    };
    let rec = json::to_string(&json!({"head": head, "step": step, "cmd": cmd, "at": time::now_utc().0})) + "\n";
    if !pins::store(wt, b, Pin::VerifyRed, &rec, None).ok {
        eprintln!("sd: warning: couldn't record the failed verify on {b}, so NEXT won't show it");
    }
}

/// verify passed in <wt>: remove the record (no record is no error); a
/// failure to remove it only warns.
pub fn clear(wt: &str, b: &str) {
    if !pins::delete(wt, b, Pin::VerifyRed).ok {
        eprintln!("sd: warning: couldn't remove the failed verify on {b}, so NEXT and ship still count HEAD red; git update-ref -d {}", name(b));
    }
}

/// A record on <from> moves to <to>, the rest of it kept; one on another
/// commit stays put. A failure to move it only warns.
pub fn carry(dir: &str, b: &str, from: &str, to: &str) {
    let was = pins::oid(dir, b, Pin::VerifyRed);
    let Some(mut rec) = pins::read(dir, b, Pin::VerifyRed).and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok()) else {
        return;
    };
    if rec.get("head").and_then(serde_json::Value::as_str) != Some(from) {
        return;
    }
    rec["head"] = json!(to);
    if !pins::store(dir, b, Pin::VerifyRed, &(json::to_string(&rec) + "\n"), Some(&was)).ok {
        eprintln!("sd: warning: couldn't move the failed verify on {b} to {}, so NEXT won't show it", &to[..to.len().min(7)]);
    }
}

/// HEAD's sha when the record is on it.
pub fn at_head(wt: &str, b: &str) -> Option<String> {
    let rec: serde_json::Value = serde_json::from_str(&pins::read(wt, b, Pin::VerifyRed)?).ok()?;
    let head = git_out(wt, &["rev-parse", "HEAD"])?;
    (rec.get("head")?.as_str()? == head).then_some(head)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::Repo;

    fn rec(r: &Repo) -> Option<serde_json::Value> {
        serde_json::from_str(&pins::read(&r.dir, "feat", Pin::VerifyRed)?).ok()
    }

    #[test]
    fn carry_moves_a_record_on_the_old_head_to_the_new_one_and_leaves_others_alone_325() {
        let r = Repo::new();
        let (old, new) = (r.git(&["rev-parse", "HEAD"]), r.git(&["rev-parse", "main"]));
        carry(&r.dir, "feat", &old, &new);
        assert_eq!(rec(&r), None);
        record(&r.dir, "feat", "implement", "make test");
        let before = rec(&r).unwrap();
        carry(&r.dir, "feat", &new, &old);
        assert_eq!(rec(&r).unwrap(), before);
        carry(&r.dir, "feat", &old, &new);
        let mut moved = before;
        moved["head"] = json!(new);
        assert_eq!(rec(&r).unwrap(), moved);
    }
}
