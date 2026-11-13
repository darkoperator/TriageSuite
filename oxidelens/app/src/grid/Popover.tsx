// A small dialog that hangs under a button: focus goes in on open and back to the button on close, Escape and a press
// outside close it. Used by the host scope and the zone picker.
import { useEffect, useRef, type ReactNode, type RefObject } from "react";
import { useDialogFocus } from "./dialogFocus";

export default function Popover({
  label, anchor, onClose, className, children,
}: {
  label: string;
  /** The button that opened it: placed under it, given focus back, and its own presses are left to it. */
  anchor: RefObject<HTMLElement | null>;
  onClose(): void;
  className?: string;
  children: ReactNode;
}): ReactNode {
  const box = useRef<HTMLDivElement>(null);
  useDialogFocus(box, anchor.current);
  useEffect(() => {
    const away = (e: MouseEvent) => {
      const t = e.target as Node;
      if (box.current && !box.current.contains(t) && !anchor.current?.contains(t)) onClose();
    };
    const escape = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    document.addEventListener("mousedown", away);
    document.addEventListener("keydown", escape);
    return () => {
      document.removeEventListener("mousedown", away);
      document.removeEventListener("keydown", escape);
    };
  }, [anchor, onClose]);
  const r = anchor.current?.getBoundingClientRect();
  const left = Math.max(0, Math.min(r?.left ?? 0, (typeof window === "undefined" ? 1200 : window.innerWidth) - 300));
  return (
    <div ref={box} className={`popover ${className ?? ""}`} role="dialog" aria-label={label} style={{ left, top: r?.bottom ?? 0 }}>
      {children}
    </div>
  );
}
