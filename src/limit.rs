// The usage-limit pause (#269): $SHEEPDOG_STATE_DIR/limit, one line, "<reset
// time> <harness>: <what it said>", written when an agent step stops on its
// harness's usage limit, hand-run or launched. sd watch launches no agent
// step before the reset. One file for every harness and model.

use crate::{config, time};
use std::fs;

/// When no reset time is known: an hour from the limit.
const UNKNOWN_RESET: i64 = 3600;

/// Record <harness>'s limit; a failure to write it only warns.
pub fn write(harness: &str, said: &str, resets: Option<&str>) {
    write_in(&config::state_dir(), harness, said, resets);
}

/// The limit's line while it holds; past its reset, the file is removed.
pub fn holding() -> Option<String> {
    holding_in(&config::state_dir())
}

/// write, in <dir>. A pause that already ends later stands.
fn write_in(dir: &str, harness: &str, said: &str, resets: Option<&str>) {
    let now = (time::now_utc().1 / 1_000_000_000) as i64;
    let at = resets.map_or_else(|| time::utc(now + UNKNOWN_RESET), str::to_string);
    if holding_in(dir).is_some_and(|l| reset(&l) >= time::epoch_ns(&at)) {
        return;
    }
    let f = format!("{dir}/limit");
    let _ = fs::create_dir_all(dir);
    if let Err(e) = fs::write(&f, format!("{at} {harness}: {said}\n")) {
        eprintln!("sd: warning: can't write {f}, so sd watch won't pause agent steps: {e}");
    }
}

fn reset(line: &str) -> Option<i128> {
    line.split(' ').next().and_then(time::epoch_ns)
}

fn holding_in(dir: &str) -> Option<String> {
    let f = format!("{dir}/limit");
    let line = fs::read_to_string(&f).ok()?;
    let line = line.trim_end();
    if reset(line).is_some_and(|at| at > time::now_utc().1) {
        return Some(line.to_string());
    }
    let _ = fs::remove_file(&f);
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_keeps_a_pause_that_ends_later_269() {
        let dir = std::env::temp_dir().join(format!("sd-limit-test-{}", std::process::id()));
        let dir = dir.to_string_lossy();
        let now = (time::now_utc().1 / 1_000_000_000) as i64;
        let (later, sooner) = (time::utc(now + 7200), time::utc(now + 600));
        write_in(&dir, "claude", "weekly", Some(&later));
        write_in(&dir, "claude", "no reset given", None);
        write_in(&dir, "claude", "session", Some(&sooner));
        assert_eq!(holding_in(&dir), Some(format!("{later} claude: weekly")));
        let further = time::utc(now + 9000);
        write_in(&dir, "pi", "daily", Some(&further));
        assert_eq!(holding_in(&dir), Some(format!("{further} pi: daily")));
        let _ = fs::remove_dir_all(&*dir);
    }
}
