//! The settings file: what the app remembers between runs that is not part
//! of a case (theme, font size, panes, histogram, per-dataset column layouts
//! and colour rules, recent cases, the analyst's name). One JSON file,
//! `settings.json` in the app's config directory.
//!
//! A save never leaves a half-written file: it writes
//! `settings.json.<pid>.<n>.partial` beside it and renames that over it. A
//! file that cannot be read is moved aside to
//! `settings.json.damaged-<yyyyMMddHHmmss>` and the defaults are used, so a
//! damaged file costs the analyst their preferences, never the app.

use crate::error::CmdError;
use oxl_engine::{Actor, ColourRule, ColumnLayout, MAX_COLUMN_WIDTH, MAX_RULES, MIN_COLUMN_WIDTH};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};

/// The settings file's format.
pub const SETTINGS_VERSION: u32 = 1;
/// The file's name in the config directory.
pub const SETTINGS_FILE: &str = "settings.json";
/// The font sizes the grid offers, in pixels.
pub const FONT_SIZES: std::ops::RangeInclusive<u8> = 10..=16;
/// Recent cases remembered, newest first.
pub const MAX_RECENT: usize = 10;
/// The largest settings document read or written. Layouts and colour rules
/// for hundreds of datasets fit in a fraction of it.
pub const MAX_SETTINGS_BYTES: usize = 1024 * 1024;
/// Characters in a recent case's path.
const MAX_PATH_CHARS: usize = 4096;
/// Characters in a dataset name keying a layout or a rule list.
const MAX_KEY_CHARS: usize = 256;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    #[default]
    Dark,
    Light,
}

/// A side pane, open or folded to its rail.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
pub enum PaneState {
    #[default]
    Open,
    Rail,
}

/// Everything the settings file holds. Every field may be missing from the
/// file and is then its default; fields the file has and this build does not
/// know are ignored, so a newer build's file still opens.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[serde(default)]
pub struct Settings {
    pub version: u32,
    pub theme: Theme,
    /// The grid's cell font, `FONT_SIZES` pixels.
    pub font_size: u8,
    pub left_pane: PaneState,
    pub right_pane: PaneState,
    /// Whether the histogram strip shows above the grid.
    pub histogram: bool,
    /// The name the app's changes are recorded under; the operating
    /// system's user when unset. 1 to 64 characters of `[A-Za-z0-9._-]`.
    pub analyst: Option<String>,
    /// Case file paths, newest first, at most `MAX_RECENT`. The backend's
    /// alone: opening a case adds one, `recent_remove` forgets one,
    /// `recent_clear` empties the list, and `settings_update` keeps the
    /// stored list whatever it is sent.
    pub recent: Vec<String>,
    /// Column layouts by dataset (relation) name.
    pub layouts: BTreeMap<String, ColumnLayout>,
    /// Colour rules by dataset (relation) name, in order.
    pub colour_rules: BTreeMap<String, Vec<ColourRule>>,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            version: SETTINGS_VERSION,
            theme: Theme::Dark,
            font_size: 12,
            left_pane: PaneState::Open,
            right_pane: PaneState::Open,
            histogram: true,
            analyst: None,
            recent: Vec::new(),
            layouts: BTreeMap::new(),
            colour_rules: BTreeMap::new(),
        }
    }
}

/// A settings file that could not be read when the app started.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ts_rs::TS)]
pub struct DamagedSettings {
    /// The name it was moved aside to in the config directory; `None` when
    /// it could not be moved (the next save replaces it).
    pub file: Option<String>,
    /// Why it could not be read.
    pub reason: String,
}

/// What `settings_get`, `settings_update`, `recent_clear` and
/// `recent_remove` answer: the settings, and the damaged file the defaults
/// replaced, until a settings update succeeds.
#[derive(Debug, Clone, PartialEq, Serialize, ts_rs::TS)]
pub struct SettingsReply {
    pub settings: Settings,
    pub damaged: Option<DamagedSettings>,
    /// Who the app's changes are recorded under now: the `analyst:<name>`
    /// label of the saved analyst name, else of the operating system's user.
    pub actor: String,
}

/// Whether `name` is an analyst name an `Actor` can carry.
fn valid_analyst(name: &str) -> bool {
    Actor::parse(&format!("analyst:{name}")).is_ok()
}

impl Settings {
    /// Refuse settings the front end sent that this build would not write:
    /// each value in its range, each list within its cap.
    pub fn check(&self) -> Result<(), CmdError> {
        let invalid = |m: String| Err(CmdError::invalid(m));
        if self.version != SETTINGS_VERSION {
            return invalid(format!(
                "settings version {} is not {SETTINGS_VERSION}",
                self.version
            ));
        }
        if !FONT_SIZES.contains(&self.font_size) {
            return invalid(format!(
                "font size {} is outside {}..={}",
                self.font_size,
                FONT_SIZES.start(),
                FONT_SIZES.end()
            ));
        }
        if let Some(name) = &self.analyst {
            if !valid_analyst(name) {
                return invalid(
                    "an analyst name is 1 to 64 characters of A-Z a-z 0-9 . _ -".to_string(),
                );
            }
        }
        if self.recent.len() > MAX_RECENT {
            return invalid(format!("at most {MAX_RECENT} recent cases"));
        }
        if self
            .recent
            .iter()
            .any(|p| p.is_empty() || p.chars().count() > MAX_PATH_CHARS)
        {
            return invalid(format!(
                "a recent case path is 1 to {MAX_PATH_CHARS} characters"
            ));
        }
        let keys = self.layouts.keys().chain(self.colour_rules.keys());
        if keys.into_iter().any(|k| k.chars().count() > MAX_KEY_CHARS) {
            return invalid(format!(
                "a dataset name in the settings is at most {MAX_KEY_CHARS} characters"
            ));
        }
        for layout in self.layouts.values() {
            if let Some(w) = layout
                .widths
                .values()
                .find(|w| !(MIN_COLUMN_WIDTH..=MAX_COLUMN_WIDTH).contains(*w))
            {
                return invalid(format!(
                    "a column width of {w} is outside {MIN_COLUMN_WIDTH}..={MAX_COLUMN_WIDTH}"
                ));
            }
        }
        if self.colour_rules.values().any(|r| r.len() > MAX_RULES) {
            return invalid(format!("at most {MAX_RULES} colour rules per dataset"));
        }
        Ok(())
    }

    /// The bytes the settings file holds for these settings, once `check`
    /// accepts them and they are within `MAX_SETTINGS_BYTES`. The cap is
    /// checked on exactly what is written, so a file a save wrote is never
    /// one the next start refuses as too large.
    fn file_bytes(&self) -> Result<Vec<u8>, CmdError> {
        self.check()?;
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|e| CmdError::new("internal", format!("settings to JSON: {e}")))?;
        if bytes.len() > MAX_SETTINGS_BYTES {
            return Err(CmdError::invalid(format!(
                "the settings are {} bytes; the most is {MAX_SETTINGS_BYTES}",
                bytes.len()
            )));
        }
        Ok(bytes)
    }

    /// Settings read from a file brought within what `check` accepts: each
    /// value out of range replaced by the nearest it allows (or its default),
    /// so a file edited by hand opens rather than counting as damaged.
    fn normalised(mut self) -> Settings {
        self.version = SETTINGS_VERSION;
        self.font_size = self.font_size.clamp(*FONT_SIZES.start(), *FONT_SIZES.end());
        if !self.analyst.as_deref().is_some_and(valid_analyst) {
            self.analyst = None;
        }
        let mut recent = Vec::new();
        for path in std::mem::take(&mut self.recent) {
            let fits = !path.is_empty() && path.chars().count() <= MAX_PATH_CHARS;
            if fits && !recent.contains(&path) && recent.len() < MAX_RECENT {
                recent.push(path);
            }
        }
        self.recent = recent;
        self.layouts
            .retain(|k, _| k.chars().count() <= MAX_KEY_CHARS);
        self.colour_rules
            .retain(|k, _| k.chars().count() <= MAX_KEY_CHARS);
        for layout in self.layouts.values_mut() {
            for w in layout.widths.values_mut() {
                *w = (*w).clamp(MIN_COLUMN_WIDTH, MAX_COLUMN_WIDTH);
            }
        }
        for rules in self.colour_rules.values_mut() {
            rules.truncate(MAX_RULES);
        }
        self
    }

    /// Put `path` first in `recent`, once, keeping at most `MAX_RECENT`.
    pub fn add_recent(&mut self, path: &str) {
        self.recent.retain(|p| p != path);
        self.recent.insert(0, path.to_string());
        self.recent.truncate(MAX_RECENT);
    }
}

/// The actor the app's changes are recorded under: the analyst named in the
/// settings, else the operating system's user (`os_user`, sanitised as the
/// CLI sanitises it).
pub fn app_actor(settings: &Settings, os_user: Option<&str>) -> Actor {
    match settings.analyst.as_deref() {
        Some(name) if valid_analyst(name) => Actor::Analyst(name.to_string()),
        _ => Actor::from_os_user(os_user.unwrap_or("")),
    }
}

/// The operating system's user name, as the CLI reads it.
pub fn os_user() -> Option<String> {
    std::env::var(if cfg!(windows) { "USERNAME" } else { "USER" }).ok()
}

/// The settings in memory and the file they are saved to.
pub struct SettingsStore {
    /// The config directory; `None` keeps the settings in memory only.
    dir: Option<PathBuf>,
    /// Held across a save, so saves reach the file in the order they were
    /// made.
    current: Mutex<Settings>,
    /// Numbers each save's partial file.
    seq: AtomicU64,
    /// The damaged file found at load, until a `save` succeeds.
    damaged: Mutex<Option<DamagedSettings>>,
}

impl SettingsStore {
    /// Settings that live in memory only, for an app state nothing saves.
    pub fn in_memory() -> SettingsStore {
        SettingsStore {
            dir: None,
            current: Mutex::new(Settings::default()),
            seq: AtomicU64::new(0),
            damaged: Mutex::new(None),
        }
    }

    /// Read `<dir>/settings.json`. A missing file is the defaults. A file
    /// that cannot be read or parsed is moved aside and the defaults are
    /// used; the second value says where it went and why, and the store
    /// keeps saying so (`damaged`) until a `save` succeeds.
    pub fn load(dir: &Path) -> (SettingsStore, Option<DamagedSettings>) {
        let file = dir.join(SETTINGS_FILE);
        let (settings, damaged) = match read_settings(&file) {
            Ok(Some(settings)) => (settings.normalised(), None),
            Ok(None) => (Settings::default(), None),
            Err(reason) => {
                let moved = set_aside(&file);
                let damaged = DamagedSettings {
                    file: moved
                        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned())),
                    reason,
                };
                (Settings::default(), Some(damaged))
            }
        };
        let store = SettingsStore {
            dir: Some(dir.to_path_buf()),
            current: Mutex::new(settings),
            seq: AtomicU64::new(0),
            damaged: Mutex::new(damaged.clone()),
        };
        (store, damaged)
    }

    /// The damaged settings file the defaults replaced at load, until the
    /// analyst's settings are next saved.
    pub fn damaged(&self) -> Option<DamagedSettings> {
        self.damaged
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    fn lock(&self) -> MutexGuard<'_, Settings> {
        self.current.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// The settings now.
    pub fn get(&self) -> Settings {
        self.lock().clone()
    }

    /// Check `settings`, write them, and make them current. On a failed
    /// write the current settings and the file are both unchanged.
    pub fn save(&self, settings: &Settings) -> Result<Settings, CmdError> {
        let bytes = settings.file_bytes()?;
        let mut current = self.lock();
        self.write(&bytes)?;
        *current = settings.clone();
        // The analyst has now chosen settings over the defaults that
        // replaced the damaged file.
        *self.damaged.lock().unwrap_or_else(|p| p.into_inner()) = None;
        Ok(settings.clone())
    }

    /// `save`, but with the stored `recent` list in place of the one in
    /// `settings`: the recent cases are the backend's (a case opened adds
    /// one, `recent_clear` empties them), so a whole-settings save from the
    /// window that was in flight when a case opened cannot put back an older
    /// list. The stored list is read under the same lock the save holds.
    pub fn save_keeping_recent(&self, settings: &Settings) -> Result<Settings, CmdError> {
        let mut current = self.lock();
        let next = Settings {
            recent: current.recent.clone(),
            ..settings.clone()
        };
        let bytes = next.file_bytes()?;
        self.write(&bytes)?;
        *current = next.clone();
        *self.damaged.lock().unwrap_or_else(|p| p.into_inner()) = None;
        Ok(next)
    }

    /// Change the current settings with `change` and save the result.
    pub fn update(&self, change: impl FnOnce(&mut Settings)) -> Result<Settings, CmdError> {
        let mut current = self.lock();
        let mut next = current.clone();
        change(&mut next);
        let bytes = next.file_bytes()?;
        self.write(&bytes)?;
        *current = next.clone();
        Ok(next)
    }

    /// Write `text` (from `Settings::file_bytes`) as the settings file.
    fn write(&self, text: &[u8]) -> Result<(), CmdError> {
        let Some(dir) = &self.dir else {
            return Ok(());
        };
        let io = |e: std::io::Error| CmdError::new("io", format!("saving the settings: {e}"));
        std::fs::create_dir_all(dir).map_err(io)?;
        let n = self.seq.fetch_add(1, Ordering::SeqCst);
        let partial = dir.join(format!(
            "{SETTINGS_FILE}.{}.{n}.partial",
            std::process::id()
        ));
        let written = (|| {
            // create_new: a file (or link) already at the partial's name is
            // never followed or truncated.
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&partial)?;
            f.write_all(text)?;
            f.sync_all()?;
            drop(f);
            std::fs::rename(&partial, dir.join(SETTINGS_FILE))
        })();
        if written.is_err() {
            let _ = std::fs::remove_file(&partial);
        }
        written.map_err(io)
    }
}

/// The settings in `file`: `None` when there is no file, `Err` with why
/// when it cannot be read or is not settings. The reason never quotes the
/// file's text.
fn read_settings(file: &Path) -> Result<Option<Settings>, String> {
    let meta = match std::fs::metadata(file) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("it could not be read: {e}")),
    };
    if !meta.is_file() {
        return Err("it is not a file".to_string());
    }
    if meta.len() > MAX_SETTINGS_BYTES as u64 {
        return Err(format!(
            "it is {} bytes; the most is {MAX_SETTINGS_BYTES}",
            meta.len()
        ));
    }
    let bytes = std::fs::read(file).map_err(|e| format!("it could not be read: {e}"))?;
    serde_json::from_slice(&bytes).map(Some).map_err(|e| {
        let what = match e.classify() {
            serde_json::error::Category::Io => "a read error",
            serde_json::error::Category::Syntax => "not JSON",
            serde_json::error::Category::Data => "JSON that is not settings",
            serde_json::error::Category::Eof => "JSON cut short",
        };
        format!("{what} at line {}, column {}", e.line(), e.column())
    })
}

/// Move a damaged settings file aside, to the first free
/// `settings.json.damaged-<stamp>[-<n>]`; where it went, or `None` when it
/// could not be moved (the defaults are used either way, and the next save
/// replaces it).
fn set_aside(file: &Path) -> Option<PathBuf> {
    let stamp = chrono::Utc::now().format("%Y%m%d%H%M%S").to_string();
    let base = format!("{SETTINGS_FILE}.damaged-{stamp}");
    let dir = file.parent()?;
    for n in 1..100 {
        let name = if n == 1 {
            base.clone()
        } else {
            format!("{base}-{n}")
        };
        let target = dir.join(name);
        if target.symlink_metadata().is_ok() {
            continue;
        }
        return std::fs::rename(file, &target).ok().map(|()| target);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn settings_round_trip_and_damaged_file() {
        let d = tempfile::tempdir().unwrap();

        // No file: the defaults, and nothing is damaged.
        let (store, damaged) = SettingsStore::load(d.path());
        assert_eq!(damaged, None);
        assert_eq!(store.get(), Settings::default());
        let defaults = store.get();
        assert_eq!(defaults.version, 1);
        assert_eq!(defaults.theme, Theme::Dark);
        assert_eq!(defaults.font_size, 12);
        assert!(defaults.histogram);

        // A save round-trips through the file and leaves no partial behind.
        let mut s = Settings {
            theme: Theme::Light,
            font_size: 14,
            right_pane: PaneState::Rail,
            histogram: false,
            analyst: Some("jdoe".into()),
            ..Settings::default()
        };
        s.add_recent("C:\\cases\\one.duckdb");
        s.add_recent("C:\\cases\\two.duckdb");
        s.layouts.insert(
            "evtxtriage_events".into(),
            ColumnLayout {
                order: vec!["EventID".into()],
                widths: [("EventID".to_string(), 120)].into(),
                hidden: vec![],
                pinned: vec!["EventID".into()],
            },
        );
        store.save(&s).unwrap();
        assert_eq!(files(d.path()), vec![SETTINGS_FILE.to_string()]);
        let (again, damaged) = SettingsStore::load(d.path());
        assert_eq!(damaged, None);
        assert_eq!(again.get(), s);
        assert_eq!(again.get().recent[0], "C:\\cases\\two.duckdb");

        // The file is JSON with snake_case names and lowercase enums.
        let text = std::fs::read_to_string(d.path().join(SETTINGS_FILE)).unwrap();
        assert!(text.contains("\"theme\": \"light\""), "{text}");
        assert!(text.contains("\"right_pane\": \"rail\""), "{text}");

        // A missing field is its default.
        std::fs::write(d.path().join(SETTINGS_FILE), r#"{"font_size": 15}"#).unwrap();
        let (partial, damaged) = SettingsStore::load(d.path());
        assert_eq!(damaged, None);
        assert_eq!(partial.get().font_size, 15);
        assert_eq!(partial.get().theme, Theme::Dark);

        // Out of range values are brought into range, not refused.
        std::fs::write(
            d.path().join(SETTINGS_FILE),
            r#"{"font_size": 40, "analyst": "no spaces allowed", "recent": ["a","a","b"]}"#,
        )
        .unwrap();
        let (fixed, damaged) = SettingsStore::load(d.path());
        assert_eq!(damaged, None);
        assert_eq!(fixed.get().font_size, 16);
        assert_eq!(fixed.get().analyst, None);
        assert_eq!(fixed.get().recent, vec!["a".to_string(), "b".to_string()]);

        // A damaged file is moved aside and the defaults are used.
        std::fs::write(d.path().join(SETTINGS_FILE), b"{not json").unwrap();
        let (store, damaged) = SettingsStore::load(d.path());
        let damaged = damaged.expect("the damaged file was reported");
        assert_eq!(damaged.reason, "not JSON at line 1, column 2");
        assert_eq!(store.damaged(), Some(damaged.clone()));
        let name = damaged.file.expect("the damaged file was moved");
        let moved = d.path().join(&name);
        assert!(name.starts_with("settings.json.damaged-"), "{name}");
        let stamp = &name["settings.json.damaged-".len()..];
        assert_eq!(stamp.len(), 14, "{name}");
        assert!(stamp.bytes().all(|b| b.is_ascii_digit()), "{name}");
        assert_eq!(std::fs::read(&moved).unwrap(), b"{not json");
        assert!(!d.path().join(SETTINGS_FILE).exists());
        assert_eq!(store.get(), Settings::default());

        // A second damaged file in the same second does not replace the first.
        std::fs::write(d.path().join(SETTINGS_FILE), b"42").unwrap();
        let (store, second) = SettingsStore::load(d.path());
        let second = second.expect("reported");
        assert_eq!(
            second.reason,
            "JSON that is not settings at line 1, column 2"
        );
        assert_ne!(second.file, Some(name));
        // A successful save ends the report.
        store.save(&Settings::default()).unwrap();
        assert_eq!(store.damaged(), None);
        assert_eq!(std::fs::read(&moved).unwrap(), b"{not json");
    }

    #[test]
    fn save_refuses_out_of_range_values() {
        let store = SettingsStore::in_memory();
        let mut s = Settings {
            font_size: 9,
            ..Settings::default()
        };
        assert_eq!(store.save(&s).unwrap_err().code, "invalid");
        s.font_size = 16;
        s.analyst = Some(String::new());
        assert_eq!(store.save(&s).unwrap_err().code, "invalid");
        s.analyst = None;
        s.recent = (0..11).map(|n| format!("c{n}.duckdb")).collect();
        assert_eq!(store.save(&s).unwrap_err().code, "invalid");
        s.recent.clear();
        s.version = 2;
        assert_eq!(store.save(&s).unwrap_err().code, "invalid");
        // Nothing refused changed what is current.
        assert_eq!(store.get(), Settings::default());
    }

    #[test]
    fn size_cap_is_checked_on_the_bytes_written() {
        // Compact JSON under the cap, the pretty JSON the file holds over it:
        // a save that succeeded here would be called damaged at next start.
        let mut s = Settings::default();
        s.layouts.insert(
            "evtxtriage_events".into(),
            ColumnLayout {
                order: vec![String::new(); 300_000],
                widths: BTreeMap::new(),
                hidden: vec![],
                pinned: vec![],
            },
        );
        let compact = serde_json::to_vec(&s).unwrap().len();
        let pretty = serde_json::to_vec_pretty(&s).unwrap().len();
        assert!(compact < MAX_SETTINGS_BYTES, "{compact}");
        assert!(pretty > MAX_SETTINGS_BYTES, "{pretty}");

        let d = tempfile::tempdir().unwrap();
        let (store, _) = SettingsStore::load(d.path());
        // (No `unwrap_err`: its panic would print 300,000 names.)
        assert!(matches!(store.save(&s), Err(e) if e.code == "invalid"));
        assert!(matches!(store.update(|c| *c = s.clone()), Err(e) if e.code == "invalid"));
        assert!(files(d.path()).is_empty(), "{:?}", files(d.path()));
        assert!(store.get() == Settings::default());
    }

    #[test]
    fn recent_is_newest_first_and_capped() {
        let mut s = Settings::default();
        for n in 0..12 {
            s.add_recent(&format!("c{n}.duckdb"));
        }
        s.add_recent("c5.duckdb");
        assert_eq!(s.recent.len(), MAX_RECENT);
        assert_eq!(s.recent[0], "c5.duckdb");
        assert_eq!(s.recent[1], "c11.duckdb");
        assert_eq!(s.recent.iter().filter(|p| *p == "c5.duckdb").count(), 1);
    }

    #[test]
    fn actor_from_settings_or_os() {
        let mut s = Settings::default();
        assert_eq!(
            app_actor(&s, Some("Jane Doe")),
            Actor::Analyst("JaneDoe".into())
        );
        assert_eq!(app_actor(&s, None), Actor::Analyst("analyst".into()));
        s.analyst = Some("jdoe".into());
        assert_eq!(
            app_actor(&s, Some("Jane Doe")),
            Actor::Analyst("jdoe".into())
        );
        // A name no actor can carry is not used.
        s.analyst = Some("j doe".into());
        assert_eq!(
            app_actor(&s, Some("ws01user")),
            Actor::Analyst("ws01user".into())
        );
    }
}
