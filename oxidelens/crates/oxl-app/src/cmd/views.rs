//! Saved views and filter presets: listing and applying them are reads;
//! saving, deleting and importing are writes, after which the window hears
//! `case-changed` with `views`.
//!
//! A view file is read and written here, the engine handing over and taking
//! its JSON text. Exporting writes `<path>.partial` beside the target and
//! renames it over the target once complete, and refuses a path inside a
//! loaded source or the case file by the engine's own check, as a CSV
//! export does. Importing reads at most the engine's file cap, from a
//! regular file only.

use super::At;
use crate::error::CmdError;
use crate::state::{AppState, Point};
use oxl_engine::{
    AppliedView, EngineError, Listing, SavedView, ViewDoc, ViewKind, MAX_VIEW_FILE_BYTES,
};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// What a saved-view write changes.
const VIEWS: &[&str] = &["views"];

/// Up to `limit` views and presets from `offset`, only those offered for
/// `relation` when one is named (`views`).
pub fn views_list(
    state: &AppState,
    at: At<'_>,
    relation: Option<&str>,
    limit: u32,
    offset: u64,
) -> Result<Listing<SavedView>, CmdError> {
    at.read(state, |c| oxl_engine::views(c, relation, limit, offset))
}

/// Save `doc` as a view or preset named `name` for the relations `pattern`
/// matches, replacing one of that name and kind (`save_view`).
pub fn view_save(
    state: &AppState,
    name: &str,
    kind: ViewKind,
    pattern: &str,
    doc: &ViewDoc,
) -> Result<SavedView, CmdError> {
    let actor = state.actor();
    state.write(VIEWS, |c| {
        oxl_engine::save_view(c, name, kind, pattern, doc, &actor)
    })
}

/// Delete view `id`, leaving a tombstone (`delete_view`).
pub fn view_delete(state: &AppState, id: i64) -> Result<(), CmdError> {
    let actor = state.actor();
    state.write(VIEWS, |c| oxl_engine::delete_view(c, id, &actor))
}

/// View `id` as it applies to `relation`, with a warning for each part
/// that names something the dataset lacks (`apply_view`).
pub fn view_apply(
    state: &AppState,
    at: At<'_>,
    id: i64,
    relation: &str,
) -> Result<AppliedView, CmdError> {
    at.read(state, |c| oxl_engine::apply_view(c, id, relation))
}

/// Write view `id` to the file `path` (`export_view`). The path is checked
/// as an export's is (`check_export_target`) on the case the read sees, and
/// again, with the write gate held, just before the file is renamed into
/// place (`AppState::serialized`), so a load that made the path part of a
/// source in between is seen: `refused`, and the partial file is removed.
/// The file is written whole or not at all.
pub fn view_export(state: &AppState, at: At<'_>, id: i64, path: &Path) -> Result<(), CmdError> {
    let json = at.read(state, |c| {
        oxl_engine::check_export_target(c, path)?;
        oxl_engine::export_view(c, id)
    })?;
    let partial = write_partial(path, json.as_bytes())?;
    state.hook(Point::BeforeRename);
    let renamed = state.serialized(|c| {
        oxl_engine::check_export_target(c, path)?;
        Ok(std::fs::rename(&partial, path)?)
    });
    if renamed.is_err() {
        // The partial file is this export's own; the error that ended the
        // export is the one worth reporting.
        let _ = std::fs::remove_file(&partial);
    }
    renamed
}

/// Save the view in the file `path`, replacing one of the same name and
/// kind (`import_view`). `invalid` for a file over `MAX_VIEW_FILE_BYTES`,
/// one that is not UTF-8 text, or anything that is not a regular file;
/// `not_found` for no file there.
pub fn view_import(state: &AppState, path: &Path) -> Result<SavedView, CmdError> {
    let json = read_capped(path)?;
    let actor = state.actor();
    state.write(VIEWS, |c| oxl_engine::import_view(c, &json, &actor))
}

/// Write `bytes` to `<path>.partial` (made new: anything already at that
/// name is refused), synced and closed, ready to be renamed over `path`;
/// its path. On any failure the partial file is removed.
fn write_partial(path: &Path, bytes: &[u8]) -> Result<PathBuf, CmdError> {
    let (partial, mut file) = oxl_engine::create_export_partial(path)?;
    // Closed (dropped) before the rename, which Windows needs.
    let written = file.write_all(bytes).and_then(|()| file.sync_all());
    drop(file);
    match written {
        Ok(()) => Ok(partial),
        Err(e) => {
            let _ = std::fs::remove_file(&partial);
            Err(EngineError::Io(e).into())
        }
    }
}

/// The text of the regular file at `path`, read up to `MAX_VIEW_FILE_BYTES`
/// and refused past it.
fn read_capped(path: &Path) -> Result<String, CmdError> {
    let meta = std::fs::metadata(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            EngineError::NotFound(format!("no file at {}", path.display()))
        } else {
            EngineError::Io(e)
        }
    })?;
    // Never a folder, and never a FIFO or device, which could block or
    // never end.
    if !meta.is_file() {
        return Err(CmdError::invalid(format!(
            "{} is not a file; a view is imported from a view file",
            path.display()
        )));
    }
    let file = std::fs::File::open(path).map_err(EngineError::Io)?;
    let mut bytes = Vec::new();
    file.take(MAX_VIEW_FILE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(EngineError::Io)?;
    if bytes.len() > MAX_VIEW_FILE_BYTES {
        return Err(CmdError::invalid(format!(
            "a view file is at most {MAX_VIEW_FILE_BYTES} bytes; {} is larger",
            path.display()
        )));
    }
    String::from_utf8(bytes).map_err(|_| {
        CmdError::invalid(format!(
            "{} is not UTF-8 text, so it is not a view file",
            path.display()
        ))
    })
}

pub mod commands {
    use crate::cmd::At;
    use crate::error::CmdError;
    use crate::state::AppState;
    use oxl_engine::{AppliedView, Listing, SavedView, ViewDoc, ViewKind};
    use std::path::PathBuf;
    use tauri::State;

    #[tauri::command(async)]
    pub fn views_list(
        app: State<'_, AppState>,
        query_id: String,
        slot: String,
        relation: Option<String>,
        limit: u32,
        offset: u64,
    ) -> Result<Listing<SavedView>, CmdError> {
        let at = At::new(&query_id, &slot);
        super::views_list(&app, at, relation.as_deref(), limit, offset)
    }

    #[tauri::command(async)]
    pub fn view_save(
        app: State<'_, AppState>,
        name: String,
        kind: ViewKind,
        pattern: String,
        doc: ViewDoc,
    ) -> Result<SavedView, CmdError> {
        super::view_save(&app, &name, kind, &pattern, &doc)
    }

    #[tauri::command(async)]
    pub fn view_delete(app: State<'_, AppState>, id: i64) -> Result<(), CmdError> {
        super::view_delete(&app, id)
    }

    #[tauri::command(async)]
    pub fn view_apply(
        app: State<'_, AppState>,
        query_id: String,
        slot: String,
        id: i64,
        relation: String,
    ) -> Result<AppliedView, CmdError> {
        super::view_apply(&app, At::new(&query_id, &slot), id, &relation)
    }

    #[tauri::command(async)]
    pub fn view_export(
        app: State<'_, AppState>,
        query_id: String,
        slot: String,
        id: i64,
        path: PathBuf,
    ) -> Result<(), CmdError> {
        super::view_export(&app, At::new(&query_id, &slot), id, &path)
    }

    #[tauri::command(async)]
    pub fn view_import(app: State<'_, AppState>, path: PathBuf) -> Result<SavedView, CmdError> {
        super::view_import(&app, &path)
    }
}
