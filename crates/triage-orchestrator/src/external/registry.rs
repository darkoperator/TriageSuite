//! The external-tool table. Adding a tool means adding its module, its typed
//! field on `ResolvedConfig`, and one entry here.

use super::tool::ExternalTool;
use super::tools::hayabusa::Hayabusa;
use super::tools::takajo::Takajo;

/// Every external tool, in execution order.
///
/// This order is load-bearing three times over: it is the order invocations run
/// in, the order their reports appear in `run_manifest.json` and on the console,
/// and the dependency order — a tool's `requires()` slot must be published by a
/// strictly earlier entry. All three are covered by the tests below.
pub const ALL: &[&dyn ExternalTool] = &[&Hayabusa, &Takajo];

/// The stable `--skip` keys, in registry order.
pub fn keys() -> Vec<&'static str> {
    ALL.iter().map(|t| t.key()).collect()
}

pub fn get(key: &str) -> Option<&'static dyn ExternalTool> {
    ALL.iter().copied().find(|t| t.key() == key)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A collision with an in-process key would be silently harmful, not loud:
    /// `main.rs` strips external keys out of `--skip` before the in-process
    /// registry validates it, so a shared key would turn `--skip <key>` into a
    /// no-op for the parser instead of an error.
    #[test]
    fn keys_are_unique_and_disjoint_from_the_in_process_registry() {
        let ext = keys();
        let mut sorted = ext.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), ext.len(), "external keys must be unique");

        for entry in crate::registry::all_tools() {
            assert!(
                !ext.contains(&entry.key),
                "external key {:?} collides with an in-process registry key",
                entry.key
            );
        }
    }

    #[test]
    fn every_key_resolves_through_get() {
        for key in keys() {
            assert_eq!(get(key).map(|t| t.key()), Some(key));
        }
        assert!(get("definitely-not-a-tool").is_none());
    }

    /// The driver satisfies `requires()` from a map built as it walks `ALL`, so a
    /// tool that depends on a *later* entry would silently never run.
    #[test]
    fn every_requirement_is_published_by_an_earlier_tool() {
        let mut published: Vec<&'static str> = Vec::new();
        for tool in ALL {
            if let Some(req) = tool.requires() {
                assert!(
                    published.contains(&req.slot),
                    "{} requires slot {:?}, which no earlier tool publishes",
                    tool.key(),
                    req.slot
                );
            }
            published.extend(tool.publishable_slots());
        }
    }
}
