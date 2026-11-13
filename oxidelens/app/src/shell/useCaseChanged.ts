// Calls `handler` on every `case-changed` event for as long as the component is mounted. The handler may change
// between renders without resubscribing.
import { useEffect, useRef } from "react";
import { onCaseChanged } from "../api";
import type { CaseChanged } from "../contract/generated/CaseChanged";

export function useCaseChanged(handler: (e: CaseChanged) => void): void {
  const ref = useRef(handler);
  useEffect(() => {
    ref.current = handler;
  });
  useEffect(() => {
    let stop: (() => void) | null = null;
    let gone = false;
    void onCaseChanged((e) => ref.current(e)).then((s) => {
      if (gone) s();
      else stop = s;
    });
    return () => {
      gone = true;
      stop?.();
    };
  }, []);
}
