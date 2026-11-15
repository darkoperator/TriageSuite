// The row keys of a dataset or merged grid: S marks Suspicious, U clears it, M makes a timeline entry per row (and the
// pane shows it), with no dialog and no confirmation; T tags, N notes, A attaches to an entry and I adds the focused
// cell's value as an IOC, each through a small dialog. Keys do nothing while a text field has focus, or with Ctrl, Alt
// or Meta held.
//
// Every action takes the rows the grid resolved *before* it writes (ruling S10): `RowSource.rows` answers the
// selection's row ids, a refusal for a selection past 10,000 rows (said in the status bar with no round trip), or null
// when there is nothing to act on or the view moved on while it was resolving. What the engine refuses is shown in the
// status bar with its own reason, never in a dialog.
import { useCallback, useEffect, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { markClear, markSet, rowsPromote } from "../api";
import { isTypingTarget } from "../shell/keys";
import AttachPicker from "./AttachPicker";
import IocForm from "./IocForm";
import NoteDialog from "./NoteDialog";
import { promotedEntry, promoteRefusals, reportFailure, rowRefusals } from "./outcomes";
import TagDialog from "./TagDialog";

/** What a grid gives the keys to act on. */
export interface RowSource {
  /** The rows to act on, resolved to ids; `{ refused }` to say why not; null for nothing (or a view that moved on). */
  rows(): Promise<number[] | { refused: string } | null>;
  /** The one row N notes: the focused row, else a selection of one; null for none. */
  noteRow(): number | null;
  /** The focused cell's whole text, for I; absent where a grid has no cells to focus. */
  cellValue?: () => Promise<string | null>;
}

type Dialog =
  | { kind: "tag"; rows: number[] }
  | { kind: "note"; rowId: number }
  | { kind: "attach"; rows: number[] }
  | { kind: "ioc"; value: string };

const KEYS = new Set(["s", "u", "m", "t", "n", "a", "i"]);

export interface RowActions {
  /** Handles a key the grid got; true when it was a row key (and its default is prevented). */
  onKey(e: KeyboardEvent): boolean;
  /** Runs one action as its key would (the row detail's buttons use the same path). */
  run(key: string): void;
  /** The open dialog, to render beside the grid; null for none. */
  dialog: ReactNode;
}

export function useRowActions(
  source: RowSource,
  { report, onEntry, enabled }: { report(e: unknown): void; onEntry(entryId: number): void; enabled: boolean },
): RowActions {
  const [dialog, setDialog] = useState<Dialog | null>(null);
  // A hidden tab keeps no dialog open: it would still be there on return, holding rows resolved for a view gone since.
  if (!enabled && dialog !== null) setDialog(null);
  const sourceRef = useRef(source);
  const onEntryRef = useRef(onEntry);
  useEffect(() => {
    sourceRef.current = source;
    onEntryRef.current = onEntry;
  });
  const live = useRef(true);
  useEffect(() => {
    live.current = true;
    return () => {
      live.current = false;
    };
  }, []);
  // One write of these keys at a time: a key pressed while one runs is ignored rather than refused as `busy`.
  const writing = useRef(false);

  const run = useCallback(
    (key: string) => {
      const src = sourceRef.current;
      void (async () => {
        try {
          if (key === "n") {
            const rowId = src.noteRow();
            if (rowId !== null) setDialog({ kind: "note", rowId });
            return;
          }
          if (key === "i") {
            const value = src.cellValue ? await src.cellValue() : null;
            if (!live.current || value === null) return;
            if (value === "") report(new Error("the focused cell is empty: there is no value to add as an IOC"));
            else setDialog({ kind: "ioc", value });
            return;
          }
          if (writing.current) return;
          const write = key === "s" || key === "u" || key === "m";
          if (write) writing.current = true;
          try {
            const rows = await src.rows();
            if (!live.current || rows === null) return;
            if (!Array.isArray(rows)) return report(new Error(rows.refused));
            if (rows.length === 0) return;
            if (key === "t") setDialog({ kind: "tag", rows });
            else if (key === "a") setDialog({ kind: "attach", rows });
            else if (key === "m") {
              const outcomes = await rowsPromote(rows);
              const refused = promoteRefusals(outcomes);
              if (refused !== null) report(new Error(refused));
              const entry = promotedEntry(outcomes);
              if (entry !== null && live.current) onEntryRef.current(entry);
            } else {
              const refused = rowRefusals(await (key === "s" ? markSet(rows) : markClear(rows)));
              if (refused !== null) report(new Error(refused));
            }
          } finally {
            if (write) writing.current = false;
          }
        } catch (e) {
          reportFailure(e, report);
        }
      })();
    },
    [report],
  );

  const onKey = (e: KeyboardEvent): boolean => {
    if (!enabled || e.ctrlKey || e.altKey || e.metaKey || isTypingTarget(e.target)) return false;
    const key = e.key.toLowerCase();
    if (!KEYS.has(key) || (key === "i" && !source.cellValue)) return false;
    e.preventDefault();
    run(key);
    return true;
  };

  const close = () => setDialog(null);
  let node: ReactNode = null;
  if (dialog?.kind === "tag") node = <TagDialog rows={dialog.rows} report={report} onClose={close} />;
  else if (dialog?.kind === "note") node = <NoteDialog rowId={dialog.rowId} onClose={close} />;
  else if (dialog?.kind === "attach") {
    node = <AttachPicker rows={dialog.rows} report={report} onAttached={(id) => onEntryRef.current(id)} onClose={close} />;
  } else if (dialog?.kind === "ioc") node = <IocForm value={dialog.value} onClose={close} />;
  return { onKey, run, dialog: node };
}
