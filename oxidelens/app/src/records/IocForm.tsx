// I: the focused cell's value as a new IOC. The type is preselected by the engine's guess (`ioc_guess`, its
// `guess_ioc_type`) and the analyst confirms or changes it; nothing is written until *Add IOC*. The value is sent as it
// is; the engine validates it for the type and stores its canonical form, and its refusal is shown here.
import { useEffect, useRef, useState, type ReactNode } from "react";
import { iocGuess, recordUpsert } from "../api";
import type { IocType } from "../contract/generated/IocType";
import { isStale, messageOf } from "./outcomes";
import RecordDialog from "./RecordDialog";

/** Every type of ruling S4, in its order, with what the form calls it. */
const TYPES: ReadonlyArray<readonly [IocType, string]> = [
  ["ip", "IP address"], ["domain", "Domain"], ["hostname", "Host name"], ["url", "URL"], ["email", "Email"],
  ["md5", "MD5"], ["sha1", "SHA-1"], ["sha256", "SHA-256"], ["file_name", "File name"], ["file_path", "File path"],
  ["registry_key", "Registry key"], ["service", "Service"], ["scheduled_task", "Scheduled task"], ["other", "Other"],
];

export default function IocForm({ value: initial, onClose }: { value: string; onClose(): void }): ReactNode {
  const [value, setValue] = useState(initial);
  const [type, setType] = useState<IocType>("other");
  const [guessing, setGuessing] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Once the analyst picks a type, a guess that arrives later does not replace it.
  const chosen = useRef(false);

  useEffect(() => {
    let live = true;
    iocGuess(initial).then(
      (guess) => {
        if (!live) return;
        setGuessing(false);
        if (guess !== null && !chosen.current) setType(guess);
      },
      (e: unknown) => {
        if (!live) return;
        setGuessing(false);
        if (!isStale(e)) setError(messageOf(e));
      },
    );
    return () => {
      live = false;
    };
  }, [initial]);

  const save = async () => {
    if (busy) return;
    setBusy(true);
    try {
      await recordUpsert({ kind: "ioc", id: null, ioc_type: type, value, tool: null, attack_ids: [], note: "" });
      onClose();
    } catch (e) {
      setBusy(false);
      if (!isStale(e)) setError(messageOf(e));
    }
  };

  return (
    <RecordDialog label="Add IOC" onClose={onClose} error={error}>
      <label className="form-field">
        <span>Value</span>
        <input type="text" aria-label="IOC value" value={value} onChange={(e) => setValue(e.target.value)} />
      </label>
      <label className="form-field">
        <span>{guessing ? "Type (guessing…)" : "Type"}</span>
        <select
          aria-label="IOC type"
          value={type}
          onChange={(e) => {
            chosen.current = true;
            setType(e.target.value as IocType);
          }}
        >
          {TYPES.map(([t, label]) => (
            <option key={t} value={t}>{label}</option>
          ))}
        </select>
      </label>
      <div className="form-buttons">
        <button type="button" disabled={busy || value === ""} onClick={() => void save()}>Add IOC</button>
        <button type="button" onClick={onClose}>Cancel</button>
      </div>
    </RecordDialog>
  );
}
