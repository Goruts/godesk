import type { Rect } from "../bindings/Rect";
import { boundsOf, fitBounds, zoomAt } from "./camera";
import { viewport } from "./viewport";

/** Zooms around the center of the visible area. */
export function zoomBy(factor: number): void {
  const { width, height } = viewport.getBounds();
  viewport.set(zoomAt(viewport.get(), { x: width / 2, y: height / 2 }, viewport.zoom * factor));
}

export function resetZoom(): void {
  const { width, height } = viewport.getBounds();
  viewport.set(zoomAt(viewport.get(), { x: width / 2, y: height / 2 }, 1));
}

/** Shows `rect` at 100 %, centered. */
export function focusRect(rect: Rect): void {
  const { width, height } = viewport.getBounds();
  viewport.set({ x: width / 2 - (rect.x + rect.width / 2), y: height / 2 - (rect.y + rect.height / 2), zoom: 1 });
}

/** Frames every given rectangle; does nothing for an empty workspace. */
export function fitToContent(rects: Rect[]): void {
  const bounds = boundsOf(rects);
  if (!bounds) return;
  viewport.set(fitBounds(bounds, viewport.getBounds()));
}
