import { create } from "zustand";
import type { AppLaunch } from "../bindings/AppLaunch";
import type { Camera } from "../bindings/Camera";
import type { EmbeddedApp } from "../bindings/EmbeddedApp";
import type { TerminalAgent } from "../bindings/TerminalAgent";
import type { TokenUsage } from "../bindings/TokenUsage";
import type { KnowledgeBase } from "../bindings/KnowledgeBase";
import type { Link } from "../bindings/Link";
import type { Rect } from "../bindings/Rect";
import type { TerminalWindow } from "../bindings/TerminalWindow";
import type { Workspace } from "../bindings/Workspace";
import { api, errorMessage, events } from "../lib/api";
import { readFlag, writeFlag } from "../lib/preferences";
import { containsRect, intersects } from "../canvas/geometry";
import { viewport } from "../canvas/viewport";

export const DEFAULT_TERMINAL_WIDTH = 720;
export const DEFAULT_TERMINAL_HEIGHT = 440;

export const DEFAULT_KB_WIDTH = 320;
export const DEFAULT_KB_HEIGHT = 200;

export const DEFAULT_APP_WIDTH = 1000;
export const DEFAULT_APP_HEIGHT = 680;
const MIN_APP_WIDTH = 320;
const MIN_APP_HEIGHT = 200;
/** Height of a card's title bar in canvas units (`.term-titlebar` in styles.css). */
const APP_TITLEBAR_HEIGHT = 30;
const CASCADE_OFFSET = 32;
const DUPLICATE_GAP = 24;
const TRANSPARENT_CANVAS_KEY = "godesk.transparentCanvas";
const CAPTURE_LAUNCHES_KEY = "godesk.captureLaunches";
const AGENT_VIEW_KEY = "godesk.agentView";

/**
 * Another program's window on the canvas (Windows only). Geometry is in
 * canvas units, like a terminal's, and `z` shares the terminals' stacking
 * order. Kept in memory only: closing Godesk gives the windows back to the
 * desktop.
 */
export type CanvasApp = EmbeddedApp & {
  workspaceId: string;
  x: number;
  y: number;
  width: number;
  height: number;
  z: number;
};

type AppState = {
  ready: boolean;
  workspaces: Workspace[];
  activeWorkspaceId: string;
  linksVisible: boolean;
  /** The canvas lets the desktop behind the window show through. */
  transparentCanvas: boolean;
  selectedLinkId: string | null;
  /** Other programs' windows on the canvas, in every workspace. */
  apps: CanvasApp[];
  /** Programs started in a terminal (`code .`) open on the canvas beside it. */
  captureLaunches: boolean;
  /** An agent started in a terminal (`claude`, `codex`…) shows in Godesk's view. */
  agentView: boolean;
  /** The coding agents in the terminals: their tokens and subagents. */
  agents: TerminalAgent[];
  /** Every agent's tokens together. */
  tokenTotal: TokenUsage;
  /** What each program's window showed when it was last hidden (PNG data URLs), by app id. */
  appPreviews: Record<string, string>;
  /** Subagent cards the person hid. */
  dismissedSubagents: string[];
  /** Subagent cards the person moved, by subagent id. */
  movedSubagents: Record<string, Rect>;
  /** Windows picked for a bulk action (Shift+click a title, Shift+drag). */
  selectedTerminalIds: string[];
  /** The folder palette (zoxide-style jump) is open. */
  folderPalette: boolean;
  /** Last error, shown briefly to the user. */
  toast: string | null;
};

type AppActions = {
  init: () => Promise<void>;
  notify: (message: string | null) => void;

  /** Creates a workspace and shows it; resolves to it, or null on failure. */
  createWorkspace: () => Promise<Workspace | null>;
  renameWorkspace: (id: string, name: string) => Promise<void>;
  /** Resolves to whether the workspace was deleted. */
  deleteWorkspace: (id: string) => Promise<boolean>;
  setActiveWorkspace: (id: string) => Promise<void>;
  saveCamera: (workspaceId: string, camera: Camera) => void;

  createTerminal: () => Promise<TerminalWindow | null>;
  /** Creates a terminal whose shell starts in `cwd` (the folder palette). */
  createTerminalInFolder: (cwd: string) => Promise<TerminalWindow | null>;
  /** Opens or closes the folder palette (zoxide-style jump). */
  setFolderPalette: (open: boolean) => void;
  /** Closes the agent a terminal shows and shows its shell again. */
  detachAgent: (id: string) => Promise<void>;
  /** Opens a copy of a terminal beside it; resolves to the copy, or null on failure. */
  duplicateTerminal: (id: string) => Promise<TerminalWindow | null>;
  closeTerminal: (id: string) => Promise<void>;
  closeTerminals: (ids: string[]) => Promise<void>;
  renameTerminal: (id: string, name: string) => Promise<void>;
  commitTerminalRect: (id: string, rect: Rect) => Promise<void>;
  focusTerminal: (id: string) => void;

  /**
   * Starts a program (or takes an open window) and puts its window on the
   * active workspace, beside `nearTerminalId` if given. `label` names it
   * while it starts. Resolves to the app, or null on failure.
   */
  openApp: (launch: AppLaunch, label: string, nearTerminalId?: string) => Promise<CanvasApp | null>;
  /** Asks the program to close its window; the card goes once it has. */
  closeApp: (id: string) => Promise<void>;
  /** Gives the window back to the desktop and drops its card. */
  releaseApp: (id: string) => Promise<void>;
  moveApp: (id: string, rect: Rect) => void;
  /** Brings the card to the front and hands the window the keyboard. */
  focusApp: (id: string) => void;

  createLink: (sourceId: string, targetId: string) => Promise<void>;
  deleteLink: (id: string) => Promise<void>;

  /** Picks a folder and adds a knowledge-base node for it to the active workspace. */
  createKnowledgeBase: () => Promise<void>;
  removeKnowledgeBase: (id: string) => Promise<void>;
  renameKnowledgeBase: (id: string, name: string) => Promise<void>;
  commitKnowledgeBaseRect: (id: string, rect: Rect) => Promise<void>;
  selectLink: (id: string | null) => void;
  /** Replaces the terminal selection; a non-empty one drops the link selection. */
  selectTerminals: (ids: string[]) => void;
  toggleTerminalSelection: (id: string) => void;
  toggleLinks: () => void;
  toggleTransparentCanvas: () => void;
  toggleCaptureLaunches: () => void;
  /** Agents open in Godesk's view (true) or on their own screen in the terminal (false). */
  setAgentView: (on: boolean) => void;
  /** Opens one terminal's agent on its own screen, in the same conversation. */
  agentToClassic: (id: string) => Promise<void>;
  dismissSubagent: (id: string) => void;
  moveSubagent: (id: string, rect: Rect) => void;
};

export type AppStore = AppState & AppActions;

type Outcome<T> = { ok: true; value: T } | { ok: false };

function mapWorkspace(
  workspaces: Workspace[],
  workspaceId: string,
  change: (ws: Workspace) => Workspace,
): Workspace[] {
  return workspaces.map((ws) => (ws.id === workspaceId ? change(ws) : ws));
}

function mapTerminal(
  workspaces: Workspace[],
  terminalId: string,
  change: (t: TerminalWindow) => TerminalWindow,
): Workspace[] {
  return workspaces.map((ws) =>
    ws.terminals.some((t) => t.id === terminalId)
      ? { ...ws, terminals: ws.terminals.map((t) => (t.id === terminalId ? change(t) : t)) }
      : ws,
  );
}

function mapKnowledgeBase(
  workspaces: Workspace[],
  id: string,
  change: (k: KnowledgeBase) => KnowledgeBase,
): Workspace[] {
  return workspaces.map((ws) =>
    (ws.knowledgeBases ?? []).some((k) => k.id === id)
      ? { ...ws, knowledgeBases: (ws.knowledgeBases ?? []).map((k) => (k.id === id ? change(k) : k)) }
      : ws,
  );
}

export function activeWorkspace(state: AppState): Workspace | undefined {
  return state.workspaces.find((ws) => ws.id === state.activeWorkspaceId);
}

export function findTerminal(state: AppState, id: string): TerminalWindow | undefined {
  for (const ws of state.workspaces) {
    const t = ws.terminals.find((t) => t.id === id);
    if (t) return t;
  }
  return undefined;
}

/** Every node of a workspace: terminals, knowledge bases and other programs. */
function windowsOf(ws: Workspace, apps: CanvasApp[]): Array<Rect & { id: string; z: number }> {
  return [...ws.terminals, ...(ws.knowledgeBases ?? []), ...apps.filter((a) => a.workspaceId === ws.id)];
}

/** A spot centered in the visible area, cascading over windows already there. */
function placementFor(windows: Rect[], width = DEFAULT_TERMINAL_WIDTH, height = DEFAULT_TERMINAL_HEIGHT): Rect {
  const center = viewport.center();
  let x = Math.round(center.x - width / 2);
  let y = Math.round(center.y - height / 2);
  while (windows.some((t) => Math.abs(t.x - x) < 4 && Math.abs(t.y - y) < 4)) {
    x += CASCADE_OFFSET;
    y += CASCADE_OFFSET;
  }
  return { x, y, width, height };
}

/** Right of the source when that spot is free and on screen; otherwise centered. The source's size unless given. */
function placementBeside(windows: Rect[], source: Rect, width = source.width, height = source.height): Rect {
  const beside = { x: source.x + source.width + DUPLICATE_GAP, y: source.y, width, height };
  const free = !windows.some((t) => intersects(t, beside));
  return free && containsRect(viewport.visibleRect(), beside) ? beside : placementFor(windows, width, height);
}

/** The z that puts a window in front of every other one of its workspace. */
function frontZ(ws: Workspace, apps: CanvasApp[], exceptId: string): number {
  return Math.max(0, ...windowsOf(ws, apps).filter((w) => w.id !== exceptId).map((w) => w.z)) + 1;
}

export const useAppStore = create<AppStore>()((set, get) => {
  let initialized = false;

  /**
   * Runs a backend call, surfacing failures as a toast. Success is `ok`, not
   * the value: commands that return nothing resolve to null.
   */
  async function attempt<T>(call: () => Promise<T>): Promise<Outcome<T>> {
    try {
      return { ok: true, value: await call() };
    } catch (error) {
      set({ toast: errorMessage(error) });
      return { ok: false };
    }
  }

  function mapApp(id: string, change: (app: CanvasApp) => CanvasApp) {
    set((s) => ({ apps: s.apps.map((a) => (a.id === id ? change(a) : a)) }));
  }

  /** Puts a window that is already on the canvas on a card in `ws`, beside `near` if given. */
  function addCard(app: EmbeddedApp, ws: Workspace, near?: TerminalWindow): CanvasApp {
    // One window, one card: a second report of it changes nothing.
    const existing = get().apps.find((a) => a.id === app.id);
    if (existing) return existing;
    const windows = windowsOf(ws, get().apps);
    const rect = near
      ? placementBeside(windows, near, DEFAULT_APP_WIDTH, DEFAULT_APP_HEIGHT)
      : placementFor(windows, DEFAULT_APP_WIDTH, DEFAULT_APP_HEIGHT);
    const card: CanvasApp = { ...app, ...rect, workspaceId: ws.id, z: frontZ(ws, get().apps, "") };
    set((s) => ({ apps: [...s.apps, card] }));
    void attempt(() => api.focusApp(card.id));
    return card;
  }

  function dropApp(id: string) {
    set((s) => ({ apps: s.apps.filter((a) => a.id !== id) }));
  }

  function setLinks(workspaceId: string, links: Link[]) {
    set((s) => ({
      workspaces: mapWorkspace(s.workspaces, workspaceId, (ws) => ({ ...ws, links })),
      selectedLinkId: links.some((l) => l.id === s.selectedLinkId) ? s.selectedLinkId : null,
    }));
  }

  function setKnowledgeBases(workspaceId: string, knowledgeBases: KnowledgeBase[]) {
    set((s) => ({ workspaces: mapWorkspace(s.workspaces, workspaceId, (ws) => ({ ...ws, knowledgeBases })) }));
  }

  return {
    ready: false,
    workspaces: [],
    activeWorkspaceId: "",
    linksVisible: true,
    transparentCanvas: readFlag(TRANSPARENT_CANVAS_KEY, true),
    selectedLinkId: null,
    apps: [],
    captureLaunches: readFlag(CAPTURE_LAUNCHES_KEY, true),
    agentView: readFlag(AGENT_VIEW_KEY, true),
    agents: [],
    tokenTotal: { input: 0, output: 0 },
    dismissedSubagents: [],
    movedSubagents: {},
    appPreviews: {},
    selectedTerminalIds: [],
    folderPalette: false,
    toast: null,

    async init() {
      if (initialized) return;
      initialized = true;
      await events.onLinksChanged((e) => setLinks(e.workspaceId, e.links));
      await events.onKnowledgeBasesChanged((e) => setKnowledgeBases(e.workspaceId, e.knowledgeBases));
      await events.onAppChanged((e) => (e.closed ? dropApp(e.id) : mapApp(e.id, (a) => ({ ...a, title: e.title }))));
      await events.onAppPreview(({ id, image }) => set((s) => ({ appPreviews: { ...s.appPreviews, [id]: image } })));
      // The window is over the card's body, below its title bar.
      await events.onAppMoved(({ id, rect }) => {
        const { zoom } = viewport;
        const topLeft = viewport.clientToWorld(rect.x, rect.y);
        get().moveApp(id, {
          x: topLeft.x,
          y: topLeft.y - APP_TITLEBAR_HEIGHT,
          width: rect.width / zoom,
          height: rect.height / zoom + APP_TITLEBAR_HEIGHT,
        });
      });
      await events.onAppCaptured(({ app, terminalId }) => {
        const ws = get().workspaces.find((w) => w.terminals.some((t) => t.id === terminalId));
        if (ws) addCard(app, ws, ws.terminals.find((t) => t.id === terminalId));
        else void api.releaseApp(app.id).catch(() => null);
      });
      void attempt(() => api.setCaptureLaunches(get().captureLaunches));
      void attempt(() => api.setAgentView(get().agentView));
      // A terminal's agent was taken over from its shell, or closed.
      await events.onTerminalChanged((changed) =>
        set((s) => ({ workspaces: mapTerminal(s.workspaces, changed.id, (t) => ({ ...t, agent: changed.agent ?? null })) })),
      );
      await events.onAgentsChanged((e) => set({ agents: e.terminals, tokenTotal: e.total }));
      const agents = await api.getAgents().catch(() => null);
      if (agents) set({ agents: agents.terminals, tokenTotal: agents.total });
      const loaded = await attempt(() => api.getState());
      if (!loaded.ok) return;
      const { workspaces, activeWorkspaceId } = loaded.value;
      set({ ready: true, workspaces, activeWorkspaceId });
    },

    notify(message) {
      set({ toast: message });
    },

    async createWorkspace() {
      const created = await attempt(() => api.createWorkspace());
      if (!created.ok) return null;
      const ws = created.value;
      set((s) => ({ workspaces: [...s.workspaces, ws] }));
      await get().setActiveWorkspace(ws.id);
      return ws;
    },

    async renameWorkspace(id, name) {
      const trimmed = name.trim();
      const previous = get().workspaces.find((ws) => ws.id === id)?.name;
      if (!trimmed || previous === undefined || trimmed === previous) return;
      const rename = (to: string) =>
        set((s) => ({ workspaces: mapWorkspace(s.workspaces, id, (ws) => ({ ...ws, name: to })) }));
      rename(trimmed);
      const { ok } = await attempt(() => api.renameWorkspace(id, trimmed));
      // Undo, unless a later rename already replaced this name.
      const current = get().workspaces.find((ws) => ws.id === id)?.name;
      if (!ok && current === trimmed) rename(previous);
    },

    async deleteWorkspace(id) {
      const deleted = await attempt(() => api.deleteWorkspace(id));
      if (!deleted.ok) return false;
      // Its programs' windows go back to the desktop rather than away.
      const orphans = get().apps.filter((a) => a.workspaceId === id);
      await Promise.all(orphans.map((a) => api.releaseApp(a.id).catch(() => null)));
      set((s) => ({
        apps: s.apps.filter((a) => a.workspaceId !== id),
        workspaces: s.workspaces.filter((ws) => ws.id !== id),
        activeWorkspaceId: deleted.value,
        selectedLinkId: null,
        selectedTerminalIds: [],
      }));
      return true;
    },

    async setActiveWorkspace(id) {
      if (get().activeWorkspaceId === id) return;
      set({ activeWorkspaceId: id, selectedLinkId: null, selectedTerminalIds: [] });
      await attempt(() => api.setActiveWorkspace(id));
    },

    saveCamera(workspaceId, camera) {
      // A late save for a workspace deleted meanwhile has nowhere to go.
      if (!get().workspaces.some((ws) => ws.id === workspaceId)) return;
      set((s) => ({ workspaces: mapWorkspace(s.workspaces, workspaceId, (ws) => ({ ...ws, camera })) }));
      void attempt(() => api.updateCamera(workspaceId, camera));
    },

    async createTerminal() {
      const ws = activeWorkspace(get());
      if (!ws) return null;
      const created = await attempt(() => api.createTerminal(ws.id, placementFor(windowsOf(ws, get().apps))));
      if (!created.ok) return null;
      const terminal = created.value;
      set((s) => ({
        workspaces: mapWorkspace(s.workspaces, ws.id, (w) => ({ ...w, terminals: [...w.terminals, terminal] })),
      }));
      return terminal;
    },

    async createTerminalInFolder(cwd) {
      const ws = activeWorkspace(get());
      if (!ws) return null;
      const created = await attempt(() => api.createTerminalIn(ws.id, placementFor(windowsOf(ws, get().apps)), cwd));
      if (!created.ok) return null;
      const terminal = created.value;
      set((s) => ({
        workspaces: mapWorkspace(s.workspaces, ws.id, (w) => ({ ...w, terminals: [...w.terminals, terminal] })),
      }));
      return terminal;
    },

    setFolderPalette(open) {
      set({ folderPalette: open });
    },

    async detachAgent(id) {
      const detached = await attempt(() => api.detachAgent(id));
      if (detached.ok) {
        set((s) => ({ workspaces: mapTerminal(s.workspaces, id, (t) => ({ ...t, agent: detached.value.agent ?? null })) }));
      }
    },

    async duplicateTerminal(id) {      const ws = get().workspaces.find((w) => w.terminals.some((t) => t.id === id));
      const source = ws?.terminals.find((t) => t.id === id);
      if (!ws || !source) return null;
      const created = await attempt(() => api.duplicateTerminal(id, placementBeside(windowsOf(ws, get().apps), source)));
      if (!created.ok) return null;
      const copy = created.value;
      set((s) => ({
        workspaces: mapWorkspace(s.workspaces, ws.id, (w) => ({ ...w, terminals: [...w.terminals, copy] })),
      }));
      return copy;
    },

    async closeTerminal(id) {
      const { ok } = await attempt(() => api.closeTerminal(id));
      if (!ok) return;
      set((s) => ({
        workspaces: s.workspaces.map((ws) => ({
          ...ws,
          terminals: ws.terminals.filter((t) => t.id !== id),
          links: ws.links.filter((l) => l.sourceId !== id && l.targetId !== id),
        })),
        selectedTerminalIds: s.selectedTerminalIds.filter((t) => t !== id),
      }));
    },

    async closeTerminals(ids) {
      await Promise.all(ids.map((id) => get().closeTerminal(id)));
    },

    async renameTerminal(id, name) {
      const trimmed = name.trim();
      if (!trimmed) return;
      set((s) => ({ workspaces: mapTerminal(s.workspaces, id, (t) => ({ ...t, name: trimmed })) }));
      await attempt(() => api.renameTerminal(id, trimmed));
    },

    async commitTerminalRect(id, rect) {
      set((s) => ({ workspaces: mapTerminal(s.workspaces, id, (t) => ({ ...t, ...rect })) }));
      const saved = await attempt(() => api.updateTerminalRect(id, rect));
      if (saved.ok) set((s) => ({ workspaces: mapTerminal(s.workspaces, id, (t) => ({ ...t, ...saved.value })) }));
    },

    focusTerminal(id) {
      const ws = get().workspaces.find((w) => w.terminals.some((t) => t.id === id));
      const terminal = ws?.terminals.find((t) => t.id === id);
      if (!ws || !terminal) return;
      const z = frontZ(ws, get().apps, id);
      if (terminal.z >= z) return; // already in front
      set((s) => ({ workspaces: mapTerminal(s.workspaces, id, (t) => ({ ...t, z })) }));
      void attempt(() => api.focusTerminal(id));
    },

    async openApp(launch, label, nearTerminalId) {
      const ws = activeWorkspace(get());
      if (!ws) return null;
      const waiting = `Opening ${label}…`;
      set({ toast: waiting });
      const opened = await attempt(() => api.openApp(launch));
      if (!opened.ok) return null;
      if (get().toast === waiting) set({ toast: null });
      // The workspace may have been deleted while the program started.
      const current = get().workspaces.find((w) => w.id === ws.id);
      if (!current) {
        void api.releaseApp(opened.value.id).catch(() => null);
        return null;
      }
      const near = nearTerminalId ? current.terminals.find((t) => t.id === nearTerminalId) : undefined;
      return addCard(opened.value, current, near);
    },

    async closeApp(id) {
      await attempt(() => api.closeApp(id));
    },

    async releaseApp(id) {
      const { ok } = await attempt(() => api.releaseApp(id));
      if (ok) dropApp(id);
    },

    moveApp(id, rect) {
      const width = Math.max(MIN_APP_WIDTH, rect.width);
      const height = Math.max(MIN_APP_HEIGHT, rect.height);
      mapApp(id, (a) => ({ ...a, ...rect, width, height }));
    },

    focusApp(id) {
      const app = get().apps.find((a) => a.id === id);
      const ws = get().workspaces.find((w) => w.id === app?.workspaceId);
      if (!app || !ws) return;
      const z = frontZ(ws, get().apps, id);
      if (app.z < z) mapApp(id, (a) => ({ ...a, z }));
      void attempt(() => api.focusApp(id));
    },

    async createLink(sourceId, targetId) {
      // The backend validates and answers with a `links-changed` event.
      await attempt(() => api.createLink(sourceId, targetId));
    },

    async deleteLink(id) {
      set((s) => ({
        workspaces: s.workspaces.map((ws) => ({ ...ws, links: ws.links.filter((l) => l.id !== id) })),
        selectedLinkId: s.selectedLinkId === id ? null : s.selectedLinkId,
      }));
      await attempt(() => api.deleteLink(id));
    },

    async createKnowledgeBase() {
      const ws = activeWorkspace(get());
      if (!ws) return;
      const picked = await attempt(() => api.pickPath(true));
      if (!picked.ok) return;
      const path = picked.value;
      if (!path) return;
      const rect = placementFor(windowsOf(ws, get().apps), DEFAULT_KB_WIDTH, DEFAULT_KB_HEIGHT);
      // The backend validates the folder and answers with a knowledge-bases-changed event.
      await attempt(() => api.createKnowledgeBase(ws.id, path, rect));
    },

    async removeKnowledgeBase(id) {
      // Optimistic: drop the node and the arrows touching it; the event confirms.
      set((s) => ({
        workspaces: s.workspaces.map((ws) => ({
          ...ws,
          knowledgeBases: (ws.knowledgeBases ?? []).filter((k) => k.id !== id),
          links: ws.links.filter((l) => l.sourceId !== id && l.targetId !== id),
        })),
      }));
      await attempt(() => api.removeKnowledgeBase(id));
    },

    async renameKnowledgeBase(id, name) {
      const trimmed = name.trim();
      if (!trimmed) return;
      set((s) => ({ workspaces: mapKnowledgeBase(s.workspaces, id, (k) => ({ ...k, name: trimmed })) }));
      await attempt(() => api.renameKnowledgeBase(id, trimmed));
    },

    async commitKnowledgeBaseRect(id, rect) {
      set((s) => ({ workspaces: mapKnowledgeBase(s.workspaces, id, (k) => ({ ...k, ...rect })) }));
      const saved = await attempt(() => api.updateKnowledgeBaseRect(id, rect));
      if (saved.ok) set((s) => ({ workspaces: mapKnowledgeBase(s.workspaces, id, (k) => ({ ...k, ...saved.value })) }));
    },

    selectLink(id) {
      set(id ? { selectedLinkId: id, selectedTerminalIds: [] } : { selectedLinkId: null });
    },

    selectTerminals(ids) {
      const current = get().selectedTerminalIds;
      if (ids.length === current.length && ids.every((id) => current.includes(id))) return;
      set(ids.length > 0 ? { selectedTerminalIds: ids, selectedLinkId: null } : { selectedTerminalIds: [] });
    },

    toggleTerminalSelection(id) {
      const current = get().selectedTerminalIds;
      get().selectTerminals(current.includes(id) ? current.filter((t) => t !== id) : [...current, id]);
    },

    toggleLinks() {
      set((s) => ({ linksVisible: !s.linksVisible, selectedLinkId: null }));
    },

    dismissSubagent(id) {
      set((s) => ({ dismissedSubagents: [...s.dismissedSubagents, id] }));
    },

    moveSubagent(id, rect) {
      set((s) => ({ movedSubagents: { ...s.movedSubagents, [id]: rect } }));
    },

    toggleCaptureLaunches() {
      const captureLaunches = !get().captureLaunches;
      writeFlag(CAPTURE_LAUNCHES_KEY, captureLaunches);
      set({ captureLaunches });
      void attempt(() => api.setCaptureLaunches(captureLaunches));
    },

    setAgentView(agentView) {
      if (get().agentView === agentView) return;
      writeFlag(AGENT_VIEW_KEY, agentView);
      set({ agentView });
      // The terminals that change say so (terminal-changed).
      void attempt(() => api.setAgentView(agentView));
    },

    async agentToClassic(id) {
      const changed = await attempt(() => api.agentToClassic(id));
      if (changed.ok) {
        set((s) => ({ workspaces: mapTerminal(s.workspaces, id, (t) => ({ ...t, agent: changed.value.agent ?? null })) }));
      }
    },

    toggleTransparentCanvas() {
      const transparentCanvas = !get().transparentCanvas;
      writeFlag(TRANSPARENT_CANVAS_KEY, transparentCanvas);
      set({ transparentCanvas });
    },
  };
});
