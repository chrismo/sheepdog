// sd view <branch> (#458): which of the watcher's cached branches an
// argument names, for the Flock's #b=<id> drill-in. No ranking and no
// picker: one match opens, anything else is listed.

use super::flock::Cached;
use crate::val::at;

/// The id of the one cached branch <arg> names: the branch named exactly,
/// else each whose PR number is <arg> or whose name holds it, ignoring
/// case. Err is what to print when that isn't one branch.
pub fn pick(list: &[Cached], arg: &str) -> Result<String, String> {
    let exact: Vec<&Cached> = list.iter().filter(|c| c.branch == arg).collect();
    let found = if exact.is_empty() {
        let (low, n) = (arg.to_lowercase(), arg.parse::<u64>().ok());
        list.iter().filter(|c| (n.is_some() && pr(c) == n) || c.branch.to_lowercase().contains(&low)).collect()
    } else {
        exact
    };
    match found.as_slice() {
        [one] => Ok(one.id.clone()),
        [] => Err(format!("{arg} matches no branch sd watch tracks")),
        many => {
            let names: Vec<String> = many.iter().map(|c| format!("  {} {}", c.repo, c.branch)).collect();
            Err(format!("{arg} matches {} branches:\n{}", many.len(), names.join("\n")))
        }
    }
}

fn pr(c: &Cached) -> Option<u64> {
    c.doc.as_ref().and_then(|d| at(d, "state.pr.number").as_u64())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn c(branch: &str, pr: u64) -> Cached {
        Cached { id: format!("r-1/{branch}"), repo: "r".into(), branch: branch.into(), doc: Some(json!({"state": {"pr": {"number": pr}}})) }
    }

    #[test]
    fn an_exact_name_wins_over_a_part_of_another() {
        let list = [c("view", 1), c("view-arg", 2)];
        assert_eq!(pick(&list, "view"), Ok("r-1/view".into()));
    }

    #[test]
    fn a_number_matches_a_pr_or_a_part_of_a_name() {
        let list = [c("377-atlas", 449), c("449-other", 3), c("x", 5)];
        assert_eq!(pick(&list, "5"), Ok("r-1/x".into()));
        assert!(pick(&list, "449").unwrap_err().contains("matches 2 branches"));
    }

    #[test]
    fn a_part_ignores_case() {
        assert_eq!(pick(&[c("377-Atlas-World", 1)], "atlas-w"), Ok("r-1/377-Atlas-World".into()));
    }
}
