// The usage-limit pause (#269): $SHEEPDOG_STATE_DIR/limit, one line, "<reset
// time> <harness>: <what it said>", written when an agent step stops on its
// harness's usage limit, hand-run or launched. sd watch launches no agent
// step before the reset. One file for every harness and model.

use crate::{config, time};
use std::fs;

fn file() -> String {
    format!("{}/limit", config::state_dir())
}

/// When no reset time is known: an hour from the limit.
const UNKNOWN_RESET: i64 = 3600;

/// Record <harness>'s limit; a failure to write it only warns.
pub fn write(harness: &str, said: &str, resets: Option<&str>) {
    let now = (time::now_utc().1 / 1_000_000_000) as i64;
    let at = resets.map_or_else(|| time::utc(now + UNKNOWN_RESET), str::to_string);
    let _ = fs::create_dir_all(config::state_dir());
    if let Err(e) = fs::write(file(), format!("{at} {harness}: {said}\n")) {
        eprintln!("sd: warning: can't write {}, so sd watch won't pause agent steps: {e}", file());
    }
}

/// The limit's line while it holds; past its reset, the file is removed.
pub fn holding() -> Option<String> {
    let line = fs::read_to_string(file()).ok()?;
    let line = line.trim_end();
    let at = line.split(' ').next().and_then(time::epoch_ns);
    if at.is_some_and(|at| at > time::now_utc().1) {
        return Some(line.to_string());
    }
    let _ = fs::remove_file(file());
    None
}
