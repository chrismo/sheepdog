// usage.sup (#121): one line of Super (SUP) per agent step, in
// $SHEEPDOG_STATE_DIR, written byte for byte as super wrote it for the bash
// harness_usage, so lines from either read the same. claude's numbers are
// from its JSON output, its tokens per step; pi's, which prints none, are
// what its transcript gained during the step; null when there are none. The
// cost is the harness's estimate, not how it was billed.

use crate::config;
use crate::json::{sup_float, sup_string};
use crate::val::{at, n, on};
use regex::Regex;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::sync::OnceLock;

pub fn file() -> String {
    format!("{}/usage.sup", config::state_dir())
}

/// Shaped like a whole record; not a cut-off append or a stray edit.
pub fn shaped(line: &str) -> bool {
    static SHAPE: OnceLock<Regex> = OnceLock::new();
    SHAPE.get_or_init(|| Regex::new(r"^\{time:.*\}$").expect("a fixed pattern compiles")).is_match(line)
}

/// One usage record's fields: a flat SUP record of strings, numbers, times,
/// bools and nulls, as record writes them. None when it doesn't parse.
pub fn sup_record(line: &str) -> Option<HashMap<String, Value>> {
    static TIME: OnceLock<Regex> = OnceLock::new();
    let time = TIME.get_or_init(|| {
        Regex::new(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d+)?(Z|[+-]\d{2}:\d{2})$").expect("a fixed pattern compiles")
    });
    let body = line.strip_prefix('{')?.strip_suffix('}')?;
    let mut out = HashMap::new();
    let mut rest = body;
    while !rest.is_empty() {
        let (k, r) = rest.split_once(':')?;
        let k = k.trim();
        if k.is_empty() || !k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return None;
        }
        let r = r.trim_start();
        let (v, r) = if r.starts_with('"') {
            let mut de = serde_json::Deserializer::from_str(r).into_iter::<Value>();
            let v = de.next()?.ok()?;
            (v, &r[de.byte_offset()..])
        } else {
            let end = r.find(',').unwrap_or(r.len());
            let tok = r[..end].trim();
            let v = match tok {
                "null" => Value::Null,
                "true" => json!(true),
                "false" => json!(false),
                t if time.is_match(t) => json!(t),
                t => {
                    if let Ok(i) = t.parse::<i64>() {
                        json!(i)
                    } else {
                        let f: f64 = t.strip_suffix('.').unwrap_or(t).parse().ok()?;
                        if !t.chars().next().is_some_and(|c| c.is_ascii_digit() || c == '-' || c == '+' || c == '.') {
                            return None;
                        }
                        json!(f)
                    }
                }
            };
            (v, &r[end..])
        };
        out.insert(k.to_string(), v);
        let r = r.trim_start();
        rest = match r.strip_prefix(',') {
            Some(r) => r,
            None if r.is_empty() => r,
            None => return None,
        };
    }
    Some(out)
}

/// A claude step's cost from its session's total_cost_usd, which on a resume
/// is the running total (#150): what it gained since <prev>, the total
/// usage.sup last knew (session_total), rounded off float noise. A total
/// below prev isn't a running total, so the step is charged all of it.
pub fn step_cost(total: f64, prev: f64) -> f64 {
    if prev > 0.0 && total >= prev {
        ((total - prev) * 1e9).round() / 1e9
    } else {
        total
    }
}

/// The session's running total in USD as usage.sup last knew it, for
/// step_cost: its last claude row's total_cost_usd, or the cost_usd of a row
/// from before #150, which was that total. 0 with none, unreadable, or for
/// pi, whose cost is per step already.
pub fn session_total(harness: &str, session: &str) -> f64 {
    if harness != "claude" {
        return 0.0;
    }
    fs::read_to_string(file()).map_or(0.0, |text| total_in(&text, session))
}

fn total_in(text: &str, session: &str) -> f64 {
    let key = format!("session:{},", sup_string(session));
    let mut total = 0.0;
    for l in text.lines().filter(|l| l.contains(&key) && shaped(l)) {
        let Some(r) = sup_record(l) else { return 0.0 };
        let r = Value::Object(r.into_iter().collect());
        if at(&r, "harness").as_str() != Some("claude") {
            continue;
        }
        let known = match at(&r, "total_cost_usd") {
            Value::Null if on(&r, "estimated") => &Value::Null,
            Value::Null => at(&r, "cost_usd"),
            t => t,
        };
        if let Some(t) = known.as_f64() {
            total = t;
        }
    }
    total
}

/// One step, as the harness saw it.
pub struct Step<'a> {
    pub time: &'a str,
    pub repo: &'a str,
    pub branch: &'a str,
    /// The step's name; "" for none.
    pub step: &'a str,
    pub harness: &'a str,
    pub session: &'a str,
    pub mode: &'a str,
    pub status: i32,
    pub ms: i64,
    /// The step's --max-budget-usd; 0 for none known.
    pub cap: f64,
    /// session_total before the step.
    pub prev: f64,
    /// claude's result object; null for none, and for pi.
    pub o: &'a Value,
    /// The harness's tally after the step and before it.
    pub a: &'a Value,
    pub b: &'a Value,
}

/// The step's record, a line of SUP. A claude step with no result (killed
/// by stepTimeout, or crashed) that still ran turns is charged its cap, the
/// most it could spend, and marked estimated; with no cap its cost is
/// unknown (null), and still marked. Its session total is unknown, so the
/// next step's cost counts from the last one known, and may count the
/// killed step's real spend again.
pub fn record(s: &Step) -> String {
    let claude = s.harness == "claude";
    let (o, a, b) = (s.o, s.a, s.b);
    let unseen = claude && o.is_null() && n(a, "entries") > n(b, "entries");
    let gained = |k: &str| at(a, k).as_f64().map(|x| x - n(b, k));
    let u = if claude && !o.is_null() {
        json!({"cost": step_cost(n(o, "total_cost_usd"), s.prev), "total": at(o, "total_cost_usd"),
               "input": n(o, "usage.input_tokens"), "cache_create": n(o, "usage.cache_creation_input_tokens"),
               "cache_read": n(o, "usage.cache_read_input_tokens"), "output": n(o, "usage.output_tokens"),
               "duration_ms": n(o, "duration_ms"), "turns": n(o, "num_turns")})
    } else if !claude && !at(a, "turns").is_null() {
        json!({"cost": gained("cost"), "input": gained("input"), "cache_create": gained("cache_create"),
               "cache_read": gained("cache_read"), "output": gained("output"), "duration_ms": s.ms,
               "turns": gained("turns")})
    } else {
        Value::Null
    };
    let float = |v: &Value| v.as_f64().map_or("null".into(), sup_float);
    let int = |k: &str| at(&u, k).as_f64().map_or("null".into(), |x| (x as i64).to_string());
    let nn = |s: &str| if s.is_empty() { "null".into() } else { sup_string(s) };
    let cost = match (&u, unseen && s.cap > 0.0) {
        (Value::Null, true) => sup_float(s.cap),
        (u, _) => float(at(u, "cost")),
    };
    format!(
        "{{time:{},repo:{},branch:{},step:{},harness:{},session:{},mode:{},status:{},cost_usd:{cost},total_cost_usd:{},\
         estimated:{unseen},input:{},cache_create:{},cache_read:{},output:{},duration_ms:{},turns:{},context:{}}}",
        s.time,
        nn(s.repo),
        nn(s.branch),
        nn(s.step),
        sup_string(s.harness),
        sup_string(s.session),
        sup_string(s.mode),
        s.status,
        float(at(&u, "total")),
        int("input"),
        int("cache_create"),
        int("cache_read"),
        int("output"),
        int("duration_ms"),
        int("turns"),
        at(a, "context").as_f64().map_or("null".into(), |x| (x as i64).to_string()),
    )
}

/// Appends <rec> to usage.sup; a step isn't failed for want of its record.
pub fn append(rec: &str) {
    let f = file();
    let _ = fs::create_dir_all(config::state_dir());
    if let Ok(mut out) = fs::OpenOptions::new().create(true).append(true).open(&f) {
        let _ = writeln!(out, "{rec}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ci/fixtures/usage.json (gone with it), the input the bash USAGE_Q
    // was stress-tested on, and what super 0.3.0 made of it.
    #[test]
    fn record_writes_what_the_bash_usage_q_wrote() {
        let rows = [
            ("2026-10-16T20:33:06Z", "/Users/chrismo/modev/sheepdog", "sd-63-bot-wait", "review", "claude", "6ece45cb-0000-4000-8000-000000000000", "new", 0, 61000, 5.0, 0.0,
             json!({"type": "result", "is_error": false, "total_cost_usd": 0.7948558, "duration_ms": 60512, "num_turns": 14, "usage": {"input_tokens": 22, "cache_creation_input_tokens": 41234, "cache_read_input_tokens": 512345, "output_tokens": 4321}}),
             json!({"entries": 14, "context": 61002}), json!({}),
             r#"{time:2026-10-16T20:33:06Z,repo:"/Users/chrismo/modev/sheepdog",branch:"sd-63-bot-wait",step:"review",harness:"claude",session:"6ece45cb-0000-4000-8000-000000000000",mode:"new",status:0,cost_usd:0.7948558,total_cost_usd:0.7948558,estimated:false,input:22,cache_create:41234,cache_read:512345,output:4321,duration_ms:60512,turns:14,context:61002}"#),
            ("2026-10-16T20:35:00Z", "/Users/chrismo/modev/sheepdog", "sd-63-bot-wait", "implement", "claude", "6ece45cb-0000-4000-8000-000000000000", "resume", 124, 1800000, 2.5, 0.7948558,
             Value::Null, json!({"entries": 40, "context": 150000}), json!({"entries": 14, "context": 61002}),
             r#"{time:2026-10-16T20:35:00Z,repo:"/Users/chrismo/modev/sheepdog",branch:"sd-63-bot-wait",step:"implement",harness:"claude",session:"6ece45cb-0000-4000-8000-000000000000",mode:"resume",status:124,cost_usd:2.5,total_cost_usd:null,estimated:true,input:null,cache_create:null,cache_read:null,output:null,duration_ms:null,turns:null,context:150000}"#),
            ("2026-10-16T20:38:00Z", "/Users/chrismo/modev/sheepdog", "sd-63-bot-wait", "describe", "claude", "6ece45cb-0000-4000-8000-000000000000", "resume", 0, 20000, 5.0, 0.7948558,
             json!({"type": "result", "is_error": false, "total_cost_usd": 1.0358558, "duration_ms": 19800, "num_turns": 2, "usage": {"input_tokens": 3, "cache_creation_input_tokens": 28419, "cache_read_input_tokens": 14355, "output_tokens": 558}}),
             json!({"entries": 42, "context": 43335}), json!({"entries": 40, "context": 150000}),
             r#"{time:2026-10-16T20:38:00Z,repo:"/Users/chrismo/modev/sheepdog",branch:"sd-63-bot-wait",step:"describe",harness:"claude",session:"6ece45cb-0000-4000-8000-000000000000",mode:"resume",status:0,cost_usd:0.241,total_cost_usd:1.0358558,estimated:false,input:3,cache_create:28419,cache_read:14355,output:558,duration_ms:19800,turns:2,context:43335}"#),
            ("2026-10-16T20:40:00Z", "/Users/chrismo/modev/sheepdog", "", "", "pi", "22222222-3333-4444-8555-666666666601", "resume", 1, 9000, 0.0, 0.0,
             Value::Null, json!({"cost": 0.75, "input": 300, "cache_create": 10, "cache_read": 900, "output": 50, "turns": 3, "context": 1260}),
             json!({"cost": 0.5, "input": 100, "cache_create": 0, "cache_read": 0, "output": 10, "turns": 1, "context": 110}),
             r#"{time:2026-10-16T20:40:00Z,repo:"/Users/chrismo/modev/sheepdog",branch:null,step:null,harness:"pi",session:"22222222-3333-4444-8555-666666666601",mode:"resume",status:1,cost_usd:0.25,total_cost_usd:null,estimated:false,input:200,cache_create:10,cache_read:900,output:40,duration_ms:9000,turns:2,context:1260}"#),
            ("2026-10-16T20:41:00Z", "/r", "feat", "implement", "pi", "s3", "new", 1, 5, 0.0, 0.0, Value::Null, json!({}), json!({}),
             r#"{time:2026-10-16T20:41:00Z,repo:"/r",branch:"feat",step:"implement",harness:"pi",session:"s3",mode:"new",status:1,cost_usd:null,total_cost_usd:null,estimated:false,input:null,cache_create:null,cache_read:null,output:null,duration_ms:null,turns:null,context:null}"#),
        ];
        for (time, repo, branch, step, harness, session, mode, status, ms, cap, prev, o, a, b, want) in &rows {
            let s = Step { time, repo, branch, step, harness, session, mode, status: *status, ms: *ms, cap: *cap, prev: *prev, o, a, b };
            assert_eq!(record(&s), *want);
            // And sd state reads it back.
            assert!(sup_record(want).is_some());
        }
    }

    #[test]
    fn session_total_is_the_last_claude_total_known_or_a_cost_from_before_150() {
        let row = |session: &str, harness: &str, rest: &str| {
            format!("{{time:2026-10-16T20:33:06Z,repo:\"/r\",branch:\"feat\",step:null,harness:\"{harness}\",session:\"{session}\",mode:\"new\",status:0,{rest}}}\n")
        };
        assert_eq!(total_in("", "s"), 0.0);
        // A row from before #150: its cost_usd was the session's total.
        let mut text = row("s", "claude", "cost_usd:0.34,estimated:false");
        assert_eq!(total_in(&text, "s"), 0.34);
        text += &row("s", "claude", "cost_usd:0.24,total_cost_usd:0.58,estimated:false");
        text += &row("other", "claude", "cost_usd:9.,total_cost_usd:9.,estimated:false");
        text += &row("s", "pi", "cost_usd:7.,total_cost_usd:null,estimated:false");
        // A killed step's estimate isn't a total: the last one known stands.
        text += &row("s", "claude", "cost_usd:5.,total_cost_usd:null,estimated:true");
        text += "{time:cut off\n";
        assert_eq!(total_in(&text, "s"), 0.58);
        // A line shaped like the session's record that doesn't read: unknown.
        text += "{time:2026-10-16T20:33:06Z,session:\"s\",oops}\n";
        assert_eq!(total_in(&text, "s"), 0.0);
    }

    #[test]
    fn step_cost_is_what_the_running_total_gained_rounded_or_all_of_a_total_that_went_down() {
        assert_eq!(step_cost(0.25, 0.0), 0.25);
        assert_eq!(step_cost(0.58, 0.34), 0.24);
        assert_eq!(step_cost(5.25, 4.5), 0.75);
        assert_eq!(step_cost(0.2, 0.5), 0.2);
    }
}
