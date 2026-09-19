//! EvtxTriage: EvtxECmd-compatible Windows .evtx event-log parser.

pub mod cli;
pub mod maps_embed;
pub mod sync;

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Mutex;

use triage_core::error::TriageError;
use triage_core::output::dataset::{DatasetSpec, JsonFraming};
use triage_core::output::duckdb::types::{ColumnType, DatasetColumnTypes, SqlType, TimeSemantics};
use triage_core::output::router::OutputRouter;
use triage_core::tool::{Scope, Tool};
use triage_evtx::{MapIndex, ParseOptions};

/// Filename stem for an individual per-log export: alphanumerics and `-` pass
/// through, every other character (path separators included) collapses to
/// `_`, and the result is trimmed of leading/trailing `_`.
///
/// The log name comes from a source filename in an untrusted capture, so it
/// must not be able to steer the output path: separators are collapsed, not
/// escaped, which also rules out `.` and `..` reaching the filesystem as path
/// components.
fn individual_stem(log_name: &str) -> String {
    let cleaned: String = log_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches('_');
    if trimmed.is_empty() {
        "unknown".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Filename for an individual per-log export (`<sanitized>.csv`). Exposed so
/// callers (and tests) can predict the name a given source log name produces.
pub fn individual_filename(log_name: &str) -> String {
    format!("{}.csv", individual_stem(log_name))
}

pub const DATASETS: &[DatasetSpec] = &[DatasetSpec {
    id: "events",
    default_basename: "EvtxTriage_Output",
    framing: JsonFraming::Ndjson,
    csv_only: false,
    override_suffix: None,
}];

/// Declared SQL types for the DuckDB view layer.
///
/// `TimeCreated` is a `String` on `EventRecord`, but every value comes from
/// `parser::time_created`, which returns `format_time(..)` on both of its
/// branches and never an empty string -- a canonical ISO-8601 instant with
/// the 100ns tick recovered from the record header where the XML SystemTime
/// lost it. The shape is therefore proven by that function, not by the field
/// type.
///
/// `ProcessId` and `ThreadId` are deliberately NOT declared even though they
/// look numeric: they are `String` on the record for EvtxECmd parity, which
/// keeps whatever the event XML carried, and an event whose `Execution` node
/// is absent or malformed leaves a non-numeric value there. Declaring BIGINT
/// would turn those rows' text into a NULL the `__text` companion could only
/// half explain.
///
/// These apply to the combined `events` dataset only. The `--split` and
/// `Individual/` exports are dynamic datasets, and the view builder attaches
/// declared types to static dataset ids alone, so those views stay
/// all-VARCHAR.
///
/// Left undeclared, all free text: `Level`, `Provider`, `Channel`,
/// `Computer`, `UserId`, `MapDescription`, `UserName`, `RemoteHost`,
/// `PayloadData1`..`6`, `ExecutableInfo`, `HiddenRecord`, `SourceFile`,
/// `Keywords` and `Payload`.
pub const COLUMN_TYPES: &[DatasetColumnTypes] = &[DatasetColumnTypes {
    dataset_id: "events",
    columns: &[
        ColumnType {
            column: "RecordNumber",
            sql_type: SqlType::UBigInt,
            time_semantics: None,
        },
        ColumnType {
            column: "EventRecordId",
            sql_type: SqlType::UBigInt,
            time_semantics: None,
        },
        ColumnType {
            column: "TimeCreated",
            sql_type: SqlType::Timestamp,
            time_semantics: Some(TimeSemantics::Utc),
        },
        ColumnType {
            column: "EventId",
            sql_type: SqlType::BigInt,
            time_semantics: None,
        },
        ColumnType {
            column: "ChunkNumber",
            sql_type: SqlType::BigInt,
            time_semantics: None,
        },
        ColumnType {
            column: "ExtraDataOffset",
            sql_type: SqlType::BigInt,
            time_semantics: None,
        },
    ],
}];

pub struct EvtxTool {
    pub maps: MapIndex,
    pub opts: ParseOptions,
    /// Additive: when true, every event is also written to a per-source-file dataset,
    /// alongside (not instead of) the combined "events" dataset.
    pub split: bool,
    /// Additive, default on: when true, every event is also written to a
    /// per-source-*log* (channel) dataset under `Individual/`, matching
    /// VeloProcessor's default behavior (it opts out via `-SkipEvtx`).
    /// `--no-individual` sets this false.
    pub individual: bool,
    /// Used output stems for `--split` collision handling across per-file parse calls.
    pub used_stems: Mutex<HashSet<String>>,
    /// Case-insensitive registry for `Individual/` export stems: lowercased
    /// stem -> the canonical (lexicographically smallest) exact-case stem
    /// chosen as that channel group's on-disk name. Real captures contain
    /// channels that differ only in case (e.g.
    /// `Microsoft-Windows-AppXPackaging/Operational` and
    /// `...AppxPackaging/Operational`, both from the same physical `.evtx`),
    /// which sanitize to filenames that collide on a case-insensitive
    /// filesystem (macOS, Windows by default) even though `individual_stem`
    /// itself preserves case. Folding those into one file here — rather than
    /// letting two different HashMap/router keys race to create what is
    /// really one physical file — is what keeps the second channel's events
    /// from silently overwriting the first's. The original exact `Channel`
    /// value survives per row regardless (see `write_individual`), so
    /// folded rows stay distinguishable.
    ///
    /// The canonical name is the lexicographically smallest spelling seen so
    /// far, not the first one encountered: `resolve_individual_stem` moves
    /// the registry entry (and, via `OutputRouter::rekey_dynamic`, the
    /// eventual publish destination of the already-open file) onto a smaller
    /// spelling as soon as one arrives, so the two orders "large-then-small"
    /// and "small-then-large" converge on the same on-disk name.
    individual_stems: Mutex<HashMap<String, String>>,
}

impl Default for EvtxTool {
    fn default() -> Self {
        EvtxTool::new(true)
    }
}

impl EvtxTool {
    /// Construct with the individual-per-log-export switch explicit. Existing
    /// construction sites (`EvtxTool { .. }` struct-literal in `main.rs`,
    /// `EvtxTool::default()` in the orchestrator registry) still work
    /// unchanged; this is the one new entry point `--no-individual` reaches
    /// through.
    pub fn new(individual: bool) -> Self {
        EvtxTool {
            maps: crate::maps_embed::load_bundled(),
            opts: ParseOptions::new(),
            split: false,
            individual,
            used_stems: Mutex::new(HashSet::new()),
            individual_stems: Mutex::new(HashMap::new()),
        }
    }

    /// Resolve the exact-case stem an `Individual/` export for `stem` should
    /// use: the lexicographically smallest spelling seen so far for `stem`'s
    /// lowercased key, canonical regardless of which order the colliding
    /// spellings are encountered in. If `stem` is smaller than the currently
    /// registered spelling, the already-open dynamic file is retargeted onto
    /// it (`OutputRouter::rekey_dynamic`) before returning — cheap and safe,
    /// since nothing is written to its final destination until
    /// `OutputRouter::finish()`. Every row keeps its own real `Channel`
    /// value regardless of which spelling names the shared file, so no row
    /// is ever lost or misattributed by this.
    ///
    /// This is the one caller `rekey_dynamic`'s contract is written against:
    /// the registered spelling only ever moves strictly downward
    /// (lexicographically smaller), never sideways or back up, so an
    /// abandoned `old_basename` can never collide with some other group's
    /// still-live target; and every candidate basename lands under the same
    /// flat `Individual/` directory, so the move is always same-directory.
    /// `rekey_dynamic`'s `Err` arms exist for a caller that breaks one of
    /// those properties, which this one does not, so its error is
    /// propagated rather than silently swallowed.
    fn resolve_individual_stem(
        &self,
        out: &mut OutputRouter,
        stem: String,
    ) -> Result<String, TriageError> {
        let lower = stem.to_lowercase();
        let mut seen = self.individual_stems.lock().unwrap();
        match seen.get(&lower) {
            None => {
                seen.insert(lower, stem.clone());
                Ok(stem)
            }
            Some(existing) if stem < *existing => {
                let old_basename = format!("Individual/{existing}");
                let new_basename = format!("Individual/{stem}");
                out.rekey_dynamic(&old_basename, &new_basename)?;
                seen.insert(lower, stem.clone());
                Ok(stem)
            }
            Some(existing) => Ok(existing.clone()),
        }
    }

    /// Allocate an unused output stem for a source path, suffixing `_1`, `_2`, …
    /// on collision across the runner's per-file parse calls.
    fn allocate_stem(&self, path: &Path) -> String {
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("output")
            .to_string();
        let mut used = self.used_stems.lock().unwrap();
        if used.insert(stem.clone()) {
            return stem;
        }
        let mut n = 1u32;
        loop {
            let candidate = format!("{stem}_{n}");
            if used.insert(candidate.clone()) {
                return candidate;
            }
            n += 1;
        }
    }
}

impl Tool for EvtxTool {
    fn binary_name(&self) -> &'static str {
        "EvtxTriage"
    }

    fn patterns(&self) -> &[&'static str] {
        &["*.evtx"]
    }

    fn validate_legacy(&self, path: &Path) -> bool {
        use std::io::Read;
        let mut buf = [0u8; 8];
        match std::fs::File::open(path).and_then(|mut f| f.read_exact(&mut buf)) {
            // .evtx files begin with the ASCII signature "ElfFile\0".
            Ok(()) => &buf == b"ElfFile\0",
            Err(_) => false,
        }
    }

    fn invalid_content_is_corrupt(&self) -> bool {
        true
    }

    fn datasets(&self) -> &'static [DatasetSpec] {
        DATASETS
    }

    fn column_types(&self) -> &'static [DatasetColumnTypes] {
        COLUMN_TYPES
    }

    fn scope(&self) -> Scope {
        Scope::SystemWide
    }

    fn resource_class(&self) -> triage_core::tool::ResourceClass {
        triage_core::tool::ResourceClass::Heavy
    }

    fn parse(&self, path: &Path, out: &mut OutputRouter) -> Result<u64, TriageError> {
        let to_err = |e: triage_evtx::EvtxTriageError| TriageError::Artifact {
            path: path.to_path_buf(),
            message: e.to_string(),
        };

        // --split is additive, not exclusive: every event always goes to the aggregate
        // "events" dataset, and also to a per-source-file dataset when --split is set.
        let split_stem = self.split.then(|| self.allocate_stem(path));
        let mut count = 0u64;
        match triage_evtx::visit_evtx_file(path, &self.opts, &self.maps, &mut |rec| {
            out.write("events", &rec)?;
            if let Some(stem) = &split_stem {
                out.write_dynamic_record(stem, &rec)?;
            }
            if self.individual {
                self.write_individual(out, &rec)?;
            }
            count += 1;
            Ok::<(), TriageError>(())
        }) {
            Ok(()) => {}
            Err(triage_evtx::VisitError::Parse(error)) => return Err(to_err(error)),
            Err(triage_evtx::VisitError::Visitor(error)) => return Err(error),
        }
        Ok(count)
    }
}

impl EvtxTool {
    /// Route one record to its per-source-log (`Channel`) dynamic side-car.
    /// Pulled out of `parse`'s closure so it can be exercised directly by a
    /// unit test with a synthetic record, without needing a real `.evtx`
    /// binary. A method (not a free function) because case-colliding
    /// channels must fold onto the same on-disk stem across calls — see
    /// `resolve_individual_stem`.
    fn write_individual(
        &self,
        out: &mut OutputRouter,
        rec: &triage_evtx::EventRecord,
    ) -> Result<(), TriageError> {
        let stem = self.resolve_individual_stem(out, individual_stem(&rec.channel))?;
        // `write_dynamic_record` appends ".csv" itself, so what it needs is
        // the bare stem, not `individual_filename`'s full name (which already
        // carries the extension). `individual_filename` is literally
        // `format!("{}.csv", individual_stem(log_name))`, so the two agree on
        // everything `individual_stem` decides.
        //
        // They do **not** agree in general, and a caller predicting a
        // filename from `individual_filename` alone will sometimes be wrong.
        // `resolve_individual_stem` below folds channels whose stems differ
        // only by case onto one file, named with the lexicographically
        // smallest spelling among them (see its doc comment), so a record
        // whose `Channel` is `Foo/Bar` can land in the file named for
        // `FOO/BAR`. `individual_filename` is a pure function of the one
        // channel name it is handed and cannot know which spellings share a
        // key, so it predicts the on-disk name only for a channel with no
        // case-colliding sibling. The router's published path is the
        // authority; the row's own `Channel` value is preserved either way.
        let basename = format!("Individual/{stem}");
        out.write_dynamic_record(&basename, rec)
    }
}

#[cfg(test)]
mod individual_tests {
    use super::*;
    use triage_core::attribution::Identity;
    use triage_core::output::layout::OutputLayoutMode;
    use triage_core::output::router::RouterOptions;
    use triage_evtx::EventRecord;

    fn sample(channel: &str) -> EventRecord {
        EventRecord {
            record_number: 1,
            event_record_id: 1,
            time_created: "2026-01-01T00:00:00.0000000Z".into(),
            event_id: 4624,
            level: "LogAlways".into(),
            provider: "Microsoft-Windows-Security-Auditing".into(),
            channel: channel.into(),
            process_id: "796".into(),
            thread_id: "9456".into(),
            computer: "HOST01".into(),
            chunk_number: 0,
            user_id: String::new(),
            map_description: String::new(),
            user_name: String::new(),
            remote_host: String::new(),
            payload_data1: String::new(),
            payload_data2: String::new(),
            payload_data3: String::new(),
            payload_data4: String::new(),
            payload_data5: String::new(),
            payload_data6: String::new(),
            executable_info: String::new(),
            hidden_record: String::new(),
            source_file: "Security.evtx".into(),
            keywords: String::new(),
            extra_data_offset: 0,
            payload: String::new(),
        }
    }

    fn router(root: &std::path::Path) -> OutputRouter {
        let mut router = OutputRouter::new(
            "EvtxTriage",
            DATASETS,
            RouterOptions {
                csv_root: Some(root.to_path_buf()),
                json_root: None,
                csvf: None,
                jsonf: None,
                pretty: false,
                overwrite: false,
                run_stamp: None,
                layout_mode: OutputLayoutMode::Flat,
            },
        )
        .unwrap();
        router.set_identity(Identity::System);
        router
    }

    /// `write_individual` (the real code `parse` calls per record) writes a
    /// real CSV with a header and the record's data under `Individual/`, for
    /// every distinct channel seen — evidence-free, unlike `tests/individual.rs`,
    /// because it drives the router directly rather than a real `.evtx` file.
    #[test]
    fn writes_one_real_file_per_channel() {
        let tmp = tempfile::tempdir().unwrap();
        let mut router = router(tmp.path());
        let tool = EvtxTool::new(true);
        tool.write_individual(&mut router, &sample("Security"))
            .unwrap();
        tool.write_individual(&mut router, &sample("Security"))
            .unwrap();
        tool.write_individual(&mut router, &sample("System"))
            .unwrap();
        router.finish().into_outcome().unwrap();

        let security =
            std::fs::read_to_string(tmp.path().join("Individual/Security_system.csv")).unwrap();
        assert!(security.contains("EventRecordId"), "missing header");
        assert_eq!(
            security.lines().count(),
            3,
            "header + two Security rows: {security}"
        );

        let system =
            std::fs::read_to_string(tmp.path().join("Individual/System_system.csv")).unwrap();
        assert_eq!(
            system.lines().count(),
            2,
            "header + one System row: {system}"
        );
    }

    /// A hostile channel name collapses to a safe stem rather than escaping
    /// `Individual/` — the same guarantee `individual_filename` documents,
    /// exercised here through the real write path.
    #[test]
    fn a_hostile_channel_name_stays_inside_individual() {
        let tmp = tempfile::tempdir().unwrap();
        let mut router = router(tmp.path());
        let tool = EvtxTool::new(true);
        tool.write_individual(&mut router, &sample("../../etc/passwd"))
            .unwrap();
        router.finish().into_outcome().unwrap();

        assert!(tmp
            .path()
            .join("Individual/etc_passwd_system.csv")
            .is_file());
        assert!(!tmp.path().join("etc").exists());
    }

    /// Real captures contain channels that differ only in case (Microsoft's
    /// own AppX/Appx inconsistency) and sanitize to filenames that collide on
    /// a case-insensitive filesystem. Both channels' events must land in one
    /// file, none lost, distinguishable afterward by the `Channel` column.
    #[test]
    fn case_colliding_channels_fold_into_one_file_without_losing_events() {
        let tmp = tempfile::tempdir().unwrap();
        let mut router = router(tmp.path());
        let tool = EvtxTool::new(true);
        tool.write_individual(
            &mut router,
            &sample("Microsoft-Windows-AppXPackaging/Operational"),
        )
        .unwrap();
        tool.write_individual(
            &mut router,
            &sample("Microsoft-Windows-AppxPackaging/Operational"),
        )
        .unwrap();
        router.finish().into_outcome().unwrap();

        // Only one file on disk for the pair, under the lexicographically
        // smaller spelling ('X' < 'x'), which also happens to be the one
        // written first here — see the reverse-order test below for the
        // case where it isn't.
        let path = tmp
            .path()
            .join("Individual/Microsoft-Windows-AppXPackaging_Operational_system.csv");
        assert!(
            path.is_file(),
            "expected the folded file at {path:?}; entries: {:?}",
            std::fs::read_dir(tmp.path().join("Individual"))
                .unwrap()
                .map(|e| e.unwrap().file_name())
                .collect::<Vec<_>>()
        );
        // On a case-insensitive filesystem `exists()` would fold case too, so
        // the real assertion is that exactly one file was written, not that
        // the differently-cased path is absent.
        let entries: Vec<_> = std::fs::read_dir(tmp.path().join("Individual"))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(entries.len(), 1, "expected exactly one file: {entries:?}");

        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content.lines().count(), 3, "header + two rows: {content}");
        assert!(content.contains("Microsoft-Windows-AppXPackaging/Operational"));
        assert!(content.contains("Microsoft-Windows-AppxPackaging/Operational"));
    }

    /// The canonical filename for a case-colliding pair must not depend on
    /// which spelling `write_individual` sees first — only on the two
    /// spellings themselves. Feeds the pair in both orders and asserts both
    /// land on the same on-disk name (the lexicographically smaller
    /// spelling): a naive "first exact-case spelling wins" rule would pass
    /// the small-then-large order below by pure luck (the first write
    /// already happens to be the smaller spelling) but fail
    /// large-then-small, since it would keep whatever arrived first instead
    /// of migrating to the smaller one that arrives second.
    #[test]
    fn the_canonical_filename_does_not_depend_on_write_order() {
        let expected = "Microsoft-Windows-AppXPackaging_Operational_system.csv";

        for (first, second) in [
            (
                "Microsoft-Windows-AppxPackaging/Operational",
                "Microsoft-Windows-AppXPackaging/Operational",
            ),
            (
                "Microsoft-Windows-AppXPackaging/Operational",
                "Microsoft-Windows-AppxPackaging/Operational",
            ),
        ] {
            let tmp = tempfile::tempdir().unwrap();
            let mut router = router(tmp.path());
            let tool = EvtxTool::new(true);
            tool.write_individual(&mut router, &sample(first)).unwrap();
            tool.write_individual(&mut router, &sample(second)).unwrap();
            router.finish().into_outcome().unwrap();

            let entries: Vec<_> = std::fs::read_dir(tmp.path().join("Individual"))
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            assert_eq!(
                entries,
                vec![expected.to_string()],
                "order [{first}, {second}] produced {entries:?}, expected [{expected}]"
            );
        }
    }
}
