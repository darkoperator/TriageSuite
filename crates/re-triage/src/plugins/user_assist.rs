//! Port of RegistryPlugin.UserAssist (UserAssist.cs + ValuesOut.cs).
//! Un-ROT13s UserAssist key value names and extracts run count, last executed,
//! focus count, and focus time.
//!
//! Fires on NTUSER.DAT at:
//!   `Software\Microsoft\Windows\CurrentVersion\Explorer\UserAssist`
//!   `Software\Microsoft\Windows\CurrentVersion\Explorer\UserAssist\*\Count`
//!
//! plugin_name() = "UserAssist" (matches C# PluginName exactly).
//!
//! Detail-CSV column order (fixture-authoritative, from
//! `<host>__<user>_NTUSER__plugin_UserAssist_NTUSER.DAT.csv`):
//!   BatchKeyPath, BatchValueName, ProgramName, RunCounter, FocusCount,
//!   FocusTime, LastExecuted
//!
//! Batch row format (C# ValuesOut):
//!   ValueData1 = "{ProgramName}"
//!   ValueData2 = "Last executed: {LastExecuted?.ToUniversalTime():yyyy-MM-dd HH:mm:ss.fffffff}"
//!   ValueData3 = "Run count: {RunCounter:N0}"
//!
//! Design note: ExtensionBlocks is imported in UserAssist.cs but is only used
//! transitively (Utils.GetFolderNameFromGuid comes from that package). No
//! shell item / extension block parsing is needed — the GUID→folder mapping is
//! a static lookup table.
//!
//! The `LastExecuted` column is a STANDALONE timestamp. The testkit normalizes
//! it from RECmd's "yyyy-MM-dd HH:mm:ss.fffffff" to ISO-8601 UTC. We emit
//! ISO-8601 UTC directly — no AcceptedDelta needed for LastExecuted.
//!
//! The embedded timestamp inside ValueData2 ("Last executed: ...") is a
//! FREE-TEXT field and is NOT normalized by the testkit. We emit RECmd's
//! literal "yyyy-MM-dd HH:mm:ss.fffffff" format there (manual 7-digit ticks).
//!
//! C# struct layout (Win7+ format, length >= 68):
//!   offset  0: session (i32)
//!   offset  4: run count (i32)
//!   offset  8: focus count (i32)
//!   offset 12: focus time milliseconds (i32)
//!   offset 60: last run filetime (i64)
//!
//! Legacy format (16 <= length < 68):
//!   offset  4: run count (i32)
//!   offset  8: last run filetime (i64)
//!   no focus count / focus time

use chrono::DateTime;
use notatin::cell_key_node::CellKeyNode;
use triage_core::timestamp::WinTimestamp;
use triage_registry::hive::Hive;
use triage_registry::plugin::{PluginRow, PluginValue, RegistryPlugin};

pub struct UserAssist;

// ─── Known-folder GUID → display name map ────────────────────────────────────
//
// These are the Windows Known Folder IDs observed across all capture fixtures.
// The GUIDs are in uppercase. When a decoded value name contains a GUID that
// matches, the GUID is replaced with the folder name (in braces), e.g.
// `{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\cmd.exe` → `{System}\cmd.exe`.
//
// For unmapped GUIDs the C# Utils.GetFolderNameFromGuid returns the string
// `{Unmapped GUID: <GUID>}`, so the replacement produces
// `{Unmapped GUID: <GUID>}\...` in the ProgramName column.

fn known_folder_name(guid_upper: &str) -> Option<&'static str> {
    match guid_upper {
        // System (Windows\System32)
        "1AC14E77-02E7-4E5D-B744-2EB1AE5198B7" => Some("System"),
        // Windows directory
        "F38BF404-1D43-42F2-9305-67DE0B28FC23" => Some("Windows"),
        // Program Files (x64)
        "6D809377-6AF0-444B-8957-A3773F02200E" => Some("ProgramFilesX64"),
        // Program Files (x86)
        "7C5A40EF-A0FB-4BFC-874A-C0F2E0B9FA8E" => Some("ProgramFilesX86"),
        // System (x86) — SysWOW64
        "D65231B0-B2F1-4857-A4CE-A8E7C6EA7D27" => Some("SystemX86"),
        // Common Programs (All Users\Programs in Start Menu)
        "0139D44E-6AFE-49F2-8690-3DAFCAE6FFB8" => Some("Common Programs"),
        // User Pinned (taskbar / start menu pinned items)
        "9E3995AB-1F9C-4F13-B827-48B24B6C7174" => Some("User Pinned"),
        // Programs (user's Programs in Start Menu)
        "A77F5D77-2E2B-44C3-A6A2-ABA601054A51" => Some("Programs"),
        _ => None,
    }
}

// ─── ROT13 ───────────────────────────────────────────────────────────────────

/// Rotate ASCII letters by 13 positions; all other characters are unchanged.
/// Matches C# `Helpers.Rot13Transform` exactly.
fn rot13(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'a'..='z' => (((c as u8 - b'a') + 13) % 26 + b'a') as char,
            'A'..='Z' => (((c as u8 - b'A') + 13) % 26 + b'A') as char,
            _ => c,
        })
        .collect()
}

// ─── GUID → folder-name substitution ─────────────────────────────────────────

/// Mirrors C# `Utils.GetFolderNameFromGuid(guid)` where `guid` is the bare hex
/// string (without braces), in its original case from the decoded value name.
/// Returns the folder display name (no braces) for known GUIDs, or
/// `Unmapped GUID: <original_case_guid>` (no braces) for unknown ones.
fn get_folder_name_from_guid(guid_original: &str) -> String {
    let guid_upper = guid_original.to_uppercase();
    match known_folder_name(&guid_upper) {
        Some(name) => name.to_string(),
        None => format!("Unmapped GUID: {guid_original}"),
    }
}

/// Find the first GUID-like pattern in `s` using word-boundary matching
/// (mirrors C# `Regex.Match(unrot, @"\b[A-F0-9]{8}(?:-[A-F0-9]{4}){3}-[A-F0-9]{12}\b",
/// RegexOptions.IgnoreCase)`), then replace the bare GUID with the result of
/// `GetFolderNameFromGuid`.
///
/// The replacement is a literal string replace of the matched bare GUID, so:
/// - `{GUID}\path` → `{FolderName}\path` (braces from original value remain)
/// - `{GUID}` (unknown) → `{Unmapped GUID: GUID}` (original braces remain)
/// - `prefix-GUID\path` → `prefix-Unmapped GUID: GUID\path` (no braces added)
fn replace_guid_in_name(s: &str) -> String {
    // Scan for the first word-boundary-delimited GUID pattern.
    // A word boundary occurs where one side is a word char ([A-Za-z0-9_])
    // and the other is not. We check this around the GUID match.
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        // Try to parse a GUID starting at i.
        if let Some((guid_original, guid_start, guid_end)) = try_parse_guid_word_boundary(&chars, i)
        {
            let replacement = get_folder_name_from_guid(&guid_original);
            // Rebuild the string: before_guid + replacement + after_guid.
            let before: String = chars[..guid_start].iter().collect();
            let after: String = chars[guid_end..].iter().collect();
            return format!("{before}{replacement}{after}");
        }
        i += 1;
    }
    s.to_string()
}

/// Check if `ch` is a word character (matches `\w` = `[A-Za-z0-9_]`).
fn is_word_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_'
}

/// Try to parse an 8-4-4-4-12 GUID starting at position `start` in `chars`,
/// with word boundaries at both ends.
/// Returns `(GUID_original_case, start_idx, end_idx)` on success.
fn try_parse_guid_word_boundary(chars: &[char], start: usize) -> Option<(String, usize, usize)> {
    // GUID = 8hex - 4hex - 4hex - 4hex - 12hex = 36 chars total.
    let expected_lens: &[usize] = &[8, 4, 4, 4, 12];
    let total_len = 8 + 1 + 4 + 1 + 4 + 1 + 4 + 1 + 12; // = 36
    let end = start + total_len;

    if end > chars.len() {
        return None;
    }

    // Check word boundary before start: position before `start` must be a
    // non-word char (or start of string), and chars[start] must be a word char.
    if !chars[start].is_ascii_hexdigit() {
        return None;
    }
    if start > 0 && is_word_char(chars[start - 1]) {
        return None; // No word boundary
    }

    // Check word boundary after end: chars[end-1] must be a word char,
    // and the char at `end` (if any) must be a non-word char.
    if !chars[end - 1].is_ascii_hexdigit() {
        return None;
    }
    if end < chars.len() && is_word_char(chars[end]) {
        return None; // No word boundary
    }

    // Parse the GUID segments separated by dashes.
    let mut pos = start;
    let mut guid_parts = Vec::new();
    for (seg_idx, &seg_len) in expected_lens.iter().enumerate() {
        if pos + seg_len > end {
            return None;
        }
        // Collect seg_len hex chars (preserving original case).
        let seg: String = chars[pos..pos + seg_len].iter().collect();
        if !seg.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        guid_parts.push(seg); // preserve original case
        pos += seg_len;

        // After each segment except the last, expect a dash.
        if seg_idx < expected_lens.len() - 1 {
            if pos >= end || chars[pos] != '-' {
                return None;
            }
            pos += 1;
        }
    }

    if pos != end {
        return None;
    }

    let guid_original = guid_parts.join("-");
    Some((guid_original, start, end))
}

// ─── Timestamp helpers ────────────────────────────────────────────────────────

/// Convert a Windows FILETIME (100-ns ticks since 1601-01-01 UTC) to UTC DateTime.
fn filetime_to_datetime(ft: i64) -> Option<DateTime<chrono::Utc>> {
    const FILETIME_TO_UNIX_SECS: i64 = 11_644_473_600;
    let secs = (ft / 10_000_000) - FILETIME_TO_UNIX_SECS;
    let nanos = ((ft % 10_000_000) * 100) as u32;
    DateTime::from_timestamp(secs, nanos)
}

/// Format a `DateTime<Utc>` as RECmd's literal "yyyy-MM-dd HH:mm:ss.fffffff"
/// (7 fractional digits = 100-nanosecond ticks). Used in the embedded
/// "Last executed: ..." ValueData2 field (not normalized by testkit).
fn dt_to_recmd_literal(dt: DateTime<chrono::Utc>) -> String {
    let ticks = dt.timestamp_subsec_nanos() / 100;
    format!("{}.{ticks:07}", dt.format("%Y-%m-%d %H:%M:%S"))
}

/// Format a `DateTime<Utc>` as ISO-8601 UTC with 7 fractional digits.
/// Used for the standalone `LastExecuted` detail column (auto-normalized).
fn dt_to_iso8601(dt: DateTime<chrono::Utc>) -> String {
    WinTimestamp::from_unix_nanos(dt.timestamp(), dt.timestamp_subsec_nanos()).to_string()
}

// ─── FocusTime formatting ─────────────────────────────────────────────────────

/// Format focus-time milliseconds as C# TimeSpan's `@"d'd, 'h'h, 'mm'm, 'ss's'"`.
/// C#: `TimeSpan.FromMilliseconds(ms).ToString(@"d'd, 'h'h, 'mm'm, 'ss's'")`.
///
/// The format tokens are:
///   d   — total whole days (not zero-padded)
///   h   — hours component (0-23, not zero-padded)
///   mm  — minutes component (00-59, zero-padded 2 digits)
///   ss  — seconds component (00-59, zero-padded 2 digits)
fn format_focus_time(ms: i32) -> String {
    let total_secs = (ms as i64) / 1000;
    let days = total_secs / 86400;
    let rem = total_secs % 86400;
    let hours = rem / 3600;
    let rem2 = rem % 3600;
    let mins = rem2 / 60;
    let secs = rem2 % 60;
    format!("{days}d, {hours}h, {mins:02}m, {secs:02}s")
}

// ─── Binary struct parsing ────────────────────────────────────────────────────

/// Parse a UserAssist binary blob.
///
/// Returns `(run, last_run_opt, focus_count_opt, focus_time_ms)`.
/// Mirrors C# UserAssist.ProcessKeys exactly.
fn parse_user_assist_blob(raw: &[u8]) -> (i32, Option<DateTime<chrono::Utc>>, Option<i32>, i32) {
    if raw.len() < 16 {
        return (0, None, None, 0);
    }

    let run = i32::from_le_bytes(raw[4..8].try_into().unwrap_or([0; 4]));

    // Legacy: extract lastRun from offset 8 (an i64 filetime).
    let legacy_ft = i64::from_le_bytes(raw[8..16].try_into().unwrap_or([0; 8]));
    let mut last_run = filetime_to_datetime(legacy_ft);
    let mut focus_count: Option<i32> = None;
    let mut focus_time_ms: i32 = 0;

    // Win7+ format: 68 bytes or more.
    if raw.len() >= 68 {
        focus_count = Some(i32::from_le_bytes(raw[8..12].try_into().unwrap_or([0; 4])));
        focus_time_ms = i32::from_le_bytes(raw[12..16].try_into().unwrap_or([0; 4]));
        let win7_ft = i64::from_le_bytes(raw[60..68].try_into().unwrap_or([0; 8]));
        last_run = filetime_to_datetime(win7_ft);
    }

    // C#: if lastRun?.Year < 1970 → lastRun = null
    last_run =
        last_run.filter(|dt| dt.format("%Y").to_string().parse::<i32>().unwrap_or(0) >= 1970);

    (run, last_run, focus_count, focus_time_ms)
}

// ─── Row emission ─────────────────────────────────────────────────────────────

/// Emit rows for one `Count` key node from its pre-collected values.
fn emit_rows(key_path: &str, values: &[PluginValue]) -> Vec<PluginRow> {
    let mut rows = Vec::new();

    for v in values {
        // ROT13-decode the value name.
        let mut unrot = rot13(&v.name);

        // Replace any GUID found in the decoded name with the folder name.
        // C# catches ArgumentException from Regex.Match but that can't happen here.
        unrot = replace_guid_in_name(&unrot);

        let (run, last_run, focus_count, focus_time_ms) = parse_user_assist_blob(&v.raw);

        let focus_time_str = format_focus_time(focus_time_ms);
        let last_run_recmd = last_run.map(dt_to_recmd_literal).unwrap_or_default();
        let last_run_iso = last_run.map(dt_to_iso8601).unwrap_or_default();

        // C# uses {RunCounter:N0} which uses locale-specific thousands separator.
        // On en-US that is comma-thousands. Match C# N0 format:
        let run_n0 = format_n0(run);

        rows.push(PluginRow {
            batch_value_name: v.name.clone(),
            batch_value_data1: unrot.clone(),
            batch_value_data2: format!("Last executed: {last_run_recmd}"),
            batch_value_data3: format!("Run count: {run_n0}"),
            detail_columns: vec![
                ("BatchKeyPath".to_string(), key_path.to_string()),
                ("BatchValueName".to_string(), v.name.clone()),
                ("ProgramName".to_string(), unrot),
                ("RunCounter".to_string(), run.to_string()),
                (
                    "FocusCount".to_string(),
                    focus_count.map(|fc| fc.to_string()).unwrap_or_default(),
                ),
                ("FocusTime".to_string(), focus_time_str),
                ("LastExecuted".to_string(), last_run_iso),
            ],
        });
    }

    rows
}

/// Format an integer with comma-thousands (C# N0 on en-US culture).
fn format_n0(n: i32) -> String {
    let s = n.abs().to_string();
    let mut result = String::new();
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len();
    for (i, c) in chars.iter().enumerate() {
        if i > 0 && (len - i).is_multiple_of(3) {
            result.push(',');
        }
        result.push(*c);
    }
    if n < 0 {
        format!("-{result}")
    } else {
        result
    }
}

// ─── RegistryPlugin impl ──────────────────────────────────────────────────────

impl RegistryPlugin for UserAssist {
    fn plugin_name(&self) -> &'static str {
        "UserAssist"
    }

    fn key_paths(&self) -> &[&'static str] {
        &[
            r"Software\Microsoft\Windows\CurrentVersion\Explorer\UserAssist",
            r"Software\Microsoft\Windows\CurrentVersion\Explorer\UserAssist\*\Count",
        ]
    }

    fn process_with_hive(
        &self,
        key: &mut CellKeyNode,
        values: &[PluginValue],
        _hive: &mut Hive,
    ) -> Vec<PluginRow> {
        let key_name = key.key_name.clone();
        let key_path = key.path.trim_start_matches('\\').to_string();

        if key_name == "UserAssist" {
            // Matched the root UserAssist key. This key itself has no values —
            // its content is in GUID subkeys → Count subkeys. The engine's
            // Recursive batch entry fires the plugin on each `*\Count` subkey
            // via the wildcard key_path, so we simply return empty rows here.
            Vec::new()
        } else {
            // Matched a `*\Count` key via the wildcard pattern. Process all
            // registry values in this key: ROT13-decode the value name, parse
            // the binary blob, emit one row per value.
            emit_rows(&key_path, values)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rot13_roundtrip() {
        let s = "Hello, World! 123";
        assert_eq!(rot13(&rot13(s)), s);
        assert_eq!(rot13("HRZR_PGYFRFFVBA"), "UEME_CTLSESSION");
        assert_eq!(
            rot13("Zvpebfbsg.Trgfgnegrq_8jrxlo3q8oojr!Ncc"),
            "Microsoft.Getstarted_8wekyb3d8bbwe!App"
        );
    }

    #[test]
    fn format_focus_time_basic() {
        // 0 ms → "0d, 0h, 00m, 00s"
        assert_eq!(format_focus_time(0), "0d, 0h, 00m, 00s");
        // 7 * 60 * 1000 = 420000 ms → "0d, 0h, 07m, 00s"
        assert_eq!(format_focus_time(420_000), "0d, 0h, 07m, 00s");
        // 37 * 60 * 1000 + 2 * 1000 = 2222000 ms → "0d, 0h, 37m, 02s"
        assert_eq!(format_focus_time(2_222_000), "0d, 0h, 37m, 02s");
    }

    #[test]
    fn format_n0_thousands() {
        assert_eq!(format_n0(0), "0");
        assert_eq!(format_n0(87), "87");
        assert_eq!(format_n0(1000), "1,000");
        assert_eq!(format_n0(1_234_567), "1,234,567");
    }

    #[test]
    fn guid_replacement_known_with_braces() {
        // {1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\cmd.exe → {System}\cmd.exe
        // The GUID regex matches the bare hex; Replace(bare_hex, "System") leaves
        // the surrounding braces intact, giving {System}.
        let input = "{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\\cmd.exe";
        let result = replace_guid_in_name(input);
        assert_eq!(result, r"{System}\cmd.exe");
    }

    #[test]
    fn guid_replacement_unmapped_with_braces() {
        // {BB044BFD-...} → {Unmapped GUID: BB044BFD-...}
        // Replace(bare_hex, "Unmapped GUID: hex") leaves the surrounding {} intact.
        let input = "Microsoft.AutoGenerated.{BB044BFD-25B7-2FAA-22A8-6371A93E0456}";
        let result = replace_guid_in_name(input);
        assert_eq!(
            result,
            "Microsoft.AutoGenerated.{Unmapped GUID: BB044BFD-25B7-2FAA-22A8-6371A93E0456}"
        );
    }

    #[test]
    fn guid_replacement_bare_unmapped() {
        // Update-970a3cfe-aae6-466a-b357-d09ba94b53dd\file.exe
        // → Update-Unmapped GUID: 970a3cfe-...\file.exe (original lowercase preserved)
        let input = r"C:\Users\foo\Temp\Update-970a3cfe-aae6-466a-b357-d09ba94b53dd\file.exe";
        let result = replace_guid_in_name(input);
        assert_eq!(
            result,
            r"C:\Users\foo\Temp\Update-Unmapped GUID: 970a3cfe-aae6-466a-b357-d09ba94b53dd\file.exe"
        );
    }

    #[test]
    fn guid_replacement_no_guid() {
        let input = "Microsoft.Getstarted_8wekyb3d8bbwe!App";
        assert_eq!(replace_guid_in_name(input), input);
    }

    #[test]
    fn parse_blob_win7_format() {
        // Build a 72-byte blob with known values:
        // offset 4: run=14 (i32 LE)
        // offset 8: focus_count=21 (i32 LE)
        // offset 12: focus_time=420000 ms (i32 LE)
        // offset 60: filetime for 2024-06-28 23:08:10.9235262 UTC
        //   unix_secs=1719616090, ticks=9235262
        //   ft = (1719616090 + 11644473600) * 10_000_000 + 9235262 = 133640896909235262
        let mut raw = vec![0u8; 72];
        raw[4..8].copy_from_slice(&14i32.to_le_bytes());
        raw[8..12].copy_from_slice(&21i32.to_le_bytes());
        raw[12..16].copy_from_slice(&420_000i32.to_le_bytes());
        let ft: i64 = 133_640_896_909_235_262;
        raw[60..68].copy_from_slice(&ft.to_le_bytes());

        let (run, last_run, focus_count, focus_ms) = parse_user_assist_blob(&raw);
        assert_eq!(run, 14);
        assert_eq!(focus_count, Some(21));
        assert_eq!(focus_ms, 420_000);
        let dt = last_run.expect("should have a last_run");
        assert_eq!(
            dt.format("%Y-%m-%d %H:%M:%S").to_string(),
            "2024-06-28 23:08:10"
        );
    }

    #[test]
    fn plugin_name_and_key_paths() {
        let p = UserAssist;
        assert_eq!(p.plugin_name(), "UserAssist");
        let kps = p.key_paths();
        assert!(kps.contains(&r"Software\Microsoft\Windows\CurrentVersion\Explorer\UserAssist"));
        assert!(
            kps.contains(&r"Software\Microsoft\Windows\CurrentVersion\Explorer\UserAssist\*\Count")
        );
    }
}
