//! `--sync`: refresh the bundled maps corpus from the EricZimmerman/evtx repo
//! into the workspace `resources/evtx-maps/` directory. Refreshed maps take
//! effect on the next build (the corpus is embedded at compile time).

use indicatif::{ProgressBar, ProgressStyle};
use percent_encoding::{utf8_percent_encode, AsciiSet, CONTROLS};
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;

const RAW_BASE: &str = "https://raw.githubusercontent.com/EricZimmerman/evtx/master";
const TREE_API: &str =
    "https://api.github.com/repos/EricZimmerman/evtx/git/trees/master?recursive=1";

/// Characters to percent-encode within a URL path. Map filenames contain spaces
/// and other characters curl rejects in a raw URL; `/` is kept as the separator.
const PATH_ENCODE: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'`')
    .add(b'{')
    .add(b'}')
    .add(b'^')
    .add(b'|');

fn dest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../resources/evtx-maps")
}

fn curl(url: &str) -> std::io::Result<Vec<u8>> {
    let out = Command::new("curl")
        .args(["-fsSL", "-H", "User-Agent: TriageSuite", url])
        .output()?;
    if !out.status.success() {
        return Err(std::io::Error::other(format!(
            "curl failed for {url}: {}",
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    Ok(out.stdout)
}

/// Download every `evtx/Maps/*.map` path from the repo tree into the corpus dir.
pub fn sync_maps() -> std::io::Result<usize> {
    let dir = dest_dir();
    std::fs::create_dir_all(&dir)?;
    let tree = curl(TREE_API)?;
    let json: serde_json::Value = serde_json::from_slice(&tree)
        .map_err(|e| std::io::Error::other(format!("parse tree: {e}")))?;
    let paths: Vec<String> = json
        .get("tree")
        .and_then(|t| t.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|n| n.get("path").and_then(|p| p.as_str()))
                .filter(|p| {
                    let l = p.to_ascii_lowercase();
                    l.starts_with("evtx/maps/") && l.ends_with(".map")
                })
                .map(|p| p.to_string())
                .collect()
        })
        .unwrap_or_default();
    let bar = ProgressBar::new(paths.len() as u64);
    bar.set_style(
        ProgressStyle::with_template(
            "Syncing maps [{bar:30}] {pos}/{len} ({percent}%) {elapsed} {msg}",
        )
        .unwrap()
        .progress_chars("=>-"),
    );
    let mut n = 0;
    for path in &paths {
        let name = path.rsplit('/').next().unwrap_or(path);
        bar.set_message(name.to_string());
        // Percent-encode the path so filenames with spaces produce a valid URL.
        let encoded = utf8_percent_encode(path, PATH_ENCODE).to_string();
        let bytes = curl(&format!("{RAW_BASE}/{encoded}"))?;
        std::fs::File::create(dir.join(name))?.write_all(&bytes)?;
        n += 1;
        bar.inc(1);
    }
    bar.finish_with_message(format!("{n} maps downloaded"));
    Ok(n)
}
