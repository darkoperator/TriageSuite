# The OxideLens app: developer notes

The desktop app is a Tauri v2 shell (`crates/oxl-app`) around a React and
TypeScript front end (`app/`). The engine (`crates/oxl-engine`) holds every
rule; the backend drives it from a window, and the front end renders what the
backend answers and keeps what it is showing. These notes cover how to run it,
the contract between the two halves, how queries are superseded and
cancelled, the conventions front-end code follows, the case record's screens
and keys, where state is kept, and the debug-only bridge an agent can drive
the window through. All names in the examples are synthetic.

## Running it

You need the Rust toolchain, Node 22, and on Windows the WebView2 runtime
(part of current Windows). The first build compiles DuckDB from source and
takes several minutes; later builds are quick.

```
cd oxidelens
npm --prefix app ci                 # the front end's packages, the Tauri CLI among them
```

**The app, in development.** From `crates/oxl-app`, run the Tauri CLI:

```
cd crates/oxl-app
../../app/node_modules/.bin/tauri dev      # or `cargo tauri dev`, with tauri-cli 2 installed
```

(In PowerShell the same path with backslashes runs the CLI's `tauri.cmd`.)
The CLI starts the Vite dev server (`beforeDevCommand`, http://localhost:5173)
and builds and runs the debug binary, which loads the front end from it and
reloads it as you edit. A debug build carries the MCP bridge (below) and opens
a console window on Windows, where the backend's panics land.

**The mock dev page.** The front end alone, in a browser, on a fake backend:

```
npm --prefix app run dev:mock      # http://localhost:5173, with a MOCK banner
```

`app/src/mock/` answers every command of the contract from a seeded
5,000-row, 40-column `mock_events` relation (with hostile cell text: markup, a
bidi override, a zero-width joiner, BEL, a formula, a 5,000-character value)
and a Hayabusa-like `mock_hayabusa`, with the backend's supersede, cancel,
`busy` and `case-changed` behaviour. Its case record (`mock/records.ts`) holds
an analyst's reviewed entry, an agent's unreviewed one, two undated entries,
records of every kind, links, a tag, a note and history, with the engine
fixtures' hostile record values (`HOSTILE_RECORD`) among their text. It is a
compile-time switch
(`__OXL_MOCK__`, true only for that dev server), so no other build holds any of
it. The mock holds no rules and shows every time in UTC: use it for layout and
interaction, never to check what the engine answers.

**A test build.** A release binary that serves the built front end itself:

```
cd crates/oxl-app
../../app/node_modules/.bin/tauri build    # or `cargo tauri build`
```

The CLI builds the front end first (`beforeBuildCommand`) and then
`target/release/oxl-app` (`.exe` on Windows). Bundling is switched off in
`tauri.conf.json`, so there is no installer: copy the binary. A release build
has no MCP bridge and no console window. `cargo build --release -p oxl-app`
on its own is not a test build: without the CLI the binary is built in dev
mode and looks for the dev server.

**Tests and checks.** From `oxidelens/`: `cargo test`, `cargo clippy
--all-targets -- -D warnings` and `cargo fmt --check`. From `app/`: `npm run
lint`, `npm test` and `npm run build`. `scripts/check.sh` at the repository
root runs them all. Three ignored tests are run on purpose:

- `cargo test -p oxl-app --test record_fixtures -- --ignored` rewrites the
  recorded engine answers the front end's tests use
  (`app/src/test/fixtures/*.json`).
- `cargo test -p oxl-app --test walkthrough -- --ignored --nocapture
  real_case_walkthrough`, with `OXL_WALKTHROUGH_CASE` set to a case file, runs
  the end-to-end walkthrough (below) on that case and prints what each step
  saw and how long it took.
- The same with `real_case_performance` and `OXL_PERF_CASE` prints the grid's
  costs on that case, warm: the first page of the largest relation, a
  filter-row change, group headers, the histogram, value counts, and
  full-width blocks of the widest relations. The test's header lists the
  variables that choose the columns.

Both open the case through the app, which upgrades a format 1 or 2 case in
place, and the walkthrough writes to it: the display zone, a saved view, marks,
entries, records and IOCs, and its exports next to the case file.
`real_case_walkthrough` also times the case record there: M on 1,000 rows, a
re-check of 100 evidence rows, the four sheet lists, *Sweep all* over 20 IOCs
and the three exports. **Run them on a copy, never on an analyst's case.**

`tests/walkthrough.rs` is also an ordinary test on the two-host fixture: new
case, load the run, open `evtxtriage_events`, filter row `Security`, exclude
`4624` in the Values tab, group by `Channel` then `EventId`, brush the
histogram, switch the zone to `America/New_York` and check a time, save a
view, close and reopen the case, apply the view, export CSV, open a session.
Then the case record, on a case with WS01's logons (`two_hosts_logons`):
S on three rows, M on one, take the account and the IOC the candidates offer,
read the account's logons and the IOC's sightings, mark the entry reviewed,
export the timeline, workbook and report, and read the four sheets; and on
the first case, evidence through a later run (`rerun_ws01_events`): one
evidence row moves to its identical new row, the other stays "from a
superseded run", and removing the first run is refused until confirmed.
It calls the same command functions the window's `invoke` reaches, with the
arguments the front end builds, so a change that breaks the window's main path
fails `cargo test`. What it cannot see is rendering: layout, focus and drawing
are checked by hand in the window.

## The contract

The window and the backend share one contract, generated from the Rust types
and checked in under `app/src/contract/generated/`. Never edit those files by
hand.

**One TypeScript type per Rust type.** Every type a command takes or answers
derives `ts_rs::TS` beside its serde derives (in the engine behind its `ts`
feature), and ts-rs reads the serde attributes, so a rename, a default or an
untagged variant is the same on both sides. Each type becomes one
`<Type>.ts`. An `Option` is `T | null`, and every field is required in
TypeScript: the front end always sends complete objects, and serde defaults
are tolerance for other callers, not something the front end relies on. A
64-bit integer (row ids, counts, offsets) is `number`, because what the
window receives is JSON that `JSON.parse` reads into a `number`; every such
value is far below 2^53.

**`commands.ts`** holds `COMMANDS` (every command name, in the contract's
order), `CommandArgs` (each command's arguments, named as `invoke` takes them:
the Rust parameter in camelCase, so `query_id` is `queryId`), `CommandResult`,
`EVENTS` with `EventPayload`, and `CommandError`. The order of `COMMANDS` is
the order of `contract::COMMANDS` in `crates/oxl-app/src/contract.rs`, of the
signature table beside it, and of the `handlers!` list in `src/lib.rs`; a test
holds all three to one list.

**Drift fails a test.** `cargo test -p oxl-app --test contract` compares the
checked-in files byte for byte with what the Rust types generate, and calls
every command through Tauri's own IPC to check it takes exactly the arguments
the contract names. To regenerate after a change:

```
OXL_WRITE_CONTRACT=1 cargo test -p oxl-app --test contract
```

On the front end, `app/src/api.ts` is the only caller of `invoke` and
`listen`: one typed wrapper per command, `isCmdError`, and the event
listeners. The mock backend answers exactly the commands `api.ts` calls, and
`app/src/mock/backend.test.ts` fails otherwise.

**Adding a command:** write it as a plain function over `AppState` in
`src/cmd/`, with a `#[tauri::command(async)]` wrapper in that module's
`commands` (no command runs on the window's thread); add its name to
`COMMANDS`, its signature to the table (with `read!` if it reads the case,
which adds `queryId` and `slot`) and its wrapper to `handlers!`, all in the
same place in the order; regenerate the contract; add the wrapper to `api.ts`
and an answer to the mock.

**Errors.** A failed command rejects with `{ code, message }`. `code` is
what to branch on: the engine's `not_found`, `invalid`, `refused`,
`case_locked`, `needs_upgrade`, `engine`, `io` and `internal`, and the app's
own `no_case`, `busy` (another write is running), `superseded` and
`interrupted`. `message` is for a person and is shown through `displaySafe`.

## Query slots and cancellation

No query runs on the window's thread, and every read can be superseded or
cancelled.

**Every read takes `queryId` and `slot` first.** The window makes a fresh
query id for each call (`newQueryId()`); the slot names what the answer fills
on screen. The grid's slots are per tab: `tab:<id>:page:<offset>`,
`tab:<id>:counts`, `tab:<id>:hist`, `tab:<id>:hist-brush`, `tab:<id>:export`
and so on.

**A newer query on a slot supersedes the one there.** The registry
(`src/queries.rs`) interrupts the older query, which answers `superseded`.
Typing ten characters into the filter row in a second leaves exactly the last
state's rows, counts and histogram on screen: the nine earlier queries end
`superseded`, and the front end never draws an answer for a state it is no
longer showing.

**`query_cancel` stops queries by id**, and they answer `interrupted`. An id
cancelled before its query starts is remembered (the newest 256), and that
query answers `interrupted` without running. DuckDB clears an interrupt when
the next statement starts, so a stopped query is interrupted again every
50 ms until its call returns.

**Reads run beside each other and beside a write.** The open case has one
writer connection, the only one that changes the case, and 8 readers cloned
from it. A read registers first, then waits for a reader (giving up as soon as
it is superseded or cancelled), so a slot's queries keep the order they
arrived in. A write that arrives while another write runs answers `busy`
rather than queueing; after every successful write the backend sends
`case-changed` with what it touched (`sources` for a load, `views`,
`display_zone`; for the case record `marks`, `tags`, `notes`, `entries` and
`records`, as `src/cmd/record.rs` lists per command), and the front end reads
again what that affects.

**A CSV export is a read and a job.** `grid_export` runs on a reader under its
query id, announces `job-started` and `job-finished`, and registers a cancel
flag with its query: `query_cancel` with the export's id (or a newer query on
its slot, or the case closing) stops it between statements, and its partial
file is removed. Stopped after its file was renamed into place, it answers
its report, because the file is there. **One export runs at a time** across
all tabs: the shell holds the one cancel slot, and a second export is refused
in the window until the first ends.

**The record exports** (`export_timeline`, `export_workbook`,
`export_report`) are reads and jobs in the same way. The record is the
analyst's notes, read whole and quickly, so each holds the write gate for all
its work: it waits for a running write first (giving up when stopped), and a
write that arrives while it runs answers `busy`. No load can then make the
target part of a source between the engine's checks and its rename.
`ioc_sightings` is a read that also carries a cancel flag, which the engine
looks at between relations; the window sweeps one IOC at a time on the
`sightings` slot.

**Closing the case** stops every registered query, lets none register until
it is done (`no_case`), waits for the readers to come back, and drops them
before the writer, so the file can be opened again as soon as `case_close`
returns. Quitting the app closes the case off the event loop's thread and
waits for that a bounded time.

## Writing front-end code

**Where things are.** `app/src/api.ts` is the only caller of the backend
(`call()`, one typed wrapper per command, `newQueryId`, `isCmdError`, the
event listeners) and the only importer of `@tauri-apps/api`. The contract
types in `app/src/contract/generated/` are written by `cargo test -p oxl-app
--test contract`; regenerate them, never edit them. `app/src/text.ts` has
`displaySafe` and `displaySafeMultiline`. `app/src/shell/slots.tsx` names the
mount points a screen fills, and `useShell()` (`shell/context.ts`) is how a
screen reaches the shell: status bar, errors, settings writes.
`grid/dialogFocus.ts` has `useDialogFocus` and `trapTab`, `shell/keys.ts`
has `isTypingTarget`, and `shell/useCaseChanged.ts` subscribes to
`case-changed`.

**Answers that arrive late.** A read can come back after the screen has moved
on: the state, zone, rules, tab or case changed while it ran. The front end
draws only answers that belong to what it shows now, and every async path
follows the same four rules. `grid/useGridData.ts` is the reference:

- Key the request by what it reads: `stableKey([state, zone, rules])`, not
  object identity, so an equal state does not refetch and a changed one does.
- Take a version (or key) before the call, and compare it after *every*
  `await` and in the rejection path too. A mismatch drops the answer, whatever
  it holds.
- Record the query ids in flight. When the version moves on, cancel them with
  `query_cancel` and forget the settled ones. A copy or export run gets its own
  session (`GridStore.beginCopy()`) that a newer run or a state change ends.
- Treat `superseded`, `interrupted` and `no_case` as "not drawn", never as
  errors.

When one step reads state that another step writes, swap them together in
one record (`grid/useMatches.ts` keeps `{key, state, running}` and swaps it
in one layout effect), so a render between the two never pairs a new key
with an old state.

**Settings writes** go through the shell's one queue (`shell/Shell.tsx`;
screens reach it through `useShell()`'s `saveLayout`, `saveColourRules` and
`setHistogram`): plain changes coalesce, an awaited save (`saveSettings`, the
settings dialog's) is its own entry, and each
write is the last confirmed reply plus the queued changes. A second write
path can persist an older object last, so there is none.

**Keyboard and focus.** Every action has a keyboard path. Use a `<button>`
for anything clickable. A dialog focuses its first control when it opens and
gives focus back to the control that opened it when it closes
(`useDialogFocus(box, returnTo)`; pass `returnTo` when a menu opened it,
because the menu is gone by then). A modal keeps Tab inside it (`trapTab`).
Shortcuts do nothing while `isTypingTarget` is true. ESLint
(`jsx-a11y`) fails a mouse-only handler on an element that cannot take focus.
Where pointer-only is deliberate, the disable comment says what the keyboard
path is instead.

**Tests.** Screen tests call the real `api.ts` against the mock backend
(`src/test/backend.ts`: `callLog`, `callsOf`, `overrideCommand`,
`emitEvent`). Shapes the engine really answers come from the recorded
fixtures (`src/test/fixtures.ts`). Every component that renders case data
also renders the mock's hostile rows. For each async path, a test holds the
first answer back, changes the state, then releases the answer and asserts
it is not drawn.

## The case record in the window

The case record's screens are four sheets pinned after the Overview tab,
**Timeline**, **Hosts**, **Accounts** and **IOCs**, each tab with a count (one
page, up to 1,000, then `1000+`), and the editors they open in the right pane.
Every rule is the engine's: the screens send what the analyst asked for and
show the engine's answer, or its refusal in place (the status bar for a row
key, the editor for an edit), never in a dialog.

**Marks.** Suspicious amber ● `#f5a524`, Malicious red ■ `#f0565b`, Agent:
malicious purple ◆ `#a78bfa` (the light theme uses contrast-adjusted shades of
the same three); nothing else uses these colours or glyphs, and a colour rule
never hides a mark. The left pane counts each state and opens the worklist of
its rows across every dataset; a dataset with marked rows shows its name in
the colour of its most severe mark, with the count beside it.

**The row keys** on a dataset grid (`records/useRowActions.tsx`), inactive
while a text field has focus or with Ctrl, Alt or Meta held:

| Key | What it does |
|---|---|
| S | Mark the selection Suspicious; no dialog, no confirmation |
| U | Clear its Suspicious marks; no dialog |
| M | Make a timeline entry per row, with the row as evidence, and show it in the entry editor; no dialog |
| T | Tag or untag the selection (a small dialog) |
| N | A note on the focused row (a small dialog) |
| A | Attach the selection to an entry picked from the most recent, typing to narrow |
| I | The focused cell's value as a new IOC, its type the engine's guess, confirmed before it is added |

Each acts on the row ids the grid resolved the selection to before it writes,
at most 10,000 (more is refused in the status bar with the count); after the
re-read that follows, the selection is rebuilt from those ids. On a grouped
grid the keys act on the focused data row, and I is the only cell key; a
merged tab has the row keys but no I. The key strip lists only the keys live
on the tab shown. The row detail lists the entries holding the row, each
labelled "added by agent: <name>" when an agent made it.

**Timeline** (`records/TimelineSheet.tsx`): the entries by time, the undated
("needs a time") last, as a table or as Host lanes on one time axis;
filtered by text, status, host and tactic, and grouped by host, tactic or
status; past 1,000 entries it pages. Keys on the table: ↑ ↓ select, N new
entry, Enter edit, R mark reviewed, Del remove (the evidence rows stay in
their datasets).

**The entry editor** (`records/EntryEditor.tsx`): the time in UTC with where
it came from (the row or typed), title, description, ATT&CK tactic and
technique ids, status with *Mark reviewed* (refused for an undated entry),
the records it links (each opens its sheet with the record selected; more
are linked from a picker that filters through the engine and pages), its
evidence rows, each re-checked on every read (unchanged, changed, from a
superseded run, source removed) and opening its dataset at the row, its
history, and after M the **candidates**: the accounts and IOCs its rows name,
each added and linked with one click.

**Hosts, Accounts and IOCs** (`records/RecordSheet.tsx`): a grid of the
records with the engine's computed columns (entries, first and last seen,
hosts seen), a filter row and header sorts that go to the engine, grouping,
and the record editor in the right pane. Keys: ↑ ↓ select, Enter edit. An
account's editor shows its **logon summary**: successes and failures by host
and logon type, each count opening its rows in a dataset tab with a chip that
reads as the account (`Logons: ACME\jdoe · type 10 · WS01 · success`), and
the rows that do not parse counted. The IOCs sheet has *Find sightings* per
IOC and *Sweep all*, one IOC at a time on the `sightings` slot with its
progress and *Cancel*; what they find is kept in the window until the next
`case-changed`, and *Show hits* opens the find as a merged tab. A grid cell
whose value is a live IOC is underlined.

**Export** in the top bar writes the timeline CSV, the XLSX workbook or the
Markdown report through a save dialog, as the status-bar export job with its
cancel; a cancel is the neutral line "Export cancelled.", and a refusal (the
case file, a path inside a loaded run) is shown as the engine words it.

## What is kept where

| Where | What |
|---|---|
| **The case file** (travels with the case) | Sources, relations and rows; marks, tags and notes; timeline entries and their evidence (with each row's snapshot), host, account and IOC records and their links; the history; saved views and filter presets; the display zone (`ox.setting`, default `UTC`); the run sessions each load read |
| **The settings file** (per machine): `settings.json` in the app's config directory | Theme (dark or light), the grid's font size (10 to 16 px), the side panes (open or folded), the histogram strip on or off, column layouts per dataset, colour rules per dataset, recent cases (the newest 10), the analyst name |
| **The webview's localStorage** (per machine, per case) | The open tabs and each tab's scroll position, under `oxl.tabs.v1.<case_id>` |

The **analyst name** is what the app's changes are recorded under
(`analyst:<name>`); unset, it is the operating system's user. Layouts and
colour rules are keyed by dataset (relation) name, so they follow a dataset
from case to case on one machine. Applying a saved view that carries a layout
or colour rules writes them into the settings for that dataset: applying a
view is the analyst choosing them. A colour rule of the view that names a
column the dataset lacks is kept, switched off, with a warning (switched back
on, the engine skips it and says why). Every open tab of a dataset follows
its layout in the settings: when it changes to one a tab is not showing (a
view applied, or a column edited, in another tab), the tab takes it, and its
next edit builds on it. A tab's own edit is saved 250 ms after the last one;
until then it keeps what it shows, and that save is the latest write.

The **recent cases** are the backend's alone. Opening or making a case puts
it first; `recent_remove` forgets one (the *×* beside it on the welcome
screen) and `recent_clear` forgets them all (*Forget recent cases* in the
settings dialog). The window's other settings changes go through one queue
that sends the whole settings object with `settings_update`, and that command
keeps the stored `recent` whatever list it is sent, so a save that was on its
way when a case opened cannot put back an older list. The queue's patch type
(`SettingsPatch`) has no `recent` field.

The settings file is written whole: a save writes a `.partial` file beside it
and renames it over the old one. The 1 MiB cap a start reads is checked on
exactly the bytes a save writes (the file is pretty-printed JSON), so a save
the app accepted is never one the next start refuses as too large. A file that cannot be read is moved aside to
`settings.json.damaged-<yyyyMMddHHmmss>` and the defaults are used; the
window hears `settings-damaged`, and every settings reply names the damaged
file until a save succeeds.

**Opening a case from step 1 or step 2** (format 1 or 2) upgrades it in place
to format 3, `1 → 2 → 3` in one transaction, with a `case_upgrade` history row
per step; every source, relation, mark, tag, note and saved view is
unchanged, and a run whose sessions were lost by an earlier upgrade has them
read again. A command that opens a case read-only (every CLI read) reads an
older case as it is.

## The debug-only MCP bridge

Debug builds register `tauri-plugin-mcp-bridge` (`src/main.rs`, under
`cfg(debug_assertions)`), bound to `127.0.0.1`. It lets an agent with the
Tauri MCP server drive and screenshot the window: the server connects to the
plugin's WebSocket, on port 9223 or the next free one up to 9322. Release
builds do not register it, and the `mcp-bridge:default` permission in
`capabilities/default.json` then grants nothing. Treat a debug build as a
development tool: anything on the machine can reach that port while it runs.

The bridge drives the window as a user would. To check the backend's answers
without a window, call the command functions directly, as
`tests/walkthrough.rs` and `tests/commands.rs` do.
