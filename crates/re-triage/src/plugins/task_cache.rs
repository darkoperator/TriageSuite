//! Port of RegistryPlugin.TaskCache (TaskCache.cs + ValuesOut.cs). Extracts
//! scheduled task metadata from Windows Task Scheduler registry keys.
//!
//! Key path: Microsoft\Windows NT\CurrentVersion\Schedule\TaskCache\Tasks
//! Each subkey is a GUID representing one task.
//!
//! Detail-CSV column order (fixture-authoritative):
//!   Version, BatchKeyPath, KeyName, BatchValueName, Path, CreatedOn, LastStart,
//!   LastStop, TaskState, LastActionResult, Source, Description, Command,
//!   Arguments, SecurityDescriptor, Author

use chrono::DateTime;
use notatin::cell_key_node::CellKeyNode;
use notatin::cell_value::CellValue;
use triage_core::timestamp::WinTimestamp;
use triage_registry::hive::Hive;
use triage_registry::plugin::{PluginRow, PluginValue, RegistryPlugin};

pub struct TaskCache;

/// FILETIME → ISO-8601 UTC (`2024-06-21T02:02:54.3720419Z`).
/// Used for standalone timestamp detail-CSV columns (testkit normalizes
/// the reference from RECmd format; we emit ISO-8601 to match after normalization).
fn filetime_to_iso8601(ft: u64) -> Option<String> {
    let secs = (ft / 10_000_000) as i64 - 11_644_473_600;
    let nanos = ((ft % 10_000_000) * 100) as u32;
    let dt: DateTime<chrono::Utc> = DateTime::from_timestamp(secs, nanos)?;
    Some(WinTimestamp::from_unix_nanos(dt.timestamp(), dt.timestamp_subsec_nanos()).to_string())
}

/// FILETIME → RECmd literal `yyyy-MM-dd HH:mm:ss.fffffff` (UTC).
/// Used in BatchValueData free-text fields (testkit does NOT normalize embedded timestamps).
fn filetime_to_recmd_literal(ft: u64) -> Option<String> {
    let secs = (ft / 10_000_000) as i64 - 11_644_473_600;
    let nanos = ((ft % 10_000_000) * 100) as u32;
    let dt: DateTime<chrono::Utc> = DateTime::from_timestamp(secs, nanos)?;
    let ticks = dt.timestamp_subsec_nanos() / 100;
    Some(format!("{}.{:07}", dt.format("%Y-%m-%d %H:%M:%S"), ticks))
}

fn get_str_value(key: &CellKeyNode, name: &str) -> String {
    for v in key.value_iter() {
        if v.get_pretty_name() == name {
            let (content, _) = v.get_content();
            return triage_registry::value::render_value_data(&content);
        }
    }
    String::new()
}

fn get_binary_value(key: &CellKeyNode, name: &str) -> Option<Vec<u8>> {
    for v in key.value_iter() {
        if v.get_pretty_name() == name {
            let (content, _) = v.get_content();
            if let CellValue::Binary(b) = content {
                return Some(b);
            }
        }
    }
    None
}

/// Parse the DynamicInfo binary blob.
struct DynamicInfo {
    ver: i32,
    created: u64,    // FILETIME
    last_start: u64, // FILETIME, 0 if absent
    task_state: i32,
    last_action_result: i32,
    last_stop: u64, // FILETIME, 0 if absent
}

fn parse_dynamic_info(raw: &[u8]) -> Option<DynamicInfo> {
    if raw.len() < 0x24 {
        return None;
    }
    let ver = i32::from_le_bytes(raw[0..4].try_into().ok()?);
    let created = u64::from_le_bytes(raw[4..12].try_into().ok()?);
    let last_start = u64::from_le_bytes(raw[0xC..0x14].try_into().ok()?);
    let task_state = i32::from_le_bytes(raw[0x14..0x18].try_into().ok()?);
    let last_action_result = i32::from_le_bytes(raw[0x18..0x1C].try_into().ok()?);
    let last_stop = u64::from_le_bytes(raw[0x1C..0x24].try_into().ok()?);
    Some(DynamicInfo {
        ver,
        created,
        last_start,
        task_state,
        last_action_result,
        last_stop,
    })
}

const ERROR_PARSING_ACTIONS: &str = "Error parsing Actions binary";

/// Parse the Actions binary blob for Command and Arguments (UTF-16LE strings).
///
/// Mirrors C# TaskCache.cs behaviour exactly:
///
/// - The outer guard (`blobA.ValueDataRaw.Length >= 4`) is replicated here:
///   if `raw.len() < 4` we return empty (blob absent / too short to enter try).
/// - If `raw.len()` is 4 or 5, C#'s `BitConverter.ToInt32(rawA, 2)` reads
///   bytes [2..6] and throws `IndexOutOfRangeException` → error sentinel.
/// - Before magic is confirmed (`magic != 0x6666` or `magicOffset + 2 > len`
///   after a valid contextLen), C# does NOT throw; command stays "". We match
///   that by returning `(String::new(), String::new())`.
/// - Once `magic == 0x6666` is confirmed, C# does `BitConverter.ToInt32` for
///   cmdLen and argLen without additional bounds guards. If those accesses are
///   out-of-range, C# throws → error sentinel. We return the sentinel in those
///   cases too.
/// - The C# string-copy calls are wrapped in `if (cmdLen > 0 && cmdOffset + cmdLen
///   <= rawA.Length)` guards, so they never throw; we match by returning "".
fn parse_actions(raw: &[u8]) -> (String, String) {
    let len = raw.len();

    // C# outer guard: `blobA.ValueDataRaw.Length >= 4`
    if len < 4 {
        return (String::new(), String::new());
    }

    // C# inner try begins here. `BitConverter.ToInt32(rawA, 2)` reads [2..6].
    // If len < 6, that throws → error sentinel.
    if len < 6 {
        return (ERROR_PARSING_ACTIONS.to_string(), String::new());
    }

    let context_len = i32::from_le_bytes(raw[2..6].try_into().unwrap());

    // Negative contextLen: `magicOffset = 6 + contextLen` could be <= 0.
    // C# does NOT throw here (it just computes an offset); the subsequent
    // `if (magicOffset + 2 <= rawA.Length)` guard prevents any further throw.
    // We match: if contextLen < 0, magic_offset arithmetic underflows in usize,
    // so guard it explicitly and return empty (not error sentinel) as C# would.
    if context_len < 0 {
        return (String::new(), String::new());
    }

    let magic_offset = 6 + context_len as usize;

    // C# guard: `if (magicOffset + 2 <= rawA.Length)` — if false, no throw.
    if magic_offset + 2 > len {
        return (String::new(), String::new());
    }

    let magic = i16::from_le_bytes(raw[magic_offset..magic_offset + 2].try_into().unwrap());

    // Only Execution Actions (0x6666) are parsed.
    if magic != 0x6666 {
        return (String::new(), String::new());
    }

    // --- Past this point C# has no more explicit bounds guards for ToInt32 ---
    // Any out-of-range access throws → error sentinel.

    let cmd_len_offset = magic_offset + 6;
    if cmd_len_offset + 4 > len {
        // C#: `BitConverter.ToInt32(rawA, cmdLenOffset)` throws → error sentinel.
        return (ERROR_PARSING_ACTIONS.to_string(), String::new());
    }
    let cmd_len = i32::from_le_bytes(raw[cmd_len_offset..cmd_len_offset + 4].try_into().unwrap());
    let cmd_offset = cmd_len_offset + 4;

    // C# guard: `if (cmdLen > 0 && cmdOffset + cmdLen <= rawA.Length)` → no throw.
    let command = if cmd_len > 0 {
        let end = cmd_offset + cmd_len as usize;
        if end <= len {
            decode_utf16le_trimmed(&raw[cmd_offset..end])
        } else {
            String::new()
        }
    } else {
        String::new()
    };

    let arg_len_offset = cmd_offset + cmd_len.max(0) as usize;
    if arg_len_offset + 4 > len {
        // C#: `BitConverter.ToInt32(rawA, argLenOffset)` throws → error sentinel.
        // Note: command is already parsed (may be non-empty), but C# sets
        // `command = "Error parsing Actions binary"` in the catch, overwriting it.
        return (ERROR_PARSING_ACTIONS.to_string(), String::new());
    }
    let arg_len = i32::from_le_bytes(raw[arg_len_offset..arg_len_offset + 4].try_into().unwrap());
    let arg_offset = arg_len_offset + 4;

    // C# guard: `if (argLen > 0 && argOffset + argLen <= rawA.Length)` → no throw.
    let arguments = if arg_len > 0 {
        let end = arg_offset + arg_len as usize;
        if end <= len {
            decode_utf16le_trimmed(&raw[arg_offset..end])
        } else {
            String::new()
        }
    } else {
        String::new()
    };

    (command, arguments)
}

fn decode_utf16le_trimmed(bytes: &[u8]) -> String {
    let words: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    let s: String = char::decode_utf16(words.iter().copied())
        .filter_map(|r| r.ok())
        .collect();
    s.trim_matches('\0').to_string()
}

impl RegistryPlugin for TaskCache {
    fn plugin_name(&self) -> &'static str {
        "TaskCache"
    }

    fn key_paths(&self) -> &[&'static str] {
        &[r"Microsoft\Windows NT\CurrentVersion\Schedule\TaskCache\Tasks"]
    }

    fn process_with_hive(
        &self,
        key: &mut CellKeyNode,
        _values: &[PluginValue],
        hive: &mut Hive,
    ) -> Vec<PluginRow> {
        let mut rows = Vec::new();

        let subkeys = hive.sub_keys(key);

        for sub in subkeys {
            let guid = sub.key_name.clone();

            // Parse DynamicInfo — skip subkey if absent
            let dynamic_raw = match get_binary_value(&sub, "DynamicInfo") {
                Some(b) => b,
                None => continue,
            };
            let di = match parse_dynamic_info(&dynamic_raw) {
                Some(d) => d,
                None => continue,
            };

            // Read string values
            let source = get_str_value(&sub, "Source");
            let author = get_str_value(&sub, "Author");
            let description = get_str_value(&sub, "Description");
            let security_descriptor = get_str_value(&sub, "SecurityDescriptor");
            let path = get_str_value(&sub, "Path");

            // Parse Actions blob
            let actions_raw = get_binary_value(&sub, "Actions").unwrap_or_default();
            let (command, arguments) = if actions_raw.len() >= 4 {
                parse_actions(&actions_raw)
            } else {
                (String::new(), String::new())
            };

            // Timestamps
            let created_iso = filetime_to_iso8601(di.created).unwrap_or_default();
            let created_recmd = filetime_to_recmd_literal(di.created).unwrap_or_default();
            let last_start_iso = if di.last_start > 0 {
                filetime_to_iso8601(di.last_start).unwrap_or_default()
            } else {
                String::new()
            };
            let last_start_recmd = if di.last_start > 0 {
                filetime_to_recmd_literal(di.last_start).unwrap_or_default()
            } else {
                String::new()
            };
            let last_stop_iso = if di.last_stop > 0 {
                filetime_to_iso8601(di.last_stop).unwrap_or_default()
            } else {
                String::new()
            };
            let last_stop_recmd = if di.last_stop > 0 {
                filetime_to_recmd_literal(di.last_stop).unwrap_or_default()
            } else {
                String::new()
            };

            // BatchValueData fields (embedded timestamps → RECmd literal format)
            let vd1 = format!("Created on: {created_recmd}");
            let vd2 = format!("Last start: {last_start_recmd}, Last stop: {last_stop_recmd}");
            let vd3 = format!("Path: {path}, Command: {command}, Arguments: {arguments}");

            rows.push(PluginRow {
                batch_value_name: String::new(),
                batch_value_data1: vd1,
                batch_value_data2: vd2,
                batch_value_data3: vd3,
                detail_columns: vec![
                    ("Version".to_string(), di.ver.to_string()),
                    ("BatchKeyPath".to_string(), String::new()),
                    ("KeyName".to_string(), guid),
                    ("BatchValueName".to_string(), String::new()),
                    ("Path".to_string(), path),
                    ("CreatedOn".to_string(), created_iso),
                    ("LastStart".to_string(), last_start_iso),
                    ("LastStop".to_string(), last_stop_iso),
                    ("TaskState".to_string(), di.task_state.to_string()),
                    (
                        "LastActionResult".to_string(),
                        di.last_action_result.to_string(),
                    ),
                    ("Source".to_string(), source),
                    ("Description".to_string(), description),
                    ("Command".to_string(), command),
                    ("Arguments".to_string(), arguments),
                    ("SecurityDescriptor".to_string(), security_descriptor),
                    ("Author".to_string(), author),
                ],
            });
        }

        rows
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_name_and_key_paths() {
        assert_eq!(TaskCache.plugin_name(), "TaskCache");
        assert_eq!(
            TaskCache.key_paths(),
            &[r"Microsoft\Windows NT\CurrentVersion\Schedule\TaskCache\Tasks"]
        );
    }

    #[test]
    fn detail_column_order() {
        let expected = [
            "Version",
            "BatchKeyPath",
            "KeyName",
            "BatchValueName",
            "Path",
            "CreatedOn",
            "LastStart",
            "LastStop",
            "TaskState",
            "LastActionResult",
            "Source",
            "Description",
            "Command",
            "Arguments",
            "SecurityDescriptor",
            "Author",
        ];
        assert_eq!(expected.len(), 16);
    }

    #[test]
    fn parse_actions_empty() {
        let (cmd, args) = parse_actions(&[]);
        assert!(cmd.is_empty());
        assert!(args.is_empty());
    }

    #[test]
    fn decode_utf16le_basic() {
        // "AB" in UTF-16LE
        let bytes = [b'A', 0, b'B', 0];
        assert_eq!(decode_utf16le_trimmed(&bytes), "AB");
    }

    #[test]
    fn filetime_to_iso_known_value() {
        // Known FILETIME: 2026-03-12 22:52:48.7227042 UTC
        // ft = 134178295687227042
        let out = filetime_to_iso8601(134_178_295_687_227_042).unwrap();
        assert!(out.starts_with("2026-03-12T22:52:48"), "got {out}");
        assert!(out.ends_with('Z'), "got {out}");
    }
}
