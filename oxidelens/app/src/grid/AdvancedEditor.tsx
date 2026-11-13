// The advanced editor: AND / OR groups of condition rows (column, operator, value), nested up to the depth a chip may
// have (`MAX_CHIP_DEPTH`: the grid wraps the enabled chips in one more AND), saved as one Advanced chip. The engine is
// asked before the chip is added; its `invalid` answer shows inline and nothing is added.
import { useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import type { Chip } from "../contract/generated/Chip";
import type { ColumnMeta } from "../contract/generated/ColumnMeta";
import type { FilterNode } from "../contract/generated/FilterNode";
import type { Op } from "../contract/generated/Op";
import type { SqlType } from "../contract/generated/SqlType";
import { displaySafe } from "../text";
import {
  ALL_OPS, MARK_COLUMN, MARK_LABEL, MARK_VALUES, MAX_CHIP_DEPTH, advancedChipId, clipLabel, describe, leaf, opTakesValue, orderedValue,
} from "./chips";
import { trapTab, useDialogFocus } from "./dialogFocus";

type Rule = { kind: "rule"; key: number; column: string; op: Op; value: string; value2: string; caseSensitive: boolean };
type Group = { kind: "group"; key: number; mode: "and" | "or"; children: Item[] };
type Item = Rule | Group;

/** A group's own depth: the root is 1, its leaves 2. A leaf may sit at `MAX_CHIP_DEPTH`, so a group at most one less. */
export const MAX_GROUP_DEPTH = MAX_CHIP_DEPTH - 1;

let keys = 0;
const nextKey = () => ++keys;

const ORDERED: readonly Op[] = ["gt", "gte", "lt", "lte", "between"];
const isOrdered = (op: Op) => ORDERED.includes(op);
/** A BOOLEAN column's ordered value is true or false (the engine takes a JSON boolean there). */
const boolText = (v: string) => (v === "false" ? "false" : "true");

/**
 * The operators a column takes. By the engine's rules (filter/compile.rs `leaf_sql`) every operator reads any column's
 * text, the ordered ones also take the typed value of a typed column, and only `conversion_failed` needs a typed column.
 * Mark takes `eq` and `ne` here (`in` is the panel's).
 */
function opsFor(column: string, typed: boolean) {
  if (column === MARK_COLUMN) return ALL_OPS.filter((o) => o.op === "eq" || o.op === "ne");
  return ALL_OPS.filter((o) => typed || o.op !== "conversion_failed");
}

function newRule(column: string): Rule {
  return {
    kind: "rule", key: nextKey(), column, op: column === MARK_COLUMN ? "eq" : "contains", value: column === MARK_COLUMN ? "suspicious" : "",
    value2: "", caseSensitive: false,
  };
}

function toNode(item: Item, typeOf: (column: string) => SqlType | null): FilterNode | string {
  if (item.kind === "group") {
    if (item.children.length === 0) return "A group has no conditions: add one or remove the group.";
    const parts: FilterNode[] = [];
    for (const c of item.children) {
      const n = toNode(c, typeOf);
      if (typeof n === "string") return n;
      parts.push(n);
    }
    return item.mode === "and" ? { and: parts } : { or: parts };
  }
  const { column, op } = item;
  if (!opTakesValue(op)) return leaf(column, op);
  if (op === "in" || op === "not_in") return leaf(column, op, item.value.split("\n").filter((l) => l !== ""), item.caseSensitive);
  const type = typeOf(column);
  const text = (v: string) => orderedValue(type, type === "BOOLEAN" ? boolText(v) : v);
  if (op === "between") return leaf(column, op, [text(item.value), text(item.value2)], item.caseSensitive);
  if (isOrdered(op)) return leaf(column, op, text(item.value), item.caseSensitive);
  return leaf(column, op, item.value, item.caseSensitive);
}

function update(item: Item, key: number, f: (i: Item) => Item | null): Item | null {
  if (item.key === key) return f(item);
  if (item.kind === "rule") return item;
  return { ...item, children: item.children.flatMap((c) => update(c, key, f) ?? []) };
}

export interface AdvancedEditorProps {
  columns: readonly ColumnMeta[];
  validate(chip: Chip): Promise<string | null>;
  onAdd(chip: Chip): void;
  onClose(): void;
}

export default function AdvancedEditor(p: AdvancedEditorProps): ReactNode {
  const choices = [
    ...p.columns.map((c) => ({ name: c.name, label: c.name, typed: c.sql_type !== null, type: c.sql_type })),
    { name: MARK_COLUMN, label: "Mark", typed: false, type: null as SqlType | null },
  ];
  const first = choices[0]?.name ?? MARK_COLUMN;
  const [root, setRoot] = useState<Group>(() => ({ kind: "group", key: nextKey(), mode: "and", children: [newRule(first)] }));
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const box = useRef<HTMLDivElement>(null);

  const edit = (key: number, f: (i: Item) => Item | null) => {
    setError(null);
    setRoot((r) => (update(r, key, f) as Group | null) ?? r);
  };
  const typedOf = (column: string) => choices.find((c) => c.name === column)?.typed ?? false;
  const typeOf = (column: string): SqlType | null => choices.find((c) => c.name === column)?.type ?? null;

  const save = async () => {
    const node = toNode(root, typeOf);
    if (typeof node === "string") {
      setError(node);
      return;
    }
    const chip: Chip = { id: advancedChipId(), label: clipLabel(describe(node)), enabled: true, origin: "advanced", filter: node };
    setBusy(true);
    const refusal = await p.validate(chip);
    setBusy(false);
    if (refusal !== null) {
      setError(refusal);
      return;
    }
    p.onAdd(chip);
    p.onClose();
  };

  useDialogFocus(box);
  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    trapTab(e, box.current);
    if (e.key === "Escape") {
      e.stopPropagation();
      p.onClose();
    }
  };

  const renderRule = (r: Rule, parent: Group): ReactNode => {
    const ops = opsFor(r.column, typedOf(r.column));
    const mark = r.column === MARK_COLUMN;
    return (
      <div key={r.key} className="adv-rule" role="group" aria-label="Condition">
        <select
          aria-label="Column"
          value={r.column}
          onChange={(e) => {
            const column = e.target.value;
            const allowed = opsFor(column, typedOf(column));
            const op = allowed.some((o) => o.op === r.op) ? r.op : allowed[0].op;
            edit(r.key, (i) => ({ ...(i as Rule), column, op, value: column === MARK_COLUMN ? "suspicious" : (i as Rule).value }));
          }}
        >
          {choices.map((c) => <option key={c.name} value={c.name}>{displaySafe(c.label)}</option>)}
        </select>
        <select aria-label="Operator" value={r.op} onChange={(e) => edit(r.key, (i) => ({ ...(i as Rule), op: e.target.value as Op }))}>
          {ops.map((o) => <option key={o.op} value={o.op}>{o.label}</option>)}
        </select>
        {isOrdered(r.op) && typeOf(r.column) === "BOOLEAN" && (
          <>
            <select aria-label="Value" value={boolText(r.value)} onChange={(e) => edit(r.key, (i) => ({ ...(i as Rule), value: e.target.value }))}>
              <option value="true">true</option>
              <option value="false">false</option>
            </select>
            {r.op === "between" && (
              <select aria-label="Upper value" value={boolText(r.value2)} onChange={(e) => edit(r.key, (i) => ({ ...(i as Rule), value2: e.target.value }))}>
                <option value="true">true</option>
                <option value="false">false</option>
              </select>
            )}
          </>
        )}
        {opTakesValue(r.op) && mark && (
          <select aria-label="Value" value={r.value} onChange={(e) => edit(r.key, (i) => ({ ...(i as Rule), value: e.target.value }))}>
            {MARK_VALUES.map((v) => <option key={v} value={v}>{MARK_LABEL[v]}</option>)}
          </select>
        )}
        {opTakesValue(r.op) && !mark && (r.op === "in" || r.op === "not_in") && (
          <textarea
            aria-label="Value"
            placeholder="one value per line"
            rows={3}
            value={r.value}
            onChange={(e) => edit(r.key, (i) => ({ ...(i as Rule), value: e.target.value }))}
          />
        )}
        {opTakesValue(r.op) && !mark && r.op !== "in" && r.op !== "not_in" && !(isOrdered(r.op) && typeOf(r.column) === "BOOLEAN") && (
          <input type="text" aria-label="Value" spellCheck={false} placeholder={isOrdered(r.op) && typeOf(r.column) === "TIMESTAMP" ? "UTC: YYYY-MM-DDTHH:MM:SSZ" : undefined} value={r.value} onChange={(e) => edit(r.key, (i) => ({ ...(i as Rule), value: e.target.value }))} />
        )}
        {r.op === "between" && typeOf(r.column) !== "BOOLEAN" && (
          <input type="text" aria-label="Upper value" spellCheck={false} value={r.value2} onChange={(e) => edit(r.key, (i) => ({ ...(i as Rule), value2: e.target.value }))} />
        )}
        {opTakesValue(r.op) && !mark && (
          <button
            type="button"
            className="ft-aa"
            aria-pressed={r.caseSensitive}
            aria-label="Match case"
            title="Match case"
            onClick={() => edit(r.key, (i) => ({ ...(i as Rule), caseSensitive: !(i as Rule).caseSensitive }))}
          >
            Aa
          </button>
        )}
        <button
          type="button"
          aria-label={`Remove condition${parent.children.length > 1 ? "" : " (last in its group)"}`}
          onClick={() => edit(r.key, () => null)}
        >
          ×
        </button>
      </div>
    );
  };

  const renderGroup = (g: Group, depth: number, isRoot: boolean): ReactNode => (
    <div key={g.key} className="adv-group" role="group" aria-label={`Group, level ${depth}`}>
      <div className="adv-group-head">
        <select aria-label="Match" value={g.mode} onChange={(e) => edit(g.key, (i) => ({ ...(i as Group), mode: e.target.value as "and" | "or" }))}>
          <option value="and">all of (AND)</option>
          <option value="or">any of (OR)</option>
        </select>
        {!isRoot && <button type="button" aria-label="Remove group" onClick={() => edit(g.key, () => null)}>×</button>}
      </div>
      {g.children.map((c) => (c.kind === "rule" ? renderRule(c, g) : renderGroup(c, depth + 1, false)))}
      <div className="adv-group-foot">
        <button type="button" onClick={() => edit(g.key, (i) => ({ ...(i as Group), children: [...(i as Group).children, newRule(first)] }))}>
          Add condition
        </button>
        <button
          type="button"
          disabled={depth >= MAX_GROUP_DEPTH}
          title={depth >= MAX_GROUP_DEPTH ? "Groups cannot nest any deeper" : undefined}
          onClick={() =>
            edit(g.key, (i) => ({
              ...(i as Group),
              children: [...(i as Group).children, { kind: "group", key: nextKey(), mode: "and", children: [newRule(first)] } as Group],
            }))
          }
        >
          Add group
        </button>
      </div>
    </div>
  );

  return (
    <div className="modal-scrim">
      <div ref={box} className="adv-editor" role="dialog" aria-label="Advanced filter" aria-modal="true" onKeyDown={onKeyDown}>
        <div className="viewer-title">Advanced filter</div>
        <div className="adv-body">{renderGroup(root, 1, true)}</div>
        {error !== null && <div role="alert" className="form-error">{displaySafe(error)}</div>}
        <div className="viewer-buttons">
          <button type="button" onClick={p.onClose}>Cancel</button>
          <button type="button" disabled={busy} onClick={() => void save()}>Add filter</button>
        </div>
      </div>
    </div>
  );
}
