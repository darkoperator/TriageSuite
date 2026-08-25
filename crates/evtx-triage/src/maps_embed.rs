//! The bundled EricZimmerman/evtx maps corpus, embedded at compile time from
//! the workspace-root `resources/evtx-maps/` directory (refreshed by `--sync`).

use include_dir::{include_dir, Dir};
use triage_evtx::MapIndex;

static MAPS_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../../resources/evtx-maps");

/// Build a `MapIndex` from every bundled `.map` file.
pub fn load_bundled() -> MapIndex {
    let pairs: Vec<(&str, &str)> = MAPS_DIR
        .files()
        .filter_map(|f| {
            let name = f.path().file_name()?.to_str()?;
            let text = f.contents_utf8()?;
            Some((name, text))
        })
        .collect();
    MapIndex::from_contents(pairs)
}

/// Number of embedded `.map` files (for diagnostics/tests).
pub fn embedded_count() -> usize {
    MAPS_DIR.files().count()
}
