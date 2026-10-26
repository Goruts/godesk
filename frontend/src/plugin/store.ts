/**
 * The canvas state, backed by Goia's engine through the SDK.
 *
 * godesk's own store drives its own backend (create_terminal, workspaces, links
 * detected in Rust). Here there is no godesk backend: the nodes ARE Goia's agent
 * sessions, discovered by polling (Goia emits no fleet-change event), and the
 * things godesk kept in Rust that Goia has no place for — where each node sits,
 * which links the user drew — are the plugin's own, persisted as opaque state
 * Goia keeps but never reads (`stateGet`/`stateSet`).
 */
import { create } from "zustand";
import type { Camera } from "../bindings/Camera";
import type { Link } from "../bindings/Link";
import type { Rect } from "../bindings/Rect";
import type { TerminalWindow } from "../bindings/TerminalWindow";
import { sdk, type Session, type TermSpan } from "./sdk";

const NODE_W = 420;
const NODE_H = 300;
const GRID_GAP = 40;
const FLEET_POLL_MS = 2_000;
const LOG_POLL_MS = 1_200;
const LOG_TAIL_LINES = 200;
const PERSIST_DEBOUNCE_MS = 500;

/** What the plugin keeps between sessions, as opaque state inside Goia. */
interface Persisted {
  camera: Camera;
  /** Where each session's node sits, by session id. */
  positions: Record<string, Rect>;
  /** The links the user drew by hand (detected links are never persisted). */
  manualLinks: Link[];
}

interface CanvasState {
  sessions: Session[];
  /** One node per session, in canvas coordinates. */
  nodes: TerminalWindow[];
  logs: Record<string, TermSpan[][]>;
  manualLinks: Link[];
  detectedLinks: Link[];
  camera: Camera;
  selectedLinkId: string | null;
  /** The node attached live (interactive xterm); others show the polled mirror. */
  focusedId: string | null;
  loaded: boolean;

  init: () => () => void;
  setCamera: (camera: Camera) => void;
  moveNode: (id: string, rect: Rect) => void;
  focus: (id: string | null) => void;
  addLink: (sourceId: string, targetId: string) => void;
  deleteLink: (id: string) => void;
  selectLink: (id: string | null) => void;
}

let positions: Record<string, Rect> = {};
let persistTimer = 0;
let topZ = 1;

function autoPlace(index: number): Rect {
  // A simple grid for a session we have never seen; the user drags from there.
  const cols = 3;
  return {
    x: (index % cols) * (NODE_W + GRID_GAP),
    y: Math.floor(index / cols) * (NODE_H + GRID_GAP),
    width: NODE_W,
    height: NODE_H,
  };
}

/** Build the node list from the live sessions and whatever positions we have. */
function nodesFrom(sessions: Session[]): TerminalWindow[] {
  return sessions.map((s, i) => {
    const rect = positions[s.id] ?? (positions[s.id] = autoPlace(i));
    return {
      id: s.id,
      name: s.name || s.id,
      x: rect.x,
      y: rect.y,
      width: rect.width,
      height: rect.height,
      z: ++topZ,
      agent: s.provider,
    };
  });
}

export const useCanvas = create<CanvasState>((set, get) => {
  const persist = () => {
    window.clearTimeout(persistTimer);
    persistTimer = window.setTimeout(() => {
      const snapshot: Persisted = {
        camera: get().camera,
        positions,
        manualLinks: get().manualLinks,
      };
      void sdk.stateSet(JSON.stringify(snapshot)).catch(() => {});
    }, PERSIST_DEBOUNCE_MS);
  };

  return {
    sessions: [],
    nodes: [],
    logs: {},
    manualLinks: [],
    detectedLinks: [],
    camera: { x: 0, y: 0, zoom: 1 },
    selectedLinkId: null,
    focusedId: null,
    loaded: false,

    init: () => {
      let fleetTimer = 0;
      let logTimer = 0;
      let active = true;

      void (async () => {
        // Restore the saved layout first, so nodes land where they were.
        try {
          const raw = await sdk.stateGet();
          if (raw) {
            const saved = JSON.parse(raw) as Partial<Persisted>;
            if (saved.positions) positions = saved.positions;
            if (saved.camera) set({ camera: saved.camera });
            if (saved.manualLinks) set({ manualLinks: saved.manualLinks });
          }
        } catch {
          /* no saved state, or not hosted — the empty canvas is fine */
        }
        set({ loaded: true });

        const pollFleet = async () => {
          try {
            const sessions = await sdk.sessionsList();
            if (!active) return;
            set({ sessions, nodes: nodesFrom(sessions) });
          } catch {
            /* keep what we have */
          }
          if (active) fleetTimer = window.setTimeout(pollFleet, FLEET_POLL_MS);
        };

        const pollLogs = async () => {
          const ids = get().sessions.filter((s) => s.streamable).map((s) => s.id);
          if (ids.length > 0) {
            try {
              const bulk = await sdk.sessionsLogsBulk(ids, LOG_TAIL_LINES);
              if (!active) return;
              const logs = { ...get().logs };
              for (const l of bulk) if (!l.error) logs[l.id] = l.lines;
              set({ logs });
            } catch {
              /* keep what we have */
            }
          }
          if (active) logTimer = window.setTimeout(pollLogs, LOG_POLL_MS);
        };

        // Detected links from the inter-agent bus: who spoke to whom, recently.
        try {
          const beats = await sdk.busRecent(100);
          if (active) {
            const links: Link[] = [];
            const seen = new Set<string>();
            for (const b of beats) {
              if (!b.from || !b.to) continue;
              const key = `${b.from}→${b.to}`;
              if (seen.has(key)) continue;
              seen.add(key);
              links.push({
                id: `detected:${key}`,
                sourceId: b.from,
                targetId: b.to,
                kind: "detected",
                lastActivityAt: b.at ?? Date.now(),
              });
            }
            set({ detectedLinks: links });
          }
        } catch {
          /* bus is optional */
        }

        void pollFleet();
        void pollLogs();
      })();

      return () => {
        active = false;
        window.clearTimeout(fleetTimer);
        window.clearTimeout(logTimer);
      };
    },

    setCamera: (camera) => {
      set({ camera });
      persist();
    },

    moveNode: (id, rect) => {
      positions[id] = rect;
      set({ nodes: get().nodes.map((n) => (n.id === id ? { ...n, ...rect } : n)) });
      persist();
    },

    focus: (id) => set({ focusedId: id }),

    addLink: (sourceId, targetId) => {
      if (sourceId === targetId) return;
      const id = `manual:${sourceId}→${targetId}`;
      if (get().manualLinks.some((l) => l.id === id)) return;
      const link: Link = { id, sourceId, targetId, kind: "manual", lastActivityAt: Date.now() };
      set({ manualLinks: [...get().manualLinks, link] });
      persist();
    },

    deleteLink: (id) => {
      set({ manualLinks: get().manualLinks.filter((l) => l.id !== id), selectedLinkId: null });
      persist();
    },

    selectLink: (id) => set({ selectedLinkId: id }),
  };
});

/** Manual and detected links together, for drawing. */
export function allLinks(s: Pick<CanvasState, "manualLinks" | "detectedLinks">): Link[] {
  return [...s.manualLinks, ...s.detectedLinks];
}
