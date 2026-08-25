use crate::error::RunExit;
use std::fmt;

/// Final execution summary counts (spec section 9).
#[derive(Debug, Default, Clone)]
pub struct RunSummary {
    pub discovered: u64,
    /// Files that passed content validation, before deduplication
    /// (validated = parsed + failed + deduped).
    pub validated: u64,
    pub supported: u64,
    pub unsupported: u64,
    pub corrupt: u64,
    pub unreadable: u64,
    pub parsed: u64,
    pub skipped: u64,
    pub deduped: u64,
    pub failed: u64,
    pub records: u64,
    pub inaccessible: u64,
}

impl RunSummary {
    /// Exit-code policy: no failures = success (including zero artifacts
    /// found, spec 3.6); failures alongside successes = partial (5);
    /// failures with no successes = fatal (6).
    pub fn exit(&self) -> RunExit {
        if self.failed == 0 {
            RunExit::Success
        } else if self.parsed > 0 {
            RunExit::Partial
        } else {
            RunExit::Fatal
        }
    }
}

impl fmt::Display for RunSummary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "--- Summary ---")?;
        writeln!(f, "Discovered: {}", self.discovered)?;
        writeln!(f, "Validated: {}", self.validated)?;
        writeln!(f, "Supported: {}", self.supported)?;
        writeln!(f, "Unsupported: {}", self.unsupported)?;
        writeln!(f, "Corrupt: {}", self.corrupt)?;
        writeln!(f, "Unreadable: {}", self.unreadable)?;
        writeln!(f, "Parsed: {}", self.parsed)?;
        writeln!(f, "Skipped: {}", self.skipped)?;
        writeln!(f, "Deduplicated: {}", self.deduped)?;
        writeln!(f, "Failed: {}", self.failed)?;
        writeln!(f, "Inaccessible: {}", self.inaccessible)?;
        write!(f, "Records emitted: {}", self.records)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::RunExit;

    #[test]
    fn exit_code_rules() {
        let mut s = RunSummary::default();
        assert_eq!(s.exit(), RunExit::Success); // nothing found = success

        s.parsed = 3;
        assert_eq!(s.exit(), RunExit::Success);

        s.failed = 1;
        assert_eq!(s.exit(), RunExit::Partial); // some ok, some failed

        s.parsed = 0;
        assert_eq!(s.exit(), RunExit::Fatal); // everything failed
    }

    #[test]
    fn display_includes_all_required_counts() {
        let s = RunSummary {
            discovered: 7,
            validated: 6,
            supported: 5,
            unsupported: 1,
            corrupt: 0,
            unreadable: 0,
            parsed: 5,
            skipped: 1,
            deduped: 0,
            failed: 1,
            records: 1234,
            inaccessible: 2,
        };
        let text = s.to_string();
        for needle in [
            "Discovered: 7",
            "Validated: 6",
            "Parsed: 5",
            "Skipped: 1",
            "Deduplicated: 0",
            "Failed: 1",
            "Records emitted: 1234",
            "Inaccessible: 2",
        ] {
            assert!(text.contains(needle), "missing {needle:?} in {text:?}");
        }
    }
}
