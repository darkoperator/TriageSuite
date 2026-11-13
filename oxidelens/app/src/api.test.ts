import { describe, expect, it, vi } from "vitest";
import * as api from "./api";
import { call, isCmdError, newQueryId, CMD_ERROR_CODES } from "./api";
import { callLog, emitEvent, overrideCommand } from "./test/backend";
import type { GridState } from "./contract/generated/GridState";

// `@tauri-apps/api/core` is mocked for every test by `test/setup.ts`: `invoke` goes to the mock backend, is logged in
// `callLog`, and `overrideCommand` answers (or rejects) a command by hand.

const state: GridState = {
  relation: "mock_events",
  chips: [],
  search: { text: "", regex: false, case_sensitive: false, mode: "filter" },
  hosts: [],
  sort: [],
  columns: null,
};

describe("call", () => {
  it("passes the command and its args to invoke and returns the result", async () => {
    overrideCommand("case_open", () => ({ ok: 1 }));
    await expect(call("case_open", { path: "/c.duckdb" })).resolves.toEqual({ ok: 1 });
    expect(callLog).toEqual([{ cmd: "case_open", args: { path: "/c.duckdb" } }]);
  });

  it("rethrows a command error as is", async () => {
    const error = { code: "case_locked", message: "another process holds the case" };
    overrideCommand("case_open", () => Promise.reject(error));
    await expect(call("case_open", { path: "/c.duckdb" })).rejects.toBe(error);
  });

  it("keeps every code of the contract", async () => {
    for (const code of CMD_ERROR_CODES) {
      overrideCommand("case_close", () => Promise.reject({ code, message: "m" }));
      await expect(call("case_close")).rejects.toEqual({ code, message: "m" });
    }
  });

  it("wraps a string rejection as an internal error", async () => {
    overrideCommand("case_close", () => Promise.reject("command case_close not found"));
    await expect(call("case_close")).rejects.toEqual({ code: "internal", message: "command case_close not found" });
  });

  it("wraps an Error as an internal error keeping its message", async () => {
    overrideCommand("case_close", () => {
      throw new Error("ipc went away");
    });
    await expect(call("case_close")).rejects.toEqual({ code: "internal", message: "ipc went away" });
  });

  it("wraps an object with an unknown code as an internal error", async () => {
    overrideCommand("case_close", () => Promise.reject({ code: "timeout", message: "plugin timed out" }));
    await expect(call("case_close")).rejects.toEqual({ code: "internal", message: "plugin timed out" });
  });
});

describe("isCmdError", () => {
  it("accepts only {code, message} with a known code and a string message", () => {
    expect(isCmdError({ code: "invalid", message: "m" })).toBe(true);
    expect(isCmdError({ code: "invalid" })).toBe(false);
    expect(isCmdError({ code: 1, message: "m" })).toBe(false);
    expect(isCmdError({ code: "timeout", message: "m" })).toBe(false);
    expect(isCmdError("invalid")).toBe(false);
    expect(isCmdError(null)).toBe(false);
    for (const code of CMD_ERROR_CODES) expect(isCmdError({ code, message: "m" })).toBe(true);
  });

  it("names the twelve codes the app answers with", () => {
    expect([...CMD_ERROR_CODES]).toEqual([
      "not_found", "invalid", "refused", "case_locked", "needs_upgrade", "engine",
      "io", "internal", "no_case", "busy", "superseded", "interrupted",
    ]);
  });
});

describe("newQueryId", () => {
  it("is a fresh non-empty string every time", () => {
    const ids = new Set(Array.from({ length: 200 }, () => newQueryId()));
    expect(ids.size).toBe(200);
    for (const id of ids) expect(id).toMatch(/^[\w-]+$/);
  });
});

describe("the typed wrappers", () => {
  it("send the camelCase argument names the contract names", async () => {
    await api.caseOpen("/cases/acme.case.duckdb");
    await api.caseNew("/cases/new.case.duckdb", "ACME");
    await api.gridPage(state, 50, 100, { zone: "UTC", rules: [] }, "grid", "q1");
    await api.gridValues(state, "Channel", ["c1"], null, "commonest", 20, 0, "values", "q2");
    await api.gridHistogram(state, null, 80, "UTC", "histogram", "q3");
    await api.gridExport(state, "/out.csv", "UTC", "export", "q4");
    await api.rowView(7, "UTC", "detail", "q5");
    await api.queryCancel(["q1", "q2"]);
    expect(callLog.map((c) => c.cmd)).toEqual([
      "case_open", "case_new", "grid_page", "grid_values", "grid_histogram", "grid_export", "row_view", "query_cancel",
    ]);
    expect(callLog[0].args).toEqual({ path: "/cases/acme.case.duckdb" });
    expect(callLog[1].args).toEqual({ path: "/cases/new.case.duckdb", name: "ACME" });
    expect(callLog[2].args).toEqual({
      queryId: "q1", slot: "grid", state, limit: 50, offset: 100, options: { zone: "UTC", rules: [] },
    });
    expect(Object.keys(callLog[3].args!).sort()).toEqual(
      ["column", "contains", "exceptChips", "limit", "offset", "order", "queryId", "slot", "state"],
    );
    expect(callLog[4].args).toMatchObject({ column: null, maxBuckets: 80, zone: "UTC" });
    expect(callLog[5].args).toMatchObject({ path: "/out.csv", zoneTimes: "UTC" });
    expect(callLog[6].args).toEqual({ queryId: "q5", slot: "detail", rowId: 7, zone: "UTC" });
    expect(callLog[7].args).toEqual({ queryIds: ["q1", "q2"] });
  });

  it("make a query id when the caller gives none", async () => {
    await api.caseOpen("/cases/acme.case.duckdb");
    await api.caseSummary("status");
    await api.caseSummary("status");
    const [a, b] = callLog.filter((c) => c.cmd === "case_summary").map((c) => c.args!.queryId as string);
    expect(a).toBeTruthy();
    expect(a).not.toBe(b);
  });

  it("return what the command answers", async () => {
    await api.caseOpen("/cases/acme.case.duckdb");
    const page = await api.gridPage(state, 3, 0, { zone: "UTC", rules: [] }, "grid");
    expect(page.cells).toHaveLength(3);
  });
});

describe("events", () => {
  it("onCaseChanged hands the payload over until it is unlistened", async () => {
    const seen = vi.fn();
    const off = await api.onCaseChanged(seen);
    emitEvent("case-changed", { kinds: ["marks"], generation: 4 });
    expect(seen).toHaveBeenCalledWith({ kinds: ["marks"], generation: 4 });
    off();
    emitEvent("case-changed", { kinds: ["marks"], generation: 5 });
    expect(seen).toHaveBeenCalledTimes(1);
  });

  it("onJob hands over a start and a finish, told apart", async () => {
    const seen = vi.fn();
    const off = await api.onJob(seen);
    emitEvent("job-started", { kind: "load_run", label: "run-1" });
    emitEvent("job-finished", { kind: "load_run", ok: false, message: "disk full" });
    expect(seen.mock.calls).toEqual([
      [{ phase: "started", kind: "load_run", label: "run-1" }],
      [{ phase: "finished", kind: "load_run", ok: false, message: "disk full" }],
    ]);
    off();
    emitEvent("job-started", { kind: "load_run", label: "run-2" });
    expect(seen).toHaveBeenCalledTimes(2);
  });

  it("onSettingsDamaged hands the payload over", async () => {
    const seen = vi.fn();
    await api.onSettingsDamaged(seen);
    emitEvent("settings-damaged", { file: "settings.json", reason: "not JSON" });
    expect(seen).toHaveBeenCalledWith({ file: "settings.json", reason: "not JSON" });
  });
});
