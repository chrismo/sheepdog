// What an agent step may run: Claude Code permission rules. A leaf, so config
// can read it without depending on act (#338).

use regex::Regex;

/// Whether <cmd> is only plain words, safe to put in a permission rule as is.
pub fn plain_words(cmd: &str) -> bool {
    let plain = Regex::new(r"^[A-Za-z0-9_./:=@+-]+( [A-Za-z0-9_./:=@+-]+)*$").expect("a fixed pattern compiles");
    plain.is_match(cmd)
}

/// What the agent may run beyond reading and read-only git: <verify> when
/// it's plain words (a rule's * is a wildcard), and <allow>'s lines.
pub fn agent_rules(verify: &str, allow: &str) -> Vec<String> {
    let mut rules: Vec<String> = Vec::new();
    if plain_words(verify) {
        rules.push(format!("Bash({verify})"));
    }
    rules.extend(allow.lines().filter(|r| !r.is_empty()).map(str::to_string));
    rules
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_rules_take_plain_verify_as_a_bash_rule_and_allows_lines() {
        assert_eq!(agent_rules("cargo test", "WebFetch\n\nBash(make *)"), ["Bash(cargo test)", "WebFetch", "Bash(make *)"]);
        assert_eq!(agent_rules("make && make test", "WebFetch"), ["WebFetch"]);
        assert!(agent_rules("", "").is_empty());
    }

    #[test]
    fn rules_is_a_leaf() {
        let code = include_str!("rules.rs").split("#[cfg(test)]").next().unwrap();
        for needle in ["crate::", "super::"] {
            assert!(!code.contains(needle), "rules.rs reaches another module via {needle}");
        }
    }
}
