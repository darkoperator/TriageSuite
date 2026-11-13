// A session (a Timeline Explorer session file of a loaded run) as tabs: one dataset tab per relation it feeds, scoped to
// the hosts the session names for that relation. The tabs are ordinary scoped tabs (a seed), saved with the case's tabs.
import type { Session } from "../contract/generated/Session";
import { hash } from "../grid/chips";
import { initialGridState } from "../grid/gridState";
import { stableKey } from "../grid/useGridData";
import type { Tab } from "../shell/tabs";

const clip = (t: string) => (t.length > 60 ? t.slice(0, 59) + "…" : t);

export function sessionTabs(s: Session): Tab[] {
  const hostsByRelation = new Map<string, string[]>();
  for (const r of s.relations) {
    const hosts = hostsByRelation.get(r.relation) ?? [];
    if (!hosts.includes(r.host)) hosts.push(r.host);
    hostsByRelation.set(r.relation, hosts);
  }
  return [...hostsByRelation].map(([relation, hosts]) => {
    const seed = { state: { ...initialGridState(relation), hosts }, by: [] as string[] };
    return {
      id: `session:${hash(stableKey([s.source_id, s.name, relation, hosts]))}`,
      kind: "dataset" as const,
      relation,
      title: `${clip(s.name)} · ${relation}`,
      seed,
    };
  });
}
