-- OxideLens case file: the upgrade from format 2 to format 3, run in the
-- same transaction as the rest of the upgrade (see case/mod.rs), after
-- upgrade_1_2.sql for a format-1 case, so a failure anywhere leaves the
-- file at the format it had.
--
-- It must make exactly what schema.sql makes for a new case beyond format 2;
-- tests/case_upgrade.rs compares the two catalogs, comments and view
-- definitions included.

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
-- The stored mark as it is, not a constant: the engine writes only
-- suspicious, so any other value is a damaged case, which the readers
-- report instead of showing the row unmarked.
SELECT m.row_id, m.mark, m.marked_by AS "by"
FROM ox.row_mark AS m
WHERE NOT EXISTS (SELECT 1 FROM evidence_state AS s WHERE s.row_id = m.row_id);
COMMENT ON VIEW ox.row_state IS 'One row per marked row: malicious when it is evidence on a live entry an analyst created or reviewed; else agent_malicious when it is evidence only on live unreviewed agent entries; else the stored mark from ox.row_mark (suspicious; any other value is a damaged case).';
COMMENT ON COLUMN ox.row_state.row_id IS 'The marked row''s _ox_row.';
COMMENT ON COLUMN ox.row_state.mark IS 'malicious, agent_malicious or suspicious.';
COMMENT ON COLUMN ox.row_state."by" IS 'Who the mark is from (actor label): the creator of the lowest-numbered entry that gives it, or the setter of the suspicious mark.';

COMMENT ON COLUMN ox.history.target_kind IS 'source, relation, row_mark, row_tag, note, case, saved_view, entry, evidence, host, account, ioc or link.';
