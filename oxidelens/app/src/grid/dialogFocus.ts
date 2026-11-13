// Focus for dialogs: on open focus moves to the first control, Tab and Shift+Tab can be kept inside (a modal), and on
// close focus goes back to the control that was focused when the dialog opened (when it is still there).
import { useEffect, type KeyboardEvent, type RefObject } from "react";

const FOCUSABLE = "button, [href], input, select, textarea, [tabindex]";

function focusables(box: HTMLElement): HTMLElement[] {
  return [...box.querySelectorAll<HTMLElement>(FOCUSABLE)].filter(
    (el) => el.tabIndex >= 0 && !(el as HTMLButtonElement).disabled && el.getAttribute("aria-hidden") !== "true",
  );
}

/**
 * Focus the first control of `box` on mount; give focus back on unmount to `returnTo` (the control that asked for the
 * dialog, passed in because a menu that opened it may be gone by the time this runs) or, without one, to the element
 * focused at mount.
 */
export function useDialogFocus(box: RefObject<HTMLElement | null>, returnTo?: HTMLElement | null): void {
  useEffect(() => {
    const invoker = returnTo ?? (document.activeElement as HTMLElement | null);
    const el = box.current;
    if (el) (focusables(el)[0] ?? el).focus();
    return () => {
      if (invoker && invoker.isConnected) invoker.focus();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps -- captured once, when the dialog opens
  }, [box]);
}

/** Keeps Tab and Shift+Tab inside `box`: the last control wraps to the first and the reverse. */
export function trapTab(e: KeyboardEvent<HTMLElement>, box: HTMLElement | null): void {
  if (e.key !== "Tab" || !box) return;
  const items = focusables(box);
  if (items.length === 0) {
    e.preventDefault();
    return;
  }
  const first = items[0];
  const last = items[items.length - 1];
  const active = document.activeElement;
  if (e.shiftKey && (active === first || !box.contains(active))) {
    e.preventDefault();
    last.focus();
  } else if (!e.shiftKey && (active === last || !box.contains(active))) {
    e.preventDefault();
    first.focus();
  }
}
