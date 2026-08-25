use crate::winpath;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::Path;

/// Profile names treated as special (non-interactive) per spec section 4.1.
pub const SPECIAL_PROFILES: &[&str] = &[
    "default",
    "default user",
    "public",
    "all users",
    "systemprofile",
    "localservice",
    "networkservice",
];

/// What the source path says about the owning user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserDerivation {
    /// A normal interactive profile (Users/<u>, Documents and Settings/<u>).
    Interactive(String),
    /// A special/service profile; routed to system output.
    Special(String),
    /// A Recycle Bin SID directory with no resolved username.
    Sid(String),
    /// No user information in the path.
    None,
}

/// Output identity: which directory a record's output lands in (spec 4.3).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Identity {
    User(String),
    System,
    Unknown,
}

fn is_sid(seg: &str) -> bool {
    let mut parts = seg.split('-');
    if parts.next() != Some("S") {
        return false;
    }
    let rest: Vec<&str> = parts.collect();
    rest.len() >= 3
        && rest
            .iter()
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
}

/// Recognized Windows top-level markers that follow a capture's drive-root.
/// Their presence distinguishes a real drive anchor from a single-letter
/// username (e.g. `Users/a/...`).
fn is_top_level_marker(seg: &str) -> bool {
    winpath::eq_ci(seg, "users")
        || winpath::eq_ci(seg, "documents and settings")
        || winpath::eq_ci(seg, "windows")
        || winpath::eq_ci(seg, "$recycle.bin")
        || winpath::eq_ci(seg, "recycler")
        || winpath::eq_ci(seg, "recycled")
}

/// Index just after the capture's drive-root anchor: the last drive-root
/// segment that is followed (anywhere later) by a recognized Windows top-level
/// marker. The host filesystem prefix (a POSIX path, or a different drive) is
/// excluded so only in-capture attribution markers are considered. Returns 0
/// when no such anchor exists (back-compat for capture-relative / marker-less
/// paths).
fn drive_anchor(segs: &[String]) -> usize {
    for i in (0..segs.len()).rev() {
        if winpath::is_drive_segment(&segs[i])
            && segs[i + 1..].iter().any(|s| is_top_level_marker(s))
        {
            return i + 1;
        }
    }
    0
}

/// Derive the owning user from a source path (spec section 4.1).
pub fn derive_user(path: &Path) -> UserDerivation {
    let all = winpath::segments(&path.to_string_lossy());
    let anchor = drive_anchor(&all);
    let segs = &all[anchor..];
    for (i, seg) in segs.iter().enumerate() {
        // The candidate username segment must not be the final segment —
        // the final segment is the artifact file itself.
        let not_terminal = i + 1 < segs.len().saturating_sub(1);
        let next = segs.get(i + 1);

        if winpath::eq_ci(seg, "users") || winpath::eq_ci(seg, "documents and settings") {
            if let (true, Some(name)) = (not_terminal, next) {
                let lower = name.to_lowercase();
                if SPECIAL_PROFILES.contains(&lower.as_str()) {
                    return UserDerivation::Special(name.clone());
                }
                return UserDerivation::Interactive(name.clone());
            }
        }
        // Assumes the standard Windows directory name "ServiceProfiles" under "Windows".
        if winpath::eq_ci(seg, "serviceprofiles")
            && i > 0
            && winpath::eq_ci(&segs[i - 1], "windows")
        {
            if let (true, Some(name)) = (not_terminal, next) {
                return UserDerivation::Special(name.clone());
            }
        }
        let recycle = winpath::eq_ci(seg, "$recycle.bin")
            || winpath::eq_ci(seg, "recycler")
            || winpath::eq_ci(seg, "recycled");
        if recycle {
            if let Some(name) = next {
                if is_sid(name) {
                    return UserDerivation::Sid(name.clone());
                }
            }
        }
    }
    UserDerivation::None
}

/// Replace characters invalid on common filesystems with `_` (spec 4.2).
/// Dot-only names ("." and "..") are also neutralized to "_" to prevent
/// path traversal when a derived name is used as an output directory component.
pub fn sanitize_name(name: &str) -> String {
    sanitize_component(name)
}

/// Sanitize an untrusted capture-derived string for use as one filesystem
/// component on Windows, macOS, and Linux.
pub fn sanitize_component(name: &str) -> String {
    let mapped: String = name
        .trim()
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if (c as u32) < 0x20 => '_',
            c => c,
        })
        .collect::<String>()
        .trim_end_matches([' ', '.'])
        .to_string();
    let reserved = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    let mut safe = if mapped.is_empty() || mapped == "." || mapped == ".." {
        "_".to_string()
    } else if reserved.iter().any(|r| mapped.eq_ignore_ascii_case(r)) {
        format!("_{mapped}")
    } else {
        mapped
    };
    if safe.chars().count() > 80 {
        safe = safe.chars().take(80).collect();
    }
    safe
}

/// Deterministically disambiguates distinct raw identities that sanitize to
/// the same output component.
#[derive(Default)]
pub struct ComponentAllocator {
    assigned: HashMap<String, String>,
}

impl ComponentAllocator {
    pub fn allocate(&mut self, raw: &str) -> String {
        let normalized = raw.to_lowercase();
        let base = sanitize_component(raw);
        match self.assigned.get(&base) {
            None => {
                self.assigned.insert(base.clone(), normalized);
                base
            }
            Some(existing) if existing == &normalized => base,
            Some(_) => {
                let digest = Sha256::digest(normalized.as_bytes());
                let suffix: String = digest.iter().take(4).map(|b| format!("{b:02x}")).collect();
                let truncated: String = base.chars().take(71).collect();
                let candidate = format!("{truncated}-{suffix}");
                self.assigned.insert(candidate.clone(), normalized);
                candidate
            }
        }
    }
}

/// Assigns stable output identities across one run, handling sanitized-name
/// collisions between distinct profiles with a SHA-256-derived suffix.
#[derive(Default)]
pub struct Attributor {
    /// sanitized (possibly suffixed) name -> normalized profile key
    assigned: HashMap<String, String>,
}

impl Attributor {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn identity_for(&mut self, path: &Path) -> Identity {
        let (raw_name, profile_key) = match derive_user(path) {
            UserDerivation::Interactive(n) | UserDerivation::Sid(n) => {
                // SIDs are case-insensitive; lowercasing is harmless and keeps keys uniform.
                let key = n.to_lowercase();
                (n, key)
            }
            UserDerivation::Special(_) => return Identity::System,
            UserDerivation::None => return Identity::Unknown,
        };
        let base = sanitize_name(&raw_name);

        // "unknown" is reserved for Identity::Unknown's output directory
        // (users/unknown). A real Windows account named "unknown" would alias
        // that directory, so we always force such profiles into the suffixed
        // path rather than assigning them the plain base name.
        let force_suffix = base.eq_ignore_ascii_case("unknown");

        if force_suffix {
            // Fall through directly to the suffixed-name logic below.
        } else {
            match self.assigned.get(&base) {
                None => {
                    self.assigned.insert(base.clone(), profile_key);
                    return Identity::User(base);
                }
                Some(existing) if *existing == profile_key => return Identity::User(base),
                Some(_) => {}
            }
        }

        let digest = Sha256::digest(profile_key.as_bytes());
        let short_suffix: String = digest.iter().take(4).map(|b| format!("{b:02x}")).collect();
        let short_name = format!("{base}-{short_suffix}");
        match self.assigned.get(&short_name) {
            Some(existing) if *existing == profile_key => {
                // Stable re-encounter of the same profile via the suffixed name.
                Identity::User(short_name)
            }
            Some(_) => {
                // 32-bit prefix collision: fall back to full SHA-256 suffix.
                let full_suffix: String = digest.iter().map(|b| format!("{b:02x}")).collect();
                let full_name = format!("{base}-{full_suffix}");
                self.assigned.insert(full_name.clone(), profile_key);
                Identity::User(full_name)
            }
            None => {
                self.assigned.insert(short_name.clone(), profile_key);
                Identity::User(short_name)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn derive(p: &str) -> UserDerivation {
        derive_user(Path::new(p))
    }

    #[test]
    fn standard_profile_patterns() {
        assert_eq!(
            derive("C/Users/alice/AppData/Roaming/x.lnk"),
            UserDerivation::Interactive("alice".into())
        );
        assert_eq!(
            derive("C:/Users/Bob/NTUSER.DAT"),
            UserDerivation::Interactive("Bob".into())
        );
        assert_eq!(
            derive("uploads/auto/C%3A/Users/charlie/Desktop/a.lnk"),
            UserDerivation::Interactive("charlie".into())
        );
        assert_eq!(
            derive("D/Documents and Settings/old guy/Recent/a.lnk"),
            UserDerivation::Interactive("old guy".into())
        );
        // percent-encoded non-ASCII usernames decode correctly
        assert_eq!(
            derive("C/Users/Ren%C3%A9/Desktop/a.lnk"),
            UserDerivation::Interactive("Ren\u{e9}".into())
        );
    }

    #[test]
    fn host_prefix_with_users_segment_does_not_mask_in_capture_sid() {
        // capture stored under the analyst's own home dir
        let p = "/Users/carlosperez/Documents/devprojects/test captures/Coll/uploads/auto/C%3A/$Recycle.Bin/S-1-5-21-111-222-333-500/$I123456";
        assert_eq!(
            derive_user(Path::new(p)),
            UserDerivation::Sid("S-1-5-21-111-222-333-500".into())
        );
    }

    #[test]
    fn host_prefix_does_not_mask_in_capture_user() {
        let p = "/Users/carlosperez/cases/test captures/Coll/uploads/auto/C%3A/Users/alice/AppData/Roaming/x.lnk";
        assert_eq!(
            derive_user(Path::new(p)),
            UserDerivation::Interactive("alice".into())
        );
    }

    #[test]
    fn host_prefix_with_documents_and_settings_in_capture() {
        let p = "/home/analyst/Users/bob/evidence/D%3A/Documents and Settings/charlie/Recent/x.lnk";
        // host has BOTH /Users/bob AND the dir literally named Users; the in-capture
        // anchor is the D: drive root, so charlie wins.
        assert_eq!(
            derive_user(Path::new(p)),
            UserDerivation::Interactive("charlie".into())
        );
    }

    #[test]
    fn single_letter_username_is_not_mistaken_for_drive_anchor() {
        // 'a' is a single-letter username, not a drive root (no Windows marker follows it)
        let p = "C%3A/Users/a/AppData/Roaming/x.lnk";
        assert_eq!(
            derive_user(Path::new(p)),
            UserDerivation::Interactive("a".into())
        );
    }

    #[test]
    fn no_drive_anchor_falls_back_to_first_match() {
        // marker-less / already-relative path: unchanged behavior
        assert_eq!(
            derive_user(Path::new("Users/bob/Desktop/x.lnk")),
            UserDerivation::Interactive("bob".into())
        );
    }

    #[test]
    fn special_profiles_are_not_interactive_users() {
        assert_eq!(
            derive("C/Windows/ServiceProfiles/LocalService/NTUSER.DAT"),
            UserDerivation::Special("LocalService".into())
        );
        assert_eq!(
            derive("C/Users/Default/NTUSER.DAT"),
            UserDerivation::Special("Default".into())
        );
        assert_eq!(
            derive("C/Users/All Users/x"),
            UserDerivation::Special("All Users".into())
        );
    }

    #[test]
    fn recycle_bin_sid_fallback() {
        assert_eq!(
            derive("C/$Recycle.Bin/S-1-5-21-1004336348-1177238915-682003330-1001/$I123456"),
            UserDerivation::Sid("S-1-5-21-1004336348-1177238915-682003330-1001".into())
        );
        assert_eq!(
            derive("C/RECYCLER/S-1-5-21-99-88-77-500/INFO2"),
            UserDerivation::Sid("S-1-5-21-99-88-77-500".into())
        );
    }

    #[test]
    fn unattributable_paths_are_none() {
        assert_eq!(
            derive("C/Windows/Prefetch/CMD.EXE-12345678.pf"),
            UserDerivation::None
        );
        // username segment must not be the final segment (that is the file itself)
        assert_eq!(derive("C/Users/desktop.ini"), UserDerivation::None);
    }

    #[test]
    fn sanitization_replaces_invalid_chars() {
        assert_eq!(sanitize_name("a<b>:c\"d/e\\f|g?h*i"), "a_b__c_d_e_f_g_h_i");
    }

    #[test]
    fn sanitize_component_handles_empty_reserved_controls_and_length() {
        assert_eq!(sanitize_component(""), "_");
        assert_eq!(sanitize_component(".."), "_");
        assert_eq!(sanitize_component("CON"), "_CON");
        assert_eq!(sanitize_component("a/b\\c\u{0001}"), "a_b_c_");
        assert_eq!(sanitize_component(&"é".repeat(100)).chars().count(), 80);
    }

    #[test]
    fn component_allocator_suffixes_sanitized_collisions() {
        let mut allocator = ComponentAllocator::default();
        let first = allocator.allocate("a/b");
        let second = allocator.allocate("a\\b");
        assert_eq!(first, "a_b");
        assert!(second.starts_with("a_b-"));
        assert_ne!(first, second);
    }

    #[test]
    fn attributor_assigns_identities_and_handles_collisions() {
        let mut a = Attributor::new();
        // same user, same path family -> same directory
        let id1 = a.identity_for(Path::new("C/Users/alice/Recent/1.lnk"));
        let id2 = a.identity_for(Path::new("C/Users/alice/Desktop/2.lnk"));
        assert_eq!(id1, Identity::User("alice".into()));
        assert_eq!(id1, id2);
        // special profile -> system
        assert_eq!(
            a.identity_for(Path::new("C/Windows/ServiceProfiles/LocalService/x")),
            Identity::System
        );
        // no derivation -> unknown
        assert_eq!(
            a.identity_for(Path::new("C/Windows/Prefetch/X.pf")),
            Identity::Unknown
        );
        // two distinct profiles sanitizing to the same name -> hash suffix on second
        let c1 = a.identity_for(Path::new("C/Users/bob?/f.lnk"));
        let c2 = a.identity_for(Path::new("C/Users/bob*/f.lnk"));
        assert_eq!(c1, Identity::User("bob_".into()));
        match c2 {
            Identity::User(n) => {
                assert!(n.starts_with("bob_-"), "got {n}");
                assert_eq!(n.len(), "bob_-".len() + 8);
            }
            other => panic!("expected User, got {other:?}"),
        }
    }

    #[test]
    fn dot_names_cannot_escape_output_root() {
        assert_eq!(sanitize_name(".."), "_");
        assert_eq!(sanitize_name("."), "_");
        let mut a = Attributor::new();
        let id = a.identity_for(Path::new("C/Users/../AppData/file.lnk"));
        assert_eq!(id, Identity::User("_".into()));
    }

    #[test]
    fn attributor_is_stable_on_reencounter_after_collision() {
        let mut a = Attributor::new();
        let c1 = a.identity_for(Path::new("C/Users/bob?/f.lnk"));
        let c2 = a.identity_for(Path::new("C/Users/bob*/f.lnk"));
        let c1again = a.identity_for(Path::new("C/Users/bob?/g.lnk"));
        let c2again = a.identity_for(Path::new("C/Users/bob*/g.lnk"));
        assert_eq!(c1, c1again);
        assert_eq!(c2, c2again);
        assert_ne!(c1, c2);
    }

    #[test]
    fn literal_unknown_user_does_not_alias_unknown_directory() {
        let mut a = Attributor::new();
        let id = a.identity_for(Path::new("C/Users/unknown/f.lnk"));
        match &id {
            Identity::User(n) => {
                assert_ne!(n.to_lowercase(), "unknown");
                assert!(n.to_lowercase().starts_with("unknown-"), "got {n}");
            }
            other => panic!("expected suffixed User, got {other:?}"),
        }
        // stable on re-encounter
        let again = a.identity_for(Path::new("C/Users/unknown/g.lnk"));
        assert_eq!(id, again);
    }
}
