import type { AppPlacement } from "../bindings/AppPlacement";
import type { Rect } from "../bindings/Rect";
import { api } from "../lib/api";
import { type AppStore, useAppStore } from "../store/appStore";
import { intersects } from "./geometry";
import { layout } from "./layout";
import { viewport } from "./viewport";

/**
 * Below this zoom, or this size on screen (CSS px), a program's window is
 * hidden and its card shows what it is instead. Another program's content
 * cannot be scaled: it would stay full size, cropped, while everything else
 * shrinks.
 */
const MIN_SHOWN_ZOOM = 0.75;
const MIN_SHOWN_WIDTH = 480;
const MIN_SHOWN_HEIGHT = 320;
/** Page elements drawn above every window. */
const OVERLAYS = ".dock, .popover, .dialog, .toast, .marquee:not([hidden])";
/** Menus, dialogs and toasts come and go without telling anyone. */
const OVERLAY_POLL_MS = 250;

function clientRect(el: Element): Rect {
  const r = el.getBoundingClientRect();
  return { x: r.left, y: r.top, width: r.width, height: r.height };
}

const HIDDEN = { x: 0, y: 0, width: 0, height: 0 };

/**
 * Where each program's window goes, in client pixels: over its card's body
 * when its workspace shows and it is big enough to use, with holes where the
 * dock, menus and windows in front of it are drawn.
 */
export function collectPlacements(state: Pick<AppStore, "apps" | "workspaces" | "activeWorkspaceId">): AppPlacement[] {
  const { apps, workspaces, activeWorkspaceId } = state;
  if (apps.length === 0) return [];
  const overlays = [...document.querySelectorAll(OVERLAYS)].map(clientRect);
  const ws = workspaces.find((w) => w.id === activeWorkspaceId);
  return apps.map((app) => {
    const body = document.querySelector(`[data-app-id="${app.id}"] .app-body`);
    if (!ws || app.workspaceId !== ws.id || !body) return { id: app.id, rect: HIDDEN, visible: false, cutouts: [] };
    const rect = clientRect(body);
    const inFront = [
      ...ws.terminals.filter((t) => t.z > app.z).map((t) => `[data-terminal-id="${t.id}"]`),
      ...apps.filter((a) => a.workspaceId === ws.id && a.z > app.z).map((a) => `[data-app-id="${a.id}"]`),
    ]
      .map((selector) => document.querySelector(selector))
      .filter((el) => el !== null)
      .map(clientRect);
    return {
      id: app.id,
      rect,
      visible: viewport.zoom >= MIN_SHOWN_ZOOM && rect.width >= MIN_SHOWN_WIDTH && rect.height >= MIN_SHOWN_HEIGHT,
      cutouts: [...overlays, ...inFront].filter((r) => r.width > 0 && r.height > 0 && intersects(r, rect)),
    };
  });
}

/**
 * Keeps the native windows on their cards: once per frame after anything
 * that moves a card on screen (camera, drag, store, window size), and on a
 * slow poll for overlays. Unchanged placements are not sent again.
 */
export function installAppPlacement(): () => void {
  let frame = 0;
  let last = "";
  const flush = () => {
    frame = 0;
    const placements = collectPlacements(useAppStore.getState());
    const key = JSON.stringify(placements);
    if (key === last) return;
    last = key;
    // The card shows what the window is while the window is hidden.
    for (const p of placements) {
      document.querySelector(`[data-app-id="${p.id}"]`)?.toggleAttribute("data-native-hidden", !p.visible);
    }
    if (placements.length > 0) void api.placeApps(placements).catch(() => undefined);
  };
  const schedule = () => {
    if (!frame) frame = requestAnimationFrame(flush);
  };
  const unsubscribe = [viewport.subscribe(schedule), layout.subscribe(schedule), useAppStore.subscribe(schedule)];
  window.addEventListener("resize", schedule);
  const timer = window.setInterval(schedule, OVERLAY_POLL_MS);
  return () => {
    for (const stop of unsubscribe) stop();
    window.removeEventListener("resize", schedule);
    window.clearInterval(timer);
    if (frame) cancelAnimationFrame(frame);
  };
}
