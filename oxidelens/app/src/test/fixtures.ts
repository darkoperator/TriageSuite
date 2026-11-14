// Typed access to the recorded command answers (fixtures/*.json, recorded by `cargo test -p oxl-app --test
// record_fixtures -- --ignored` from a synthetic two-host case). Every call returns a fresh deep copy, safe to edit
// into a variant.
//
// The shape check is the compiler's, not a hand-kept key list: `copy<G>(json)` only accepts a JSON module whose
// inferred type (every element of every array included) is assignable to the generated type `G` with its string
// literals widened to `string` (JSON modules infer `string`, never a literal). A field the generated type requires
// and some fixture lacks, anywhere, fails `tsc` (so `npm run build`, which type-checks the test files too). Run the
// recorder again after a contract change.
import type { CaseSummary } from "../contract/generated/CaseSummary";
import type { GridCounts } from "../contract/generated/GridCounts";
import type { GridPage } from "../contract/generated/GridPage";
import type { GroupView } from "../contract/generated/GroupView";
import type { Histogram } from "../contract/generated/Histogram";
import type { HostZone } from "../contract/generated/HostZone";
import type { Listing } from "../contract/generated/Listing";
import type { RowView } from "../contract/generated/RowView";
import type { SavedView } from "../contract/generated/SavedView";
import type { Session } from "../contract/generated/Session";
import type { ValueCount } from "../contract/generated/ValueCount";
import type { ViewListing } from "../contract/generated/ViewListing";
import caseSummaryJson from "./fixtures/case_summary.json";
import gridCountsEventsJson from "./fixtures/grid_counts_events.json";
import gridGroupsChannelJson from "./fixtures/grid_groups_channel.json";
import gridHistogramEventsJson from "./fixtures/grid_histogram_events.json";
import gridHistogramEventsNyJson from "./fixtures/grid_histogram_events_ny.json";
import gridPageEventsJson from "./fixtures/grid_page_events.json";
import gridPageEventsNyJson from "./fixtures/grid_page_events_ny.json";
import gridValuesChannelJson from "./fixtures/grid_values_channel.json";
import hostZonesJson from "./fixtures/host_zones.json";
import rowViewFirstJson from "./fixtures/row_view_first.json";
import sessionsJson from "./fixtures/sessions.json";
import viewsListJson from "./fixtures/views_list.json";

/** `T` with string literals widened to `string`, number and boolean literals likewise, and tuples as arrays: what JSON infers. */
type Widen<T> = T extends string
  ? string
  : T extends number
    ? number
    : T extends boolean
      ? boolean
      : T extends null
        ? null
        : T extends readonly unknown[]
          ? Array<Widen<T[number]>>
          : T extends object
            ? { [K in keyof T]: Widen<T[K]> }
            : T;

/** A fresh deep copy of `json` as the generated type `G`; compiles only if every part of `json` fits `G`. */
function copy<G>(json: Widen<G>): G {
  return JSON.parse(JSON.stringify(json)) as G;
}

/** One loader per recorded file (by its name without `.json`). */
export const FIXTURES = {
  case_summary: () => copy<CaseSummary>(caseSummaryJson),
  grid_counts_events: () => copy<GridCounts>(gridCountsEventsJson),
  grid_groups_channel: () => copy<Listing<GroupView>>(gridGroupsChannelJson),
  grid_histogram_events: () => copy<Histogram>(gridHistogramEventsJson),
  grid_histogram_events_ny: () => copy<Histogram>(gridHistogramEventsNyJson),
  grid_page_events: () => copy<GridPage>(gridPageEventsJson),
  grid_page_events_ny: () => copy<GridPage>(gridPageEventsNyJson),
  grid_values_channel: () => copy<Listing<ValueCount>>(gridValuesChannelJson),
  host_zones: () => copy<HostZone[]>(hostZonesJson),
  row_view_first: () => copy<RowView>(rowViewFirstJson),
  sessions: () => copy<Session[]>(sessionsJson),
  views_list: () => copy<ViewListing<SavedView>>(viewsListJson),
} as const;

export const caseSummary = FIXTURES.case_summary;
export const gridCountsEvents = FIXTURES.grid_counts_events;
export const gridGroupsChannel = FIXTURES.grid_groups_channel;
export const gridHistogramEvents = FIXTURES.grid_histogram_events;
export const gridHistogramEventsNy = FIXTURES.grid_histogram_events_ny;
export const gridPageEvents = FIXTURES.grid_page_events;
export const gridPageEventsNy = FIXTURES.grid_page_events_ny;
export const gridValuesChannel = FIXTURES.grid_values_channel;
export const hostZones = FIXTURES.host_zones;
export const rowViewFirst = FIXTURES.row_view_first;
export const sessions = FIXTURES.sessions;
export const viewsList = FIXTURES.views_list;
