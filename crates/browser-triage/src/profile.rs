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
//! **full relative path** below the product directory, so two different
//! directories can never produce the same profile string.
//!
//! Identification is table-driven, over [`LAYOUTS`]. Each row names a product
//! directory and the container that platform interposes between it and the
//! profile, and the container is consumed only when it is actually present —
//! Windows has one, macOS and Linux do not. An earlier revision required the
//! container, so every macOS and Linux path fell through to [`degraded`] and
//! Chrome, Edge and Brave on one host collapsed into a single identity.

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

/// Chromium's profile container on Windows. Everything below it is the profile
/// path. macOS and Linux have no equivalent, which is why it is optional in
/// [`Layout`] rather than being required to identify a Chromium install.
const CHROMIUM_ANCHOR: &str = "User Data";
/// Firefox's profile container on Windows and macOS. Linux keeps profiles
/// directly under `~/.mozilla/firefox`, with no container.
const FIREFOX_ANCHOR: &str = "Profiles";

/// The only directory Chromium ever interposes between a profile and one of the
/// filenames we collect (`<profile>/Network/Cookies`, Chrome 96+).
///
/// This list is deliberately one entry long, and its minimality *is* the
/// anti-collision guarantee: every additional strip risks folding two real
/// directories into one profile string. No other container can hold one of our
/// artifacts, so a longer list would buy nothing and cost safety.
const STRIPPED_LEAF: &str = "Network";

/// One known install layout.
///
/// The container is optional *at match time*, not per-row: Windows interposes
/// `User Data` between the product directory and the profile, while macOS and
/// Linux put the profile directly under the product. One row therefore covers
/// a product on every platform, and the container is consumed only when it is
/// actually present.
struct Layout {
    /// The product directory as it appears on disk, matched case-insensitively.
    product: &'static str,
    container: Option<&'static str>,
    browser: &'static str,
    channel: &'static str,
    family: Family,
}

const fn chromium(product: &'static str, browser: &'static str, channel: &'static str) -> Layout {
    Layout {
        product,
        container: Some(CHROMIUM_ANCHOR),
        browser,
        channel,
        family: Family::Chromium,
    }
}

/// Opera never has a container: the product directory *is* the profile.
const fn opera(product: &'static str, browser: &'static str, channel: &'static str) -> Layout {
    Layout {
        product,
        container: None,
        browser,
        channel,
        family: Family::Chromium,
    }
}

const fn firefox(product: &'static str, browser: &'static str, channel: &'static str) -> Layout {
    Layout {
        product,
        container: Some(FIREFOX_ANCHOR),
        browser,
        channel,
        family: Family::Firefox,
    }
}

/// Every install layout this crate recognizes. The deepest match wins, so a
/// capture staged beneath a directory that happens to share a product name
/// cannot shadow the real one.
const LAYOUTS: &[Layout] = &[
    // Windows and macOS product directories.
    chromium("Chrome", "Chrome", "Stable"),
    chromium("Chrome Beta", "Chrome", "Beta"),
    chromium("Chrome Dev", "Chrome", "Dev"),
    chromium("Chrome SxS", "Chrome", "Canary"),
    chromium("Chrome for Testing", "Chrome", "Testing"),
    chromium("Edge", "Edge", "Stable"),
    chromium("Microsoft Edge", "Edge", "Stable"),
    chromium("Edge Beta", "Edge", "Beta"),
    chromium("Microsoft Edge Beta", "Edge", "Beta"),
    chromium("Edge Dev", "Edge", "Dev"),
    chromium("Microsoft Edge Dev", "Edge", "Dev"),
    chromium("Edge SxS", "Edge", "Canary"),
    chromium("Microsoft Edge Canary", "Edge", "Canary"),
    chromium("Brave-Browser", "Brave", "Stable"),
    chromium("Brave-Browser-Beta", "Brave", "Beta"),
    chromium("Brave-Browser-Nightly", "Brave", "Nightly"),
    chromium("Vivaldi", "Vivaldi", "Stable"),
    chromium("Vivaldi Snapshot", "Vivaldi", "Snapshot"),
    chromium("Chromium", "Chromium", "Stable"),
    chromium("Arc", "Arc", "Stable"),
    // Linux package directories under ~/.config, which name the channel in the
    // directory itself and never carry a container.
    chromium("google-chrome", "Chrome", "Stable"),
    chromium("google-chrome-beta", "Chrome", "Beta"),
    chromium("google-chrome-unstable", "Chrome", "Dev"),
    chromium("microsoft-edge", "Edge", "Stable"),
    chromium("microsoft-edge-beta", "Edge", "Beta"),
    chromium("microsoft-edge-dev", "Edge", "Dev"),
    chromium("brave-browser", "Brave", "Stable"),
    chromium("vivaldi", "Vivaldi", "Stable"),
    opera("Opera Stable", "Opera", "Stable"),
    opera("Opera Beta", "Opera", "Beta"),
    opera("Opera Developer", "Opera", "Dev"),
    opera("Opera GX Stable", "Opera GX", "Stable"),
    opera("Opera Crypto Stable", "Opera Crypto", "Stable"),
    opera("Opera Neon", "Opera Neon", "Stable"),
    // `firefox` also matches the Linux `~/.mozilla/firefox` directory, which
    // has no `Profiles` container.
    firefox("Firefox", "Firefox", "Stable"),
    firefox("Firefox Developer Edition", "Firefox", "Dev"),
    firefox("Firefox Nightly", "Firefox", "Nightly"),
    firefox("LibreWolf", "LibreWolf", ""),
    firefox("Waterfox", "Waterfox", ""),
    firefox("SeaMonkey", "SeaMonkey", ""),
    firefox("Pale Moon", "Pale Moon", ""),
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

/// The deepest segment matching a layout for `family`, with that layout.
///
/// Deepest, not first, so a capture staged under a directory that happens to be
/// named after a browser cannot shadow the real install further down.
fn deepest_layout<'a>(dir: &[String], family: Family) -> Option<(usize, &'a Layout)> {
    (0..dir.len()).rev().find_map(|i| {
        LAYOUTS
            .iter()
            .find(|layout| layout.family == family && eq_ci(layout.product, &dir[i]))
            .map(|layout| (i, layout))
    })
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

    if let Some((index, layout)) = deepest_layout(dir, family) {
        return from_layout(dir, index, layout);
    }
    // A fork this crate does not know, but whose container is still present.
    if let Some(id) = from_anchor(dir, family) {
        return id;
    }
    degraded(dir, family, anchor_of(family))
}

fn anchor_of(family: Family) -> &'static str {
    match family {
        Family::Chromium => CHROMIUM_ANCHOR,
        Family::Firefox => FIREFOX_ANCHOR,
    }
}

/// The profile path below a product directory, and the identification to go
/// with it.
fn from_layout(dir: &[String], index: usize, layout: &Layout) -> BrowserId {
    let mut start = index + 1;
    // Consume the container only when this platform actually has one.
    if let Some(container) = layout.container {
        if dir.get(start).is_some_and(|seg| eq_ci(seg, container)) {
            start += 1;
        }
    }
    let profile = profile_below(dir, start, layout.family)
        // Nothing below the product directory: Opera, where the product dir is
        // the profile, and the rare artifact sitting beside the profiles.
        .unwrap_or_else(|| dir[index].clone());

    let channel = match layout.family {
        Family::Firefox => firefox_channel(&profile, layout.channel),
        Family::Chromium => layout.channel.to_string(),
    };

    BrowserId {
        family: layout.family,
        browser: layout.browser.to_string(),
        channel,
        profile,
        note: String::new(),
    }
}

/// The `/`-joined profile path, or `None` when there is nothing below `start`.
fn profile_below(dir: &[String], start: usize, family: Family) -> Option<String> {
    let mut segs: Vec<&str> = dir.get(start..)?.iter().map(String::as_str).collect();
    // `<profile>/Network/Cookies` (Chrome 96+). One strip, never more, and
    // never the last remaining segment: a profile literally named `Network`
    // must survive, or it would collide with the profile above it.
    if family == Family::Chromium
        && segs.len() > 1
        && segs.last().is_some_and(|s| eq_ci(s, STRIPPED_LEAF))
    {
        segs.pop();
    }
    (!segs.is_empty()).then(|| segs.join("/"))
}

/// Firefox encodes the channel in the profile name as well as the product
/// directory, and the profile name is the more reliable of the two when a
/// single install hosts several channels' profiles — which is the normal case
/// on Windows and macOS, where every channel shares one `Profiles` directory.
fn firefox_channel(profile: &str, default: &str) -> String {
    let lower = profile.to_ascii_lowercase();
    for (suffix, channel) in [
        (".default-esr", "ESR"),
        (".dev-edition-default", "Dev"),
        (".default-nightly", "Nightly"),
        (".default-beta", "Beta"),
    ] {
        if lower.ends_with(suffix) {
            return channel.to_string();
        }
    }
    default.to_string()
}

/// A container with an unrecognized product above it: still a confident
/// profile path, with the fork named rather than discarded.
fn from_anchor(dir: &[String], family: Family) -> Option<BrowserId> {
    let anchor = last_anchor(dir, anchor_of(family))?;
    let product = anchor.checked_sub(1).map(|i| dir[i].as_str()).unwrap_or("");
    let profile =
        profile_below(dir, anchor + 1, family).unwrap_or_else(|| anchor_of(family).to_string());
    let browser = match family {
        Family::Chromium => format!("Chromium ({product})"),
        Family::Firefox => format!("Firefox ({product})"),
    };
    let channel = match family {
        Family::Firefox => firefox_channel(&profile, ""),
        Family::Chromium => String::new(),
    };
    Some(BrowserId {
        family,
        browser,
        channel,
        profile,
        note: String::new(),
    })
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
    ///
    /// This previously asserted the profile was `User Data`, because the strip
    /// was unconditional and emptied the path. That collapsed
    /// `User Data/Network/<artifact>` and `User Data/<artifact>` onto one
    /// profile string, which is the collision class this module exists to
    /// prevent.
    #[test]
    fn a_profile_named_network_is_not_stripped_away() {
        let got = chromium("/Google/Chrome/User Data/Network/History");
        assert_eq!(got.profile, "Network");
        assert_ne!(
            got.profile,
            chromium("/Google/Chrome/User Data/History").profile,
            "a profile named Network must not collide with the product root"
        );
    }

    /// macOS and Linux put the profile directly under the product directory,
    /// with no `User Data`. Every one of these used to fall through to
    /// `degraded()`, so Chrome, Edge and Brave on one host all reported as
    /// `Chromium (Unknown)` with profile `Default` and became one identity.
    #[test]
    fn macos_and_linux_chromium_layouts_are_attributed() {
        let cases = [
            (
                "/Users/a/Library/Application Support/Google/Chrome/Default/History",
                "Chrome",
                "Stable",
                "Default",
            ),
            (
                "/Users/a/Library/Application Support/Microsoft Edge/Profile 1/History",
                "Edge",
                "Stable",
                "Profile 1",
            ),
            (
                "/Users/a/Library/Application Support/BraveSoftware/Brave-Browser/Default/History",
                "Brave",
                "Stable",
                "Default",
            ),
            (
                "/home/a/.config/google-chrome/Default/History",
                "Chrome",
                "Stable",
                "Default",
            ),
            (
                "/home/a/.config/google-chrome-beta/Default/History",
                "Chrome",
                "Beta",
                "Default",
            ),
            (
                "/home/a/.config/BraveSoftware/Brave-Browser/Profile 2/History",
                "Brave",
                "Stable",
                "Profile 2",
            ),
            (
                "/home/a/.config/chromium/Default/History",
                "Chromium",
                "Stable",
                "Default",
            ),
        ];
        for (path, browser, channel, profile) in cases {
            let got = chromium(path);
            assert_eq!(got.browser, browser, "{path}");
            assert_eq!(got.channel, channel, "{path}");
            assert_eq!(got.profile, profile, "{path}");
            assert!(got.note.is_empty(), "{path}");
        }
    }

    /// Three browsers on one macOS host must remain three identities.
    #[test]
    fn macos_browsers_do_not_collapse_into_one_identity() {
        let root = "/Users/a/Library/Application Support";
        let chrome = chromium(&format!("{root}/Google/Chrome/Default/History"));
        let edge = chromium(&format!("{root}/Microsoft Edge/Default/History"));
        let brave = chromium(&format!(
            "{root}/BraveSoftware/Brave-Browser/Default/History"
        ));
        let ids = [&chrome, &edge, &brave].map(|id| (&id.browser, &id.profile));
        assert_eq!(ids[0].1, ids[1].1, "the profile name is shared");
        assert_ne!(ids[0].0, ids[1].0);
        assert_ne!(ids[1].0, ids[2].0);
        assert_ne!(ids[0].0, ids[2].0);
    }

    /// The same snapshot collision the module was written for, on macOS.
    #[test]
    fn macos_snapshot_profiles_stay_distinct() {
        let root = "/Users/a/Library/Application Support/Google/Chrome";
        let live = chromium(&format!("{root}/Default/History"));
        let snap = chromium(&format!("{root}/Snapshots/116.0.5845.97/Default/History"));
        assert_eq!(live.profile, "Default");
        assert_eq!(snap.profile, "Snapshots/116.0.5845.97/Default");
    }

    /// Linux Firefox keeps profiles under `~/.mozilla/firefox` with no
    /// `Profiles` container, and macOS has the container.
    #[test]
    fn firefox_resolves_with_and_without_the_profiles_container() {
        let linux = firefox("/home/a/.mozilla/firefox/ab12cd.default-release/places.sqlite");
        assert_eq!(linux.browser, "Firefox");
        assert_eq!(linux.profile, "ab12cd.default-release");
        assert!(linux.note.is_empty());

        let macos = firefox(
            "/Users/a/Library/Application Support/Firefox/Profiles/xy.default-release/places.sqlite",
        );
        assert_eq!(macos.browser, "Firefox");
        assert_eq!(macos.profile, "xy.default-release");
    }

    /// Every channel shares one `Profiles` directory on Windows and macOS, so
    /// the profile suffix is the only thing that distinguishes them.
    #[test]
    fn firefox_nightly_and_beta_are_read_from_the_profile_suffix() {
        let base = "/Users/a/AppData/Roaming/Mozilla/Firefox/Profiles";
        assert_eq!(
            firefox(&format!("{base}/x1.default-nightly/places.sqlite")).channel,
            "Nightly"
        );
        assert_eq!(
            firefox(&format!("{base}/q9.default-beta/places.sqlite")).channel,
            "Beta"
        );
    }

    /// An Electron app's `Network/Cookies` has no product directory at all, so
    /// it degrades — but it must not silently claim a browser profile.
    #[test]
    fn an_electron_app_is_not_mistaken_for_a_browser_profile() {
        let got = id(
            "/Users/a/AppData/Roaming/discord/Network/Cookies",
            ArtifactKind::ChromiumCookies,
        );
        assert_eq!(got.browser, "Chromium (Unknown)");
        assert!(
            !got.note.is_empty(),
            "a degraded identification must say so"
        );
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
