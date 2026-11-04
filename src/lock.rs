// branch.<b>.sd-lock: "<pid>@<host> <time>" from sd act attach, or
// "held@<host> <time>" from sd act hold (#427). A hold has no pid, so
// act::lock_stale never clears it.

const HELD: &str = "held@";

pub fn hold(host: &str, time: &str) -> String {
    format!("{HELD}{host} {time}")
}

/// When <lock> is a hold, the time it was taken.
pub fn held_since(lock: &str) -> Option<&str> {
    lock.strip_prefix(HELD).map(|rest| rest.split_once(' ').map_or("", |(_, t)| t))
}

/// How the one who holds <lock> lets it go.
pub fn let_go(lock: &str) -> &'static str {
    if held_since(lock).is_some() {
        "sd act release"
    } else {
        "exit that session"
    }
}

pub fn who_since(lock: &str) -> (String, &str) {
    match held_since(lock) {
        Some(t) => ("held (sd act hold)".into(), t),
        None => {
            let (who, t) = lock.split_once(' ').unwrap_or((lock, ""));
            (format!("session {who}"), t)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hold_reads_back_with_its_time_and_a_sessions_lock_isnt_one() {
        let h = hold("host", "2026-11-04T09:00:00Z");
        assert_eq!(h, "held@host 2026-11-04T09:00:00Z");
        assert_eq!(held_since(&h), Some("2026-11-04T09:00:00Z"));
        assert_eq!(held_since("123@host 2026-11-04T09:00:00Z"), None);
        assert_eq!(held_since(""), None);
        assert_eq!(let_go(&h), "sd act release");
        assert_eq!(let_go("123@host 2026-11-04T09:00:00Z"), "exit that session");
    }

    #[test]
    fn who_since_names_the_holder() {
        assert_eq!(who_since("held@h 2026-11-04T09:00:00Z"), ("held (sd act hold)".into(), "2026-11-04T09:00:00Z"));
        assert_eq!(who_since("1@h 2026-10-15T00:00:00Z"), ("session 1@h".into(), "2026-10-15T00:00:00Z"));
        assert_eq!(who_since("chrismo"), ("session chrismo".into(), ""));
    }
}
