// A branch's row in last-pass, what sd watch status prints (#317).

use crate::val::{at, t, text};
use serde_json::Value;

/// repo TAB branch TAB glyph TAB why TAB PR URL. why is what was notified
/// and the note; a branch that only waits has neither, so its why is what
/// each NEXT step waits on, in sd state's own words ("CI running", "waiting
/// on review from alice", "waiting on 377-atlas-settings (#413) to merge
/// first"). The URL (empty with no PR) lets a row be pasted as it is.
pub fn row(name: &str, b: &str, glyph: &str, mut why: Vec<String>, s: &Value) -> String {
    let next: &[Value] = at(s, "next").as_array().map_or(&[], Vec::as_slice);
    if !next.is_empty() && next.iter().all(|x| at(x, "who") == "wait") {
        why.extend(next.iter().map(|x| text(at(x, "say"))).filter(|m| !m.is_empty()));
    }
    format!("{name}\t{b}\t{glyph}\t{}\t{}\n", why.join("; "), t(s, "pr.url"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn wait(key: &str, say: &str) -> Value {
        json!({"key": key, "who": "wait", "say": say, "cmd": null})
    }

    fn state(next: Vec<Value>) -> Value {
        json!({"pr": {"number": 7, "url": "https://example/pr/7"}, "next": next})
    }

    #[test]
    fn row_of_a_waiting_branch_says_what_each_step_waits_on_then_the_prs_url() {
        let parent = state(vec![wait("parent-wait", "waiting on 377-atlas-settings (#413) to merge first")]);
        assert_eq!(row("sheepdog", "b", "⏳", vec![], &parent), "sheepdog\tb\t⏳\twaiting on 377-atlas-settings (#413) to merge first\thttps://example/pr/7\n");
        let ci = state(vec![wait("ci-wait", "CI running")]);
        assert_eq!(row("sheepdog", "b", "⏳", vec![], &ci), "sheepdog\tb\t⏳\tCI running\thttps://example/pr/7\n");
        let both = state(vec![wait("ci-wait", "CI running"), wait("bot-wait", "waiting on the review bot (/review-bot)")]);
        assert_eq!(row("sheepdog", "b", "⏳", vec![], &both), "sheepdog\tb\t⏳\tCI running; waiting on the review bot (/review-bot)\thttps://example/pr/7\n");
    }

    #[test]
    fn row_keeps_the_notified_reasons_when_a_step_isnt_a_wait_and_the_url_column_is_empty_with_no_pr() {
        let mixed = state(vec![json!({"key": "ci-red", "who": "you", "say": "CI red", "cmd": null}), wait("ci-wait", "CI running")]);
        assert_eq!(row("r", "b", "❌", vec!["CI red".into()], &mixed), "r\tb\t❌\tCI red\thttps://example/pr/7\n");
        let no_pr = json!({"pr": null, "next": [wait("ci-wait", "CI running")]});
        assert_eq!(row("r", "b", "⏳", vec![], &no_pr), "r\tb\t⏳\tCI running\t\n");
        let unsure = json!({"pr": null, "next": []});
        assert_eq!(row("r", "b", "⚠️", vec!["sd state gave no next steps".into()], &unsure), "r\tb\t⚠️\tsd state gave no next steps\t\n");
    }
}
