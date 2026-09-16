//! Synthetic inputs for orchestrator tests: a Velociraptor collection on
//! disk or inside a zip, and stub executables that stand in for external
//! tools. Nothing here is a parsable artifact; these exercise discovery,
//! extraction and orchestration, not parsing.

use std::io::Write;
use std::path::{Path, PathBuf};

const CLIENT_INFO_PLATFORM: &str = "Microsoft Windows 11 Enterprise";
const CLIENT_INFO_VERSION: &str = "23H2";

/// The OS string `capture::host_from_collection` derives from
/// [`write_collection`]'s `client_info.json`.
pub const COLLECTION_OS: &str = "Microsoft Windows 11 Enterprise 23H2";

/// Relative path of the one file placed under `uploads/`. URL-encoded, as
/// Velociraptor writes segments, and with an extension no parser claims so
/// that a run over the collection reports zero matches rather than a
/// corrupt artifact.
pub const COLLECTION_MARKER_FILE: &str = "uploads/auto/C%3A/Windows/Prefetch/MARKER.txt";

/// Relative paths of the artifacts the orchestrator's pre-flight gate
/// (`validate_capture`) requires: the four registry hives and at least one
/// event log, under Velociraptor's URL-encoded upload tree.
///
/// The bytes written at these paths are placeholders, and that is sound
/// precisely because the gate never opens a file: it reads names out of a
/// directory listing or out of a zip's central directory, so names are all
/// it can see. A parser handed one of these would fail, so a test using
/// [`write_gate_passing_collection`] must keep the parsers away from them
/// with `--only` (`pe`/`le` discover `*.pf`/`*.lnk` and nothing else) -- the
/// same rule that governs the rest of this module.
pub const GATE_ARTIFACT_FILES: [&str; 5] = [
    "uploads/auto/C%3A/Windows/System32/config/SYSTEM",
    "uploads/auto/C%3A/Windows/System32/config/SOFTWARE",
    "uploads/auto/C%3A/Windows/System32/config/SAM",
    "uploads/auto/C%3A/Windows/System32/config/SECURITY",
    "uploads/auto/C%3A/Windows/System32/winevt/Logs/Security.evtx",
];

const GATE_ARTIFACT_BODY: &str = "placeholder: see GATE_ARTIFACT_FILES";

fn client_info(host: &str) -> String {
    format!(
        r#"{{"Hostname":"{host}","Platform":"{CLIENT_INFO_PLATFORM}","PlatformVersion":"{CLIENT_INFO_VERSION}"}}"#
    )
}

/// A minimal Velociraptor collection directory: the two marker files and an
/// empty `uploads/auto/.../Prefetch` tree.
pub fn write_collection(dir: &Path, host: &str) {
    let marker = dir.join(COLLECTION_MARKER_FILE);
    std::fs::create_dir_all(marker.parent().unwrap()).unwrap();
    std::fs::write(dir.join("uploads.json"), "{}").unwrap();
    std::fs::write(dir.join("client_info.json"), client_info(host)).unwrap();
}

/// The same collection zipped, with its contents under `prefix` (`""` for
/// the archive root, which is how the offline collector writes them, or
/// `"Wrapper/"` for a re-zipped capture). Entries are stored uncompressed
/// so the test does not depend on the deflate backend.
pub fn write_collection_zip(path: &Path, prefix: &str, host: &str) {
    write_zip(path, prefix, &collection_entries(host, false));
}

/// [`write_collection`] plus [`GATE_ARTIFACT_FILES`], so the collection
/// passes the orchestrator's pre-flight gate and a test can exercise the
/// `run` path with that gate active. Read that constant's note before use.
pub fn write_gate_passing_collection(dir: &Path, host: &str) {
    write_collection(dir, host);
    for name in GATE_ARTIFACT_FILES {
        let path = dir.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, GATE_ARTIFACT_BODY).unwrap();
    }
}

/// The zipped form of [`write_gate_passing_collection`].
pub fn write_gate_passing_collection_zip(path: &Path, prefix: &str, host: &str) {
    write_zip(path, prefix, &collection_entries(host, true));
}

/// The `(entry name, body)` pairs one zipped collection is made of.
fn collection_entries(host: &str, gate_artifacts: bool) -> Vec<(String, String)> {
    let mut entries = vec![
        ("uploads.json".to_string(), "{}".to_string()),
        ("client_info.json".to_string(), client_info(host)),
        (COLLECTION_MARKER_FILE.to_string(), "marker".to_string()),
    ];
    if gate_artifacts {
        entries.extend(
            GATE_ARTIFACT_FILES
                .iter()
                .map(|name| (name.to_string(), GATE_ARTIFACT_BODY.to_string())),
        );
    }
    entries
}

fn write_zip(path: &Path, prefix: &str, entries: &[(String, String)]) {
    let f = std::fs::File::create(path).unwrap();
    let mut w = zip::ZipWriter::new(f);
    let opts =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, body) in entries {
        w.start_file(format!("{prefix}{name}"), opts).unwrap();
        w.write_all(body.as_bytes()).unwrap();
    }
    w.finish().unwrap();
}

/// Write `body` to `path` and mark it executable.
#[cfg(unix)]
pub fn write_executable(path: &Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, body).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// An executable stub that scans its argv for `output_flag` and, at the path
/// that follows it, either writes a placeholder file (`as_dir == false`) or
/// creates a directory holding a `report.txt` (`as_dir == true`). Enough to
/// exercise real orchestration and chaining without the actual binaries.
#[cfg(unix)]
pub fn write_stub(dir: &Path, name: &str, output_flag: &str, as_dir: bool) -> PathBuf {
    let produce = if as_dir {
        "mkdir -p \"$a\"\n    echo stub > \"$a/report.txt\""
    } else {
        "echo stub > \"$a\""
    };
    let body = format!(
        "#!/bin/sh\nprev=\"\"\nfor a in \"$@\"; do\n  if [ \"$prev\" = \"{output_flag}\" ]; then\n    {produce}\n  fi\n  prev=\"$a\"\ndone\nexit 0\n"
    );
    let path = dir.join(name);
    write_executable(&path, &body);
    path
}
