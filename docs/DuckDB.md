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
