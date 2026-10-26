import { invoke } from "@tauri-apps/api/core";
import { type UnlistenFn, listen } from "@tauri-apps/api/event";
import type { AgentsChangedEvent } from "../bindings/AgentsChangedEvent";
import type { AppCapturedEvent } from "../bindings/AppCapturedEvent";
import type { AppChangedEvent } from "../bindings/AppChangedEvent";
import type { AppMovedEvent } from "../bindings/AppMovedEvent";
import type { AppPreviewEvent } from "../bindings/AppPreviewEvent";
import type { AppLaunch } from "../bindings/AppLaunch";
import type { AppPlacement } from "../bindings/AppPlacement";
import type { AppSnapshot } from "../bindings/AppSnapshot";
import type { AttachResult } from "../bindings/AttachResult";
import type { Camera } from "../bindings/Camera";
import type { EmbeddedApp } from "../bindings/EmbeddedApp";
import type { KnowledgeBase } from "../bindings/KnowledgeBase";
import type { KnowledgeBasesChangedEvent } from "../bindings/KnowledgeBasesChangedEvent";
import type { Link } from "../bindings/Link";
import type { LinksChangedEvent } from "../bindings/LinksChangedEvent";
import type { NativeWindow } from "../bindings/NativeWindow";
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
/** Progress of the one-time offline voice download (`whisper-progress`). */
export type WhisperProgress = { phase: "model" | "binary" | "done"; received?: number; total?: number };

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
  /** Adds a terminal whose shell starts in `cwd`. */
  createTerminalIn: (workspaceId: string, rect: Rect, cwd: string) =>
    invoke<TerminalWindow>("create_terminal_in", { workspaceId, rect, cwd }),
  /** The folders matching `query`, most frecent first (zoxide-style). */
  listDirs: (query: string) => invoke<string[]>("list_dirs", { query }),
  /** Whether the offline voice model and engine are installed. */
  whisperReady: () => invoke<boolean>("whisper_ready"),
  /** Whether live (streaming) dictation is available (the persistent server). */
  whisperCanStream: () => invoke<boolean>("whisper_can_stream"),
  /** Downloads the offline voice model and engine (progress via `whisper-progress`). */
  whisperInstall: () => invoke<null>("whisper_install"),
  /** Transcribes a 16 kHz mono WAV (base64) offline in `lang` ("es", "auto"…). */
  transcribe: (wav: string, lang: string) => invoke<string>("transcribe", { wav, lang }),
  /** Closes the agent a terminal shows and shows its shell again. */
  detachAgent: (terminalId: string) => invoke<TerminalWindow>("detach_agent", { terminalId }),
  /** Whether an agent started in a shell is shown by Godesk or on its own screen. */
  setAgentView: (on: boolean) => invoke<null>("set_agent_view", { on }),
  /** Opens a terminal's agent on its own screen, in the same conversation. */
  agentToClassic: (terminalId: string) => invoke<TerminalWindow>("agent_to_classic", { terminalId }),
  /** Answers a permission the agent asked for. */
  answerAgentPermission: (terminalId: string, requestId: string, decision: "allow" | "always" | "deny") =>
    invoke<null>("answer_agent_permission", { terminalId, requestId, decision }),
  /** The model the agent works with from the next turn; null for its default. */
  setAgentModel: (terminalId: string, model: string | null) =>
    invoke<null>("set_agent_model", { terminalId, model }),
  /** Sends what the user wrote to the agent of a terminal. */
  sendAgentMessage: (terminalId: string, text: string) =>
    invoke<null>("send_agent_message", { terminalId, text }),
  /** What the agent wrote so far, to draw the conversation again, and how many lines that is. */
  agentOutput: (terminalId: string) => invoke<{ text: string; lines: number }>("agent_output", { terminalId }),
  /** Stops the turn the agent of a terminal is working on. */
  interruptAgent: (terminalId: string) => invoke<null>("interrupt_agent", { terminalId }),
  /** Starts the agent of a terminal again, on the same conversation. */
  restartAgent: (terminalId: string) => invoke<null>("restart_agent", { terminalId }),
  /** Where the agent works. */
  agentFolder: (terminalId: string) => invoke<string | null>("agent_folder", { terminalId }),
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

  /** Adds a knowledge-base node for the folder `path` to a workspace. */
  createKnowledgeBase: (workspaceId: string, path: string, rect: Rect) =>
    invoke<KnowledgeBase>("create_knowledge_base", { workspaceId, path, rect }),
  removeKnowledgeBase: (knowledgeBaseId: string) =>
    invoke<null>("remove_knowledge_base", { knowledgeBaseId }),
  renameKnowledgeBase: (knowledgeBaseId: string, name: string) =>
    invoke<null>("rename_knowledge_base", { knowledgeBaseId, name }),
  updateKnowledgeBaseRect: (knowledgeBaseId: string, rect: Rect) =>
    invoke<Rect>("update_knowledge_base_rect", { knowledgeBaseId, rect }),

  /** The folder the terminal's program (or idle shell) is in; null when unknown. */
  terminalFolder: (terminalId: string) => invoke<string | null>("terminal_folder", { terminalId }),
  /** Asks for a folder or a file; null when cancelled. */
  pickPath: (folder: boolean) => invoke<string | null>("pick_path", { folder }),

  // Other programs' windows on the canvas (Windows only).
  listWindows: () => invoke<NativeWindow[]>("list_windows"),
  /** Resolves once the program's window has appeared; it stays hidden until placed. */
  openApp: (launch: AppLaunch) => invoke<EmbeddedApp>("open_app", { launch }),
  placeApps: (placements: AppPlacement[]) => invoke<null>("place_apps", { placements }),
  focusApp: (appId: string) => invoke<null>("focus_app", { appId }),
  /** Asks the program to close; an `app-changed` event says when it has. */
  closeApp: (appId: string) => invoke<null>("close_app", { appId }),
  /** Gives the window back to the desktop. */
  releaseApp: (appId: string) => invoke<null>("release_app", { appId }),
  /** Tokens and subagents of the agents in the terminals, as last read. */
  getAgents: () => invoke<AgentsChangedEvent>("get_agents"),
  /** How many terminals run a program, to warn before closing. */
  runningPrograms: () => invoke<number>("running_programs"),
  /** Saves every terminal for the next start and gives other programs' windows back. */
  prepareClose: () => invoke<null>("prepare_close"),
  /** Whether Claude Code and Codex are told, in their global instructions, how to reach other terminals. */
  agentNotesInstalled: () => invoke<boolean>("agent_notes_installed"),
  setAgentNotes: (on: boolean) => invoke<null>("set_agent_notes", { on }),
  /** Whether programs started in terminals (`code .`) come onto the canvas. */
  setCaptureLaunches: (on: boolean) => invoke<null>("set_capture_launches", { on }),
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
  /** A line an agent wrote, with its number in the agent's history. */
  onAgentLine: (terminalId: string, handler: (line: string, seq: number) => void): Promise<UnlistenFn> =>
    listen<{ seq: number; line: string }>(`agent-line-${terminalId}`, (e) => handler(e.payload.line, e.payload.seq)),
  /** The agent of a terminal is gone. */
  onAgentExit: (terminalId: string, handler: (code: number | null) => void): Promise<UnlistenFn> =>
    listen<number | null>(`agent-exit-${terminalId}`, (e) => handler(e.payload)),
  /** A terminal now shows its agent, or its shell again. */
  onTerminalChanged: (handler: (terminal: TerminalWindow) => void): Promise<UnlistenFn> =>
    listen<TerminalWindow>("terminal-changed", (e) => handler(e.payload)),
  onLinksChanged: (handler: (e: LinksChangedEvent) => void): Promise<UnlistenFn> =>
    listen<LinksChangedEvent>("links-changed", (e) => handler(e.payload)),
  onKnowledgeBasesChanged: (handler: (e: KnowledgeBasesChangedEvent) => void): Promise<UnlistenFn> =>
    listen<KnowledgeBasesChangedEvent>("knowledge-bases-changed", (e) => handler(e.payload)),
  onAppChanged: (handler: (e: AppChangedEvent) => void): Promise<UnlistenFn> =>
    listen<AppChangedEvent>("app-changed", (e) => handler(e.payload)),
  /** The agents' tokens or subagents changed. */
  onAgentsChanged: (handler: (e: AgentsChangedEvent) => void): Promise<UnlistenFn> =>
    listen<AgentsChangedEvent>("agents-changed", (e) => handler(e.payload)),
  /** Progress of the one-time offline voice download. */
  onWhisperProgress: (handler: (e: WhisperProgress) => void): Promise<UnlistenFn> =>
    listen<WhisperProgress>("whisper-progress", (e) => handler(e.payload)),
  /** What a window showed when it was hidden, for its card. */
  onAppPreview: (handler: (e: AppPreviewEvent) => void): Promise<UnlistenFn> =>
    listen<AppPreviewEvent>("app-preview", (e) => handler(e.payload)),
  /** A window was moved or resized by its own frame; its card follows. */
  onAppMoved: (handler: (e: AppMovedEvent) => void): Promise<UnlistenFn> =>
    listen<AppMovedEvent>("app-moved", (e) => handler(e.payload)),
  /** A program started in a terminal showed a window, already taken onto the canvas. */
  onAppCaptured: (handler: (e: AppCapturedEvent) => void): Promise<UnlistenFn> =>
    listen<AppCapturedEvent>("app-captured", (e) => handler(e.payload)),
  onCanvasPinch: (handler: (e: CanvasPinchEvent) => void): Promise<UnlistenFn> =>
    listen<CanvasPinchEvent>("canvas-pinch", (e) => handler(e.payload)),
};

export function errorMessage(error: unknown): string {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  return String(error);
}
