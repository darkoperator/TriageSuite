// The Hosts sheet (screen 10's Hosts, as a grid): the case's host records, their role, and what the engine computes
// from their linked entries. Hosts follow the evidence (ruling S7): attaching a row from a host links it, making the
// record when the case has none. No card, badge or activity verdict: only what the analyst or an agent typed.
import { useCallback, type ReactNode } from "react";
import type { HostRecord } from "../contract/generated/HostRecord";
import type { RecordQuery } from "../contract/generated/RecordQuery";
import type { TabBodyProps } from "../shell/slots";
import { displaySafe } from "../text";
import RecordSheet, { fmt, RecordStatusText, timeText, type Column, type GroupOption, type RowFilters } from "./RecordSheet";

type Host = { kind: "host" } & HostRecord;

const COLUMNS: Column<Host>[] = [
  { header: "Name", cell: (r) => displaySafe(r.name), sort: (r) => r.name, filter: "text" },
  { header: "Role", cell: (r) => displaySafe(r.role), sort: (r) => r.role },
  { header: "Entries", cell: (r) => fmt(r.entries), sort: (r) => r.entries, num: true },
  { header: "First seen (UTC)", cell: (r) => timeText(r.first_seen), sort: (r) => r.first_seen },
  { header: "Last seen (UTC)", cell: (r) => timeText(r.last_seen), sort: (r) => r.last_seen },
  { header: "Status", cell: (r) => <RecordStatusText record={r} />, sort: (r) => r.status },
];
const GROUPS: GroupOption<Host>[] = [
  { key: "role", label: "role", of: (r) => (r.role === "" ? "no role" : r.role) },
  { key: "status", label: "status", of: (r) => r.status },
];

const NOUN: [string, string] = ["host", "hosts"];

export default function HostsSheet({ active }: TabBodyProps): ReactNode {
  const query = useCallback((f: RowFilters): RecordQuery => ({ kind: "host", text: f.text }), []);
  return (
    <RecordSheet<Host>
      active={active} kind="host" sheet="hosts" label="Host records" noun={NOUN} columns={COLUMNS} groups={GROUPS} query={query}
    />
  );
}
