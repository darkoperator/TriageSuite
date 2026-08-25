use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use evtx::{EvtxParser, ParserSettings};
use roxmltree::Document;
use time::OffsetDateTime;

use crate::error::{EvtxTriageError, Result};
use crate::maps::{normalize_binxml_value, resolve_entry, MapIndex};
use crate::record::EventRecord;

// ---------------------------------------------------------------------------
// Timestamp utilities
// ---------------------------------------------------------------------------

// TimeCreated is rendered as ISO 8601 UTC (`yyyy-MM-ddTHH:mm:ss.fffffffZ`) via
// the shared `triage_core::timestamp::format_iso8601`.
use triage_core::timestamp::format_iso8601 as format_time;

fn parse_system_time(s: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok()
}

/// Convert the record's binary header timestamp (`jiff::Timestamp`, re-exported
/// as `evtx::Timestamp`) to a `time::OffsetDateTime`, preserving full 100ns
/// FILETIME precision.
fn jiff_to_offset(ts: evtx::Timestamp) -> Option<OffsetDateTime> {
    OffsetDateTime::from_unix_timestamp_nanos(ts.as_nanosecond()).ok()
}

/// The event's TimeCreated, at full 100ns precision when recoverable.
///
/// The `evtx` crate's XML `SystemTime` is only microsecond-precise, so the 7th
/// fractional digit is lost (EvtxECmd reads the raw FILETIME and keeps it). The
/// record's binary header timestamp *does* carry the 100ns tick. When the
/// header timestamp denotes the same instant as the XML SystemTime (equal to
/// the microsecond), use it to recover the lost digit; otherwise — e.g.
/// forwarded events whose write-time differs from the event time — keep the
/// authoritative XML SystemTime.
fn time_created(system_time: OffsetDateTime, header_ts: evtx::Timestamp) -> String {
    if let Some(ht) = jiff_to_offset(header_ts) {
        let micros = |t: OffsetDateTime| t.unix_timestamp_nanos() / 1_000;
        if micros(ht) == micros(system_time) {
            return format_time(ht);
        }
    }
    format_time(system_time)
}

// ---------------------------------------------------------------------------
// ParseOptions and filter logic
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Clone)]
pub struct ParseOptions {
    /// Include only these Event IDs. Empty = include all.
    pub include_ids: HashSet<u32>,
    /// Exclude these Event IDs.
    pub exclude_ids: HashSet<u32>,
    /// Include only channels whose name contains this substring (case-insensitive).
    pub channel_filter: Option<String>,
    /// Skip events before this UTC datetime.
    pub start_date: Option<OffsetDateTime>,
    /// Skip events after this UTC datetime.
    pub end_date: Option<OffsetDateTime>,
    /// Time discrepancy threshold in seconds (default 1).
    pub tdt_threshold_secs: f64,
}

impl ParseOptions {
    pub fn new() -> Self {
        Self {
            tdt_threshold_secs: 1.0,
            ..Default::default()
        }
    }
}

fn passes_filters(channel: &str, event_id: u32, time: OffsetDateTime, opts: &ParseOptions) -> bool {
    if let Some(ch) = &opts.channel_filter {
        if !channel.to_lowercase().contains(&ch.to_lowercase()) {
            return false;
        }
    }
    if let Some(start) = opts.start_date {
        if time < start {
            return false;
        }
    }
    if let Some(end) = opts.end_date {
        if time > end {
            return false;
        }
    }
    if !opts.include_ids.is_empty() && !opts.include_ids.contains(&event_id) {
        return false;
    }
    if opts.exclude_ids.contains(&event_id) {
        return false;
    }
    true
}

// ---------------------------------------------------------------------------
// XML field extraction helpers
// ---------------------------------------------------------------------------

fn xml_text<'a>(doc: &'a Document<'a>, path: &[&str]) -> &'a str {
    let mut node = doc.root_element();
    for &seg in path {
        match node
            .children()
            .find(|n| n.is_element() && n.tag_name().name() == seg)
        {
            Some(n) => node = n,
            None => return "",
        }
    }
    node.text().unwrap_or("")
}

fn xml_attr<'a>(doc: &'a Document<'a>, path: &[&str], attr: &str) -> &'a str {
    let mut node = doc.root_element();
    for &seg in path {
        match node
            .children()
            .find(|n| n.is_element() && n.tag_name().name() == seg)
        {
            Some(n) => node = n,
            None => return "",
        }
    }
    node.attribute(attr).unwrap_or("")
}

fn extract_event_data_raw(doc: &Document<'_>) -> Vec<String> {
    let event = doc.root_element();
    let event_data = match event
        .children()
        .find(|n| n.is_element() && n.tag_name().name() == "EventData")
    {
        Some(n) => n,
        None => return vec![],
    };

    event_data
        .children()
        .filter(|n| n.is_element() && n.tag_name().name() == "Data")
        .map(|n| {
            let name = n.attribute("Name").unwrap_or("");
            let val = n.text().unwrap_or("");
            if name.is_empty() {
                val.to_string()
            } else {
                format!("{name}: {val}")
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Level name helper
// ---------------------------------------------------------------------------

fn level_name(level: u8) -> &'static str {
    match level {
        0 => "LogAlways",
        1 => "Critical",
        2 => "Error",
        3 => "Warning",
        4 => "Information",
        5 => "Verbose",
        _ => "Unknown",
    }
}

// ---------------------------------------------------------------------------
// Keywords
// ---------------------------------------------------------------------------

/// Translate a `System/Keywords` value into EvtxECmd's friendly text. Only the
/// standard reserved audit keywords are named; any other value (provider-defined
/// keywords need the provider manifest, which we don't carry) is returned as the
/// original hex string.
fn keywords_friendly(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let hex = trimmed.trim_start_matches("0x").trim_start_matches("0X");
    let Ok(bits) = u64::from_str_radix(hex, 16) else {
        return trimmed.to_string();
    };
    const AUDIT_SUCCESS: u64 = 0x0020_0000_0000_0000;
    const AUDIT_FAILURE: u64 = 0x0010_0000_0000_0000;
    let mut names = Vec::new();
    if bits & AUDIT_SUCCESS != 0 {
        names.push("Audit success");
    }
    if bits & AUDIT_FAILURE != 0 {
        names.push("Audit failure");
    }
    if names.is_empty() {
        trimmed.to_string()
    } else {
        names.join(", ")
    }
}

// ---------------------------------------------------------------------------
// Payload (raw event data as JSON, matching EvtxECmd's Payload column)
// ---------------------------------------------------------------------------

/// Build the `Payload` column: the event's `EventData`/`UserData` rendered as
/// JSON in the same shape EvtxECmd emits, e.g.
/// `{"EventData":{"Data":[{"@Name":"X","#text":"Y"}]}}`.
fn build_payload(doc: &Document<'_>) -> String {
    let root = doc.root_element();
    for container in ["EventData", "UserData"] {
        if let Some(node) = root
            .children()
            .find(|n| n.is_element() && n.tag_name().name() == container)
        {
            let data: Vec<serde_json::Value> = node
                .children()
                .filter(|n| n.is_element() && n.tag_name().name() == "Data")
                .map(|n| {
                    // EvtxECmd omits the "#text" key entirely when a Data
                    // element has no value node (e.g. <Data Name="ProcessName"/>).
                    let name = n.attribute("Name");
                    let text = n.text().map(normalize_binxml_value);
                    match (name, text) {
                        (Some(name), Some(text)) => {
                            serde_json::json!({ "@Name": name, "#text": text })
                        }
                        (Some(name), None) => serde_json::json!({ "@Name": name }),
                        (None, Some(text)) => serde_json::json!({ "#text": text }),
                        (None, None) => serde_json::json!({}),
                    }
                })
                .collect();
            let obj = serde_json::json!({ container: { "Data": data } });
            return obj.to_string();
        }
    }
    String::new()
}

// ---------------------------------------------------------------------------
// Main parse function
// ---------------------------------------------------------------------------

pub fn parse_evtx_file(
    path: &Path,
    opts: &ParseOptions,
    maps: &MapIndex,
    on_record: &mut impl FnMut(EventRecord),
) -> Result<()> {
    match visit_evtx_file(path, opts, maps, &mut |record| {
        on_record(record);
        Ok::<(), std::convert::Infallible>(())
    }) {
        Ok(()) => Ok(()),
        Err(VisitError::Parse(error)) => Err(error),
        Err(VisitError::Visitor(never)) => match never {},
    }
}

#[derive(Debug)]
pub enum VisitError<E> {
    Parse(EvtxTriageError),
    Visitor(E),
}

/// Stream records to a fallible visitor without collecting the complete log.
pub fn visit_evtx_file<E>(
    path: &Path,
    opts: &ParseOptions,
    maps: &MapIndex,
    on_record: &mut impl FnMut(EventRecord) -> std::result::Result<(), E>,
) -> std::result::Result<(), VisitError<E>> {
    let mut parser = EvtxParser::from_path(path)
        .map_err(|e| VisitError::Parse(EvtxTriageError::evtx_parse(path, e.to_string())))?;
    let settings = Arc::new(ParserSettings::default());

    // Iterate chunk-by-chunk so each record carries EvtxECmd's ChunkNumber (the
    // 0-based index of the chunk it lives in). `records()` would flatten this away.
    for (chunk_idx, chunk_res) in parser.chunks().enumerate() {
        let mut chunk_data = match chunk_res {
            Ok(c) => c,
            Err(e) => {
                eprintln!("warning: skipping chunk in {}: {e}", path.display());
                continue;
            }
        };
        let mut chunk = match chunk_data.parse(Arc::clone(&settings)) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("warning: cannot parse chunk in {}: {e}", path.display());
                continue;
            }
        };
        for rec_res in chunk.iter() {
            let serialized = match rec_res.and_then(|r| r.into_xml()) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("warning: skipping record in {}: {e}", path.display());
                    continue;
                }
            };
            let mut visitor_error = None;
            process_record(
                &serialized.data,
                serialized.event_record_id,
                serialized.timestamp,
                chunk_idx as i64,
                path,
                opts,
                maps,
                &mut |record| {
                    if visitor_error.is_none() {
                        if let Err(error) = on_record(record) {
                            visitor_error = Some(error);
                        }
                    }
                },
            );
            if let Some(error) = visitor_error {
                return Err(VisitError::Visitor(error));
            }
        }
    }

    Ok(())
}

/// Parse one record's XML into an `EventRecord`, applying filters and maps.
/// Skips the record (no callback) on malformed XML, an unparseable timestamp,
/// or a filter miss.
#[allow(clippy::too_many_arguments)]
fn process_record(
    xml: &str,
    event_record_id: u64,
    header_ts: evtx::Timestamp,
    chunk_number: i64,
    path: &Path,
    opts: &ParseOptions,
    maps: &MapIndex,
    on_record: &mut impl FnMut(EventRecord),
) {
    let doc = match Document::parse(xml) {
        Ok(d) => d,
        Err(_) => return,
    };

    let channel = xml_text(&doc, &["System", "Channel"]).to_owned();
    let event_id: u32 = xml_text(&doc, &["System", "EventID"])
        .trim()
        .parse()
        .unwrap_or(0);

    let system_time_str = xml_attr(&doc, &["System", "TimeCreated"], "SystemTime");
    let system_time = match parse_system_time(system_time_str) {
        Some(t) => t,
        None => return,
    };

    if !passes_filters(&channel, event_id, system_time, opts) {
        return;
    }

    let provider = xml_attr(&doc, &["System", "Provider"], "Name").to_owned();
    let level = level_name(
        xml_text(&doc, &["System", "Level"])
            .trim()
            .parse::<u8>()
            .unwrap_or(0),
    )
    .to_owned();
    let computer = xml_text(&doc, &["System", "Computer"]).to_owned();
    let process_id = xml_attr(&doc, &["System", "Execution"], "ProcessID").to_owned();
    let thread_id = xml_attr(&doc, &["System", "Execution"], "ThreadID").to_owned();
    // The record's own security principal (a SID). EvtxECmd puts this in UserId,
    // distinct from the map-derived UserName.
    let user_id = xml_attr(&doc, &["System", "Security"], "UserID").to_owned();
    let keywords = keywords_friendly(xml_text(&doc, &["System", "Keywords"]));
    let payload = build_payload(&doc);

    // Maps: resolve each entry's template, assign to its named output column.
    let mut map_description = String::new();
    let mut user_name = String::new();
    let mut remote_host = String::new();
    let mut executable_info = String::new();
    let mut payload_slots: [String; 6] = Default::default();

    if let Some(map) = maps.lookup(&channel, event_id) {
        map_description = map.description.clone();
        for entry in &map.entries {
            let resolved = resolve_entry(entry, xml);
            match entry.property.as_str() {
                "UserName" => user_name = resolved,
                "RemoteHost" => remote_host = resolved,
                "ExecutableInfo" => executable_info = resolved,
                "MapDescription" => map_description = resolved,
                "PayloadData1" => payload_slots[0] = resolved,
                "PayloadData2" => payload_slots[1] = resolved,
                "PayloadData3" => payload_slots[2] = resolved,
                "PayloadData4" => payload_slots[3] = resolved,
                "PayloadData5" => payload_slots[4] = resolved,
                "PayloadData6" => payload_slots[5] = resolved,
                _ => {}
            }
        }
    } else {
        let raw = extract_event_data_raw(&doc);
        for (i, v) in raw.into_iter().take(6).enumerate() {
            payload_slots[i] = v;
        }
    }

    let [p1, p2, p3, p4, p5, p6] = payload_slots;
    on_record(EventRecord {
        // EvtxECmd's RecordNumber equals the EventRecordId.
        record_number: event_record_id,
        event_record_id,
        time_created: time_created(system_time, header_ts),
        event_id,
        level,
        provider,
        channel,
        process_id,
        thread_id,
        computer,
        chunk_number,
        user_id,
        map_description,
        user_name,
        remote_host,
        payload_data1: p1,
        payload_data2: p2,
        payload_data3: p3,
        payload_data4: p4,
        payload_data5: p5,
        payload_data6: p6,
        executable_info,
        // We only parse live records, never slack/recovered ones.
        hidden_record: "False".to_owned(),
        source_file: path.to_owned(),
        keywords,
        extra_data_offset: 0,
        payload,
    });
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn sample_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("sample_files")
    }

    fn empty_maps() -> MapIndex {
        MapIndex::load(std::path::Path::new("/nonexistent"))
    }

    // vendored from EVTXTriage: the standalone project's `crate::collection`
    // module is not vendored into triage-evtx (discovery lives in the binary
    // crate), so the sample-driven tests do a minimal local `.evtx` discovery.
    fn discover_sample_evtx(dir: &std::path::Path) -> Vec<PathBuf> {
        let mut files = Vec::new();
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) == Some("evtx") {
                    files.push(path);
                }
            }
        }
        files
    }

    #[test]
    fn parses_sample_files_without_error() {
        let files = discover_sample_evtx(&sample_dir());
        if files.is_empty() {
            return;
        }
        let opts = ParseOptions::new();
        let maps = empty_maps();
        let mut count = 0usize;

        for path in &files {
            parse_evtx_file(path, &opts, &maps, &mut |_| count += 1).unwrap();
        }
        assert!(count > 0, "expected at least one parsed event");
    }

    #[test]
    fn id_include_filter_restricts_output() {
        let files = discover_sample_evtx(&sample_dir());
        if files.is_empty() {
            return;
        }
        let mut opts = ParseOptions::new();
        opts.include_ids.insert(99999); // unlikely to exist

        let maps = empty_maps();
        let mut count = 0usize;
        for path in &files {
            parse_evtx_file(path, &opts, &maps, &mut |_| count += 1).unwrap();
        }
        assert_eq!(count, 0);
    }

    #[test]
    fn id_exclude_filter_removes_events() {
        let files = discover_sample_evtx(&sample_dir());
        if files.is_empty() {
            return;
        }
        let opts_all = ParseOptions::new();
        let maps = empty_maps();
        let mut all_records = Vec::new();
        for path in &files {
            parse_evtx_file(path, &opts_all, &maps, &mut |r| all_records.push(r)).unwrap();
        }

        if all_records.is_empty() {
            return;
        }

        let exclude_id = all_records[0].event_id;
        let mut opts_ex = ParseOptions::new();
        opts_ex.exclude_ids.insert(exclude_id);

        let mut filtered = Vec::new();
        for path in &files {
            parse_evtx_file(path, &opts_ex, &maps, &mut |r| filtered.push(r)).unwrap();
        }
        assert!(filtered.iter().all(|r| r.event_id != exclude_id));
    }

    #[test]
    fn schema_invariants_hold() {
        let files = discover_sample_evtx(&sample_dir());
        if files.is_empty() {
            return;
        }
        let opts = ParseOptions::new();
        let maps = empty_maps();
        let mut records = Vec::new();
        for path in &files {
            parse_evtx_file(path, &opts, &maps, &mut |r| records.push(r)).unwrap();
        }
        for r in &records {
            // EvtxECmd parity: RecordNumber mirrors EventRecordId, ChunkNumber is
            // the 0-based chunk index, and live records are never hidden.
            assert_eq!(r.record_number, r.event_record_id);
            assert!(r.chunk_number >= 0);
            assert_eq!(r.hidden_record, "False");
        }
    }

    #[test]
    fn level_zero_is_log_always() {
        assert_eq!(level_name(0), "LogAlways");
    }

    #[test]
    fn keywords_audit_success_is_named() {
        assert_eq!(keywords_friendly("0x8020000000000000"), "Audit success");
        assert_eq!(keywords_friendly("0x8010000000000000"), "Audit failure");
        // Unknown provider keywords fall back to the raw hex.
        assert_eq!(
            keywords_friendly("0x0000000000000080"),
            "0x0000000000000080"
        );
    }

    const SECURITY_4624_XML: &str = r#"<Event xmlns="http://schemas.microsoft.com/win/2004/08/events/event">
  <System>
    <Provider Name="Microsoft-Windows-Security-Auditing"/>
    <EventID>4624</EventID>
    <Level>0</Level>
    <Channel>Security</Channel>
    <Computer>STDC1.umbralabs.dev</Computer>
    <Execution ProcessID="796" ThreadID="9456"/>
    <Security/>
    <Keywords>0x8020000000000000</Keywords>
    <TimeCreated SystemTime="2026-02-22T07:41:39.5117297Z"/>
    <EventRecordID>1837248</EventRecordID>
  </System>
  <EventData>
    <Data Name="SubjectUserSid">S-1-0-0</Data>
    <Data Name="TargetUserName">STDC1$</Data>
    <Data Name="TargetDomainName">UMBRALABS.DEV</Data>
    <Data Name="LogonType">3</Data>
    <Data Name="IpAddress">fe80::1</Data>
    <Data Name="TargetLogonId">0x17ba98af</Data>
    <Data Name="LogonGuid">A3F72D14-DC88-456C-ED77-AC006BC206A1</Data>
  </EventData>
</Event>"#;

    const SECURITY_4624_MAP: &str = r#"
Channel: Security
EventId: 4624
Description: Successful logon
Maps:
  - Property: PayloadData1
    PropertyValue: "Target: %domain%\\%user%"
    Values:
      - {Name: domain, Value: '/Event/EventData/Data[@Name="TargetDomainName"]'}
      - {Name: user, Value: '/Event/EventData/Data[@Name="TargetUserName"]'}
  - Property: UserName
    PropertyValue: "%u%"
    Values:
      - {Name: u, Value: '/Event/EventData/Data[@Name="TargetUserName"]'}
  - Property: RemoteHost
    PropertyValue: "%ip%"
    Values:
      - {Name: ip, Value: '/Event/EventData/Data[@Name="IpAddress"]'}
  - Property: ExecutableInfo
    PropertyValue: "-"
"#;

    /// End-to-end check of `process_record`: the produced row must match the
    /// EvtxECmd column semantics (Level, RecordNumber, ChunkNumber, ProcessId,
    /// keyword translation, map-derived UserName/RemoteHost, JSON Payload).
    #[test]
    fn process_record_matches_evtxecmd_semantics() {
        let maps = MapIndex::from_contents([("Security_4624.map", SECURITY_4624_MAP)]);
        let opts = ParseOptions::new();
        let mut out = Vec::new();
        process_record(
            SECURITY_4624_XML,
            1837248,
            // Header timestamp at epoch (≠ the event SystemTime) so TimeCreated
            // falls back to the authoritative XML SystemTime in this unit test.
            evtx::Timestamp::UNIX_EPOCH,
            0,
            std::path::Path::new("Security.evtx"),
            &opts,
            &maps,
            &mut |r| out.push(r),
        );
        assert_eq!(out.len(), 1);
        let r = &out[0];
        assert_eq!(r.record_number, 1837248);
        assert_eq!(r.event_record_id, 1837248);
        assert_eq!(r.time_created, "2026-02-22T07:41:39.5117297Z");
        assert_eq!(r.event_id, 4624);
        assert_eq!(r.level, "LogAlways");
        assert_eq!(r.process_id, "796");
        assert_eq!(r.thread_id, "9456");
        assert_eq!(r.chunk_number, 0);
        assert_eq!(r.user_id, ""); // Security@UserID absent -> empty, not a fallback
        assert_eq!(r.user_name, "STDC1$"); // map-derived, lives in its own column
        assert_eq!(r.remote_host, "fe80::1");
        assert_eq!(r.executable_info, "-");
        assert_eq!(r.payload_data1, "Target: UMBRALABS.DEV\\STDC1$");
        assert_eq!(r.keywords, "Audit success");
        assert_eq!(r.hidden_record, "False");
        assert_eq!(r.extra_data_offset, 0);
        assert!(
            r.payload.starts_with(
                r##"{"EventData":{"Data":[{"@Name":"SubjectUserSid","#text":"S-1-0-0"}"##
            ),
            "payload was: {}",
            r.payload
        );
        // EvtxECmd casing convention: hex uppercased, GUID lowercased.
        assert!(
            r.payload.contains(r##""#text":"0x17BA98AF""##),
            "hex not uppercased in payload: {}",
            r.payload
        );
        assert!(
            r.payload
                .contains(r##""#text":"a3f72d14-dc88-456c-ed77-ac006bc206a1""##),
            "GUID not lowercased in payload: {}",
            r.payload
        );
    }

    #[test]
    fn normalize_binxml_value_matches_evtxecmd_casing() {
        use crate::maps::normalize_binxml_value;
        assert_eq!(normalize_binxml_value("0x17ba98af"), "0x17BA98AF");
        assert_eq!(normalize_binxml_value("0x0"), "0x0");
        assert_eq!(
            normalize_binxml_value("A3F72D14-DC88-456C-ED77-AC006BC206A1"),
            "a3f72d14-dc88-456c-ed77-ac006bc206a1"
        );
        // Non-hex / non-GUID values pass through unchanged.
        assert_eq!(normalize_binxml_value("STDC1$"), "STDC1$");
        assert_eq!(normalize_binxml_value("fe80::1"), "fe80::1");
        assert_eq!(normalize_binxml_value("%%1842"), "%%1842");
    }
}
