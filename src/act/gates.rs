// Each migrated verb's gate over Derived (#348). A verb joins MIGRATED in
// the PR that moves its guard here; parity runs on every tests/next.rs
// fixture and on generated states. A gate here destructures Derived without
// .., so a new field doesn't compile until each gate claims or ignores it.

use super::gate::Gate;
use crate::next::Derived;
use serde_json::Value;

/// A verb whose gate NEXT asks.
pub struct Migrated {
    pub verb: &'static str,
    /// Whether NEXT's steps offer the verb.
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
