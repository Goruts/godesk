import { isTypingTarget } from "../canvas/interaction";
import { api } from "../lib/api";
import { useAppStore } from "../store/appStore";
import { confirmDialog, useDialogStore } from "../store/dialogStore";
import { focusTerminalView } from "./TerminalView";

/** Creates a terminal in the center of the view and gives it keyboard focus. */
export async function openNewTerminal(): Promise<void> {
  const terminal = await useAppStore.getState().createTerminal();
  // xterm is created in an effect after the window mounts; the view takes
  // the focus then.
  if (terminal) focusTerminalView(terminal.id);
}

export function terminalCount(count: number): string {
  return `${count} terminal${count === 1 ? "" : "s"}`;
}

let closingSelection = false;

/** Closes every selected terminal, asking once if any still runs a program. */
export async function closeSelectedTerminals(): Promise<void> {
  const ids = useAppStore.getState().selectedTerminalIds;
  // A second press while the first is still checking must not ask twice.
  if (closingSelection || ids.length === 0) return;
  closingSelection = true;
  try {
    const busy = await Promise.all(ids.map((id) => api.terminalHasForegroundProcess(id).catch(() => false)));
    const running = busy.filter(Boolean).length;
    if (running > 0) {
      const ok = await confirmDialog({
        title: `Close ${terminalCount(ids.length)}?`,
        message:
          running === 1
            ? "A program is still running in one of them. Closing it will stop that program."
            : `Programs are still running in ${running} of them. Closing them will stop those programs.`,
        confirmLabel: `Close ${terminalCount(ids.length)}`,
        danger: true,
      });
      if (!ok) return;
    }
    await useAppStore.getState().closeTerminals(ids);
  } finally {
    closingSelection = false;
  }
}

/**
 * Global shortcuts, registered in the capture phase so they win over xterm:
 * Ctrl/Cmd+Shift+T opens a terminal. Outside text fields and terminals,
 * Delete/Backspace closes the selected terminals or removes the selected
 * link, and Escape clears the selection.
 */
export function installShortcuts(): () => void {
  const onKeyDown = (e: KeyboardEvent) => {
    if ((e.ctrlKey || e.metaKey) && e.shiftKey && e.key.toLowerCase() === "t") {
      e.preventDefault();
      e.stopPropagation();
      // Holding the keys must not open a terminal per auto-repeat.
      if (!e.repeat) void openNewTerminal();
      return;
    }
    if (isTypingTarget(e.target) || useDialogStore.getState().pending) return;
    const { selectedLinkId, selectedTerminalIds, deleteLink, selectLink, selectTerminals } = useAppStore.getState();
    if (e.key === "Delete" || e.key === "Backspace") {
      if (selectedTerminalIds.length > 0) {
        e.preventDefault();
        void closeSelectedTerminals();
      } else if (selectedLinkId) {
        e.preventDefault();
        void deleteLink(selectedLinkId);
      }
    } else if (e.key === "Escape") {
      selectTerminals([]);
      selectLink(null);
    }
  };
  window.addEventListener("keydown", onKeyDown, true);
  return () => window.removeEventListener("keydown", onKeyDown, true);
}
