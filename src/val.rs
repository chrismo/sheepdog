// Reading a JSON record the way the SuperDB queries sd state and NEXT
// replaced did: a missing field reads as null, on() is "true", n() is "the
// number, else 0", and text is what an f-string printed.

use serde_json::Value;

pub static NULL: Value = Value::Null;

/// The field at a dotted path; null when it or a parent is missing or null.
pub fn at<'a>(v: &'a Value, path: &str) -> &'a Value {
    path.split('.').try_fold(v, |cur, k| cur.as_object()?.get(k)).unwrap_or(&NULL)
}

/// f-string text: strings raw, null as nothing.
pub fn text(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        Value::Number(n) => crate::json::number(n),
        _ => crate::json::to_string(v),
    }
}

pub fn t(v: &Value, p: &str) -> String {
    text(at(v, p))
}

pub fn on(v: &Value, p: &str) -> bool {
    at(v, p) == &Value::Bool(true)
}

pub fn n(v: &Value, p: &str) -> f64 {
    at(v, p).as_f64().unwrap_or(0.0)
}

pub fn sj(v: &Value, sep: &str) -> String {
    v.as_array().map_or(String::new(), |a| a.iter().map(text).collect::<Vec<_>>().join(sep))
}
