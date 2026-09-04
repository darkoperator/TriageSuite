# BrowserTriage

BrowserTriage parses browser artifacts from Chromium-family browsers (Chrome, Edge, Brave, Opera,
Vivaldi, Arc and other Chromium forks) and Firefox-family browsers (Firefox, LibreWolf, Waterfox,
SeaMonkey, Pale Moon) into eight typed datasets plus a derived timeline.

It is a TriageSuite parser, not a wrapper around anything: the SQLite and JSON parsing is our own,
using the same `triage-sqlite` evidence-safe database layer as the rest of the suite.

## Why this exists alongside SQLETriage

`SQLETriage` already ships around two dozen browser `.smap` maps and can dump the same tables.
BrowserTriage earns its place on five things a generic SQL-map engine cannot do:

1. **Typed timestamps.** The maps emit `datetime(t/1000000 + …, 'unixepoch')`, a string at
   whole-second precision. BrowserTriage emits a `WinTimestamp` with sub-second precision, and an
   unset value renders as an empty cell rather than an epoch.
2. **Browser and profile attribution.** Every row carries `Browser`, `Browser Channel` and
   `Profile`, so one file per user holds every browser and every profile without ambiguity.
3. **The JSON artifacts.** `Bookmarks`, `Preferences`, `logins.json` and `extensions.json` are not
   databases and the map engine cannot read them at all.
4. **Cross-artifact joins.** Downloads are assembled from `downloads` plus `downloads_url_chains`;
   Firefox downloads from two separate annotation rows; bookmark folder paths from a parent walk.
5. **A derived timeline** over every instant the typed datasets carry.

The two tools coexist deliberately. `SQLETriage` is opt-in, so the default orchestrator run does
not double-parse; use `--only sqle` when you want a raw table dump instead of a normalized one.

## The completeness contract

This tool exists because its predecessor discarded 41% of what it extracted, in a single run,
while exiting 0. So the parsing rules are explicit:

- **No timestamp filter, anywhere.** A row with a zero or null timestamp is emitted with an empty
  cell. It is never dropped and never given an epoch.
- **`LEFT JOIN`, never `INNER JOIN`,** plus an orphan pass for the other side. A visit whose URL
  row is gone, and a URL whose visit rows are gone, are both emitted — the latter marked
  `Record Type = URL Only`, because a URL with a visit count and no visits is a history-deletion
  indicator and is often the most interesting row in the file.
- **Total cell accessors.** A cell of an unexpected storage class is converted, never skipped.
- **Nothing is silently discarded.** Anything that would otherwise vanish — a dangling foreign
  key, undecodable bytes, an unrecognized encryption prefix, an epoch fallback, a sparse record —
  is recorded in that row's `Notes` column.

Because `OutputRouter` writes one file per `(user, dataset)` and appends for the life of the run,
no output filename is ever derived from a source artifact. Two browser profiles cannot collide.

## Supported browsers

| Family | Browsers | Container |
|---|---|---|
| Chromium | Chrome, Edge, Brave, Opera / Opera GX / Opera Crypto / Opera Neon, Vivaldi, Arc, Chromium, unrecognized forks | `User Data` on Windows; none on macOS or Linux, and none for Opera |
| Firefox | Firefox, LibreWolf, Waterfox, SeaMonkey, Pale Moon | `Profiles` on Windows and macOS; none under `~/.mozilla/firefox` |

Identification is driven by a table of install layouts, each naming a product directory and the
container that platform interposes between it and the profile. The container is consumed only when
it is actually present, so one entry covers a browser on every platform: `Google/Chrome/User Data/Default`,
`Library/Application Support/Google/Chrome/Default` and `.config/google-chrome/Default` all resolve
to Chrome, profile `Default`. The deepest matching product directory wins, so a capture staged
beneath a directory that happens to share a browser's name cannot shadow the real install.

Channels (`Stable`, `Beta`, `Dev`, `Canary`, `Nightly`, `ESR`, `Snapshot`, `Testing`) come from the
product directory. Firefox refines them from the profile-name suffix (`.default-esr`,
`.dev-edition-default`, `.default-nightly`, `.default-beta`), because on Windows and macOS every
channel shares one `Profiles` directory and the product directory alone cannot tell them apart.

**Profile identification is the full relative path below the product directory**, not the containing
directory. `Snapshots/116.0.5845.97/Default` and `Default` are therefore distinct with no special
case — this is the specific collision that cost the previous tool most of its output. Exactly one
segment is ever stripped: a trailing `Network` (`<profile>/Network/Cookies`, Chrome 96+), and never
the last remaining segment, so a profile literally named `Network` survives instead of collapsing
onto the directory above it.

A path with no recognizable product directory and no container — an artifact handed over with `-f`,
or an Electron application that shares Chromium's filenames — is reported as `Chromium (Unknown)` or
`Firefox (Unknown)` with the profile taken from the containing directory and a note on every row
saying the attribution was degraded.

Velociraptor's URL-encoded paths (`C%3A`) are handled by `winpath::segments`, the same
normalization user attribution uses.

## Artifacts read

| Dataset | Chromium source | Firefox source |
|---|---|---|
| History | `History` → `urls`, `visits` | `places.sqlite` → `moz_places`, `moz_historyvisits` |
| Downloads | `History` → `downloads`, `downloads_url_chains` | `places.sqlite` → `moz_annos` |
| Cookies | `Cookies` (or `Network/Cookies`) → `cookies` | `cookies.sqlite` → `moz_cookies` |
| Autofill | `Web Data` → `autofill` | `formhistory.sqlite` → `moz_formhistory` |
| Bookmarks | `Bookmarks` (JSON tree) | `places.sqlite` → `moz_bookmarks` |
| Logins | `Login Data`, `Login Data For Account` → `logins` | `logins.json` |
| Keyword Searches | `History` → `keyword_search_terms` | `places.sqlite` → `moz_places_metadata`, `moz_inputhistory` |
| Extensions | `Preferences`, `Secure Preferences` → `extensions.settings` | `extensions.json` → `addons[]` |

## Output

Nine datasets. Under the orchestrator (nested layout):

```
<out>/<HOST>/BrowserTriage/users/<user>/
  BrowserTriage_Output.csv                  History (primary)
  BrowserTriage_Output_Downloads.csv
  BrowserTriage_Output_Cookies.csv
  BrowserTriage_Output_Autofill.csv
  BrowserTriage_Output_Bookmarks.csv
  BrowserTriage_Output_Logins.csv
  BrowserTriage_Output_KeywordSearches.csv
  BrowserTriage_Output_Extensions.csv
  BrowserTriage_Output_Timeline.csv         CSV only
```

Every artifact record opens with `Browser`, `Browser Channel`, `Profile` and closes with `Notes`,
`Source File`. Discriminators come first because one file holds every browser and profile
belonging to that user, and it is unreadable without them on the left.

History is the primary dataset, so `--csvf case.csv` produces `case.csv`, `case_Downloads.csv`,
`case_Cookies.csv` and so on.

### The timeline

A cross-artifact index over every non-null instant, with columns `Timestamp`, `Timestamp Type`,
`Browser`, `Profile`, `Artifact`, `Value`, `Source File`.

Multi-timestamp records fan out, following the same pattern as PETriage's `_Timeline`: a completed
download contributes both a `Download Started` and a `Download Completed` row. Twenty-one
`Timestamp Type` values are used, covering visits, downloads, cookies, autofill, bookmarks,
logins, searches and extensions.

Rows are written in artifact-discovery order and are **not sorted**, matching the suite's existing
`_Timeline` outputs — sort in Timeline Explorer or Excel. A record with no instant contributes no
timeline row; it is still fully present in its typed dataset.

The timeline is typically two to three times larger than all eight typed datasets combined,
because cookies alone fan out four ways. `--no-timeline` suppresses it.

## Flags

Standard `CommonArgs` (`-d/--directory`, `-f/--file`, `--csv`, `--json`, `--csvf`, `--jsonf`,
`--pretty`, `--overwrite`, `--nested-output`, `--debug`, `--trace`, `-q/--quiet`), plus:

```
--no-timeline    Skip the derived _Timeline dataset
```

`TriageSuite run` accepts `--no-timeline` too, and it reaches this tool the same way `--hunt`
reaches SQLETriage. The timeline is emitted by default in both.

## What is deliberately not extracted

**No decryption is attempted, ever.** Specifically:

- **Chromium cookie values** (`v10`/`v11`/`v20`/DPAPI). The row reports `Value Encrypted`,
  `Encryption Scheme` and `Value Length` — never the ciphertext. Chrome 127+ App-Bound encryption
  (`v20`) is machine- and SYSTEM-bound and is *not decryptable from a dead capture by any tool*.
- **Chromium passwords.** `Password Present`, `Password Encryption` and `Password Length` only.
  Chromium usernames *are* plaintext and are emitted — they are account-attribution evidence, not
  secrets.
- **Firefox usernames and passwords**, both NSS-encrypted. This is why `Username Encrypted`
  exists: a blank `Username` with that flag set means "there is one, we chose not to decrypt it",
  which is not the same as "there is no username".

Emitting credential material would make a triage CSV more sensitive than the evidence it came
from. A test greps every produced file for the password bytes and their hex and base64 encodings.

**Out of scope in this version:** Safari and IE/Edge Legacy (`WebCacheV01.dat`); browser cache;
session and tab recovery; LocalStorage and IndexedDB; favicons, Top Sites and Shortcuts; deleted
record carving from SQLite free pages and WAL; and the `credit_cards`, `local_addresses` and
`keywords` tables in `Web Data` — so the Autofill dataset is not the whole of that file.

## Sensitivity

Two things to know before sharing the output:

- `Source File` holds the absolute path of the source database, which embeds your collection's
  mount path.
- Firefox cookie **values are plaintext** and are emitted in full. Chromium's are not, because
  they are encrypted — not because of a policy difference.

## Known behaviours worth expecting

- **Chromium `Snapshots/<version>` profiles** are stale copies left by a browser update, so their
  rows duplicate the live profile's older state. They are deliberately not filtered — pre-rollback
  state is evidence — but grouping only by user will double-count. Group by `Profile`.
- **This tool is exempt from the orchestrator's content dedupe**, via
  `Tool::dedupe_by_content() -> false`. `TriageSuite run` normally hashes every discovered file and
  parses identical content once, first discovered wins — right for a tool whose rows don't depend
  on where the file was, wrong here. A browser update leaves `Snapshots/<version>` copies
  byte-identical to the live profile, and every row here carries a `Profile` derived from the path,
  so collapsing them would keep the content and silently drop the second profile's attribution —
  making "which profiles held this extension" unanswerable. Duplicate content is legible in the
  output (`Profile` and `Source File` say which copy a row came from); missing attribution is not
  recoverable. On the reference capture the exemption is worth 38 files and 218 rows, and it makes
  the orchestrated run row-for-row identical to the standalone one across all nine datasets. Other
  tools are unaffected: `evtx`, `le` and `jle` still deduplicate.
- **A capture holding both `<profile>/Cookies` and `<profile>/Network/Cookies`** produces rows from
  both, distinguishable only by `Source File`. They are not deduplicated, because deduplication
  drops rows.
- **Chromium extension "stubs"** — entries under `extensions.settings` carrying only `state`,
  `disable_reasons` and `active_permissions`, with no manifest or location. They are extensions
  the profile knew about but never fully installed. They are emitted with a `Notes` label so a row
  of blanks explains itself.
- **`Typed Count` means different things per family**: a count in Chromium, a 0/1 flag in Firefox.
- **Schema drift is handled, not assumed.** Columns are projected against the live schema, so one
  added in a recent Chromium reads as null on an older profile instead of failing the table.
  Renamed columns (`lower_term`/`normalized_term`, `secure`/`is_secure`) are resolved from the
  schema rather than guessed.

## Examples

Run through the orchestrator over a whole capture (default-on):

```bash
TriageSuite run /mnt/triage --out ./results --csv
```

Only browser artifacts:

```bash
TriageSuite run /mnt/triage --out ./results --csv --only browser
```

Skip it:

```bash
TriageSuite run /mnt/triage --out ./results --csv --skip browser
```

Standalone over a mounted profile tree, without the timeline:

```bash
BrowserTriage -d /mnt/triage --csv ./out --no-timeline
```

A single artifact:

```bash
BrowserTriage -f "/mnt/triage/Users/alice/AppData/Local/Google/Chrome/User Data/Default/History" --csv ./out
```
