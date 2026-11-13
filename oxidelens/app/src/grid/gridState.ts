// The grid's `GridState` (what the grid shows) and the pure edits the header makes to it. Later tasks add their own
// edits here (chips, search, columns, groups) as functions from state to state: the state is replaced, never mutated,
// because `useGridData` resets when it sees a new object.
import type { GridState } from "../contract/generated/GridState";
import type { SortKey } from "../contract/generated/SortKey";

export function initialGridState(relation: string): GridState {
  return {
    relation,
    chips: [],
    search: { text: "", regex: false, case_sensitive: false, mode: "filter" },
    hosts: [],
    sort: [],
    columns: null,
  };
}

/**
 * What a header click does to the sort. A plain click cycles that one column ascending, descending, off, and drops
 * the other keys; with `additive` (shift) the column joins the existing keys, or cycles within them.
 */
export function nextSort(sort: readonly SortKey[], column: string, additive: boolean): SortKey[] {
  const at = sort.findIndex((k) => k.column === column);
  if (!additive) {
    if (at < 0) return [{ column, descending: false }];
    return sort[at].descending ? [] : [{ column, descending: true }];
  }
  if (at < 0) return [...sort, { column, descending: false }];
  if (!sort[at].descending) return sort.map((k, i) => (i === at ? { column, descending: true } : k));
  return sort.filter((_, i) => i !== at);
}
