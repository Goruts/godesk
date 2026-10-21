import { useCallback, useEffect, useRef, useState } from "react";
import { isTypingTarget } from "../canvas/interaction";
import { useDialogStore } from "../store/dialogStore";

/**
 * Open state of a dock panel. Escape or a press outside the button and the
 * panel closes it, except while a text field inside handles its own Escape
 * (renaming) or a confirmation started from the panel is pending.
 */
export function usePopover() {
  const [open, setOpen] = useState(false);
  const buttonRef = useRef<HTMLButtonElement>(null);
  const panelRef = useRef<HTMLDivElement>(null);

  /** Closes the panel; `refocus` hands the keyboard back to its button. */
  const close = useCallback((refocus: boolean) => {
    setOpen(false);
    if (refocus) buttonRef.current?.focus();
  }, []);

  useEffect(() => {
    if (!open) return;
    const confirming = () => useDialogStore.getState().pending !== null;
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || confirming() || isTypingTarget(e.target)) return;
      e.stopPropagation();
      close(true);
    };
    const onPointerDown = (e: PointerEvent) => {
      const target = e.target as Node;
      if (confirming() || panelRef.current?.contains(target) || buttonRef.current?.contains(target)) return;
      close(false);
    };
    window.addEventListener("keydown", onKeyDown, true);
    window.addEventListener("pointerdown", onPointerDown, true);
    return () => {
      window.removeEventListener("keydown", onKeyDown, true);
      window.removeEventListener("pointerdown", onPointerDown, true);
    };
  }, [open, close]);

  return { open, toggle: () => setOpen((o) => !o), close, buttonRef, panelRef };
}
