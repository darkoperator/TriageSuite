// The zone note is sampled per case: another case with the same zone and dataset name, or the same case after a write,
// is read again rather than keeping what the last one said.
import { renderHook, waitFor } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { overrideCommand, callsOf } from "../test/backend";
import { caseSummary, gridPageEvents, rowViewFirst } from "../test/fixtures";
import { offsetText, useZoneOffset } from "./useZoneOffset";

function answer(offset: string, date: string) {
  overrideCommand("grid_page", () => ({ ...gridPageEvents(), row_ids: [1] }));
  overrideCommand("row_view", () => {
    const v = rowViewFirst();
    for (const f of v.fields) if (f.display !== null) f.display = `${date} 21:13:58.2040000 ${offset}`;
    return v;
  });
}
const inZone = (caseId: string) => ({ ...caseSummary(), case_id: caseId, display_zone: "America/New_York" });

describe("the zone note", () => {
  it("offsetText_reads_the_engines_text_and_the_date_of_the_sampled_row", () => {
    expect(offsetText("2026-01-04 21:13:58.2040000 -05:00")).toBe("UTC−05:00 on 2026-01-04");
    expect(offsetText("2026-07-04 21:13:58 +05:30")).toBe("UTC+05:30 on 2026-07-04");
    expect(offsetText("2026-01-04 21:13:58 Z")).toBe("UTC+00:00 on 2026-01-04");
    expect(offsetText("not a time")).toBeNull();
    expect(offsetText("21:13:58 -05:00")).toBeNull(); // no date to qualify it with
  });

  it("another_case_with_the_same_zone_and_dataset_is_sampled_again", async () => {
    answer("-05:00", "2026-01-04");
    const { result, rerender } = renderHook(({ s, g }) => useZoneOffset(s, g), { initialProps: { s: inZone("aaaa"), g: 0 } });
    await waitFor(() => expect(result.current).toBe("UTC−05:00 on 2026-01-04"));
    answer("-04:00", "2026-07-04");
    rerender({ s: inZone("bbbb"), g: 0 });
    await waitFor(() => expect(result.current).toBe("UTC−04:00 on 2026-07-04"));
    // the same case after a write (a load, a mark) is read again too
    answer("-05:00", "2026-11-02");
    rerender({ s: inZone("bbbb"), g: 1 });
    await waitFor(() => expect(result.current).toBe("UTC−05:00 on 2026-11-02"));
    expect(callsOf("row_view")).toHaveLength(3);
  });
});
