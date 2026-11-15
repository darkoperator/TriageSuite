// The IOCs sheet (screen 09, without its verdicts, VT column, auto-extraction and Velociraptor hunt, D21): the case's
// IOC records, the type strip with each type's count, what the engine computes from their linked entries, the
// sightings found this session (`Sightings`: an IOC's own *Find sightings* or *Sweep all*), and in the editor the
// sightings panel. Grouping by tool is the tools view.
import { useCallback, useEffect, useMemo, useState, type ReactNode } from "react";
import { recordsList } from "../api";
import type { IocRecord } from "../contract/generated/IocRecord";
import type { IocType } from "../contract/generated/IocType";
import type { RecordOrder } from "../contract/generated/RecordOrder";
import type { RecordQuery } from "../contract/generated/RecordQuery";
import { useShell } from "../shell/context";
import type { TabBodyProps } from "../shell/slots";
import { displaySafe } from "../text";
import { reportFailure } from "./outcomes";
import { IOC_TYPES, iocTypeLabel } from "./RecordEditor";
import RecordSheet, { fmt, RecordStatusText, timeText, type Column, type GroupOption, type RowFilters } from "./RecordSheet";
import { sightingCell, SightingsPanel, SweepControl, useSightings } from "./Sightings";
import { useCaseRead } from "./useCaseRead";

type Ioc = { kind: "ioc" } & IocRecord;

/** A count is what one page of the engine's `MAX_LIMIT` holds; `1000+` when the page was cut short. */
const LIMIT = 1000;
const GROUPS: GroupOption<Ioc>[] = [
  { key: "tool", label: "tool", of: (r) => r.tool ?? "no tool" },
  { key: "type", label: "type", of: (r) => iocTypeLabel(r.ioc_type) },
  { key: "status", label: "status", of: (r) => r.status },
];

/** The type strip: All and every type the case holds, each with its count; a click narrows the sheet to it. */
function TypeStrip({ active, type, onType }: { active: boolean; type: IocType | null; onType(t: IocType | null): void }): ReactNode {
  const { summary, change, reportError } = useShell();
  const counts = useCaseRead<Map<IocType | "all", string>>(
    active ? JSON.stringify([summary.case_id, "ioc-types"]) : null,
    change.generation,
    async (next) => {
      const out = new Map<IocType | "all", string>();
      for (const t of [null, ...IOC_TYPES.map((x) => x.type)]) {
        const got = await recordsList({ kind: "ioc", ioc_type: t, tool: null, text: null }, LIMIT, 0, "sheet:iocs:types", next());
        out.set(t ?? "all", got.truncated ? `${fmt(LIMIT)}+` : fmt(got.items.length));
      }
      return out;
    },
    reportError,
  );
  useEffect(() => {
    if (counts?.error !== undefined) reportFailure(counts.error, reportError);
  }, [counts, reportError]);
  const got = counts?.value;
  const shown = IOC_TYPES.filter((t) => t.type === type || (got !== undefined && got.get(t.type) !== "0"));
  return (
    <div className="type-strip" role="group" aria-label="IOC types">
      <button type="button" aria-pressed={type === null} onClick={() => onType(null)}>
        {`All ${got?.get("all") ?? ""}`.trimEnd()}
      </button>
      {shown.map((t) => (
        <button key={t.type} type="button" aria-pressed={type === t.type} onClick={() => onType(t.type)}>
          {`${t.label} ${got?.get(t.type) ?? ""}`.trimEnd()}
        </button>
      ))}
    </div>
  );
}

const NOUN: [string, string] = ["IOC", "IOCs"];

export default function IocsSheet({ active }: TabBodyProps): ReactNode {
  const { change, reportError } = useShell();
  const [type, setType] = useState<IocType | null>(null);
  const store = useSightings(change.generation, reportError);
  const results = store.results;
  const columns = useMemo<Column<Ioc>[]>(
    () => [
      { header: "Value", cell: (r) => displaySafe(r.value), order: "value", filter: "text" },
      { header: "Type", cell: (r) => iocTypeLabel(r.ioc_type), order: "type" },
      { header: "Tool", cell: (r) => displaySafe(r.tool ?? ""), order: "tool", filter: "tool" },
      { header: "Entries", cell: (r) => fmt(r.entries), order: "entries", num: true },
      { header: "Hosts seen", cell: (r) => displaySafe(r.hosts_seen.join(", ")), order: "hosts_seen" },
      { header: "First seen", zoned: true, cell: (r) => timeText(r.first_seen_shown), order: "first_seen" },
      { header: "Last seen", zoned: true, cell: (r) => timeText(r.last_seen_shown), order: "last_seen" },
      // Sightings live in this session only (S5): the engine has nothing to order them by.
      { header: "Sightings", cell: (r) => sightingCell(results.get(r.id)), num: true },
      { header: "Status", cell: (r) => <RecordStatusText record={r} />, order: "status" },
    ],
    [results],
  );
  const query = useCallback(
    (f: RowFilters, order: RecordOrder | null): RecordQuery => ({ kind: "ioc", ioc_type: type, tool: f.tool, text: f.text, order }),
    [type],
  );
  const extra = useCallback((r: Ioc) => <SightingsPanel ioc={r} store={store} />, [store]);
  return (
    <RecordSheet<Ioc>
      active={active} kind="ioc" sheet="iocs" label="IOC records" noun={NOUN} columns={columns} groups={GROUPS} query={query}
      queryKey={type ?? ""} strip={<TypeStrip active={active} type={type} onType={setType} />} bar={<SweepControl store={store} />} extra={extra}
    />
  );
}
