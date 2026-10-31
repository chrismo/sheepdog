// Each migrated verb's gate over Derived (#348). A verb joins MIGRATED in
// the PR that moves its guard here; parity runs on every tests/next.rs
// fixture and on generated states, whose fields are drawn independently on
// purpose: parity must hold on every state, not only those sd state makes.
// A Migrated's offered reads steps' key and who, which offer takes from
// Derived alone (tested below); cmd and say also read the record. A gate here destructures Derived without
// .., so a new field doesn't compile until each gate claims or ignores it.

use super::gate::Gate;
use crate::next::Derived;
use serde_json::Value;

/// A verb whose gate NEXT asks.
pub struct Migrated {
    pub verb: &'static str,
    /// Whether NEXT's steps offer the verb, by key and who only.
    pub offered: fn(&[Value]) -> bool,
    pub gate: fn(&Derived) -> Gate,
}

pub const MIGRATED: &[Migrated] = &[];

/// Each verb NEXT's <steps> offer while its gate refuses (runs() false).
pub fn parity(d: &Derived, steps: &[Value]) -> Vec<String> {
    MIGRATED
        .iter()
        .filter(|m| (m.offered)(steps))
        .filter_map(|m| match (m.gate)(d) {
            g if g.runs() => None,
            g => Some(format!("NEXT offers {} while its gate says {g:?}", m.verb)),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::next::derived_arb::check;
    use serde_json::json;

    /// The record from tests/next.rs's READY, which disagrees with most
    /// generated values: the steps' keys and who must not change with it.
    #[test]
    fn offer_takes_each_steps_key_and_who_from_derived_alone() {
        let ready = json!({"branch": "feat", "pr": {"number": 7, "state": "OPEN", "base": "main"},
            "sync": {"target": "main", "default": "main", "ahead": 1, "old_base": "b0b0b0b0b0b0"},
            "push": {"remote_sha": "h1", "ahead": 0}, "threads": {"owed_bot": 2},
            "question": {"at_head": true, "step": "address", "kind": "gave-up", "text": "why?"},
            "ci": {"verdict": "red", "for_head": true}});
        let shape = |steps: Vec<Value>| steps.iter().map(|s| format!("{}/{}", s["key"], s["who"])).collect::<Vec<_>>();
        check("offer's shape", |d| {
            let (a, b) = (shape(crate::next::offer(d, &json!({}), "origin")), shape(crate::next::offer(d, &ready, "origin")));
            if a == b {
                Ok(())
            } else {
                Err(format!("{a:?} vs {b:?}"))
            }
        });
    }

    #[test]
    fn next_offers_no_migrated_verb_its_gate_refuses_on_generated_states() {
        let s = json!({});
        check("parity", |d| {
            let steps = crate::next::offer(d, &s, "origin");
            let off = parity(d, &steps);
            if off.is_empty() {
                Ok(())
            } else {
                Err(off.join("; "))
            }
        });
    }
}
