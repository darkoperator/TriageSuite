-- OxideLens case file: the upgrade from format 1 to format 2, run in the
-- same transaction as the rest of the upgrade (see case/mod.rs), so a
-- failure anywhere leaves the file at format 1.
--
-- With upgrade_2_3.sql after it, it must make exactly what schema.sql
-- makes for a new case beyond format 1; tests/case_upgrade.rs compares the
-- catalogs, comments included.

CREATE SEQUENCE ox.view_seq START 1;

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

COMMENT ON TABLE ox.meta IS 'Facts about the case file itself: name, format, created_by, created_at, case_id (32 lowercase hex digits, random, fixed for the life of the case).';
COMMENT ON COLUMN ox.history.target_kind IS 'source, relation, row_mark, row_tag, note, case or saved_view.';
