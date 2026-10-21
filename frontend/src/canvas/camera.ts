import type { Camera } from "../bindings/Camera";
import type { Rect } from "../bindings/Rect";

// A canvas point `p` is drawn on screen at `p * zoom + (x, y)`, matching the
// `translate(x, y) scale(zoom)` transform of the world layer.

export const MIN_ZOOM = 0.1;
export const MAX_ZOOM = 4;
/** Below this zoom terminals are unreadable and a whole window is its drag handle. */
export const LOW_ZOOM = 0.4;

export type Point = { x: number; y: number };
export type Size = { width: number; height: number };

export function clampZoom(zoom: number): number {
  if (!Number.isFinite(zoom)) return 1;
  return Math.min(MAX_ZOOM, Math.max(MIN_ZOOM, zoom));
}

export function screenToWorld(camera: Camera, point: Point): Point {
  return { x: (point.x - camera.x) / camera.zoom, y: (point.y - camera.y) / camera.zoom };
}

export function worldToScreen(camera: Camera, point: Point): Point {
  return { x: point.x * camera.zoom + camera.x, y: point.y * camera.zoom + camera.y };
}

/** Zooms keeping the canvas point under `anchor` (screen coordinates) fixed. */
export function zoomAt(camera: Camera, anchor: Point, nextZoom: number): Camera {
  const zoom = clampZoom(nextZoom);
  const world = screenToWorld(camera, anchor);
  return { x: anchor.x - world.x * zoom, y: anchor.y - world.y * zoom, zoom };
}

/**
 * Canvas offset from `origin`, the canvas point grabbed when a drag began, to
 * the canvas point now under `pointer` (screen coordinates). Reading it with
 * the live camera keeps the grabbed point under the pointer at any zoom, even
 * if the camera pans or zooms mid-drag.
 */
export function dragOffset(camera: Camera, origin: Point, pointer: Point): Point {
  const p = screenToWorld(camera, pointer);
  return { x: p.x - origin.x, y: p.y - origin.y };
}

export function panBy(camera: Camera, dx: number, dy: number): Camera {
  return { ...camera, x: camera.x + dx, y: camera.y + dy };
}

/** Converts a wheel delta to pixels regardless of the event's delta mode. */
export function wheelPixels(delta: number, deltaMode: number): number {
  if (deltaMode === 1) return delta * 16; // lines
  if (deltaMode === 2) return delta * 400; // pages
  return delta;
}

/** Multiplicative zoom step for a vertical wheel/pinch delta in pixels. */
export function wheelZoomFactor(deltaPixels: number): number {
  return Math.exp(-deltaPixels * 0.0015);
}

export function boundsOf(rects: Iterable<Rect>): Rect | null {
  let minX = Infinity;
  let minY = Infinity;
  let maxX = -Infinity;
  let maxY = -Infinity;
  for (const r of rects) {
    minX = Math.min(minX, r.x);
    minY = Math.min(minY, r.y);
    maxX = Math.max(maxX, r.x + r.width);
    maxY = Math.max(maxY, r.y + r.height);
  }
  if (minX === Infinity) return null;
  return { x: minX, y: minY, width: maxX - minX, height: maxY - minY };
}

/** Camera that frames `bounds` inside a viewport, never zooming past `maxZoom`. */
export function fitBounds(bounds: Rect, viewport: Size, padding = 64, maxZoom = 1): Camera {
  const availableW = Math.max(1, viewport.width - padding * 2);
  const availableH = Math.max(1, viewport.height - padding * 2);
  const zoom = clampZoom(Math.min(availableW / bounds.width, availableH / bounds.height, maxZoom));
  const centerX = bounds.x + bounds.width / 2;
  const centerY = bounds.y + bounds.height / 2;
  return { x: viewport.width / 2 - centerX * zoom, y: viewport.height / 2 - centerY * zoom, zoom };
}
