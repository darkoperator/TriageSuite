// The host scope: `Hosts: all N ▾` opens a checklist of the case's hosts. The choice is `GridState.hosts`; empty means
// every host, so ticking every host is the same as none chosen, and the last ticked host cannot be unticked.
import { useCallback, useRef, useState, type ReactNode } from "react";
import { displaySafe } from "../text";
import Popover from "./Popover";

/** The hosts a choice stands for: none chosen is all of them. */
export function effectiveHosts(all: readonly string[], chosen: readonly string[]): string[] {
  return chosen.length === 0 ? [...all] : all.filter((h) => chosen.includes(h));
}

export default function HostScope({
  hosts, chosen, onChange,
}: {
  /** Every host of the case. */
  hosts: readonly string[];
  /** `GridState.hosts`. */
  chosen: readonly string[];
  onChange(next: string[]): void;
}): ReactNode {
  const [open, setOpen] = useState(false);
  const button = useRef<HTMLButtonElement>(null);
  const close = useCallback(() => setOpen(false), []);
  const ticked = effectiveHosts(hosts, chosen);
  const label =
    ticked.length === hosts.length ? `all ${hosts.length}` : ticked.length === 1 ? displaySafe(ticked[0]) : `${ticked.length} of ${hosts.length}`;

  const toggle = (host: string) => {
    const next = ticked.includes(host) ? ticked.filter((h) => h !== host) : hosts.filter((h) => h === host || ticked.includes(h));
    if (next.length === 0) return;
    onChange(next.length === hosts.length ? [] : next);
  };

  return (
    <>
      <button ref={button} type="button" className="scope-button" aria-haspopup="dialog" aria-expanded={open} onClick={() => setOpen((o) => !o)}>
        {`Hosts: ${label} ▾`}
      </button>
      {open && (
        <Popover label="Hosts" anchor={button} onClose={close} className="host-scope">
          <ul className="scope-list">
            {hosts.map((h) => (
              <li key={h}>
                <label>
                  <input
                    type="checkbox"
                    aria-label={displaySafe(h)}
                    checked={ticked.includes(h)}
                    disabled={ticked.length === 1 && ticked[0] === h}
                    onChange={() => toggle(h)}
                  />
                  <span className="scope-name">{displaySafe(h)}</span>
                </label>
              </li>
            ))}
          </ul>
          <button type="button" className="scope-all" disabled={chosen.length === 0} onClick={() => onChange([])}>
            All hosts
          </button>
        </Popover>
      )}
    </>
  );
}
