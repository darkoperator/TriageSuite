// The three marks as the screens draw them: amber ● Suspicious, red ■ Malicious, purple ◆ Agent: malicious. The glyphs
// and their colours are reserved (theme tokens `--suspicious`, `--malicious`, `--agent`); a colour rule never reaches
// them. The class is picked from a literal map by the engine's `MarkState`, never built from a string.
import type { ReactNode } from "react";
import type { MarkCounts } from "../contract/generated/MarkCounts";
import type { MarkState } from "../contract/generated/MarkState";
import type { RelationMarks } from "../contract/generated/RelationMarks";

export const MARK_GLYPH: Record<MarkState, string> = { suspicious: "●", malicious: "■", agent_malicious: "◆" };
/** What a glyph is called to a screen reader. */
export const MARK_LABEL: Record<MarkState, string> = { suspicious: "suspicious", malicious: "malicious", agent_malicious: "agent: malicious" };
/** The mark's name in a sentence or a heading. */
export const MARK_NAME: Record<MarkState, string> = { suspicious: "Suspicious", malicious: "Malicious", agent_malicious: "Agent: malicious" };
const MARK_CLASS: Record<MarkState, string> = { suspicious: "mark-suspicious", malicious: "mark-malicious", agent_malicious: "mark-agent" };

/** Most severe first (ruling S12): Malicious, then Agent: malicious, then Suspicious. */
export const MARK_STATES: readonly MarkState[] = ["malicious", "agent_malicious", "suspicious"];

const own = (k: unknown): k is MarkState => typeof k === "string" && Object.hasOwn(MARK_CLASS, k);

/** The class of a mark's colour; "" for no mark, or one this build does not know. */
export const markClass = (mark: MarkState | null | undefined): string => (own(mark) ? MARK_CLASS[mark] : "");

/** A relation's most severe mark, or null when none of its rows is marked. */
export function mostSevere(counts: Pick<RelationMarks, MarkState> | Pick<MarkCounts, MarkState>): MarkState | null {
  return MARK_STATES.find((m) => counts[m] > 0) ?? null;
}

/**
 * A mark's glyph. Alone it is an image named by its mark; `decorative` hides it from screen readers where the text
 * beside it already names the mark.
 */
export default function MarkGlyph({ mark, decorative = false }: { mark: MarkState | null | undefined; decorative?: boolean }): ReactNode {
  if (!own(mark)) return null;
  if (decorative) {
    return (
      <span className={MARK_CLASS[mark]} aria-hidden="true">
        {MARK_GLYPH[mark]}
      </span>
    );
  }
  return (
    <span className={MARK_CLASS[mark]} role="img" aria-label={MARK_LABEL[mark]}>
      {MARK_GLYPH[mark]}
    </span>
  );
}
