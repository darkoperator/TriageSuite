# SQLETriage

SQLETriage is a map-driven SQLite querying engine: it loads a corpus of `.smap`
map files, each of which names a SQLite database (by filename and/or a
schema-detection SQL query) and lists one or more named SQL queries to run
against it, then executes every query that matches a given database file and
emits one output row per result row. A database "matches" a map in one of two
ways: by default the database's filename must match the map's `FileName`
pattern *and* the map's `IdentifyQuery` must return the expected
`IdentifyValue` against the file's schema; with `--hunt`, the filename check is
skipped entirely and every file is opened and probed by content (the
`IdentifyQuery`/`IdentifyValue` check alone decides applicability). Because a
single database file can satisfy more than one map's identification check, and
a single map can define several queries, one input file can produce several
distinct output datasets.

## Target Windows versions

The bundled map corpus (93 `.smap` files, confirmed by directory listing at
`resources/sqlite-maps/`) is broad and artifact-specific per map, not a single
"Windows Search parser." Two maps do target the Windows Search Index
specifically:

- `Windows_Search_Index_Windows_db.smap` — `Windows.db`, containing
  `SystemIndex_1_Properties`, `SystemIndex_1_PropertyStore`, and
  `SystemIndex_1_PropertyStore_Metadata` tables. This is the SQLite-backed
  Windows Search index used on builds where Windows replaced the older
  ESE-based `Windows.edb` index.
- `Windows_Search_Index_Windows_gather_db.smap` — `Windows-gather.db`,
  containing `SystemIndex_Gthr`, `SystemIndex_GthrPth`, and
  `SystemIndex_GthrAppOwner` tables.

Neither map file states an exact Windows build/version range in its metadata,
so no specific version claim beyond "the SQLite-based Windows Search index" can
be confirmed from source.

Beyond Windows Search, the rest of the corpus targets other SQLite-backed
artifacts, most (but not all) also Windows-hosted: Chromium-family browser
databases (History, Cookies, Web Data, Favicons, Top Sites, etc. — 13 maps),
Firefox databases (places/History, Cookies, Bookmarks, Form History, Favicons
— 8 maps), Edge-specific databases (Collections, Navigation History, History
Screenshots), Dropbox client databases (11 maps), Google Drive client
databases (5 maps), the "Your Phone"/Phone Link app databases (5 maps),
Bitdefender antivirus databases (4 maps), the DiagTrack `EventTranscript.db`
(2 maps), and roughly two dozen single-map entries for other Windows
applications (Sticky Notes, TeraCopy, WPNDatabase notifications,
RemoteDesktopManager, Nessus, Notion, Msty, Ivanti, IDrive, FileZilla,
FastStone Image Viewer, Cylance, ActivitiesCache, 4K Video Downloader, Windows
Update StoreDB, Photos). A small number of maps target Android (SMS, MMS,
contacts, logs) and iOS (SMS, Health, Photos, Accounts, Calls, Cellular Usage)
SQLite databases, which are not Windows artifacts at all.

In short: SQLETriage itself is not tied to any one artifact or Windows
version — it runs whatever SQLite query map(s) it is given, and applicability
is entirely map-dependent. The README's "Windows Search parser" description
reflects two specific bundled maps, not the whole corpus.

## Compatibility

SQLETriage's map loader (`crates/sqle-triage/src/map.rs`) parses the same
`.smap` YAML structure used by Eric Zimmerman's SQLECmd: `Description`,
`CSVPrefix`, `FileName`, `IdentifyQuery`, `IdentifyValue`, and a `Queries` list
of `Name`/`Query`/`BaseFileName` (with `BindToTable` accepted as an alias for
older maps). This is map-format compatible by construction — the bundled
corpus is synced directly from the upstream SQLECmd Maps repository
(`github.com/EricZimmerman/SQLECmd`) via `--sync`
(`crates/sqle-triage/src/sync.rs`), so any `.smap` file written for SQLECmd is
usable by SQLETriage as-is.

Cell rendering is compatible output with SQLECmd's own formatting choices,
per comments in `crates/sqle-triage/src/render.rs`: REAL values that are
integer-valued render without a decimal point (`5.0` → `"5"`), BLOBs render as
`0x` + uppercase hex unless `--noblob` is given (in which case the literal
`<BLOB>` marker is written), and every output row gets a trailing
`SourceFile` column appended (the path of the database file the row came
from) — mirroring SQLECmd's own trailing `SourceFile` column.

## Flags

Input (exactly one required, from the shared `CommonArgs`):
```
-d, --directory <DIR>         Recursively discover candidate files under this directory
-f, --file <FILE>             Explicit database file (repeatable)
```

Output (at least one required):
```
--csv <DIR>                   Write Zimmerman-compatible CSV output beneath this directory
--json <DIR>                  Write Zimmerman-compatible JSON output beneath this directory
--csvf <NAME>                 Override the default CSV basename
--jsonf <NAME>                Override the default JSON basename
--pretty                      Pretty-print JSON (whitespace only)
--overwrite                   Allow replacement of existing output files
--nested-output                Preserve the legacy nested output layout under <root>/<ToolName>/<identity>/
```

Diagnostics:
```
-q, --quiet                   Suppress per-file informational messages
--debug                        Emit debug-level diagnostics to stderr
--trace                        Emit trace-level diagnostics to stderr (implies --debug)
```

SQLETriage-specific (`crates/sqle-triage/src/cli.rs`):
```
--hunt                         Inspect every discovered file by content instead of limiting
                                discovery to known SQLite database filename patterns
--no-dedupe                     Disable dedupe (default: skip databases whose SHA-1 content
                                hash was already seen in this run)
--noblob                        Suppress BLOB values in query output (writes "<BLOB>" instead
                                of hex-encoded content)
--sync                          Refresh the bundled map corpus from the upstream SQLECmd
                                GitHub repository into resources/sqlite-maps/, then exit
                                (takes effect on the next build, since the corpus is
                                embedded at compile time)
```

Without `--hunt`, file discovery is limited to a fixed set of filename
patterns (from `crates/sqle-triage/src/lib.rs`): `*.db`, `*.sqlite`,
`*.sqlite3`, and the literal names `History`, `Cookies`, `Web Data`,
`places.sqlite`, `favicons`, `shortcuts`, `ActivitiesCache.db`, and
`wpndatabase.db`. With `--hunt`, every file (`*`) is opened and validated by
SQLite header magic regardless of name.

## Output layout

Output uses the shared TriageSuite layout (`triage-core::output::layout`).
By default (flat mode):
```
<out>/
  <MapCSVPrefix>_<QueryBaseFileName>_<identity>.csv
  <MapCSVPrefix>_<QueryBaseFileName>_<identity>.json
```
With `--nested-output` (legacy nested tree):
```
<out>/
  SQLETriage/
    users/
      <identity>/
        <MapCSVPrefix>_<QueryBaseFileName>.csv
        <MapCSVPrefix>_<QueryBaseFileName>.json
    system/
      <MapCSVPrefix>_<QueryBaseFileName>.csv
      <MapCSVPrefix>_<QueryBaseFileName>.json
```

The basename for each dataset is `{CSVPrefix}_{BaseFileName}` — the map's
`CSVPrefix` field joined with the individual query's `BaseFileName`
(`crates/sqle-triage/src/engine.rs`, `run_maps`). For example, the Windows
Search `Windows.db` map (`CSVPrefix: Windows`) with its "Joined PropertyStore
Metadata" query (`BaseFileName: Joined_PropertyStore_Metadata`) produces
`Windows_Joined_PropertyStore_Metadata.csv`. Because output files are opened
in append mode and keyed by `(identity, basename)`, multiple source database
files that resolve to the same identity and the same map/query basename are
merged into one CSV/NDJSON pair; the CSV header row is written only once, the
first time that basename is created for that identity. SQLETriage declares no
static datasets (`DATASETS` is empty in `crates/sqle-triage/src/lib.rs`) —
every output file is created dynamically at runtime based on which maps
matched.

## Output fields

Unlike TriageSuite's other tools, SQLETriage has no fixed record struct.
Each output dataset's columns are exactly the column list returned by that
map query's own `SELECT` statement — i.e. whatever aliases/column names the
SQL author put in the query (confirmed in `crates/sqle-triage/src/engine.rs`:
`db.query_with_columns(&q.query)` returns `(columns, rows)`, and `columns`
becomes the CSV header verbatim). SQLETriage appends exactly one extra column
to every dataset beyond what the query itself returns: `SourceFile`, holding
the full path of the source database file. There is no other injected or
normalized column — no fixed timestamp, identity, or path fields beyond what
each map's query selects.

## Bundled maps

93 `.smap` files ship under `resources/sqlite-maps/` and are embedded into
the binary at compile time via `include_dir!` (`crates/sqle-triage/src/map.rs`).
They cover, by rough grouping (counts from the bundled corpus):

- **Windows Search Index** (2 maps): `Windows.db` (SystemIndex_1_Properties /
  PropertyStore / PropertyStore_Metadata) and `Windows-gather.db`
  (SystemIndex_Gthr / GthrPth / GthrAppOwner)
- **Chromium-family browsers** (13 maps): History, Cookies, Autofill entries
  and profiles, Downloads, Favicons, Top Sites, Keyword Searches, Network
  Action Predictor, Omnibox Shortcuts, Masked Credit Cards, Media History
  Playback/Session
- **Edge browser** (3 maps): Collections, Navigation History, History
  Screenshots
- **Firefox** (8 maps): places.sqlite History (current and legacy schema),
  Cookies, Bookmarks, Form History, Favicons, Downloads (Downloads and Places
  variants)
- **Dropbox desktop client** (11 maps): Aggregation DB, Configurations, File
  Cache, Icon DB, Instance DB, Non-Local Resources, Recent Items, SFJ
  Resources, Starred Items, Sync History, Tray Thumbnails
- **Google Drive desktop client** (5 maps): Changes, CloudGraph DB, metadata
  sqlite DB, Snapshot DB, Sync Config DB
- **Your Phone / Phone Link** (5 maps): Contacts DB, Notifications DB,
  Phone DB SMS Messages, Photos DB, Settings DB
- **Bitdefender** (4 maps): Antiphishing, cache, es, RansomwareRecover
- **Other single-artifact Windows maps** (~23 maps): ActivitiesCache,
  WPNDatabase (Notifications, WNSPushChannel), EventTranscript.db (data
  sampling and no-data-sampling variants), TeraCopy (History, MainDB), Sticky
  Notes (Microsoft and SimpleStickyNotes), RemoteDesktopManager, Nessus
  Preferences, Notion Entries, Msty Database, Ivanti IvAppMon, IDrive File
  Backup, FileZilla Queue, FastStone Image Viewer, Cylance chpDatabase,
  4K Video Downloader History, pCloud, Photos, Windows Update StoreDB
- **Android** (7 maps): Calls, Contacts2 DB, Frosting, LocalAppState, Logs,
  mmssms DB, SMS
- **iOS** (9 maps): Accounts (and Accounts4), Calls, Cellular Usage,
  HealthDb (and HealthDb_Secure), Photos, SMS
- **Test fixtures** (6 maps, prefixed `TestFiles_`): BlobTest variants,
  CarsDB, Contacts — used for corpus/engine testing rather than real-world
  triage

Run with `--sync` to refresh this corpus from the upstream SQLECmd Maps
repository before rebuilding; the corpus is compiled into the binary, so a
`--sync` run only takes effect on the next build.

## Examples

```
# Default: scan for known SQLite database filenames only, run all matching bundled maps
SQLETriage -d /mnt/triage --csv ./out --json ./out

# Hunt mode: inspect every file's content, not just known filenames
SQLETriage -d /mnt/triage --hunt --csv ./out

# Explicit single database file, CSV only, suppress BLOB content
SQLETriage -f /mnt/triage/C/Users/alice/AppData/Local/Microsoft/Windows/Windows.db --csv ./out --noblob

# Refresh the bundled map corpus from upstream SQLECmd before the next build
SQLETriage --sync
```
