-- OxideLens case file, format 1: the case tables in schema ox.
--
-- Every table and column carries a COMMENT, because an analyst or an agent
-- reading the case with plain SQL has nothing else to go on. Every time
-- column holds UTC text in the form YYYY-MM-DDTHH:MM:SS.ffffffZ, which sorts
-- in time order. Row ids (_ox_row) come from ox.row_seq and are unique across
-- every loaded table in the case.

CREATE SCHEMA ox;

CREATE SEQUENCE ox.row_seq START 1;
CREATE SEQUENCE ox.source_seq START 1;
CREATE SEQUENCE ox.note_seq START 1;
CREATE SEQUENCE ox.history_seq START 1;

CREATE TABLE ox.meta (
    key VARCHAR PRIMARY KEY,
    value VARCHAR NOT NULL
);
COMMENT ON TABLE ox.meta IS 'Facts about the case file itself: name, format, created_by, created_at.';
COMMENT ON COLUMN ox.meta.key IS 'The fact''s name.';
COMMENT ON COLUMN ox.meta.value IS 'The fact''s value, as text.';

CREATE TABLE ox.source (
    id BIGINT PRIMARY KEY,
    kind VARCHAR NOT NULL,
    path VARCHAR NOT NULL,
    run_id VARCHAR,
    generation VARCHAR,
    loaded_at VARCHAR NOT NULL,
    loaded_by VARCHAR NOT NULL,
    manifest_json VARCHAR,
    removed BOOLEAN NOT NULL DEFAULT false
);
COMMENT ON TABLE ox.source IS 'One row per load: a TriageSuite run or a loose file. Its data lives in schema src_<id>.';
COMMENT ON COLUMN ox.source.id IS 'Source id, from ox.source_seq; names the schema src_<id>.';
COMMENT ON COLUMN ox.source.kind IS 'run (a TriageSuite output root) or file (a loose file).';
COMMENT ON COLUMN ox.source.path IS 'The path that was loaded, made absolute with links resolved at load time.';
COMMENT ON COLUMN ox.source.run_id IS 'The run''s id from datasets.json; NULL for a loose file.';
COMMENT ON COLUMN ox.source.generation IS 'The datasets.json generation that was loaded; NULL for a loose file.';
COMMENT ON COLUMN ox.source.loaded_at IS 'When it was loaded (UTC text).';
COMMENT ON COLUMN ox.source.loaded_by IS 'Who loaded it (actor label).';
COMMENT ON COLUMN ox.source.manifest_json IS 'The text of the run''s run_manifest.json, when it had one.';
COMMENT ON COLUMN ox.source.removed IS 'True once removed: its schema is dropped, its annotations are kept.';

CREATE TABLE ox.source_table (
    source_id BIGINT NOT NULL,
    relation VARCHAR NOT NULL,
    table_schema VARCHAR NOT NULL,
    first_row BIGINT,
    last_row BIGINT,
    rows BIGINT NOT NULL
);
COMMENT ON TABLE ox.source_table IS 'One row per table a source materialized, and the row-id range it took.';
COMMENT ON COLUMN ox.source_table.source_id IS 'The ox.source this table belongs to.';
COMMENT ON COLUMN ox.source_table.relation IS 'The relation (view name) the table feeds.';
COMMENT ON COLUMN ox.source_table.table_schema IS 'The schema holding the table, src_<source id>.';
COMMENT ON COLUMN ox.source_table.first_row IS 'The smallest _ox_row in the table; NULL when it has no rows.';
COMMENT ON COLUMN ox.source_table.last_row IS 'The largest _ox_row in the table; NULL when it has no rows.';
COMMENT ON COLUMN ox.source_table.rows IS 'Row count at load time.';

CREATE TABLE ox.source_dataset (
    source_id BIGINT NOT NULL,
    relation VARCHAR NOT NULL,
    host VARCHAR,
    rows BIGINT NOT NULL,
    active BOOLEAN NOT NULL
);
COMMENT ON TABLE ox.source_dataset IS 'One row per relation and host of each source; a later run of the same host supersedes it.';
COMMENT ON COLUMN ox.source_dataset.source_id IS 'The ox.source the rows came from.';
COMMENT ON COLUMN ox.source_dataset.relation IS 'The relation the rows feed.';
COMMENT ON COLUMN ox.source_dataset.host IS 'The host the rows describe; NULL for a loose file loaded without one.';
COMMENT ON COLUMN ox.source_dataset.rows IS 'Row count for this relation and host at load time.';
COMMENT ON COLUMN ox.source_dataset.active IS 'False once superseded or removed: the rows leave the relation''s view.';

CREATE TABLE ox.relation (
    name VARCHAR PRIMARY KEY,
    kind VARCHAR NOT NULL,
    tool VARCHAR,
    primary_time VARCHAR,
    primary_time_chosen BOOLEAN NOT NULL DEFAULT false,
    evidence_path_column VARCHAR,
    host_column VARCHAR
);
COMMENT ON TABLE ox.relation IS 'One row per queryable relation: a view in main unioning every active source table.';
COMMENT ON COLUMN ox.relation.name IS 'The view name in schema main.';
COMMENT ON COLUMN ox.relation.kind IS 'dataset (from a run) or file (a loose file).';
COMMENT ON COLUMN ox.relation.tool IS 'The TriageSuite tool that wrote it; NULL for a loose file.';
COMMENT ON COLUMN ox.relation.primary_time IS 'The timestamp column the timeline uses; NULL when there is none.';
COMMENT ON COLUMN ox.relation.primary_time_chosen IS 'True once an analyst set primary_time (none included): a later load leaves it alone instead of choosing a default.';
COMMENT ON COLUMN ox.relation.evidence_path_column IS 'The column naming the evidence path, when the dataset has one.';
COMMENT ON COLUMN ox.relation.host_column IS 'The column naming the host, when there is one.';

CREATE TABLE ox.column_catalog (
    relation VARCHAR NOT NULL,
    name VARCHAR NOT NULL,
    ordinal INTEGER NOT NULL,
    text_column VARCHAR NOT NULL,
    typed_column VARCHAR,
    sql_type VARCHAR,
    time_semantics VARCHAR,
    origin VARCHAR NOT NULL,
    PRIMARY KEY (relation, name)
);
COMMENT ON TABLE ox.column_catalog IS 'One row per logical column of each relation: where its text is and where its typed value is.';
COMMENT ON COLUMN ox.column_catalog.relation IS 'The relation the column belongs to.';
COMMENT ON COLUMN ox.column_catalog.name IS 'The column''s name as the analyst sees it.';
COMMENT ON COLUMN ox.column_catalog.ordinal IS 'Display position, from 1.';
COMMENT ON COLUMN ox.column_catalog.text_column IS 'The physical column holding the original cell text.';
COMMENT ON COLUMN ox.column_catalog.typed_column IS 'The physical column holding the typed value; NULL when untyped.';
COMMENT ON COLUMN ox.column_catalog.sql_type IS 'The typed column''s SQL type; NULL when untyped.';
COMMENT ON COLUMN ox.column_catalog.time_semantics IS 'utc, local or unknown for a timestamp; NULL otherwise.';
COMMENT ON COLUMN ox.column_catalog.origin IS 'Who typed it: suite, app-map, sniffed or analyst; text when untyped.';

CREATE TABLE ox.inventory_dataset (
    source_id BIGINT NOT NULL,
    view VARCHAR NOT NULL,
    raw_view VARCHAR NOT NULL,
    tool VARCHAR NOT NULL,
    dataset_id VARCHAR NOT NULL,
    source VARCHAR NOT NULL,
    evidence_path_column VARCHAR,
    metadata_columns_json VARCHAR NOT NULL,
    columns_json VARCHAR NOT NULL,
    effective_types_json VARCHAR NOT NULL
);
COMMENT ON TABLE ox.inventory_dataset IS 'One row per dataset of each loaded run, as its datasets.json described it.';
COMMENT ON COLUMN ox.inventory_dataset.source_id IS 'The ox.source (run) this inventory came from.';
COMMENT ON COLUMN ox.inventory_dataset.view IS 'The dataset''s typed view name.';
COMMENT ON COLUMN ox.inventory_dataset.raw_view IS 'The dataset''s all-text view name.';
COMMENT ON COLUMN ox.inventory_dataset.tool IS 'The TriageSuite tool that wrote it.';
COMMENT ON COLUMN ox.inventory_dataset.dataset_id IS 'The tool''s dataset id.';
COMMENT ON COLUMN ox.inventory_dataset.source IS 'internal (a suite parser) or external (a third-party tool).';
COMMENT ON COLUMN ox.inventory_dataset.evidence_path_column IS 'The column naming the evidence path, when the dataset has one.';
COMMENT ON COLUMN ox.inventory_dataset.metadata_columns_json IS 'The injected metadata column names, as JSON.';
COMMENT ON COLUMN ox.inventory_dataset.columns_json IS 'The union of the included files'' headers, as a JSON array.';
COMMENT ON COLUMN ox.inventory_dataset.effective_types_json IS 'The suite''s declared column types, as a JSON object.';

CREATE TABLE ox.inventory_file (
    source_id BIGINT NOT NULL,
    view VARCHAR NOT NULL,
    path VARCHAR NOT NULL,
    host VARCHAR NOT NULL,
    identity VARCHAR,
    role VARCHAR NOT NULL,
    included_in_view BOOLEAN NOT NULL,
    derived_into VARCHAR,
    header_json VARCHAR,
    warnings_json VARCHAR NOT NULL
);
COMMENT ON TABLE ox.inventory_file IS 'One row per file of each dataset of each loaded run.';
COMMENT ON COLUMN ox.inventory_file.source_id IS 'The ox.source (run) this inventory came from.';
COMMENT ON COLUMN ox.inventory_file.view IS 'The dataset (typed view name) the file belongs to.';
COMMENT ON COLUMN ox.inventory_file.path IS 'The file''s path relative to the run''s output root.';
COMMENT ON COLUMN ox.inventory_file.host IS 'The host the file describes.';
COMMENT ON COLUMN ox.inventory_file.identity IS 'user:<name>, system or unknown; NULL for a merged file.';
COMMENT ON COLUMN ox.inventory_file.role IS 'primary, merged or slice.';
COMMENT ON COLUMN ox.inventory_file.included_in_view IS 'Whether the file''s rows were loaded.';
COMMENT ON COLUMN ox.inventory_file.derived_into IS 'For a slice: the merged file it was folded into.';
COMMENT ON COLUMN ox.inventory_file.header_json IS 'The file''s own header as a JSON array; NULL when it could not be read.';
COMMENT ON COLUMN ox.inventory_file.warnings_json IS 'The file''s warnings, as a JSON array.';

CREATE TABLE ox.inventory_note (
    source_id BIGINT NOT NULL,
    kind VARCHAR NOT NULL,
    view VARCHAR,
    dataset_id VARCHAR,
    detail_json VARCHAR NOT NULL
);
COMMENT ON TABLE ox.inventory_note IS 'One row per run warning, dropped type override and inventory-only file of each loaded run.';
COMMENT ON COLUMN ox.inventory_note.source_id IS 'The ox.source (run) this inventory came from.';
COMMENT ON COLUMN ox.inventory_note.kind IS 'warning, dropped_override or inventory_only.';
COMMENT ON COLUMN ox.inventory_note.view IS 'The view it concerns, when it concerns one.';
COMMENT ON COLUMN ox.inventory_note.dataset_id IS 'The dataset id it concerns, when it concerns one.';
COMMENT ON COLUMN ox.inventory_note.detail_json IS 'The entry exactly as datasets.json held it, as JSON.';

CREATE TABLE ox.row_mark (
    row_id BIGINT NOT NULL,
    mark VARCHAR NOT NULL,
    marked_by VARCHAR NOT NULL,
    marked_at VARCHAR NOT NULL
);
COMMENT ON TABLE ox.row_mark IS 'At most one mark per row (_ox_row).';
COMMENT ON COLUMN ox.row_mark.row_id IS 'The marked row''s _ox_row.';
COMMENT ON COLUMN ox.row_mark.mark IS 'The mark, for example suspicious.';
COMMENT ON COLUMN ox.row_mark.marked_by IS 'Who set the mark (actor label).';
COMMENT ON COLUMN ox.row_mark.marked_at IS 'When the mark was set (UTC text).';

CREATE TABLE ox.tag (
    name VARCHAR PRIMARY KEY,
    created_by VARCHAR NOT NULL,
    created_at VARCHAR NOT NULL
);
COMMENT ON TABLE ox.tag IS 'The case''s tags.';
COMMENT ON COLUMN ox.tag.name IS 'The tag''s name.';
COMMENT ON COLUMN ox.tag.created_by IS 'Who created the tag (actor label).';
COMMENT ON COLUMN ox.tag.created_at IS 'When the tag was created (UTC text).';

CREATE TABLE ox.row_tag (
    row_id BIGINT NOT NULL,
    tag VARCHAR NOT NULL,
    tagged_by VARCHAR NOT NULL,
    tagged_at VARCHAR NOT NULL
);
COMMENT ON TABLE ox.row_tag IS 'One row per tag on a row.';
COMMENT ON COLUMN ox.row_tag.row_id IS 'The tagged row''s _ox_row.';
COMMENT ON COLUMN ox.row_tag.tag IS 'The ox.tag name.';
COMMENT ON COLUMN ox.row_tag.tagged_by IS 'Who tagged the row (actor label).';
COMMENT ON COLUMN ox.row_tag.tagged_at IS 'When the row was tagged (UTC text).';

CREATE TABLE ox.note (
    id BIGINT PRIMARY KEY,
    row_id BIGINT NOT NULL,
    text VARCHAR NOT NULL,
    written_by VARCHAR NOT NULL,
    written_at VARCHAR NOT NULL,
    deleted BOOLEAN NOT NULL DEFAULT false
);
COMMENT ON TABLE ox.note IS 'Notes on rows. A deleted note is kept and flagged, never removed.';
COMMENT ON COLUMN ox.note.id IS 'Note id, from ox.note_seq.';
COMMENT ON COLUMN ox.note.row_id IS 'The row''s _ox_row.';
COMMENT ON COLUMN ox.note.text IS 'The note''s text.';
COMMENT ON COLUMN ox.note.written_by IS 'Who wrote it (actor label).';
COMMENT ON COLUMN ox.note.written_at IS 'When it was written (UTC text).';
COMMENT ON COLUMN ox.note.deleted IS 'True once deleted.';

-- "at" and "by" are reserved words in DuckDB's grammar: every statement that
-- names these two columns must quote them.
CREATE TABLE ox.history (
    id BIGINT PRIMARY KEY,
    "at" VARCHAR NOT NULL,
    "by" VARCHAR NOT NULL,
    target_kind VARCHAR NOT NULL,
    target_id VARCHAR NOT NULL,
    action VARCHAR NOT NULL,
    before_json VARCHAR,
    after_json VARCHAR
);
COMMENT ON TABLE ox.history IS 'Append-only log of every change to the case; nothing updates or deletes it.';
COMMENT ON COLUMN ox.history.id IS 'Entry id, from ox.history_seq; increases with time.';
COMMENT ON COLUMN ox.history."at" IS 'When the change was made (UTC text).';
COMMENT ON COLUMN ox.history."by" IS 'Who made it (actor label).';
COMMENT ON COLUMN ox.history.target_kind IS 'source, relation, row_mark, row_tag or note.';
COMMENT ON COLUMN ox.history.target_id IS 'The changed object''s id, as text.';
COMMENT ON COLUMN ox.history.action IS 'What was done, for example source_load or mark_set.';
COMMENT ON COLUMN ox.history.before_json IS 'The object before the change, as JSON; NULL when it did not exist.';
COMMENT ON COLUMN ox.history.after_json IS 'The object after the change, as JSON; NULL when it no longer exists.';
