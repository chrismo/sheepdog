// UTC times as sheepdog writes them (date -u +%FT%TZ), with no time crate.

use regex::Regex;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn epoch_ns(t: &str) -> Option<i128> {
    static T: OnceLock<Regex> = OnceLock::new();
    let r = T.get_or_init(|| {
        Regex::new(r"^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2}):(\d{2})(\.\d+)?(Z|([+-])(\d{2}):(\d{2}))$").expect("a fixed pattern compiles")
    });
    let c = r.captures(t)?;
    let g = |i: usize| c.get(i).map_or(0i64, |m| m.as_str().parse().unwrap_or(0));
    let (y, m, d) = (g(1), g(2), g(3));
    // days from civil (Howard Hinnant)
    let y2 = if m <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    let mut secs = days * 86400 + g(4) * 3600 + g(5) * 60 + g(6);
    if c.get(9).is_some() {
        let off = g(10) * 3600 + g(11) * 60;
        secs -= if &c[9] == "+" { off } else { -off };
    }
    let frac = c.get(7).map_or(0i128, |m| {
        let digits = &m.as_str()[1..];
        let padded = format!("{digits:0<9}");
        padded[..9].parse().unwrap_or(0)
    });
    Some(i128::from(secs) * 1_000_000_000 + frac)
}

pub fn now_utc() -> (String, i128) {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()) as i64;
    (utc(secs), i128::from(secs) * 1_000_000_000)
}

pub fn utc(secs: i64) -> String {
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    // civil from days (Howard Hinnant)
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_ns_reads_utc_offsets_and_fractions() {
        assert_eq!(epoch_ns("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(epoch_ns("2026-10-16T13:00:00Z").map(|n| n / 1_000_000_000), Some(1_792_155_600));
        assert_eq!(epoch_ns("2026-10-16T15:00:00+02:00"), epoch_ns("2026-10-16T13:00:00Z"));
        assert_eq!(epoch_ns("1970-01-01T00:00:01.5Z"), Some(1_500_000_000));
        assert_eq!(epoch_ns("soon"), None);
    }

    #[test]
    fn utc_writes_epoch_seconds_as_sheepdog_writes_times() {
        assert_eq!(utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(utc(1_792_128_000), "2026-10-16T05:20:00Z");
        assert_eq!(epoch_ns(&utc(1_792_155_601)), Some(1_792_155_601 * 1_000_000_000));
    }

    #[test]
    fn now_utc_reads_back_as_itself() {
        let (t, ns) = now_utc();
        assert_eq!(epoch_ns(&t), Some(ns));
    }
}
