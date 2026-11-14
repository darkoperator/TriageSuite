-- OxideLens case file, format 3: the case tables in schema ox. An older
-- case is brought to this in one transaction: format 1 (step 1) by
-- upgrade_1_2.sql, then format 2 (step 2) by upgrade_2_3.sql, which must
-- make the same tables; tests/case_upgrade.rs compares the catalogs.
--
-- Every table, view and column carries a COMMENT, because an analyst or an
-- agent reading the case with plain SQL has nothing else to go on. Every
-- time column holds UTC text in the form YYYY-MM-DDTHH:MM:SS.ffffffZ, which
-- sorts in time order. Row ids (_ox_row) come from ox.row_seq and are
-- unique across every loaded table in the case.

CREATE SCHEMA ox;

CREATE SEQUENCE ox.row_seq START 1;
CREATE SEQUENCE ox.source_seq START 1;
CREATE SEQUENCE ox.note_seq START 1;
CREATE SEQUENCE ox.history_seq START 1;
CREATE SEQUENCE ox.view_seq START 1;

CREATE TABLE ox.meta (
    key VARCHAR PRIMARY KEY,
    value VARCHAR NOT NULL
);
COMMENT ON TABLE ox.meta IS 'Facts about the case file itself: name, format, created_by, created_at, case_id (32 lowercase hex digits, random, fixed for the life of the case).';
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
    role VARCHAR NOT NULL,
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
COMMENT ON COLUMN ox.column_catalog.role IS 'What the column holds: data (the source''s own), or the metadata column run_id, host, identity or output_file. Search reads data and host columns only.';

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
COMMENT ON COLUMN ox.inventory_file.included_in_view IS 'Whether the suite included the file in its view. The case loaded it unless the load''s history row lists its dataset and host under skipped.';
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
COMMENT ON TABLE ox.row_mark IS 'At most one mark per row (_ox_row). The engine enforces it, not a constraint: supersession repoints row_id, so a mark is only inserted where none exists.';
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
COMMENT ON COLUMN ox.history.target_kind IS 'source, relation, row_mark, row_tag, note, case, saved_view, entry, evidence, host, account, ioc or link.';
COMMENT ON COLUMN ox.history.target_id IS 'The changed object''s id, as text.';
COMMENT ON COLUMN ox.history.action IS 'What was done, for example source_load or mark_set.';
COMMENT ON COLUMN ox.history.before_json IS 'The object before the change, as JSON; NULL when it did not exist.';
COMMENT ON COLUMN ox.history.after_json IS 'The object after the change, as JSON; NULL when it no longer exists.';

CREATE TABLE ox.saved_view (
    id BIGINT PRIMARY KEY,
    name VARCHAR NOT NULL,
    kind VARCHAR NOT NULL,
    pattern VARCHAR NOT NULL,
    doc_json VARCHAR NOT NULL,
    created_by VARCHAR NOT NULL,
    created_at VARCHAR NOT NULL,
    updated_by VARCHAR NOT NULL,
    updated_at VARCHAR NOT NULL,
    deleted BOOLEAN NOT NULL DEFAULT false
);
COMMENT ON TABLE ox.saved_view IS 'Saved grid views and filter presets. A deleted view is kept and flagged, never removed.';
COMMENT ON COLUMN ox.saved_view.id IS 'View id, from ox.view_seq.';
COMMENT ON COLUMN ox.saved_view.name IS 'The view''s name; one name per kind.';
COMMENT ON COLUMN ox.saved_view.kind IS 'view (a whole grid state) or preset (filter chips only).';
COMMENT ON COLUMN ox.saved_view.pattern IS 'The relations it applies to: a relation name, where * stands for any run of characters.';
COMMENT ON COLUMN ox.saved_view.doc_json IS 'What the view holds (chips, search, hosts, grouping, sort, layout, colour rules), as JSON.';
COMMENT ON COLUMN ox.saved_view.created_by IS 'Who saved it first (actor label).';
COMMENT ON COLUMN ox.saved_view.created_at IS 'When it was saved first (UTC text).';
COMMENT ON COLUMN ox.saved_view.updated_by IS 'Who saved it last (actor label).';
COMMENT ON COLUMN ox.saved_view.updated_at IS 'When it was saved last (UTC text).';
COMMENT ON COLUMN ox.saved_view.deleted IS 'True once deleted.';

CREATE TABLE ox.run_session (
    source_id BIGINT NOT NULL,
    name VARCHAR NOT NULL,
    files_json VARCHAR NOT NULL
);
COMMENT ON TABLE ox.run_session IS 'One row per Timeline Explorer session name a loaded run carried, read when it was loaded: the <name>.tle_sess files of that name in the run''s Sessions folders (the root''s and each Processed-* host folder''s), merged.';
COMMENT ON COLUMN ox.run_session.source_id IS 'The ox.source (run) the session files came with.';
COMMENT ON COLUMN ox.run_session.name IS 'The session''s name: its files'' name without .tle_sess.';
COMMENT ON COLUMN ox.run_session.files_json IS 'The paths the session''s files name, as written (absolute, where the run was written), each once, in the order read, as a JSON array.';

CREATE TABLE ox.setting (
    key VARCHAR PRIMARY KEY,
    value VARCHAR NOT NULL,
    set_by VARCHAR NOT NULL,
    set_at VARCHAR NOT NULL
);
COMMENT ON TABLE ox.setting IS 'Settings of the case itself, for example display_zone (the IANA zone the case''s times are shown in; UTC when absent).';
COMMENT ON COLUMN ox.setting.key IS 'The setting''s name.';
COMMENT ON COLUMN ox.setting.value IS 'The setting''s value, as text.';
COMMENT ON COLUMN ox.setting.set_by IS 'Who set it last (actor label).';
COMMENT ON COLUMN ox.setting.set_at IS 'When it was set last (UTC text).';

CREATE SEQUENCE ox.entry_seq START 1;
CREATE SEQUENCE ox.host_seq START 1;
CREATE SEQUENCE ox.account_seq START 1;
CREATE SEQUENCE ox.ioc_seq START 1;

-- "time" is a type name in DuckDB's grammar: every statement that names
-- ox.entry's time column quotes it.
CREATE TABLE ox.entry (
    id BIGINT PRIMARY KEY,
    "time" VARCHAR,
    time_source VARCHAR,
    title VARCHAR NOT NULL,
    description VARCHAR NOT NULL,
    tactic VARCHAR,
    technique_ids VARCHAR NOT NULL,
    status VARCHAR NOT NULL,
    created_by VARCHAR NOT NULL,
    created_at VARCHAR NOT NULL,
    modified_by VARCHAR NOT NULL,
    modified_at VARCHAR NOT NULL,
    reviewed_by VARCHAR,
    reviewed_at VARCHAR,
    deleted BOOLEAN NOT NULL DEFAULT false
);
COMMENT ON TABLE ox.entry IS 'Timeline entries, shown as E-<id>. A deleted entry is kept and flagged, never removed; deleting it removes its links to records and keeps its evidence, which then marks no row.';
COMMENT ON COLUMN ox.entry.id IS 'Entry id, from ox.entry_seq.';
COMMENT ON COLUMN ox.entry."time" IS 'When it happened (UTC text, full precision); NULL when no time is known yet, which sorts the entry into the undated group and keeps it from being reviewed.';
COMMENT ON COLUMN ox.entry.time_source IS 'Where time came from: row (an evidence row''s primary time, a UTC instant) or typed (entered by hand); NULL when time is NULL.';
COMMENT ON COLUMN ox.entry.title IS 'The entry''s title.';
COMMENT ON COLUMN ox.entry.description IS 'The entry''s description; empty when there is none.';
COMMENT ON COLUMN ox.entry.tactic IS 'The ATT&CK tactic; NULL when none is set.';
COMMENT ON COLUMN ox.entry.technique_ids IS 'The ATT&CK technique ids (T1234 or T1234.001), as a JSON array of text.';
COMMENT ON COLUMN ox.entry.status IS 'draft, unreviewed (everything an agent creates) or reviewed (only an analyst reviews).';
COMMENT ON COLUMN ox.entry.created_by IS 'Who created it (actor label); an agent''s label stays after review.';
COMMENT ON COLUMN ox.entry.created_at IS 'When it was created (UTC text).';
COMMENT ON COLUMN ox.entry.modified_by IS 'Who changed it last (actor label).';
COMMENT ON COLUMN ox.entry.modified_at IS 'When it was changed last (UTC text).';
COMMENT ON COLUMN ox.entry.reviewed_by IS 'The analyst who reviewed it (actor label); NULL until reviewed.';
COMMENT ON COLUMN ox.entry.reviewed_at IS 'When it was reviewed (UTC text); NULL until reviewed.';
COMMENT ON COLUMN ox.entry.deleted IS 'True once deleted.';

CREATE TABLE ox.evidence (
    entry_id BIGINT NOT NULL,
    row_id BIGINT NOT NULL,
    relation VARCHAR NOT NULL,
    host VARCHAR,
    row_hash VARCHAR NOT NULL,
    snapshot_json VARCHAR NOT NULL,
    attached_by VARCHAR NOT NULL,
    attached_at VARCHAR NOT NULL
);
COMMENT ON TABLE ox.evidence IS 'Rows attached to entries as evidence, at most once per entry and row. The engine enforces it, not a constraint: supersession repoints row_id. The snapshot is kept when the row''s source is superseded or removed.';
COMMENT ON COLUMN ox.evidence.entry_id IS 'The ox.entry the row is evidence for.';
COMMENT ON COLUMN ox.evidence.row_id IS 'The row''s _ox_row; after a newer run, the identical row''s (same row_hash), or the old row''s when none matches.';
COMMENT ON COLUMN ox.evidence.relation IS 'The relation the row was attached from.';
COMMENT ON COLUMN ox.evidence.host IS 'The host the row describes; NULL for a row with no host.';
COMMENT ON COLUMN ox.evidence.row_hash IS 'The row''s hash when it was attached, which re-check compares and supersession matches on.';
COMMENT ON COLUMN ox.evidence.snapshot_json IS 'The row''s cells as text when it was attached, as a JSON object of column name to text.';
COMMENT ON COLUMN ox.evidence.attached_by IS 'Who attached it (actor label).';
COMMENT ON COLUMN ox.evidence.attached_at IS 'When it was attached (UTC text).';

CREATE TABLE ox.host (
    id BIGINT PRIMARY KEY,
    name VARCHAR NOT NULL,
    role VARCHAR NOT NULL,
    note VARCHAR NOT NULL,
    status VARCHAR NOT NULL,
    created_by VARCHAR NOT NULL,
    created_at VARCHAR NOT NULL,
    modified_by VARCHAR NOT NULL,
    modified_at VARCHAR NOT NULL,
    deleted BOOLEAN NOT NULL DEFAULT false
);
COMMENT ON TABLE ox.host IS 'Host records, shown as H-<id>. A deleted record is kept and flagged, never removed; deleting it removes its links.';
COMMENT ON COLUMN ox.host.id IS 'Host id, from ox.host_seq.';
COMMENT ON COLUMN ox.host.name IS 'The host''s name, as the data names it.';
COMMENT ON COLUMN ox.host.role IS 'What the host is, for example domain controller; empty when not set.';
COMMENT ON COLUMN ox.host.note IS 'The analyst''s or agent''s note; empty when there is none.';
COMMENT ON COLUMN ox.host.status IS 'unreviewed (everything an agent creates) or reviewed.';
COMMENT ON COLUMN ox.host.created_by IS 'Who created it (actor label); an agent''s label stays after review.';
COMMENT ON COLUMN ox.host.created_at IS 'When it was created (UTC text).';
COMMENT ON COLUMN ox.host.modified_by IS 'Who changed it last (actor label).';
COMMENT ON COLUMN ox.host.modified_at IS 'When it was changed last (UTC text).';
COMMENT ON COLUMN ox.host.deleted IS 'True once deleted.';

CREATE TABLE ox.account (
    id BIGINT PRIMARY KEY,
    name VARCHAR NOT NULL,
    domain VARCHAR,
    sid VARCHAR,
    description VARCHAR NOT NULL,
    privilege VARCHAR NOT NULL,
    note VARCHAR NOT NULL,
    status VARCHAR NOT NULL,
    created_by VARCHAR NOT NULL,
    created_at VARCHAR NOT NULL,
    modified_by VARCHAR NOT NULL,
    modified_at VARCHAR NOT NULL,
    deleted BOOLEAN NOT NULL DEFAULT false
);
COMMENT ON TABLE ox.account IS 'Account records, shown as A-<id>. A deleted record is kept and flagged, never removed; deleting it removes its links.';
COMMENT ON COLUMN ox.account.id IS 'Account id, from ox.account_seq.';
COMMENT ON COLUMN ox.account.name IS 'The account''s name, without its domain.';
COMMENT ON COLUMN ox.account.domain IS 'The account''s domain; NULL for a local account or one whose domain is not known.';
COMMENT ON COLUMN ox.account.sid IS 'The account''s security identifier; NULL when not known.';
COMMENT ON COLUMN ox.account.description IS 'What the account is for; empty when not set.';
COMMENT ON COLUMN ox.account.privilege IS 'standard, local_admin, domain_admin, service or unknown.';
COMMENT ON COLUMN ox.account.note IS 'The analyst''s or agent''s note; empty when there is none.';
COMMENT ON COLUMN ox.account.status IS 'unreviewed (everything an agent creates) or reviewed.';
COMMENT ON COLUMN ox.account.created_by IS 'Who created it (actor label); an agent''s label stays after review.';
COMMENT ON COLUMN ox.account.created_at IS 'When it was created (UTC text).';
COMMENT ON COLUMN ox.account.modified_by IS 'Who changed it last (actor label).';
COMMENT ON COLUMN ox.account.modified_at IS 'When it was changed last (UTC text).';
COMMENT ON COLUMN ox.account.deleted IS 'True once deleted.';

CREATE TABLE ox.ioc (
    id BIGINT PRIMARY KEY,
    type VARCHAR NOT NULL,
    value VARCHAR NOT NULL,
    tool VARCHAR,
    attack_ids VARCHAR NOT NULL,
    note VARCHAR NOT NULL,
    status VARCHAR NOT NULL,
    created_by VARCHAR NOT NULL,
    created_at VARCHAR NOT NULL,
    modified_by VARCHAR NOT NULL,
    modified_at VARCHAR NOT NULL,
    deleted BOOLEAN NOT NULL DEFAULT false
);
COMMENT ON TABLE ox.ioc IS 'Indicator of compromise records, shown as I-<id>; type and value are unique among live ones. A deleted record is kept and flagged, never removed; deleting it removes its links.';
COMMENT ON COLUMN ox.ioc.id IS 'IOC id, from ox.ioc_seq.';
COMMENT ON COLUMN ox.ioc.type IS 'ip, domain, hostname, url, email, md5, sha1, sha256, file_name, file_path, registry_key, service, scheduled_task or other.';
COMMENT ON COLUMN ox.ioc.value IS 'The indicator, validated for its type and stored in canonical form (an ip canonical, a domain, hostname or hash lower-case).';
COMMENT ON COLUMN ox.ioc.tool IS 'The tool the indicator belongs to; NULL when none.';
COMMENT ON COLUMN ox.ioc.attack_ids IS 'The ATT&CK ids (T1234 or T1234.001), as a JSON array of text.';
COMMENT ON COLUMN ox.ioc.note IS 'The analyst''s or agent''s note; empty when there is none.';
COMMENT ON COLUMN ox.ioc.status IS 'unreviewed (everything an agent creates) or reviewed.';
COMMENT ON COLUMN ox.ioc.created_by IS 'Who created it (actor label); an agent''s label stays after review.';
COMMENT ON COLUMN ox.ioc.created_at IS 'When it was created (UTC text).';
COMMENT ON COLUMN ox.ioc.modified_by IS 'Who changed it last (actor label).';
COMMENT ON COLUMN ox.ioc.modified_at IS 'When it was changed last (UTC text).';
COMMENT ON COLUMN ox.ioc.deleted IS 'True once deleted.';

CREATE TABLE ox.entry_host (
    entry_id BIGINT NOT NULL,
    record_id BIGINT NOT NULL,
    linked_by VARCHAR NOT NULL,
    linked_at VARCHAR NOT NULL
);
COMMENT ON TABLE ox.entry_host IS 'Links from entries to host records, at most one per pair. The engine enforces it, not a constraint: a merge repoints record_id. Unlinking removes the row.';
COMMENT ON COLUMN ox.entry_host.entry_id IS 'The linked ox.entry.';
COMMENT ON COLUMN ox.entry_host.record_id IS 'The linked ox.host.';
COMMENT ON COLUMN ox.entry_host.linked_by IS 'Who linked them (actor label).';
COMMENT ON COLUMN ox.entry_host.linked_at IS 'When they were linked (UTC text).';

CREATE TABLE ox.entry_account (
    entry_id BIGINT NOT NULL,
    record_id BIGINT NOT NULL,
    linked_by VARCHAR NOT NULL,
    linked_at VARCHAR NOT NULL
);
COMMENT ON TABLE ox.entry_account IS 'Links from entries to account records, at most one per pair. The engine enforces it, not a constraint: a merge repoints record_id. Unlinking removes the row.';
COMMENT ON COLUMN ox.entry_account.entry_id IS 'The linked ox.entry.';
COMMENT ON COLUMN ox.entry_account.record_id IS 'The linked ox.account.';
COMMENT ON COLUMN ox.entry_account.linked_by IS 'Who linked them (actor label).';
COMMENT ON COLUMN ox.entry_account.linked_at IS 'When they were linked (UTC text).';

CREATE TABLE ox.entry_ioc (
    entry_id BIGINT NOT NULL,
    record_id BIGINT NOT NULL,
    linked_by VARCHAR NOT NULL,
    linked_at VARCHAR NOT NULL
);
COMMENT ON TABLE ox.entry_ioc IS 'Links from entries to IOC records, at most one per pair. The engine enforces it, not a constraint: a merge repoints record_id. Unlinking removes the row.';
COMMENT ON COLUMN ox.entry_ioc.entry_id IS 'The linked ox.entry.';
COMMENT ON COLUMN ox.entry_ioc.record_id IS 'The linked ox.ioc.';
COMMENT ON COLUMN ox.entry_ioc.linked_by IS 'Who linked them (actor label).';
COMMENT ON COLUMN ox.entry_ioc.linked_at IS 'When they were linked (UTC text).';

-- Ruling S2: one row per marked row, the most severe state winning. An
-- entry is an analyst's when an analyst created it (its creator is not an
-- agent:<name> label) or it is reviewed, which only an analyst does.
CREATE VIEW ox.row_state AS
WITH live_evidence AS (
    SELECT v.row_id,
           e.id AS entry_id,
           e.created_by,
           NOT starts_with(e.created_by, 'agent:') OR e.status = 'reviewed' AS analysts
    FROM ox.evidence AS v
    JOIN ox.entry AS e ON e.id = v.entry_id
    WHERE NOT e.deleted
),
evidence_state AS (
    SELECT row_id,
           bool_or(analysts) AS analysts,
           arg_min(created_by, entry_id) FILTER (WHERE analysts) AS analyst_by,
           arg_min(created_by, entry_id) AS agent_by
    FROM live_evidence
    GROUP BY row_id
)
SELECT row_id,
       CASE WHEN analysts THEN 'malicious' ELSE 'agent_malicious' END AS mark,
       CASE WHEN analysts THEN analyst_by ELSE agent_by END AS "by"
FROM evidence_state
UNION ALL
SELECT m.row_id, 'suspicious' AS mark, m.marked_by AS "by"
FROM ox.row_mark AS m
WHERE m.mark = 'suspicious'
  AND NOT EXISTS (SELECT 1 FROM evidence_state AS s WHERE s.row_id = m.row_id);
COMMENT ON VIEW ox.row_state IS 'One row per marked row: malicious when it is evidence on a live entry an analyst created or reviewed; else agent_malicious when it is evidence only on live unreviewed agent entries; else suspicious from ox.row_mark.';
COMMENT ON COLUMN ox.row_state.row_id IS 'The marked row''s _ox_row.';
COMMENT ON COLUMN ox.row_state.mark IS 'malicious, agent_malicious or suspicious.';
COMMENT ON COLUMN ox.row_state."by" IS 'Who the mark is from (actor label): the creator of the lowest-numbered entry that gives it, or the setter of the suspicious mark.';
