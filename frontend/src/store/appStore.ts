import { create } from "zustand";
import type { Camera } from "../bindings/Camera";
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
const CASCADE_OFFSET = 32;
const DUPLICATE_GAP = 24;
const TRANSPARENT_CANVAS_KEY = "godesk.transparentCanvas";

type AppState = {
  ready: boolean;
  workspaces: Workspace[];
  activeWorkspaceId: string;
  linksVisible: boolean;
  /** The canvas lets the desktop behind the window show through. */
  transparentCanvas: boolean;
  selectedLinkId: string | null;
  /** Windows picked for a bulk action (Shift+click a title, Shift+drag). */
  selectedTerminalIds: string[];
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
  /** Opens a copy of a terminal beside it; resolves to the copy, or null on failure. */
  duplicateTerminal: (id: string) => Promise<TerminalWindow | null>;
  closeTerminal: (id: string) => Promise<void>;
  closeTerminals: (ids: string[]) => Promise<void>;
  renameTerminal: (id: string, name: string) => Promise<void>;
  commitTerminalRect: (id: string, rect: Rect) => Promise<void>;
  focusTerminal: (id: string) => void;

  createLink: (sourceId: string, targetId: string) => Promise<void>;
  deleteLink: (id: string) => Promise<void>;
  selectLink: (id: string | null) => void;
  /** Replaces the terminal selection; a non-empty one drops the link selection. */
  selectTerminals: (ids: string[]) => void;
  toggleTerminalSelection: (id: string) => void;
  toggleLinks: () => void;
  toggleTransparentCanvas: () => void;
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

/** A spot centered in the visible area, cascading over windows already there. */
function placementFor(ws: Workspace, width = DEFAULT_TERMINAL_WIDTH, height = DEFAULT_TERMINAL_HEIGHT): Rect {
  const center = viewport.center();
  let x = Math.round(center.x - width / 2);
  let y = Math.round(center.y - height / 2);
  while (ws.terminals.some((t) => Math.abs(t.x - x) < 4 && Math.abs(t.y - y) < 4)) {
    x += CASCADE_OFFSET;
    y += CASCADE_OFFSET;
  }
  return { x, y, width, height };
}

/** Right of the source when that spot is free and on screen; otherwise centered. Same size either way. */
function placementBeside(ws: Workspace, source: TerminalWindow): Rect {
  const beside = { x: source.x + source.width + DUPLICATE_GAP, y: source.y, width: source.width, height: source.height };
  const free = !ws.terminals.some((t) => intersects(t, beside));
  return free && containsRect(viewport.visibleRect(), beside) ? beside : placementFor(ws, source.width, source.height);
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

  function setLinks(workspaceId: string, links: Link[]) {
    set((s) => ({
      workspaces: mapWorkspace(s.workspaces, workspaceId, (ws) => ({ ...ws, links })),
      selectedLinkId: links.some((l) => l.id === s.selectedLinkId) ? s.selectedLinkId : null,
    }));
  }

  return {
    ready: false,
    workspaces: [],
    activeWorkspaceId: "",
    linksVisible: true,
    transparentCanvas: readFlag(TRANSPARENT_CANVAS_KEY, true),
    selectedLinkId: null,
    selectedTerminalIds: [],
    toast: null,

    async init() {
      if (initialized) return;
      initialized = true;
      await events.onLinksChanged((e) => setLinks(e.workspaceId, e.links));
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
      set((s) => ({
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
      const created = await attempt(() => api.createTerminal(ws.id, placementFor(ws)));
      if (!created.ok) return null;
      const terminal = created.value;
      set((s) => ({
        workspaces: mapWorkspace(s.workspaces, ws.id, (w) => ({ ...w, terminals: [...w.terminals, terminal] })),
      }));
      return terminal;
    },

    async duplicateTerminal(id) {
      const ws = get().workspaces.find((w) => w.terminals.some((t) => t.id === id));
      const source = ws?.terminals.find((t) => t.id === id);
      if (!ws || !source) return null;
      const created = await attempt(() => api.duplicateTerminal(id, placementBeside(ws, source)));
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
      const topOther = Math.max(0, ...ws.terminals.filter((t) => t.id !== id).map((t) => t.z));
      if (terminal.z > topOther) return; // already in front
      set((s) => ({ workspaces: mapTerminal(s.workspaces, id, (t) => ({ ...t, z: topOther + 1 })) }));
      void attempt(() => api.focusTerminal(id));
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

    toggleTransparentCanvas() {
      const transparentCanvas = !get().transparentCanvas;
      writeFlag(TRANSPARENT_CANVAS_KEY, transparentCanvas);
      set({ transparentCanvas });
    },
  };
});
