// The verify record (#325): refs/sheepdog/<b>/verify-red, one line of JSON,
// {head, step, cmd, at}: settings.verify failed with <head> checked out, run
// by <step> (implement, address, ci-fix, review, resolve, or verify by hand).
// Every run on a clean tree writes it: a failure records the commit it ran
// on, a pass removes it. Keyed on the commit, so sd state calls it at_head
// only while HEAD is still that commit: any new commit leaves it standing on
// an old one, which NEXT and ship ignore.

use crate::git::{git, git_input, git_out, strip_nl};
use crate::{json, time};
use serde_json::json;

pub fn name(b: &str) -> String {
    format!("refs/sheepdog/{b}/verify-red")
}

/// verify failed in <wt>: record its HEAD; a failure to record it only warns.
pub fn record(wt: &str, b: &str, step: &str, cmd: &str) {
    let Some(head) = git_out(wt, &["rev-parse", "HEAD"]) else {
        eprintln!("sd: warning: couldn't read HEAD in {wt}, so the failed verify isn't recorded");
        return;
    };
    let rec = json::to_string(&json!({"head": head, "step": step, "cmd": cmd, "at": time::now_utc().0})) + "\n";
    let obj = git_input(wt, &["hash-object", "-w", "--stdin"], Some(rec.as_bytes()));
    let msg = format!("sd: verify failed ({step})");
    if !obj.ok || !git(wt, &["update-ref", "-m", &msg, &name(b), &strip_nl(obj.stdout)]).ok {
        eprintln!("sd: warning: couldn't record the failed verify on {b}, so NEXT won't show it");
    }
}

/// verify passed in <wt>: remove the record (no record is no error); a
/// failure to remove it only warns.
pub fn clear(wt: &str, b: &str) {
    let r = name(b);
    if !git(wt, &["update-ref", "-d", &r]).ok {
        eprintln!("sd: warning: couldn't remove the failed verify on {b}, so NEXT and ship still count HEAD red; git update-ref -d {r}");
    }
}
