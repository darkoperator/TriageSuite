// The display time zone picker: `TZ: UTC ▾` opens a searchable list of the engine's zone names with each host's own
// zone (from its registry) first. Choosing one asks the engine to remember it for the case; the engine announces the
// change and every view re-reads. Nothing here converts a time.
import { useCallback, useEffect, useRef, useState, type KeyboardEvent, type ReactNode, type RefObject } from "react";
import { displayZoneSet, hostZones as readHostZones, isCmdError, zoneNames } from "../api";
import type { HostZone } from "../contract/generated/HostZone";
import { displaySafe } from "../text";
import Popover from "./Popover";

/** At most this many zones are drawn at once; the search narrows the rest. */
const MAX_LISTED = 300;

interface Entry {
  key: string;
  label: string;
  zone: string | null;
}

export default function ZonePicker({
  zone, tabId, onError,
}: {
  zone: string;
  tabId: string;
  onError(e: unknown): void;
}): ReactNode {
  const [open, setOpen] = useState(false);
  const button = useRef<HTMLButtonElement>(null);
  const close = useCallback(() => setOpen(false), []);
  return (
    <>
      <button ref={button} type="button" className="scope-button" aria-haspopup="dialog" aria-expanded={open} onClick={() => setOpen((o) => !o)}>
        {`TZ: ${displaySafe(zone)} ▾`}
      </button>
      {open && <ZoneList anchor={button} zone={zone} tabId={tabId} onClose={close} onError={onError} />}
    </>
  );
}

function ZoneList({
  anchor, zone, tabId, onClose, onError,
}: {
  anchor: RefObject<HTMLElement | null>;
  zone: string;
  tabId: string;
  onClose(): void;
  onError(e: unknown): void;
}): ReactNode {
  const [names, setNames] = useState<string[] | null>(null);
  const [hosts, setHosts] = useState<HostZone[]>([]);
  const [find, setFind] = useState("");
  const list = useRef<HTMLUListElement>(null);
  const findRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    let live = true;
    const quiet = (e: unknown) => !(isCmdError(e) && (e.code === "superseded" || e.code === "interrupted"));
    zoneNames().then((n) => live && setNames(n), (e: unknown) => live && quiet(e) && onError(e));
    readHostZones(`tab:${tabId}:zones`).then((h) => live && setHosts(h), (e: unknown) => live && quiet(e) && onError(e));
    return () => {
      live = false;
    };
  }, [tabId, onError]);

  const choose = (z: string) => {
    displayZoneSet(z).then(
      () => undefined,
      (e: unknown) => onError(e),
    );
    onClose();
  };

  const needle = find.trim().toLowerCase();
  const hostEntries: Entry[] = hosts.map((h) => ({
    key: `host:${h.host}`,
    label: `${displaySafe(h.host)} · ${h.iana === null ? `${displaySafe(h.windows_name)} (no matching zone)` : displaySafe(h.iana)}`,
    zone: h.iana,
  }));
  const nameEntries: Entry[] = (names ?? []).map((n) => ({ key: `zone:${n}`, label: displaySafe(n), zone: n }));
  const matches = (e: Entry) => needle === "" || e.label.toLowerCase().includes(needle);
  const shownNames = nameEntries.filter(matches);
  const entries = [...hostEntries.filter(matches), ...shownNames.slice(0, MAX_LISTED)];

  const options = () => [...(list.current?.querySelectorAll<HTMLElement>("[role='option']:not([aria-disabled='true'])") ?? [])];
  const onOptionKey = (e: KeyboardEvent<HTMLElement>, z: string | null) => {
    const all = options();
    const at = all.indexOf(e.currentTarget);
    if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      if (z !== null) choose(z);
    } else if (e.key === "ArrowDown") {
      e.preventDefault();
      all[Math.min(all.length - 1, at + 1)]?.focus();
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      if (at <= 0) findRef.current?.focus();
      else all[at - 1]?.focus();
    }
  };

  return (
    <Popover label="Display time zone" anchor={anchor} onClose={onClose} className="zone-picker">
      <input
        type="search"
        ref={findRef}
        className="zone-find"
        aria-label="Find a zone"
        placeholder="Find a zone…"
        autoComplete="off"
        spellCheck={false}
        value={find}
        onChange={(e) => setFind(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "ArrowDown") {
            e.preventDefault();
            options()[0]?.focus();
          } else if (e.key === "Enter") {
            e.preventDefault();
            const first = entries.find((x) => x.zone !== null);
            if (first?.zone) choose(first.zone);
          }
        }}
      />
      <ul ref={list} className="zone-list" role="listbox" aria-label="Zones">
        {names === null && <li className="ft-note">Loading…</li>}
        {entries.map((x) => (
          <li
            key={x.key}
            role="option"
            tabIndex={-1}
            aria-selected={x.zone === zone}
            aria-disabled={x.zone === null}
            className={x.key.startsWith("host:") ? "zone-host" : undefined}
            onClick={() => x.zone !== null && choose(x.zone)}
            onKeyDown={(e) => onOptionKey(e, x.zone)}
          >
            {x.label}
          </li>
        ))}
        {names !== null && entries.length === 0 && <li className="ft-note">No zone matches.</li>}
        {shownNames.length > MAX_LISTED && <li className="ft-note">{`${shownNames.length - MAX_LISTED} more: type to narrow.`}</li>}
      </ul>
    </Popover>
  );
}
