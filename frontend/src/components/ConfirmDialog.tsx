import { useEffect, useRef } from "react";
import { useDialogStore } from "../store/dialogStore";

export function ConfirmDialog() {
  const pending = useDialogStore((s) => s.pending);
  const close = useDialogStore((s) => s.close);
  const cancelRef = useRef<HTMLButtonElement>(null);
  const confirmRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    if (!pending) return;
    // A stray Enter must not run a destructive action: focus Cancel then.
    (pending.danger ? cancelRef : confirmRef).current?.focus();
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        close(false);
      } else if (e.key === "Tab") {
        // Keep the focus inside the dialog; it only has these two buttons.
        e.preventDefault();
        e.stopPropagation();
        (document.activeElement === cancelRef.current ? confirmRef : cancelRef).current?.focus();
      }
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, [pending, close]);

  if (!pending) return null;
  return (
    <div className="dialog-backdrop" onPointerDown={() => close(false)}>
      <div
        className="dialog"
        role="alertdialog"
        aria-modal="true"
        aria-labelledby="dialog-title"
        aria-describedby="dialog-message"
        onPointerDown={(e) => e.stopPropagation()}
      >
        <h2 id="dialog-title">{pending.title}</h2>
        <p id="dialog-message">{pending.message}</p>
        <div className="dialog-actions">
          <button ref={cancelRef} type="button" className="btn" onClick={() => close(false)}>
            Cancel
          </button>
          <button
            ref={confirmRef}
            type="button"
            className={`btn ${pending.danger ? "btn-danger" : "btn-primary"}`}
            onClick={() => close(true)}
          >
            {pending.confirmLabel}
          </button>
        </div>
      </div>
    </div>
  );
}
