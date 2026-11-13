// useMatches on its own: what a step does when the state changes under it, and which ids it cancels.
import { act, render, waitFor } from "@testing-library/react";
import { startTransition, useEffect, useLayoutEffect, useState } from "react";
import { describe, expect, it } from "vitest";
import type { GridState } from "../contract/generated/GridState";
import { callsOf, overrideCommand } from "../test/backend";
import { initialGridState } from "./gridState";
import { useMatches, type Matches } from "./useMatches";

const finding = (text: string): GridState => ({
  ...initialGridState("mock_events"),
  search: { text, regex: false, case_sensitive: false, mode: "find" },
});

interface Handle {
  m: Matches;
  setState(s: GridState): void;
}

/** Called from a layout effect of the commit that shows state "b". */
const onCommit: { current: ((m: Matches) => void) | null } = { current: null };

function mountProbe() {
  const handle: { current: Handle | null } = { current: null };
  function Probe() {
    const [s, setS] = useState(finding("a"));
    const m = useMatches("t", s, true, 0, () => {});
    useLayoutEffect(() => {
      if (s.search.text === "b") onCommit.current?.(m);
    }, [s, m]);
    useEffect(() => {
      handle.current = { m, setState: setS };
    }, [m]);
    return null;
  }
  render(<Probe />);
  return handle as { current: Handle };
}

const cancelled = () => callsOf("query_cancel").flatMap((c) => (c.args as { queryIds: string[] }).queryIds);
const counts = () => callsOf("grid_match_count").map((c) => String(c.args!.queryId));

describe("useMatches", () => {
  it("a_step_that_answers_after_a_new_state_committed_but_before_its_effects_ran_moves_nothing", async () => {
    overrideCommand("grid_match_count", () => 5);
    let release: () => void = () => {};
    overrideCommand(
      "grid_matches",
      () => new Promise((res) => (release = () => res({ items: [{ offset: 3, row_id: 9, ordinal: 2 }], truncated: false }))),
    );
    const h = mountProbe();
    await waitFor(() => expect(h.current?.m.count).toBe(5));
    let result: number | null | undefined;
    let released = false;
    const running = h.current.m.step(1, null).then((r) => (result = r));
    await waitFor(() => expect(callsOf("grid_matches")).toHaveLength(1));
    // A default-priority update commits and its passive effects wait for the scheduler; the answer is released from a
    // layout effect of that very commit, so it lands after the new state is on screen and before any passive effect.
    const env = globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean };
    env.IS_REACT_ACT_ENVIRONMENT = false;
    try {
      onCommit.current = () => {
        released = true;
        release();
      };
      startTransition(() => h.current.setState(finding("b")));
      await running;
    } finally {
      onCommit.current = null;
      env.IS_REACT_ACT_ENVIRONMENT = true;
    }
    expect(result).toBeNull();
    expect(released).toBe(true); // the answer really came after the new state committed
    expect(result).toBeNull();
    await waitFor(() => expect(h.current.m.current).toBeNull());
  });

  it("a_step_invoked_after_the_layout_phase_of_a_new_state_queries_that_state_and_is_not_cancelled", async () => {
    overrideCommand("grid_match_count", () => 5);
    // each state has its own match: "a" at 3, "b" at 7
    overrideCommand("grid_matches", (a) => {
      const text = (a as { state: GridState }).state.search.text;
      return { items: [{ offset: text === "b" ? 7 : 3, row_id: 9, ordinal: text === "b" ? 4 : 2 }], truncated: false };
    });
    const h = mountProbe();
    await waitFor(() => expect(h.current?.m.count).toBe(5));
    let result: number | null | undefined;
    let running: Promise<void> | null = null;
    // The step is invoked from a layout effect of the commit that shows "b": the new state is committed and the
    // passive effects have not run.
    onCommit.current = (m) => {
      onCommit.current = null;
      running = m.step(1, null).then((r) => void (result = r));
    };
    await act(async () => {
      h.current.setState(finding("b"));
    });
    await act(async () => {
      await running;
    });
    const asked = callsOf("grid_matches");
    expect(asked).toHaveLength(1);
    expect((asked[0].args as { state: GridState }).state.search.text).toBe("b");
    expect(result).toBe(7);
    expect(cancelled()).not.toContain(String(asked[0].args!.queryId));
    await waitFor(() => expect(h.current.m.position).toBe(4));
    expect(h.current.m.current).toBe(7);
  });

  it("a_step_in_the_current_state_answers_with_the_offset_and_the_engines_rank", async () => {
    overrideCommand("grid_match_count", () => 5);
    overrideCommand("grid_matches", () => ({ items: [{ offset: 3, row_id: 9, ordinal: 2 }], truncated: false }));
    const h = mountProbe();
    await waitFor(() => expect(h.current?.m.count).toBe(5));
    await act(async () => {
      expect(await h.current.m.step(1, 40)).toBe(3); // from a row the analyst focused, not from a match
    });
    expect(h.current.m.position).toBe(2);
    expect(h.current.m.current).toBe(3);
  });

  it("settled_queries_are_not_cancelled_with_the_state_but_running_ones_are", async () => {
    overrideCommand("grid_match_count", () => 5);
    overrideCommand("grid_matches", () => ({ items: [{ offset: 3, row_id: 9, ordinal: 2 }], truncated: false }));
    const h = mountProbe();
    await waitFor(() => expect(h.current?.m.count).not.toBeNull()); // the count has settled
    await act(async () => void (await h.current.m.step(1, null))); // and so has a step
    const stepId = String(callsOf("grid_matches")[0].args!.queryId);
    await act(async () => h.current.setState(finding("b")));
    await waitFor(() => expect(callsOf("grid_match_count")).toHaveLength(2));
    expect(cancelled()).not.toContain(counts()[0]);
    expect(cancelled()).not.toContain(stepId);
    // the second state's count is still running when the third arrives: that one is cancelled
    overrideCommand("grid_match_count", () => new Promise(() => {}));
    await act(async () => h.current.setState(finding("c")));
    await waitFor(() => expect(callsOf("grid_match_count")).toHaveLength(3));
    await act(async () => h.current.setState(finding("d")));
    await waitFor(() => expect(cancelled()).toContain(counts()[2]));
  });
});
