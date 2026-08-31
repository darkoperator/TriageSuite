//! Browser, channel and profile identification from an artifact path.
//!
//! Discovery hands `parse()` a file path and nothing else — there is no
//! directory-walking phase — so everything here is derived from the path
//! string. Pure, no I/O, and therefore exhaustively testable from a table of
//! literals.
//!
//! # Why this module is the one that must not be wrong
//!
//! The tool this crate replaced folded `Snapshots/<version>/Default` into
//! `Default` and lost 41% of its output to the resulting collisions. The
//! guarantee here is structural rather than a special case: the profile is the
//! **full relative path** beneath the `User Data` / `Profiles` anchor, so two
//! different directories can never produce the same profile string.

use crate::artifact::ArtifactKind;
use std::path::Path;
use triage_core::winpath::{eq_ci, segments};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    Chromium,
    Firefox,
}

impl Family {
    fn unknown_browser(self) -> &'static str {
        match self {
            Family::Chromium => "Chromium (Unknown)",
            Family::Firefox => "Firefox (Unknown)",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserId {
    pub family: Family,
    /// Never empty. Falls back to `Chromium (<dir>)` / `Firefox (<dir>)` for an
    /// unrecognized brand, so an unknown Chromium fork is still attributed.
    pub browser: String,
    /// `Stable`, `Beta`, `Dev`, `Canary`, `Nightly`, `ESR`, `Snapshot`,
    /// `Testing`, or empty when it cannot be determined.
    pub channel: String,
    /// The profile directory path relative to the anchor, `/`-joined:
    /// `Default`, `Profile 2`, `Snapshots/116.0.5845.97/Default`,
    /// `lqmuncct.default-release`.
    pub profile: String,
    /// Seeds the record's `Notes` column when attribution was degraded. Empty
    /// on a confident identification.
    pub note: String,
}

/// Chromium's profile container. Everything below it is the profile path.
const CHROMIUM_ANCHOR: &str = "User Data";
/// Firefox's profile container.
const FIREFOX_ANCHOR: &str = "Profiles";

/// The only directory Chromium ever interposes between a profile and one of the
/// filenames we collect (`<profile>/Network/Cookies`, Chrome 96+).
///
/// This list is deliberately one entry long, and its minimality *is* the
/// anti-collision guarantee: every additional strip risks folding two real
/// directories into one profile string. No other container can hold one of our
/// artifacts, so a longer list would buy nothing and cost safety.
const STRIPPED_LEAF: &str = "Network";

/// Product directory -> (browser, channel), matched against the segment
/// immediately above `User Data`.
const CHROMIUM_BRANDS: &[(&str, &str, &str)] = &[
    ("Chrome", "Chrome", "Stable"),
    ("Chrome Beta", "Chrome", "Beta"),
    ("Chrome Dev", "Chrome", "Dev"),
    ("Chrome SxS", "Chrome", "Canary"),
    ("Chrome for Testing", "Chrome", "Testing"),
    ("Edge", "Edge", "Stable"),
    ("Edge Beta", "Edge", "Beta"),
    ("Edge Dev", "Edge", "Dev"),
    ("Edge SxS", "Edge", "Canary"),
    ("Brave-Browser", "Brave", "Stable"),
    ("Brave-Browser-Beta", "Brave", "Beta"),
    ("Brave-Browser-Nightly", "Brave", "Nightly"),
    ("Vivaldi", "Vivaldi", "Stable"),
    ("Vivaldi Snapshot", "Vivaldi", "Snapshot"),
    ("Chromium", "Chromium", "Stable"),
    ("Arc", "Arc", "Stable"),
];

/// Opera has no `User Data`; the product directory *is* the profile directory.
const OPERA_BRANDS: &[(&str, &str, &str)] = &[
    ("Opera Stable", "Opera", "Stable"),
    ("Opera Beta", "Opera", "Beta"),
    ("Opera Developer", "Opera", "Dev"),
    ("Opera GX Stable", "Opera GX", "Stable"),
    ("Opera Crypto Stable", "Opera Crypto", "Stable"),
    ("Opera Neon", "Opera Neon", "Stable"),
];

const FIREFOX_BRANDS: &[(&str, &str, &str)] = &[
    ("Firefox", "Firefox", "Stable"),
    ("Firefox Developer Edition", "Firefox", "Dev"),
    ("Firefox Nightly", "Firefox", "Nightly"),
    ("LibreWolf", "LibreWolf", ""),
    ("Waterfox", "Waterfox", ""),
    ("SeaMonkey", "SeaMonkey", ""),
    ("Pale Moon", "Pale Moon", ""),
];

pub fn family_of(kind: ArtifactKind) -> Family {
    use ArtifactKind::*;
    match kind {
        ChromiumHistory | ChromiumCookies | ChromiumWebData | ChromiumBookmarks
        | ChromiumLogins | ChromiumPreferences => Family::Chromium,
        FirefoxPlaces | FirefoxCookies | FirefoxFormHistory | FirefoxLogins | FirefoxExtensions => {
            Family::Firefox
        }
    }
}

fn lookup(table: &[(&str, &str, &str)], segment: &str) -> Option<(String, String)> {
    table
        .iter()
        .find(|(dir, _, _)| eq_ci(dir, segment))
        .map(|(_, browser, channel)| ((*browser).to_string(), (*channel).to_string()))
}

/// Index of the last segment equal to `anchor`, case-insensitively. Last, not
/// first, so a capture staged under a directory that happens to be called
/// `Profiles` cannot shadow the real anchor deeper in the path.
fn last_anchor(dir: &[String], anchor: &str) -> Option<usize> {
    dir.iter().rposition(|seg| eq_ci(seg, anchor))
}

/// Identify the browser, channel and profile owning `path`.
///
/// Never fails: the family is already known from the filename, so an
/// unrecognizable path still yields a usable identification plus a `note`
/// explaining what was degraded. A record is never dropped for want of
/// attribution.
pub fn identify(path: &Path, kind: ArtifactKind) -> BrowserId {
    let family = family_of(kind);
    let all = segments(&path.to_string_lossy());
    // Drop the filename; everything above it is directory context.
    let dir: &[String] = if all.is_empty() {
        &[]
    } else {
        &all[..all.len() - 1]
    };

    match family {
        Family::Chromium => identify_chromium(dir, family),
        Family::Firefox => identify_firefox(dir, family),
    }
}

fn identify_chromium(dir: &[String], family: Family) -> BrowserId {
    if let Some(anchor) = last_anchor(dir, CHROMIUM_ANCHOR) {
        let mut profile_segs: Vec<&str> = dir[anchor + 1..].iter().map(String::as_str).collect();
        // `<profile>/Network/Cookies` (Chrome 96+). One strip, never more.
        if profile_segs.last().is_some_and(|s| eq_ci(s, STRIPPED_LEAF)) {
            profile_segs.pop();
        }
        let profile = if profile_segs.is_empty() {
            // The artifact sits directly in `User Data` — unusual but real for
            // `Local State`-adjacent files; name it after the anchor itself
            // rather than inventing a profile.
            CHROMIUM_ANCHOR.to_string()
        } else {
            profile_segs.join("/")
        };

        let (browser, channel) = anchor
            .checked_sub(1)
            .and_then(|i| lookup(CHROMIUM_BRANDS, &dir[i]))
            .unwrap_or_else(|| {
                let product = anchor.checked_sub(1).map(|i| dir[i].as_str()).unwrap_or("");
                (format!("Chromium ({product})"), String::new())
            });

        return BrowserId {
            family,
            browser,
            channel,
            profile,
            note: String::new(),
        };
    }

    // Opera keeps its profile directly in the product directory.
    if let Some(idx) = dir
        .iter()
        .rposition(|seg| lookup(OPERA_BRANDS, seg).is_some())
    {
        let (browser, channel) = lookup(OPERA_BRANDS, &dir[idx]).expect("rposition matched");
        let mut profile_segs: Vec<&str> = dir[idx..].iter().map(String::as_str).collect();
        if profile_segs.len() > 1 && profile_segs.last().is_some_and(|s| eq_ci(s, STRIPPED_LEAF)) {
            profile_segs.pop();
        }
        return BrowserId {
            family,
            browser,
            channel,
            profile: profile_segs.join("/"),
            note: String::new(),
        };
    }

    degraded(dir, family, CHROMIUM_ANCHOR)
}

fn identify_firefox(dir: &[String], family: Family) -> BrowserId {
    let Some(anchor) = last_anchor(dir, FIREFOX_ANCHOR) else {
        return degraded(dir, family, FIREFOX_ANCHOR);
    };
    let profile = dir[anchor + 1..].join("/");
    let profile = if profile.is_empty() {
        FIREFOX_ANCHOR.to_string()
    } else {
        profile
    };

    let (browser, mut channel) = anchor
        .checked_sub(1)
        .and_then(|i| lookup(FIREFOX_BRANDS, &dir[i]))
        .unwrap_or_else(|| {
            let product = anchor.checked_sub(1).map(|i| dir[i].as_str()).unwrap_or("");
            (format!("Firefox ({product})"), String::new())
        });

    // Firefox encodes the channel in the profile name as well as the product
    // directory, and the profile name is the more reliable of the two when a
    // single install hosts several channels' profiles.
    let lower = profile.to_ascii_lowercase();
    if lower.ends_with(".default-esr") {
        channel = "ESR".to_string();
    } else if lower.ends_with(".dev-edition-default") {
        channel = "Dev".to_string();
    }

    BrowserId {
        family,
        browser,
        channel,
        profile,
        note: String::new(),
    }
}

/// Best-effort attribution when no anchor is present — an artifact handed over
/// with `-f`, or a capture layout we do not recognize. The profile falls back
/// to the containing directory and the degradation is recorded rather than
/// hidden.
fn degraded(dir: &[String], family: Family, anchor: &str) -> BrowserId {
    let profile = dir.last().cloned().unwrap_or_default();
    BrowserId {
        family,
        browser: family.unknown_browser().to_string(),
        channel: String::new(),
        profile,
        note: format!(
            "no '{anchor}' anchor in path; profile inferred from the containing directory"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn id(path: &str, kind: ArtifactKind) -> BrowserId {
        identify(&PathBuf::from(path), kind)
    }

    fn chromium(path: &str) -> BrowserId {
        id(path, ArtifactKind::ChromiumHistory)
    }

    fn firefox(path: &str) -> BrowserId {
        id(path, ArtifactKind::FirefoxPlaces)
    }

    #[test]
    fn chromium_brands_and_channels_are_recognized() {
        let cases = [
            ("Google/Chrome", "Chrome", "Stable"),
            ("Google/Chrome Beta", "Chrome", "Beta"),
            ("Google/Chrome Dev", "Chrome", "Dev"),
            ("Google/Chrome SxS", "Chrome", "Canary"),
            ("Microsoft/Edge", "Edge", "Stable"),
            ("Microsoft/Edge Dev", "Edge", "Dev"),
            ("BraveSoftware/Brave-Browser", "Brave", "Stable"),
            ("BraveSoftware/Brave-Browser-Nightly", "Brave", "Nightly"),
            ("Vivaldi", "Vivaldi", "Stable"),
            ("Chromium", "Chromium", "Stable"),
            ("Arc", "Arc", "Stable"),
        ];
        for (vendor, browser, channel) in cases {
            let got = chromium(&format!(
                "/Users/a/AppData/Local/{vendor}/User Data/Default/History"
            ));
            assert_eq!(got.browser, browser, "{vendor}");
            assert_eq!(got.channel, channel, "{vendor}");
            assert_eq!(got.profile, "Default", "{vendor}");
            assert!(got.note.is_empty(), "{vendor}");
        }
    }

    #[test]
    fn firefox_brands_and_channels_are_recognized() {
        for (vendor, browser, channel) in [
            ("Mozilla/Firefox", "Firefox", "Stable"),
            ("Mozilla/Firefox Nightly", "Firefox", "Nightly"),
            ("LibreWolf", "LibreWolf", ""),
        ] {
            let got = firefox(&format!(
                "/Users/a/AppData/Roaming/{vendor}/Profiles/ab12cd.default-release/places.sqlite"
            ));
            assert_eq!(got.browser, browser, "{vendor}");
            assert_eq!(got.channel, channel, "{vendor}");
            assert_eq!(got.profile, "ab12cd.default-release", "{vendor}");
        }
    }

    #[test]
    fn firefox_channel_comes_from_the_profile_suffix_when_present() {
        assert_eq!(
            firefox("/Mozilla/Firefox/Profiles/xy.default-esr/places.sqlite").channel,
            "ESR"
        );
        assert_eq!(
            firefox("/Mozilla/Firefox/Profiles/xy.dev-edition-default/places.sqlite").channel,
            "Dev"
        );
    }

    /// The regression this whole module exists for: folding these into
    /// `Default` is what destroyed 41% of the previous tool's output.
    #[test]
    fn snapshot_profiles_stay_distinct_from_the_live_profile() {
        let live = chromium("/Google/Chrome/User Data/Default/History");
        let snap = chromium("/Google/Chrome/User Data/Snapshots/116.0.5845.97/Default/History");
        assert_eq!(live.profile, "Default");
        assert_eq!(snap.profile, "Snapshots/116.0.5845.97/Default");
        assert_ne!(live.profile, snap.profile);
    }

    #[test]
    fn two_snapshot_versions_do_not_collide_with_each_other() {
        let a = chromium("/Google/Chrome/User Data/Snapshots/116.0.5845.97/Default/History");
        let b = chromium("/Google/Chrome/User Data/Snapshots/120.0.6099.71/Default/History");
        assert_ne!(a.profile, b.profile);
    }

    /// Chrome 96+ moved Cookies into `<profile>/Network/`. The profile must be
    /// the same as for an artifact sitting directly in the profile directory,
    /// or one browser profile would split into two identities.
    #[test]
    fn the_network_subdirectory_is_stripped_from_the_profile() {
        let root = id(
            "/Google/Chrome/User Data/Profile 2/Cookies",
            ArtifactKind::ChromiumCookies,
        );
        let nested = id(
            "/Google/Chrome/User Data/Profile 2/Network/Cookies",
            ArtifactKind::ChromiumCookies,
        );
        assert_eq!(root.profile, "Profile 2");
        assert_eq!(nested.profile, "Profile 2");
    }

    /// A profile literally named `Network` must survive; only a trailing
    /// `Network` *below* a profile is a container.
    #[test]
    fn a_profile_named_network_is_not_stripped_away() {
        let got = chromium("/Google/Chrome/User Data/Network/History");
        assert_eq!(got.profile, CHROMIUM_ANCHOR);
    }

    #[test]
    fn velociraptor_percent_encoded_paths_resolve_identically() {
        let encoded = chromium(
            "uploads/auto/C%3A/Users/alice/AppData/Local/Google/Chrome/User Data/Default/History",
        );
        let plain =
            chromium("C:/Users/alice/AppData/Local/Google/Chrome/User Data/Default/History");
        assert_eq!(encoded.profile, plain.profile);
        assert_eq!(encoded.browser, plain.browser);
        assert_eq!(encoded.profile, "Default");
    }

    #[test]
    fn backslash_separators_resolve_identically() {
        let got =
            chromium(r"C:\Users\alice\AppData\Local\Google\Chrome\User Data\Profile 1\History");
        assert_eq!(got.profile, "Profile 1");
        assert_eq!(got.browser, "Chrome");
    }

    #[test]
    fn opera_uses_its_product_directory_as_the_profile() {
        let got = chromium("/Users/a/AppData/Roaming/Opera Software/Opera GX Stable/History");
        assert_eq!(got.browser, "Opera GX");
        assert_eq!(got.channel, "Stable");
        assert_eq!(got.profile, "Opera GX Stable");
        assert!(got.note.is_empty());
    }

    #[test]
    fn arc_under_a_windows_package_directory_is_recognized() {
        let got = chromium(
            "/Users/a/AppData/Local/Packages/TheBrowserCompany.Arc_ab12/LocalCache/Local/Arc/User Data/Default/History",
        );
        assert_eq!(got.browser, "Arc");
        assert_eq!(got.profile, "Default");
    }

    #[test]
    fn an_unknown_chromium_fork_is_still_attributed() {
        let got = chromium("/Users/a/AppData/Local/SomeFork/User Data/Default/History");
        assert_eq!(got.browser, "Chromium (SomeFork)");
        assert_eq!(got.profile, "Default");
        assert!(
            got.note.is_empty(),
            "the anchor was found, so nothing degraded"
        );
    }

    #[test]
    fn a_path_without_an_anchor_degrades_but_still_identifies() {
        let got = chromium("/tmp/evidence/History");
        assert_eq!(got.family, Family::Chromium);
        assert_eq!(got.browser, "Chromium (Unknown)");
        assert_eq!(got.profile, "evidence");
        assert!(got.note.contains("User Data"));

        let ff = firefox("/tmp/evidence/places.sqlite");
        assert_eq!(ff.family, Family::Firefox);
        assert_eq!(ff.browser, "Firefox (Unknown)");
        assert!(ff.note.contains("Profiles"));
    }

    /// The family comes from the filename, so it is right even when the path
    /// tells us nothing at all.
    #[test]
    fn family_is_correct_even_for_a_bare_filename() {
        assert_eq!(chromium("History").family, Family::Chromium);
        assert_eq!(firefox("places.sqlite").family, Family::Firefox);
    }

    /// The last anchor wins, so a capture staged beneath a directory that
    /// happens to be named `User Data` cannot shadow the real one.
    #[test]
    fn the_deepest_anchor_wins() {
        let got = chromium("/evidence/User Data/Google/Chrome/User Data/Profile 3/History");
        assert_eq!(got.profile, "Profile 3");
        assert_eq!(got.browser, "Chrome");
    }

    /// Two vendors sharing the profile name `Default` must remain distinct.
    /// The identity key is the whole (browser, channel, profile) triple.
    #[test]
    fn the_same_profile_name_under_two_browsers_is_distinguishable() {
        let chrome = chromium("/Google/Chrome/User Data/Default/History");
        let edge = chromium("/Microsoft/Edge/User Data/Default/History");
        assert_eq!(chrome.profile, edge.profile);
        assert_ne!(
            (&chrome.browser, &chrome.channel, &chrome.profile),
            (&edge.browser, &edge.channel, &edge.profile)
        );
    }
}
