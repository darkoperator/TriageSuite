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
    let f = std::fs::File::create(path).unwrap();
    let mut w = zip::ZipWriter::new(f);
    let opts =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, body) in [
        ("uploads.json", "{}".to_string()),
        ("client_info.json", client_info(host)),
        (COLLECTION_MARKER_FILE, "marker".to_string()),
    ] {
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
