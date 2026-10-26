import { getCurrentWindow } from "@tauri-apps/api/window";
import { api } from "../lib/api";
import { confirmDialog } from "../store/dialogStore";

export type CloseSteps = {
  /** How many terminals run a program (an agent, a server…). */
  runningPrograms: () => Promise<number>;
  confirm: typeof confirmDialog;
  /** Saves every terminal for the next start and hands other programs' windows back. */
  prepare: () => Promise<unknown>;
  destroy: () => Promise<unknown>;
};

/**
 * What closing the window does: a casual close (×, Alt+F4, Quit) asks first
 * when programs still run, since closing stops them. Every terminal is saved
 * before the window goes, and reopens where it was on the next start.
 * Resolves to whether the window was closed.
 */
export async function closeWindow(steps: CloseSteps): Promise<boolean> {
  const running = await steps.runningPrograms().catch(() => 0);
  if (running > 0) {
    const ok = await steps.confirm({
      title: "Close Godesk?",
      message:
        `${running === 1 ? "A terminal still runs a program" : `${running} terminals still run programs`}, and ` +
        "closing stops what they are doing. Next time Godesk reopens every terminal where it was, and " +
        "Claude Code, Codex and OpenCode continue their conversation.",
      confirmLabel: "Close Godesk",
      danger: true,
    });
    if (!ok) return false;
  }
  // A failed save must not keep the window open for good.
  await steps.prepare().catch(() => undefined);
  await steps.destroy();
  return true;
}

/** Routes every close of the main window through {@link closeWindow}. */
export function installCloseGuard(): () => void {
  const window = getCurrentWindow();
  let closing = false;
  const steps: CloseSteps = {
    runningPrograms: api.runningPrograms,
    confirm: confirmDialog,
    prepare: api.prepareClose,
    destroy: () => window.destroy(),
  };
  const unlisten = window.onCloseRequested(async (event) => {
    event.preventDefault();
    // A second click on × while asking must not ask twice.
    if (closing) return;
    closing = true;
    try {
      await closeWindow(steps);
    } finally {
      closing = false;
    }
  });
  return () => void unlisten.then((stop) => stop());
}
