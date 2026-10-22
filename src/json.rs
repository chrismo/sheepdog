// JSON as super 0.3.0 prints it (-j), so sd state --json stays byte for byte
// what the bash version printed: compact, keys in order, Go's float format
// and Go's escapes (  and   escaped, no HTML escaping).

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
        Value::String(s) => string(s, out),
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
                string(k, out);
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

fn string(s: &str, out: &mut String) {
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
            '\u{2028}' | '\u{2029}' => out.push_str(&format!("\\u{:04x}", c as u32)),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    // Each expected line is super 0.3.0's -j output for the same input.
    #[test]
    fn numbers_print_as_super_prints_them() {
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
    fn strings_escape_as_super_escapes_them() {
        let v: Value = serde_json::from_str(r#"{"d":"<&>é\u0001\t\"\\/\b\f\r\n\u001f  \u007f😀"}"#).unwrap();
        assert_eq!(to_string(&v), "{\"d\":\"<&>é\\u0001\\t\\\"\\\\/\\b\\f\\r\\n\\u001f\\u2028\\u2029\u{7f}😀\"}");
    }
}
