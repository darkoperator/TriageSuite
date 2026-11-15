// The Accounts sheet (screen 10's Accounts): the case's account records, their SID and privilege, what the engine
// computes from their linked entries, and in the editor the account's logon summary (`LogonSummary`). No flag, logon
// path or verdict (D21): the logon counts are the event log's, and each opens the rows behind it.
import { useCallback, type ReactNode } from "react";
import type { AccountRecord } from "../contract/generated/AccountRecord";
import type { RecordQuery } from "../contract/generated/RecordQuery";
import type { TabBodyProps } from "../shell/slots";
import { displaySafe } from "../text";
import LogonSummaryPanel from "./LogonSummary";
import { accountText, privilegeLabel } from "./RecordEditor";
import RecordSheet, { fmt, RecordStatusText, timeText, type Column, type GroupOption, type RowFilters } from "./RecordSheet";

type Account = { kind: "account" } & AccountRecord;

const COLUMNS: Column<Account>[] = [
  { header: "Account", cell: (r) => displaySafe(accountText(r)), sort: (r) => accountText(r), filter: "text" },
  { header: "SID", cell: (r) => displaySafe(r.sid ?? ""), sort: (r) => r.sid },
  { header: "Privilege", cell: (r) => privilegeLabel(r.privilege), sort: (r) => r.privilege },
  { header: "Entries", cell: (r) => fmt(r.entries), sort: (r) => r.entries, num: true },
  { header: "Hosts seen", cell: (r) => displaySafe(r.hosts_seen.join(", ")), sort: (r) => r.hosts_seen.length, num: false },
  { header: "First seen (UTC)", cell: (r) => timeText(r.first_seen), sort: (r) => r.first_seen },
  { header: "Last seen (UTC)", cell: (r) => timeText(r.last_seen), sort: (r) => r.last_seen },
  { header: "Status", cell: (r) => <RecordStatusText record={r} />, sort: (r) => r.status },
];
const GROUPS: GroupOption<Account>[] = [
  { key: "domain", label: "domain", of: (r) => r.domain ?? "no domain" },
  { key: "privilege", label: "privilege", of: (r) => privilegeLabel(r.privilege) },
  { key: "status", label: "status", of: (r) => r.status },
];

const NOUN: [string, string] = ["account", "accounts"];

export default function AccountsSheet({ active }: TabBodyProps): ReactNode {
  const query = useCallback((f: RowFilters): RecordQuery => ({ kind: "account", text: f.text }), []);
  const extra = useCallback((r: Account) => <LogonSummaryPanel account={r} />, []);
  return (
    <RecordSheet<Account>
      active={active} kind="account" sheet="accounts" label="Account records" noun={NOUN} columns={COLUMNS} groups={GROUPS}
      query={query} extra={extra}
    />
  );
}
