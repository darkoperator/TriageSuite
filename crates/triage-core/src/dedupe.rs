use sha1::{Digest, Sha1};
use std::collections::HashSet;
use std::io::Read;
use std::path::Path;

/// Content-hash dedupe with SHA-1 (Zimmerman-compatible behavior:
/// first discovered file wins; later identical content is skipped).
#[derive(Default)]
pub struct DedupeSet {
    seen: HashSet<[u8; 20]>,
}

impl DedupeSet {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns Ok(true) if this content is new, Ok(false) if a file with
    /// identical content was already inserted.
    pub fn insert(&mut self, path: &Path) -> Result<bool, std::io::Error> {
        let mut hasher = Sha1::new();
        let mut file = std::fs::File::open(path)?;
        let mut buf = [0u8; 64 * 1024];
        loop {
            let n = file.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        Ok(self.seen.insert(hasher.finalize().into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_identical_content_regardless_of_name() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a.pf");
        let b = tmp.path().join("b.pf");
        let c = tmp.path().join("c.pf");
        std::fs::write(&a, b"same bytes").unwrap();
        std::fs::write(&b, b"same bytes").unwrap();
        std::fs::write(&c, b"different").unwrap();

        let mut set = DedupeSet::new();
        assert!(set.insert(&a).unwrap()); // first wins
        assert!(!set.insert(&b).unwrap()); // duplicate content
        assert!(set.insert(&c).unwrap());
    }

    #[test]
    fn unreadable_file_is_an_error_not_a_duplicate() {
        let mut set = DedupeSet::new();
        assert!(set.insert(std::path::Path::new("/nonexistent/x")).is_err());
    }
}
