// The settings dialog, opened from the top bar's gear: theme, cell font size, analyst name (and the actor label the
// app records changes under), the histogram switch and *Forget recent cases*. Every write goes through the shell's one
// settings queue: `update` for plain changes, `save` for the analyst name, whose refusal it shows beside the field.
import { useRef, useState, type KeyboardEvent } from "react";
import { isCmdError } from "../api";
import type { Settings as SettingsData } from "../contract/generated/Settings";
import type { SettingsReply } from "../contract/generated/SettingsReply";
import { trapTab, useDialogFocus } from "../grid/dialogFocus";
import { displaySafe } from "../text";
import "./settings.css";

export const FONT_SIZE_CHOICES = [10, 11, 12, 13, 14, 15, 16];

export default function Settings({
  settings, actor, update, save, onClose, returnTo,
}: {
  settings: SettingsData;
  /** The effective actor label the engine records changes under. */
  actor: string;
  update(patch: Partial<SettingsData>): void;
  save(patch: Partial<SettingsData>): Promise<SettingsReply>;
  onClose(): void;
  returnTo: HTMLElement | null;
}) {
  const box = useRef<HTMLDivElement>(null);
  useDialogFocus(box, returnTo);
  const [name, setName] = useState(settings.analyst ?? "");
  const latestSave = useRef(0);
  const [refusal, setRefusal] = useState<string | null>(null);

  const onKeyDown = (e: KeyboardEvent<HTMLElement>) => {
    if (e.key === "Escape") {
      e.stopPropagation();
      onClose();
    } else trapTab(e, box.current);
  };

  const saveName = async () => {
    // Only the most recently started save decides what is shown: an older save's outcome neither sets nor clears it.
    const mine = ++latestSave.current;
    setRefusal(null);
    try {
      // Exactly as typed: the backend owns the name rule and gives the reason. Only an empty field clears the name.
      await save({ analyst: name === "" ? null : name });
      if (mine === latestSave.current) setRefusal(null);
    } catch (e) {
      if (mine === latestSave.current) setRefusal(isCmdError(e) ? e.message : e instanceof Error ? e.message : String(e));
    }
  };

  return (
    <div className="modal-scrim">
      <div ref={box} className="settings-dialog" role="dialog" aria-modal="true" aria-label="Settings" tabIndex={-1} onKeyDown={onKeyDown}>
        <h2 className="settings-title">Settings</h2>

        <fieldset className="settings-row">
          <legend>Theme</legend>
          {(["dark", "light"] as const).map((t) => (
            <label key={t}>
              <input type="radio" name="theme" checked={settings.theme === t} onChange={() => update({ theme: t })} />
              {t === "dark" ? "Dark" : "Light"}
            </label>
          ))}
        </fieldset>

        <div className="settings-row">
          <label htmlFor="settings-font">Cell font size</label>
          <select id="settings-font" value={settings.font_size} onChange={(e) => update({ font_size: Number(e.target.value) })}>
            {FONT_SIZE_CHOICES.map((n) => (
              <option key={n} value={n}>{n}</option>
            ))}
          </select>
          <span className="settings-hint">px</span>
        </div>

        <div className="settings-row">
          <label htmlFor="settings-analyst">Analyst name</label>
          <input
            id="settings-analyst"
            type="text"
            value={name}
            aria-invalid={refusal !== null}
            onChange={(e) => setName(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                void saveName();
              }
            }}
          />
          <button type="button" onClick={() => void saveName()}>Save name</button>
          {refusal !== null && <div className="settings-error" role="alert">{displaySafe(refusal)}</div>}
          <div className="settings-hint">Changes are recorded as <code>{displaySafe(actor)}</code>. Empty uses the operating system user.</div>
        </div>

        <div className="settings-row">
          <label>
            <input type="checkbox" checked={settings.histogram} onChange={(e) => update({ histogram: e.target.checked })} />
            Show the histogram
          </label>
        </div>

        <div className="settings-row">
          <button type="button" disabled={settings.recent.length === 0} onClick={() => update({ recent: [] })}>Forget recent cases</button>
        </div>

        <div className="settings-buttons">
          <button type="button" onClick={onClose}>Close</button>
        </div>
      </div>
    </div>
  );
}
