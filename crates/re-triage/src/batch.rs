//! The BatchCsvOut record (RECmd-master/RECmd/BatchCsvOut.cs) — one row per
//! matched key/value. Field order is the canonical 15-column CSV order; the
//! existing OutputRouter serializes it to both CSV and NDJSON.

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct BatchRecord {
    #[serde(rename = "HivePath")]
    pub hive_path: String,
    #[serde(rename = "HiveType")]
    pub hive_type: String,
    #[serde(rename = "Description")]
    pub description: String,
    #[serde(rename = "Category")]
    pub category: String,
    #[serde(rename = "KeyPath")]
    pub key_path: String,
    #[serde(rename = "ValueName")]
    pub value_name: String,
    #[serde(rename = "ValueType")]
    pub value_type: String,
    #[serde(rename = "ValueData")]
    pub value_data: String,
    #[serde(rename = "ValueData2")]
    pub value_data2: String,
    #[serde(rename = "ValueData3")]
    pub value_data3: String,
    #[serde(rename = "Comment")]
    pub comment: String,
    /// .NET bool: capitalized "True"/"False" to match RECmd's C# serialization.
    #[serde(rename = "Recursive")]
    pub recursive: String,
    /// .NET bool: capitalized "True"/"False" to match RECmd's C# serialization.
    #[serde(rename = "Deleted")]
    pub deleted: String,
    /// ISO-8601 UTC (project rule); empty when the key has no timestamp.
    #[serde(rename = "LastWriteTimestamp")]
    pub last_write_timestamp: String,
    #[serde(rename = "PluginDetailFile")]
    pub plugin_detail_file: String,
}

/// .NET bool serialization: capitalized "True"/"False", mirroring the
/// convention established in jle-triage/src/lib.rs `dotnet_bool`.
fn dotnet_bool(b: bool) -> String {
    if b {
        "True".to_string()
    } else {
        "False".to_string()
    }
}

// ─── Batch engine ───────────────────────────────────────────────────────────

use crate::plugins::plugins_to_activate;
use crate::reb::KeyEntry;
use notatin::cell_key_node::CellKeyNode;
use notatin::cell_key_value::CellKeyValueDataTypes;
use triage_core::error::TriageError;
use triage_core::timestamp::WinTimestamp;
use triage_registry::hive::Hive;
use triage_registry::plugin::{PluginRow, PluginValue, RegistryPlugin};
use triage_registry::value::{apply_binary_convert, render, BinaryConvert};

/// Emit batch rows for one hive under one batch entry, mirroring RECmd's
/// ProcessBatchKey/BatchDumpKey/BuildBatchCsvOut.
///
/// `plugins`      — the full plugin registry (see `crate::plugins::registry()`).
/// `sink`         — receives each `BatchRecord`.
/// `detail_sink`  — receives `(plugin_name, PluginRow)` for per-plugin detail
///                  CSVs. Wired to a per-plugin CSV writer in `RegistryTool::parse`
///                  (implemented in Task 9). Each unique `plugin_name` gets its
///                  own `<PluginName>_<HiveStem>.csv` side-car file.
pub fn process_entry(
    hive: &mut Hive,
    hive_path: &str,
    entry: &KeyEntry,
    plugins: &[Box<dyn RegistryPlugin>],
    sink: &mut dyn FnMut(BatchRecord) -> Result<(), TriageError>,
    detail_sink: &mut dyn FnMut(&str, PluginRow) -> Result<(), TriageError>,
) -> Result<(), TriageError> {
    // ProcessBatch: skip entries whose hive type doesn't match this hive.
    if hive.hive_type() != entry.hive_type {
        return Ok(());
    }
    if entry.key_path == "*" {
        if let Some(root) = hive.root() {
            process_key(hive, hive_path, entry, root, plugins, sink, detail_sink)?;
        }
        return Ok(());
    }
    if entry.key_path.contains('*') {
        // Wildcard path (RECmd Program.cs lines 2023-2068):
        // ExpandKeyPath returns all matched keys. For each match, RECmd checks
        // if the ValueName exists BEFORE calling ProcessBatchKey (lines 2039-2050).
        // If the value is NOT found, RECmd does `continue` — skipping that key AND
        // ALL its descendants. This differs from the non-wildcard path (line 2080)
        // which calls ProcessBatchKey unconditionally. Mirror this guard here.
        for start in expand_key_path(hive, &entry.key_path) {
            if let Some(vn) = &entry.value_name {
                let has_value = start
                    .value_iter()
                    .any(|v| v.get_pretty_name().eq_ignore_ascii_case(vn));
                if !has_value {
                    // RECmd `continue`: skip this expanded key and all its subkeys.
                    continue;
                }
            }
            process_key(hive, hive_path, entry, start, plugins, sink, detail_sink)?;
        }
    } else {
        // Non-wildcard path (RECmd Program.cs lines 2070-2081):
        // GetKey returns the exact key; ProcessBatchKey is called directly
        // without a prior value-existence check. ProcessBatchKey internally
        // decides whether to emit (based on value_name) and recurses regardless.
        if let Some(start) = hive.get_key(&entry.key_path) {
            process_key(hive, hive_path, entry, start, plugins, sink, detail_sink)?;
        }
    }
    Ok(())
}

/// ProcessBatchKey: emit this key (or one value), then recurse if Recursive.
fn process_key(
    hive: &mut Hive,
    hive_path: &str,
    entry: &KeyEntry,
    mut key: CellKeyNode,
    plugins: &[Box<dyn RegistryPlugin>],
    sink: &mut dyn FnMut(BatchRecord) -> Result<(), TriageError>,
    detail_sink: &mut dyn FnMut(&str, PluginRow) -> Result<(), TriageError>,
) -> Result<(), TriageError> {
    dump_key(hive, hive_path, entry, &mut key, plugins, sink, detail_sink)?;
    if entry.recursive {
        for sub in hive.sub_keys(&mut key) {
            process_key(hive, hive_path, entry, sub, plugins, sink, detail_sink)?;
        }
    }
    Ok(())
}

/// BatchDumpKey: if plugins match this key and `!entry.disable_plugin`, run
/// the plugin path (RECmd: `if (plugins.Count > 0) {...} else {...}`). When
/// any plugin activates, emit `(plugin)` batch rows and collect detail rows,
/// then skip the default dump. Otherwise fall through to the default path.
fn dump_key(
    hive: &mut Hive,
    hive_path: &str,
    entry: &KeyEntry,
    key: &mut CellKeyNode,
    plugins: &[Box<dyn RegistryPlugin>],
    sink: &mut dyn FnMut(BatchRecord) -> Result<(), TriageError>,
    detail_sink: &mut dyn FnMut(&str, PluginRow) -> Result<(), TriageError>,
) -> Result<(), TriageError> {
    // ── Plugin dispatch (RECmd BatchDumpKey, "if plugins.Count > 0" branch) ──
    if !entry.disable_plugin {
        // Use get_pretty_path() (root-stripped) because plugin `key_paths`
        // patterns are root-stripped (e.g. "ControlSet00*\Services\bam\...").
        // RECmd calls Helpers.StripRootKeyNameFromKeyPath before matching;
        // notatin's equivalent is get_pretty_path(). Passing key.path (which
        // includes the root segment, e.g. "\SYSTEM\ControlSet001\...") would
        // cause plugin matching to never fire on real hives.
        let matched =
            plugins_to_activate(plugins, key.get_pretty_path(), entry.value_name.as_deref());
        if !matched.is_empty() {
            // RECmd BatchDumpKey semantic (Program.cs): "if pluginsToActivate.Count > 0"
            // takes the plugin path; the default path is skipped. This is independent
            // of whether any plugin rows are actually emitted — RECmd returns early
            // after the plugin loop even when all matched plugins yielded zero rows.
            //
            // Build PluginValues from the key's values once (shared by all matched plugins).
            let plugin_values: Vec<PluginValue> = key
                .value_iter()
                .map(|v| {
                    let name = v.get_pretty_name();
                    let data_type = v.data_type;
                    // Call get_content() once; derive both raw bytes and value_data from it.
                    let (content, _) = v.get_content();
                    let value_data = triage_registry::value::render_value_data(&content);
                    let raw = triage_registry::value::plugin_raw_bytes(&content);
                    PluginValue {
                        name,
                        raw,
                        value_data,
                        data_type,
                    }
                })
                .collect();

            for plugin in &matched {
                // The detail-CSV basename: "<PluginName>_<HiveStem>.csv".
                // The HiveStem is not available here (only hive_path is), so we
                // compute it from the hive_path basename. This matches the basename
                // written by RegistryTool::parse()'s detail_sink.
                let hive_stem = std::path::Path::new(hive_path)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("UNKNOWN");
                let detail_basename = format!("{}_{}.csv", plugin.plugin_name(), hive_stem);

                // Use process_with_hive so plugins that need subkey iteration
                // (AppPaths, UnInstall, ProfileList, Products) can access them.
                // BamDam and other value-only plugins fall back via the default
                // process_with_hive impl which delegates to process().
                let rows = plugin.process_with_hive(key, &plugin_values, hive);
                for row in rows {
                    // Emit one batch record per plugin row (ValueType = "(plugin)").
                    let kw = key.last_key_written_date_and_time();
                    let last_write =
                        WinTimestamp::from_unix_nanos(kw.timestamp(), kw.timestamp_subsec_nanos())
                            .to_string();
                    let rec = BatchRecord {
                        hive_path: hive_path.to_string(),
                        hive_type: format!("{:?}", entry.hive_type),
                        description: entry.description.clone(),
                        category: entry.category.clone(),
                        // RECmd emits KeyPath without a leading backslash
                        // (e.g. "ROOT\Microsoft\..."), but notatin's key.path
                        // has one (e.g. "\ROOT\Microsoft\..."). Strip the
                        // leading separator so our output matches RECmd's.
                        key_path: key.path.trim_start_matches('\\').to_string(),
                        value_name: row.batch_value_name.clone(),
                        value_type: "(plugin)".to_string(),
                        value_data: row.batch_value_data1.clone(),
                        value_data2: row.batch_value_data2.clone(),
                        value_data3: row.batch_value_data3.clone(),
                        comment: entry.comment.clone(),
                        recursive: dotnet_bool(entry.recursive),
                        deleted: dotnet_bool(key.cell_state.is_deleted()),
                        last_write_timestamp: last_write,
                        plugin_detail_file: detail_basename.clone(),
                    };
                    sink(rec)?;
                    // Forward the detail row to the detail sink.
                    detail_sink(plugin.plugin_name(), row)?;
                }
            }
            // RECmd-exact semantic: skip default dump whenever plugins MATCHED
            // (pluginsToActivate.Count > 0), regardless of whether any rows
            // were emitted. The Task-7 "fall through on zero rows" hack is removed.
            return Ok(());
        }
    }

    // ── Default dump (no-plugin path) ────────────────────────────────────────
    let values: Vec<_> = key.value_iter().collect();
    if let Some(vn) = &entry.value_name {
        // WATCH-POINT (Task 8): the default value's name is matched using
        // get_pretty_name(), which notatin returns as "(default)". RECmd uses
        // Eric's Registry library whose default-value ValueName spelling is
        // unverified ("(default)" vs ""). If Task 8 fixtures show RECmd emits
        // "" for the default value, switch to the raw value name here and at
        // the value_name assignment in build_record. Do not change without a
        // fixture.
        if let Some(v) = values
            .iter()
            .find(|v| v.get_pretty_name().eq_ignore_ascii_case(vn))
        {
            sink(build_record(hive_path, entry, key, Some(v)))?;
        }
        return Ok(());
    }
    if values.is_empty() {
        sink(build_record(hive_path, entry, key, None))?;
    }
    for v in &values {
        sink(build_record(hive_path, entry, key, Some(v)))?;
    }
    Ok(())
}

/// BuildBatchCsvOut: construct one BatchRecord for a key/value pair.
fn build_record(
    hive_path: &str,
    entry: &KeyEntry,
    key: &CellKeyNode,
    value: Option<&notatin::cell_key_value::CellKeyValue>,
) -> BatchRecord {
    let kw = key.last_key_written_date_and_time();
    // notatin gives a chrono DateTime<Utc>; convert via from_unix_nanos (no direct ctor).
    let last_write =
        WinTimestamp::from_unix_nanos(kw.timestamp(), kw.timestamp_subsec_nanos()).to_string();
    let deleted = key.cell_state.is_deleted();

    // HiveType column: RECmd's HiveType.ToString() yields the enum member name
    // (NtUser, Software, UsrClass, System, ...). Rust Debug for our HiveType
    // enum yields the same variant names. This is fixture-pinned in Task 8.
    let hive_type = format!("{:?}", entry.hive_type);

    let mut rec = BatchRecord {
        hive_path: hive_path.to_string(),
        hive_type,
        description: entry.description.clone(),
        category: entry.category.clone(),
        // RESOLVED (Task 8): RECmd emits KeyPath as "ROOT\..." (no leading backslash).
        // notatin's key.path is "\ROOT\..." (leading backslash). Strip the leading
        // separator so our output matches RECmd (verified against DFIRBatch.reb fixtures).
        key_path: key.path.trim_start_matches('\\').to_string(),
        value_name: String::new(),
        value_type: String::new(),
        value_data: String::new(),
        value_data2: String::new(),
        value_data3: String::new(),
        comment: entry.comment.clone(),
        recursive: dotnet_bool(entry.recursive),
        deleted: dotnet_bool(deleted),
        last_write_timestamp: last_write,
        plugin_detail_file: String::new(),
    };

    if let Some(v) = value {
        // WATCH-POINT (Task 8): the default value's name is rendered "(default)"
        // by notatin's get_pretty_name(). RECmd uses Eric's Registry library
        // whose default-value ValueName spelling is unverified ("(default)" vs
        // ""). If Task 8 fixtures show RECmd emits "" for the default value,
        // switch to the raw value name here and at the match site in dump_key.
        // Do not change without a fixture.
        rec.value_name = v.get_pretty_name();
        let rendered = render(v);
        rec.value_type = rendered.value_type.clone();
        let raw = triage_registry::value::raw_bytes(v);
        rec.value_data = compute_value_data(v.data_type, &rendered.value_data, entry, &raw);
    }

    rec
}

/// ValueData per BuildBatchCsvOut: RegBinary → "(Binary data)" unless
/// IncludeBinary, then BinaryConvert(raw); other types → rendered, but
/// only the 5 BinaryConvert cases that RECmd's non-binary switch handles
/// are applied (Epoch, Filetime, Systemtime, DateTimeTicks, OLE). Ip and
/// Sid have no case in RECmd's non-binary switch (Program.cs ~line 2573)
/// and fall through to the rendered string unchanged.
fn compute_value_data(
    dt: CellKeyValueDataTypes,
    rendered: &str,
    entry: &KeyEntry,
    raw: &[u8],
) -> String {
    if dt == CellKeyValueDataTypes::REG_BIN {
        if !entry.include_binary {
            return "(Binary data)".to_string();
        }
        // Binary branch: RECmd's binary switch covers all conversions including
        // Ip and Sid.
        return apply_binary_convert(entry.binary_convert, raw)
            .unwrap_or_else(|| triage_registry::value::bytes_to_hex_dashed(raw));
    }
    // Non-binary branch: RECmd's switch (Program.cs ~line 2573) only handles
    // Epoch, Filetime, Systemtime, DateTimeTicks, OLE — Ip, Sid, and None
    // have no case and keep the rendered string.
    match entry.binary_convert {
        BinaryConvert::Epoch
        | BinaryConvert::Filetime
        | BinaryConvert::Systemtime
        | BinaryConvert::DateTimeTicks
        | BinaryConvert::Ole => {
            apply_binary_convert(entry.binary_convert, raw).unwrap_or_else(|| rendered.to_string())
        }
        _ => rendered.to_string(),
    }
}

/// Expand a KeyPath that may contain `*` wildcards into concrete start keys.
/// No wildcard → at most one GetKey result. With `*`, walk segment-by-segment
/// matching subkey names (RECmd ExpandKeyPath).
fn expand_key_path(hive: &mut Hive, key_path: &str) -> Vec<CellKeyNode> {
    if !key_path.contains('*') {
        return hive.get_key(key_path).into_iter().collect();
    }
    let Some(root) = hive.root() else {
        return Vec::new();
    };
    let segments: Vec<&str> = key_path.split('\\').collect();
    let mut frontier = vec![root];
    for seg in segments {
        let mut next = Vec::new();
        for mut node in frontier {
            for mut sub in hive.sub_keys(&mut node) {
                let matches = if seg.contains('*') {
                    wildcard_match(&seg.to_lowercase(), &sub.key_name.to_lowercase())
                } else {
                    sub.key_name.eq_ignore_ascii_case(seg)
                };
                if matches {
                    next.push(std::mem::take(&mut sub));
                }
            }
        }
        frontier = next;
    }
    frontier
}

/// Simple `*`-glob match (no `?`), matching RECmd's wildcard semantics.
/// Made `pub(crate)` so Task 7's plugin matcher can reuse it.
pub(crate) fn wildcard_match(pattern: &str, text: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    let mut pos = 0usize;
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        match text[pos..].find(part) {
            Some(idx) => {
                if i == 0 && idx != 0 && !pattern.starts_with('*') {
                    return false;
                }
                pos += idx + part.len();
            }
            None => return false,
        }
    }
    if let Some(last) = parts.last() {
        if !last.is_empty() && !pattern.ends_with('*') {
            return text.ends_with(last);
        }
    }
    true
}

// ─── Engine integration test (env-gated) ───────────────────────────────────

#[cfg(test)]
mod engine_tests {
    use super::*;
    use crate::reb::{parse_reb, DFIR_BATCH};
    use triage_registry::hive::Hive;

    #[test]
    fn software_run_keys_emit_rows() {
        let Some((primary, logs)) = crate::testsupport::find_hive("SOFTWARE") else {
            eprintln!("SKIP: no SOFTWARE hive in test captures");
            return;
        };
        // Use recover=false to keep this test fast — the CLI default is true
        // (matching RECmd), but recover=true can take ~90s per hive. Overriding
        // to false here is intentional: do not change for test speed.
        let mut hive = Hive::open(&primary, &logs, false).unwrap();
        let entries = parse_reb(DFIR_BATCH).unwrap();
        let plugins = crate::plugins::registry();
        let mut rows = Vec::new();
        for e in &entries {
            process_entry(
                &mut hive,
                "C:\\Windows\\System32\\config\\SOFTWARE",
                e,
                &plugins,
                &mut |r| {
                    rows.push(r);
                    Ok(())
                },
                &mut |_name, _row| Ok(()),
            )
            .unwrap();
        }
        assert!(!rows.is_empty(), "DFIRBatch should match SOFTWARE keys");
        // All rows from a SOFTWARE hive must have hive_type == "Software"
        assert!(
            rows.iter().all(|r| r.hive_type == "Software"),
            "unexpected hive_type in rows"
        );
    }
}

// ─── Existing header test ────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Guard: the emitted CSV header must match RECmd's BatchCsvOut header exactly.
    /// Any field rename or reorder will break compat with RECmd consumers.
    #[test]
    fn batch_record_csv_header_matches_recmd() {
        let record = BatchRecord {
            hive_path: String::new(),
            hive_type: String::new(),
            description: String::new(),
            category: String::new(),
            key_path: String::new(),
            value_name: String::new(),
            value_type: String::new(),
            value_data: String::new(),
            value_data2: String::new(),
            value_data3: String::new(),
            comment: String::new(),
            recursive: dotnet_bool(false),
            deleted: dotnet_bool(false),
            last_write_timestamp: String::new(),
            plugin_detail_file: String::new(),
        };

        let mut buf = Vec::new();
        {
            let mut wtr = csv::Writer::from_writer(&mut buf);
            wtr.serialize(&record).expect("serialize");
            wtr.flush().expect("flush");
        }

        let text = String::from_utf8(buf).expect("utf8");
        let header = text.lines().next().expect("header line");
        assert_eq!(
            header,
            "HivePath,HiveType,Description,Category,KeyPath,ValueName,ValueType,\
ValueData,ValueData2,ValueData3,Comment,Recursive,Deleted,LastWriteTimestamp,PluginDetailFile"
        );
    }

    /// Guard: Recursive and Deleted must serialize as C# "True"/"False" (not
    /// Rust "true"/"false") to match RECmd's BatchCsvOut output.
    #[test]
    fn bool_columns_serialize_as_dotnet_capitalized() {
        let record_true_false = BatchRecord {
            hive_path: String::new(),
            hive_type: String::new(),
            description: String::new(),
            category: String::new(),
            key_path: String::new(),
            value_name: String::new(),
            value_type: String::new(),
            value_data: String::new(),
            value_data2: String::new(),
            value_data3: String::new(),
            comment: String::new(),
            recursive: dotnet_bool(true),
            deleted: dotnet_bool(false),
            last_write_timestamp: String::new(),
            plugin_detail_file: String::new(),
        };

        let mut buf = Vec::new();
        {
            let mut wtr = csv::Writer::from_writer(&mut buf);
            wtr.serialize(&record_true_false).expect("serialize");
            wtr.flush().expect("flush");
        }

        let text = String::from_utf8(buf).expect("utf8");
        let row = text.lines().nth(1).expect("data row");
        let fields: Vec<&str> = row.split(',').collect();
        // Recursive is column index 11, Deleted is index 12 (0-based).
        assert_eq!(
            fields[11], "True",
            "Recursive=true must serialize as \"True\", got {:?}",
            fields[11]
        );
        assert_eq!(
            fields[12], "False",
            "Deleted=false must serialize as \"False\", got {:?}",
            fields[12]
        );
    }
}
