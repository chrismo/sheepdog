// JSON as sheepdog prints it: compact, keys in insertion order, Go's float
// format and Go's escapes (U+2028 and U+2029 escaped, no HTML escaping). It
// began as what super 0.3.0's -j printed for the bash version, retired with
// #235; nothing holds it to those bytes now. What reads sd state --json or a
// record parses it as JSON, so the format is free to change (#379).

use serde_json::{Number, Value};

pub fn to_string(v: &Value) -> String {
    let mut out = String::new();
    write(v, &mut out);
    out
}

fn write(v: &Value, out: &mut String) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&number(n)),
        Value::String(s) => string(s, out, true),
        Value::Array(a) => {
            out.push('[');
            for (i, e) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write(e, out);
            }
            out.push(']');
        }
        Value::Object(m) => {
            out.push('{');
            for (i, (k, e)) in m.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                string(k, out, true);
                out.push(':');
                write(e, out);
            }
            out.push('}');
        }
    }
}

pub fn number(n: &Number) -> String {
    if n.is_i64() || n.is_u64() {
        return n.to_string();
    }
    float(n.as_f64().unwrap_or(0.0))
}

/// Go's strconv 'f' format, or 'e' below 1e-6 and from 1e21, shortest digits.
pub fn float(f: f64) -> String {
    let a = f.abs();
    if a != 0.0 && !(1e-6..1e21).contains(&a) {
        let s = format!("{f:e}");
        return match s.split_once('e') {
            Some((m, e)) if !e.starts_with('-') => format!("{m}e+{e}"),
            _ => s,
        };
    }
    format!("{f}")
}

/// <seps>: U+2028 and U+2029 escaped, as -j does and SUP doesn't.
fn string(s: &str, out: &mut String, seps: bool) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\u{2028}' | '\u{2029}' if seps => out.push_str(&format!("\\u{:04x}", c as u32)),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

// The same values in SUP (usage.sup, which usage::sup_record reads back) and
// in text, where an f-string printed them.

pub fn sup_string(s: &str) -> String {
    let mut out = String::new();
    string(s, &mut out, false);
    out
}

/// Go's strconv 'g' format, shortest digits: a float in an f-string.
pub fn go_g(f: f64) -> String {
    let e = format!("{f:e}");
    let (m, x) = e.split_once('e').unwrap_or((&e, "0"));
    let x: i32 = x.parse().unwrap_or(0);
    if !(-4..6).contains(&x) {
        return format!("{m}e{}{:02}", if x < 0 { '-' } else { '+' }, x.abs());
    }
    format!("{f}")
}

/// A float64 in SUP: a whole one that fits an int64 as "<int>.", else go_g.
pub fn sup_float(f: f64) -> String {
    if f.fract() == 0.0 && f.abs() < i64::MAX as f64 {
        return format!("{}.", f as i64);
    }
    go_g(f)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Format tests: they pin today's output, which began as super 0.3.0's,
    // and may change with it (#379). A number's text shows in sd state's
    // and NEXT's lines (val.rs, next.rs).
    #[test]
    fn numbers_print_in_gos_float_format() {
        let v: Value = serde_json::from_str(
            r#"{"a":1,"b":25.0,"c":0.1,"h":1e21,"i":1.5e-7,"j":100000000000000000000.0,"k":0.30000000000000004,"m":-0.0,"n":12345678.9,"o":1e-6,"p":1.7976931348623157e308}"#,
        )
        .unwrap();
        assert_eq!(
            to_string(&v),
            r#"{"a":1,"b":25,"c":0.1,"h":1e+21,"i":1.5e-7,"j":100000000000000000000,"k":0.30000000000000004,"m":-0,"n":12345678.9,"o":0.000001,"p":1.7976931348623157e+308}"#
        );
    }

    #[test]
    fn strings_escape_control_characters_and_line_separators() {
        let v: Value = serde_json::from_str(r#"{"d":"<&>é\u0001\t\"\\/\b\f\r\n\u001f  \u007f😀"}"#).unwrap();
        assert_eq!(to_string(&v), "{\"d\":\"<&>é\\u0001\\t\\\"\\\\/\\b\\f\\r\\n\\u001f\\u2028\\u2029\u{7f}😀\"}");
    }

    #[test]
    fn floats_print_in_sup_and_in_text() {
        let sup = [
            (5.0, "5."), (0.0, "0."), (1234567.0, "1234567."), (-1234567.0, "-1234567."), (1e18, "1000000000000000000."),
            (9.2233720368547e18, "9223372036854700032."), (1e19, "1e+19"), (1e20, "1e+20"), (1e21, "1e+21"),
            (1e-7, "1e-07"), (1e-6, "1e-06"), (1e-5, "1e-05"), (0.0001, "0.0001"), (0.00012, "0.00012"), (0.1, "0.1"),
            (123456.5, "123456.5"), (1234567.5, "1.2345675e+06"), (123456789012.5, "1.234567890125e+11"),
            (0.30000000000000004, "0.30000000000000004"), (-2.5, "-2.5"), (1.5e300, "1.5e+300"),
        ];
        for (f, s) in sup {
            assert_eq!(sup_float(f), s, "{f}");
        }
        assert_eq!([go_g(5.0), go_g(1234567.0), go_g(1e20), go_g(1e-7)], ["5", "1.234567e+06", "1e+20", "1e-07"]);
        assert_eq!(sup_string("q\"\\\n\té\u{1}<>&\u{2028}\r\u{8}\u{c}\u{7f}"), "\"q\\\"\\\\\\n\\té\\u0001<>&\u{2028}\\r\\b\\f\u{7f}\"");
    }
}
