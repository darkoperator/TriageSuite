# DuckDB view layer

Every `TriageSuite run` emits a small DuckDB artifact pair alongside `run_manifest.json`
that turns the run's CSV output into typed, queryable views — no schema-writing, no
per-file globbing, no guessing which columns are actually numbers or timestamps. Point
DuckDB at it and query:

```bash
duckdb -c ".read /path/to/out/duckdb/views.sql" -c "SELECT * FROM amcachetriage_device_containers LIMIT 5;"
```

That's it for the common case. The rest of this document covers the two things worth
understanding before you write a real query: which view to use, and how to tell a
missing value from a conversion failure.

## Where the artifacts live

```
<out>/
  run_manifest.json          chain-of-custody report, unchanged by this feature
  duckdb/
    datasets.json            the inventory this run produced views from
    views.sql                generated SQL: one raw/typed view pair per dataset
```

Both files are written on **every** run, best-effort, and never change the run's exit
status — they are derived convenience data, not evidence. `datasets.json` carries a
top-level `status` field that tells you what you're looking at before you load
anything:

| `status` | Meaning | `views.sql` |
|---|---|---|
| `ok` | the run published CSV | real views |
| `no-csv-output` | a `--json`-only run | comment-only, names the reason |
| `run-rejected` | the input was rejected, nothing ran | comment-only, names the reason |
| `generation-failed` | the inventory could not be serialized (a path that is not valid UTF-8) | comment-only; the error is in the top-level `warnings` |

```bash
jq '.status' /path/to/out/duckdb/datasets.json
# "ok"
```

`datasets.json` and `views.sql` are always a matched pair — both carry the same
`generation` string, so if you ever see them disagree (they shouldn't), don't trust
either.

## The raw/typed view pair, and when to use which

Every dataset gets two views. Take `AmcacheTriage`'s `device_containers` dataset:

- **`raw_amcachetriage_device_containers`** — every column as `VARCHAR`, straight off
  the CSV. Nothing can fail to load into it; a value that looks like garbage stays as
  the exact text the tool wrote.
- **`amcachetriage_device_containers`** — a typed projection over the raw view: columns
  this tool declared a SQL type for (timestamps, booleans, big integers) are
  `TRY_CAST` to that type; every other column stays `VARCHAR` exactly as in the raw
  view.

Use the typed view for anything you'd normally write a `WHERE` clause or a range
comparison against — it's the one that lets `KeyLastWriteTimestamp > '2026-01-01'` or
`IsActive = true` work without you casting by hand. Reach for the raw view only when
you suspect the typed view is lying to you (see the next section) or when a column has
no declared type at all, in which case the two views show identical text anyway.

```sql
-- typed: filter on a real TIMESTAMP
SELECT KeyName, KeyLastWriteTimestamp
FROM amcachetriage_device_containers
WHERE KeyLastWriteTimestamp > TIMESTAMP '2026-01-01 00:00:00';

-- raw: see exactly what the CSV cell contained, no cast applied
SELECT KeyName, KeyLastWriteTimestamp
FROM raw_amcachetriage_device_containers
LIMIT 5;
```

Only datasets that got at least one declared column type gain any actual casting in
their typed view — v1 declares types for `AmcacheTriage`, `PETriage`, `LETriage`,
`JLETriage`, `RBTriage`, `SrumETriage` and `WxTTriage`; every other tool's typed view is
all-`VARCHAR` too, which is still a real DuckDB view with the four metadata columns
below, just with no `TRY_CAST` in it.

## Finding conversion failures: the `col` / `col__text` idiom

This is the core of the design, so it gets its own worked example. For every column the
typed view casts, it *also* keeps the original, uncast text next to it, suffixed
`__text`. Querying both together tells you which of three states a row is in:

| `col__text` | `col` | Meaning |
|---|---|---|
| `NULL` | `NULL` | the cell was blank — no value was ever written |
| non-`NULL` | non-`NULL` | the cast succeeded |
| non-`NULL` | `NULL` | **the cast failed** — the original text survives, queryable |

A blank cell and a failed cast both leave the typed column `NULL`; without `__text`
you cannot tell them apart. The documented idiom for finding every conversion failure
in a dataset is:

```sql
SELECT *
FROM amcachetriage_device_containers
WHERE "KeyLastWriteTimestamp" IS NULL
  AND "KeyLastWriteTimestamp__text" IS NOT NULL;
```

Zero rows back means every non-blank value in that column really did convert. This is
also why the raw view still matters even for a fully-typed dataset: `col__text` in the
typed view and the same column in the raw view show the same text, but the raw view
lets you inspect *every* column that way, not just the ones with a declared type.

## The four metadata columns, and their real names

Every view — raw or typed, on every dataset — is prefixed with four columns that carry
provenance the CSV itself doesn't:

- `_triage_run_id` — the run that produced this row
- `_triage_host` — the host it came from
- `_triage_identity` — the per-user identity, or `NULL` for a merged, multi-user file
  (see below)
- `_triage_output_file` — the CSV path the row was read from, distinct from any
  `SourceFile`-style evidence-path column the tool's own record already carries

These names are checked against every real header in the dataset (case-insensitively,
and against the `__text` names above too) and suffixed `_1`, `_2`, ... until free — so
if a dataset happens to already have a column literally called `_triage_host`, the
injected one becomes `_triage_host_1` instead of colliding with it. **Never assume the
plain name is what you'll see** — look it up:

```bash
jq '.datasets[] | select(.view == "amcachetriage_device_containers") | .metadata_columns' \
  /path/to/out/duckdb/datasets.json
# {
#   "run_id": "_triage_run_id",
#   "host": "_triage_host",
#   "identity": "_triage_identity",
#   "output_file": "_triage_output_file"
# }
```

The same per-dataset record also names the column carrying the tool's own evidence
path, if it has one, as `evidence_path_column` (`null` when there isn't one) — that's a
different thing from `_triage_output_file` above and is never invented if the dataset
doesn't actually have one.

## Timestamp precision and zone semantics

Every declared timestamp column becomes a microsecond-resolution `TIMESTAMP`, never
`TIMESTAMP_NS` — `TIMESTAMP_NS`'s range silently nulls anything before 1677, which
would erase legitimate 1601 FILETIME-epoch values this suite emits on purpose. The
source text carries 100-nanosecond (7-fractional-digit) resolution; the 7th digit is
not preserved in the typed column, only in `__text`:

```sql
SELECT KeyLastWriteTimestamp, KeyLastWriteTimestamp__text
FROM amcachetriage_device_containers
LIMIT 1;
-- KeyLastWriteTimestamp:       2026-09-17 12:00:00.123456
-- KeyLastWriteTimestamp__text: 2026-09-17T12:00:00.1234567Z
```

Zone semantics are **declared per column, never assumed**. `datasets.json` records a
`time_semantics` value (`"utc"`, `"local"`, or `"unknown"`) alongside every timestamp
column's entry in `effective_types`:

```bash
jq '.datasets[] | select(.view == "amcachetriage_device_containers")
    | .effective_types.KeyLastWriteTimestamp' /path/to/out/duckdb/datasets.json
# {
#   "sql_type": "TIMESTAMP",
#   "text_column": "KeyLastWriteTimestamp__text",
#   "precision": "microsecond; source text has 100ns resolution",
#   "time_semantics": "utc"
# }
```

v1 only declares `"utc"` for columns whose value is known to come from
`triage_core::timestamp`; a column whose zone can't be established this way stays
plain `VARCHAR` rather than being typed as if it were UTC. Check `time_semantics`
before comparing a timestamp column across hosts or against a wall-clock value you
supply.

## Why merged Velo files exclude their per-user slices

Under the default `--layout velo` output tree, a per-user dataset's category-level file
(e.g. `FileSystem/<stamp>_LETriage_results.csv`) is a merge: it already contains every
row from every per-user slice, plus a trailing `TriageUser` column recording which user
each row came from. The slices themselves
(`PerUser/<stamp>_LETriage_results_jdoe.csv`, `PerUser/..._asmith.csv`, ...) are left on
disk byte-identical, because they're the Zimmerman-exact compatibility output.

The view layer scans the merged file **and not** its slices — scanning both would
double-count every row the merge already folded in. This is recorded per file in
`datasets.json`'s `role` field: the merged file is `"role": "merged"` with
`"included_in_view": true`; each slice it absorbed is `"role": "slice"` with
`"included_in_view": false` and `"derived_into"` naming the merged file it went into.
Nothing is lost by excluding the slices — the merged file's `TriageUser` column carries
the same per-row identity, which is also why a merged file's `_triage_identity` is
`NULL`: there is no single identity for the row, look at `TriageUser` instead.

```sql
SELECT TriageUser, count(*) FROM letriage_main GROUP BY TriageUser;
```

A merge that only partially succeeded inverts this: the partial merged file is
excluded and its slices are included instead, each with its own real
`_triage_identity` — over-counting is recoverable by an analyst; silently dropping
evidence is not.

## Query templates for an incident response analyst

These are ordered the way an investigation actually runs: establish what evidence
survived, establish whether it is intact, then hunt. Every query below was run against
a real collection; the outputs shown are real.

### 1. What do I actually have?

Ask the inventory, not the views — this works even if the collection has moved and the
views' paths are stale, because `datasets.json` is readable as SQL on its own:

```sql
SELECT d.tool, d.dataset_id, d.view,
       len(d.files)                   AS files,
       cardinality(d.effective_types) AS typed_cols
FROM read_json('duckdb/datasets.json') j, UNNEST(j.datasets) AS t(d)
ORDER BY d.tool, d.dataset_id;
```

A tool that ran but produced nothing has **no row here at all**, which is itself a
finding: an artifact class the host should have had and does not. Pair it with
`j.dropped_overrides` and `j.warnings` for anything the generator itself flagged.

Row counts and the real time span need the views. There is no loop over view names, and
that is fine — choosing which datasets matter is the analyst's job:

```sql
SELECT 'evtx'    AS ds, count(*) n, min(TimeCreated) lo, max(TimeCreated) hi FROM evtxtriage_events
UNION ALL SELECT 'mft',  count(*), min(Created0x10), max(Created0x10)         FROM mftriage_mft
UNION ALL SELECT 'usn',  count(*), min(UpdateTimestamp), max(UpdateTimestamp) FROM mftriage_usn
UNION ALL SELECT 'pf',   count(*), min(RunTime), max(RunTime)                 FROM petriage_timeline
ORDER BY ds;
```

Which users are represented — a profile that should be there and is not is a lead:

```sql
SELECT TriageUser, count(*) n FROM jletriage_auto GROUP BY 1 ORDER BY n DESC;
```

### 2. Is the evidence intact?

**Channel coverage and window.** Run this before concluding anything from an *absence*
of events:

```sql
SELECT Channel, count(*) AS events,
       min(TimeCreated) AS first_event, max(TimeCreated) AS last_event,
       date_diff('day', min(TimeCreated), max(TimeCreated)) AS days
FROM evtxtriage_events
GROUP BY Channel ORDER BY events DESC;
```

On a real host this returned `Application` covering 964 days and `Security` covering 24
on the same machine. That is either rollover under audit volume or a clearing, and the
difference matters before you read anything into a missing logon.

**Blackout gaps — and whether they correlate.** A gap in one channel is suspicious; the
same gap in three is the machine being switched off. Always check more than one:

```sql
WITH e AS (
  SELECT Channel, TimeCreated,
         lag(TimeCreated) OVER (PARTITION BY Channel ORDER BY TimeCreated) AS prev
  FROM evtxtriage_events
  WHERE Channel IN ('Security','System','Microsoft-Windows-Sysmon/Operational')
)
SELECT Channel, prev AS gap_start, TimeCreated AS gap_end,
       date_diff('hour', prev, TimeCreated) AS gap_hours
FROM e
WHERE prev IS NOT NULL AND date_diff('hour', prev, TimeCreated) >= 24
ORDER BY gap_hours DESC;
```

**Explicit clearing.** Zero rows is a result, not a failure:

```sql
SELECT TimeCreated, Channel, EventId, MapDescription, UserName, Computer
FROM evtxtriage_events
WHERE (Channel = 'Security' AND EventId = 1102)
   OR (Channel = 'System'   AND EventId IN (104, 1100))
ORDER BY TimeCreated;
```

**Record-number discontinuities.** Catches selective record removal that leaves the time
series looking continuous:

```sql
WITH r AS (
  SELECT SourceFile, EventRecordId,
         lag(EventRecordId) OVER (PARTITION BY SourceFile ORDER BY EventRecordId) AS prev
  FROM evtxtriage_events
)
SELECT regexp_extract(SourceFile, '[^/\\]+$') AS log,
       prev AS after_record, EventRecordId AS next_record,
       EventRecordId - prev - 1 AS missing
FROM r
WHERE prev IS NOT NULL AND EventRecordId - prev > 1
ORDER BY missing DESC;
```

**Did anything stop parsing?** The `__text` idiom above, applied as a health check — a
column that suddenly fails to convert is a format change or corruption, not an empty
host:

```sql
SELECT count(*) FILTER (WHERE TimeCreated IS NULL AND TimeCreated__text <> '') AS cast_failures,
       count(*) FILTER (WHERE TimeCreated__text = '')                          AS blank
FROM evtxtriage_events;
```

### 3. What can I hunt for?

The provider and event-ID map for a channel, with the window each ID actually covers:

```sql
SELECT Provider, EventId, MapDescription, count(*) AS n,
       min(TimeCreated) AS first, max(TimeCreated) AS last
FROM evtxtriage_events
WHERE Channel = 'Security'
GROUP BY ALL ORDER BY n DESC;
```

A `NULL` `MapDescription` means no EvtxECmd map covers that event ID, so its payload
fields are not broken out — worth knowing before assuming a field is parsed.

### 4. Pivots

**Cross-artifact timeline around an anchor.** Add or drop legs as the case needs. Note
`anchor`, not `pivot`: `PIVOT` is a reserved word in DuckDB.

```sql
WITH anchor AS (SELECT TIMESTAMP '2026-02-14 15:04:07' AS t, INTERVAL 30 MINUTE AS w)
SELECT e.ts, e.src, e.what, e.who FROM (
  SELECT TimeCreated AS ts, 'EVTX' AS src,
         Channel || ' ' || EventId || ' ' || coalesce(MapDescription, '') AS what,
         coalesce(UserName, '') AS who                          FROM evtxtriage_events
  UNION ALL SELECT RunTime,   'Prefetch',   ExecutableName, ''   FROM petriage_timeline
  UNION ALL SELECT DeletedOn, 'RecycleBin', FileName, TriageUser FROM rbtriage_main
  UNION ALL SELECT UpdateTimestamp, 'USN',
         ParentPath || '\' || Name || ' [' || UpdateReasons || ']', '' FROM mftriage_usn
) e, anchor p
WHERE e.ts BETWEEN p.t - p.w AND p.t + p.w
ORDER BY e.ts;
```

**One binary across every execution source**, which is the query that turns three
artifacts into one answer:

```sql
SELECT 'Prefetch' AS src, ExecutableName AS item, RunTime AS ts
  FROM petriage_timeline WHERE lower(ExecutableName) LIKE '%<name>%'
UNION ALL SELECT 'Amcache', FullPath, FileKeyLastWriteTimestamp
  FROM amcachetriage_unassociated_file_entries WHERE lower(FullPath) LIKE '%<name>%'
UNION ALL SELECT 'AppCompat', Path, NULL
  FROM appcompattriage_appcompat WHERE lower(Path) LIKE '%<name>%'
ORDER BY ts NULLS LAST;
```

**Deletion evidence** from the journal, which records deletions the Recycle Bin never
sees:

```sql
SELECT UpdateTimestamp, ParentPath, Name, UpdateReasons
FROM mftriage_usn
WHERE UpdateReasons LIKE '%FileDelete%'
  AND UpdateTimestamp BETWEEN TIMESTAMP '<from>' AND TIMESTAMP '<to>'
ORDER BY UpdateTimestamp DESC;
```

### Querying a dataset that has no declared types

Not every dataset declares column types; the inventory query in §1 shows which do
(`typed_cols`). An undeclared dataset still gets both views, with every column VARCHAR,
so comparisons and ordering need an explicit cast. RETriage's registry plugin output is
the case an analyst hits first — its per-plugin schemas are built at runtime, so they
cannot carry a compile-time declaration:

```sql
SELECT Name, StartMode, ImagePath,
       TRY_CAST(NameKeyLastWrite AS TIMESTAMP) AS last_write
FROM retriage_services_system
WHERE TRY_CAST(NameKeyLastWrite AS TIMESTAMP) > TIMESTAMP '<from>'
ORDER BY last_write DESC;
```

Use `TRY_CAST`, never `CAST`: one malformed cell aborts the whole query with `CAST`,
while `TRY_CAST` yields `NULL` for that row and leaves the original text beside it — the
same guarantee the declared columns give you through `__text`.

### Multiple hosts

Every view carries `_triage_host`, so the cross-host form of any query above is one
`GROUP BY _triage_host` away — provided the runs share a database. Views from separate
runs use the same view names, so loading two `views.sql` files into one session makes
the second win. To compare hosts, materialize each run into tables first:

```sql
.read /run-a/duckdb/views.sql
CREATE TABLE IF NOT EXISTS t_evtx AS SELECT * FROM evtxtriage_events LIMIT 0;
INSERT INTO t_evtx BY NAME SELECT * FROM evtxtriage_events;
-- repeat for /run-b, then query t_evtx across both
```

`BY NAME` absorbs the column differences between runs. Materialize into a database whose
tables were created from a **typed** run: if a table is created all-VARCHAR first, later
typed inserts are silently down-cast to match it, with no error.

## Moving a collection: `duckdb regenerate`

`views.sql` contains absolute paths (DuckDB's `read_csv` has no notion of a base
directory), so copying or moving an output tree breaks it. `datasets.json` stores every
path relative to `out_root` for exactly this reason. After a move, re-render
`views.sql` against the new location without re-parsing any evidence:

```bash
TriageSuite duckdb regenerate --out /new/location/of/the/collection
```

This reads only `<out>/duckdb/datasets.json`, rewrites `views.sql` with absolute paths
under the new root, and writes a new matched `generation` for the pair. It runs no
parsers and touches no evidence file.
