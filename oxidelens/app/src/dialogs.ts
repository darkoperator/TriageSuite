// The native file and folder pickers (`@tauri-apps/plugin-dialog`: the `dialog:allow-open` and `dialog:allow-save`
// capabilities). Screens call these wrappers, never the plugin, so tests replace one module. Under `npm run dev:mock`
// there is no desktop window to ask, so each picker answers a fixed fake path.
import { open, save } from "@tauri-apps/plugin-dialog";

const CASE_FILTER = [{ name: "OxideLens case", extensions: ["duckdb"] }];
const CSV_FILTER = [{ name: "CSV", extensions: ["csv"] }, { name: "All files", extensions: ["*"] }];

const single = (r: string | string[] | null): string | null => (Array.isArray(r) ? (r[0] ?? null) : r);

/** Where a new case file goes (a save dialog filtered to `*.duckdb`); `null` when the analyst cancels. */
export async function pickNewCasePath(): Promise<string | null> {
  if (__OXL_MOCK__) return "/mock/new-case.duckdb";
  return save({ title: "New case", filters: CASE_FILTER, defaultPath: "case.duckdb" });
}

/** An existing case file to open. */
export async function pickCaseFile(): Promise<string | null> {
  if (__OXL_MOCK__) return "/mock/case.duckdb";
  return single(await open({ title: "Open case", multiple: false, directory: false, filters: CASE_FILTER }));
}

/** A processed run folder to load. */
export async function pickRunFolder(): Promise<string | null> {
  if (__OXL_MOCK__) return "/mock/Processed-run";
  return single(await open({ title: "Load run", multiple: false, directory: true }));
}

const VIEW_FILTER = [{ name: "OxideLens view", extensions: ["json"] }];

/** Where a grid's CSV export goes (a save dialog); `null` when the analyst cancels. */
export async function pickCsvSavePath(): Promise<string | null> {
  if (__OXL_MOCK__) return "/mock/export.csv";
  return save({ title: "Export CSV", filters: [{ name: "CSV", extensions: ["csv"] }], defaultPath: "export.csv" });
}

/** Where a saved view's JSON file goes (a save dialog); `null` when the analyst cancels. */
export async function pickViewSavePath(): Promise<string | null> {
  if (__OXL_MOCK__) return "/mock/view.json";
  return save({ title: "Export view", filters: VIEW_FILTER, defaultPath: "view.json" });
}

/** A view file to import. */
export async function pickViewFile(): Promise<string | null> {
  if (__OXL_MOCK__) return "/mock/view.json";
  return single(await open({ title: "Import view", multiple: false, directory: false, filters: VIEW_FILTER }));
}

/** A CSV file to open as a loose file. */
export async function pickCsvFile(): Promise<string | null> {
  if (__OXL_MOCK__) return "/mock/loose.csv";
  return single(await open({ title: "Open CSV", multiple: false, directory: false, filters: CSV_FILTER }));
}
