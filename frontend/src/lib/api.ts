import { invoke } from "@tauri-apps/api/core";
import { type UnlistenFn, listen } from "@tauri-apps/api/event";
import type { AppSnapshot } from "../bindings/AppSnapshot";
import type { AttachResult } from "../bindings/AttachResult";
import type { Camera } from "../bindings/Camera";
import type { Link } from "../bindings/Link";
import type { LinksChangedEvent } from "../bindings/LinksChangedEvent";
import type { PtyExitEvent } from "../bindings/PtyExitEvent";
import type { PtyOutputEvent } from "../bindings/PtyOutputEvent";
import type { Rect } from "../bindings/Rect";
import type { TerminalWindow } from "../bindings/TerminalWindow";
import type { Workspace } from "../bindings/Workspace";

/**
 * Typed wrappers of the Rust `#[tauri::command]`s (src-tauri/src/commands.rs).
 * Commands that return `()` resolve to null, so null never means failure:
 * a failed command rejects.
 */
export const api = {
  getState: () => invoke<AppSnapshot>("get_state"),

  createWorkspace: (name?: string) => invoke<Workspace>("create_workspace", { name: name ?? null }),
  renameWorkspace: (workspaceId: string, name: string) => invoke<null>("rename_workspace", { workspaceId, name }),
  /** Resolves to the new active workspace id. */
  deleteWorkspace: (workspaceId: string) => invoke<string>("delete_workspace", { workspaceId }),
  setActiveWorkspace: (workspaceId: string) => invoke<null>("set_active_workspace", { workspaceId }),
  updateCamera: (workspaceId: string, camera: Camera) => invoke<null>("update_camera", { workspaceId, camera }),

  createTerminal: (workspaceId: string, rect: Rect) =>
    invoke<TerminalWindow>("create_terminal", { workspaceId, rect }),
  /** Opens a copy at `rect`: same folder, and the same program if one is running in it. */
  duplicateTerminal: (terminalId: string, rect: Rect) =>
    invoke<TerminalWindow>("duplicate_terminal", { terminalId, rect }),
  closeTerminal: (terminalId: string) => invoke<null>("close_terminal", { terminalId }),
  renameTerminal: (terminalId: string, name: string) => invoke<null>("rename_terminal", { terminalId, name }),
  updateTerminalRect: (terminalId: string, rect: Rect) => invoke<Rect>("update_terminal_rect", { terminalId, rect }),
  /** Resolves to the terminal's new z. */
  focusTerminal: (terminalId: string) => invoke<number>("focus_terminal", { terminalId }),
  terminalHasForegroundProcess: (terminalId: string) =>
    invoke<boolean>("terminal_has_foreground_process", { terminalId }),

  attachTerminal: (terminalId: string, cols: number, rows: number) =>
    invoke<AttachResult>("attach_terminal", { terminalId, cols, rows }),
  restartTerminal: (terminalId: string, cols: number, rows: number) =>
    invoke<null>("restart_terminal", { terminalId, cols, rows }),
  /** `data` is base64 of the raw input bytes. */
  writeTerminal: (terminalId: string, data: string) => invoke<null>("write_terminal", { terminalId, data }),
  resizeTerminal: (terminalId: string, cols: number, rows: number) =>
    invoke<null>("resize_terminal", { terminalId, cols, rows }),

  createLink: (sourceId: string, targetId: string) => invoke<Link>("create_link", { sourceId, targetId }),
  deleteLink: (linkId: string) => invoke<null>("delete_link", { linkId }),
};

/**
 * Native pinch forwarded by the backend on Linux, where WebKitGTK would
 * otherwise zoom the whole page. `scale` is cumulative since `begin`; `x`/`y`
 * are client coordinates of the gesture center.
 */
export type CanvasPinchEvent = { phase: "begin" | "update" | "end"; scale: number; x: number; y: number };

/** Backend events. PTY events are per terminal: `pty-output-<id>`, `pty-exit-<id>`. */
export const events = {
  onPtyOutput: (terminalId: string, handler: (e: PtyOutputEvent) => void): Promise<UnlistenFn> =>
    listen<PtyOutputEvent>(`pty-output-${terminalId}`, (e) => handler(e.payload)),
  onPtyExit: (terminalId: string, handler: (e: PtyExitEvent) => void): Promise<UnlistenFn> =>
    listen<PtyExitEvent>(`pty-exit-${terminalId}`, (e) => handler(e.payload)),
  onLinksChanged: (handler: (e: LinksChangedEvent) => void): Promise<UnlistenFn> =>
    listen<LinksChangedEvent>("links-changed", (e) => handler(e.payload)),
  onCanvasPinch: (handler: (e: CanvasPinchEvent) => void): Promise<UnlistenFn> =>
    listen<CanvasPinchEvent>("canvas-pinch", (e) => handler(e.payload)),
};

export function errorMessage(error: unknown): string {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  return String(error);
}
