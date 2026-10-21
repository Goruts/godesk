import { describe, expect, it } from "vitest";
import {
  MAX_ZOOM,
  MIN_ZOOM,
  boundsOf,
  clampZoom,
  dragOffset,
  fitBounds,
  panBy,
  screenToWorld,
  worldToScreen,
  zoomAt,
} from "./camera";

describe("camera", () => {
  it("round-trips screen and world coordinates", () => {
    const camera = { x: 120, y: -40, zoom: 2.5 };
    const p = { x: 33, y: 77 };
    const back = worldToScreen(camera, screenToWorld(camera, p));
    expect(back.x).toBeCloseTo(p.x);
    expect(back.y).toBeCloseTo(p.y);
  });

  it("keeps the point under the cursor fixed while zooming", () => {
    const camera = { x: 50, y: 20, zoom: 1 };
    const cursor = { x: 400, y: 300 };
    const before = screenToWorld(camera, cursor);
    const next = zoomAt(camera, cursor, 2);
    const after = screenToWorld(next, cursor);
    expect(next.zoom).toBe(2);
    expect(after.x).toBeCloseTo(before.x);
    expect(after.y).toBeCloseTo(before.y);
  });

  it("clamps zoom to 10%–400%", () => {
    expect(clampZoom(0.01)).toBe(MIN_ZOOM);
    expect(clampZoom(10)).toBe(MAX_ZOOM);
    expect(clampZoom(Number.NaN)).toBe(1);
    expect(zoomAt({ x: 0, y: 0, zoom: 1 }, { x: 0, y: 0 }, 99).zoom).toBe(MAX_ZOOM);
  });

  it("pans in screen pixels", () => {
    expect(panBy({ x: 1, y: 2, zoom: 3 }, 10, -5)).toEqual({ x: 11, y: -3, zoom: 3 });
  });

  it("converts drag distance to canvas units at any zoom", () => {
    for (const zoom of [MIN_ZOOM, 0.25, 1, 2.5, MAX_ZOOM]) {
      const camera = { x: -300, y: 120, zoom };
      const grab = { x: 500, y: 400 };
      const origin = screenToWorld(camera, grab);
      const offset = dragOffset(camera, origin, { x: grab.x + 100, y: grab.y - 40 });
      expect(offset.x).toBeCloseTo(100 / zoom);
      expect(offset.y).toBeCloseTo(-40 / zoom);
    }
  });

  it("keeps the grabbed point under the pointer when the camera changes mid-drag", () => {
    const before = { x: 40, y: 10, zoom: 0.5 };
    const pointer = { x: 600, y: 350 };
    const origin = screenToWorld(before, { x: 520, y: 300 });
    const after = panBy(zoomAt(before, { x: 100, y: 100 }, 3), 75, -20);
    const offset = dragOffset(after, origin, pointer);
    const grabbedNow = worldToScreen(after, { x: origin.x + offset.x, y: origin.y + offset.y });
    expect(grabbedNow.x).toBeCloseTo(pointer.x);
    expect(grabbedNow.y).toBeCloseTo(pointer.y);
  });

  it("frames content and centers it", () => {
    const bounds = boundsOf([
      { x: 0, y: 0, width: 400, height: 300 },
      { x: 1000, y: 500, width: 400, height: 300 },
    ])!;
    expect(bounds).toEqual({ x: 0, y: 0, width: 1400, height: 800 });
    const viewport = { width: 1000, height: 600 };
    const camera = fitBounds(bounds, viewport, 50);
    const center = worldToScreen(camera, { x: 700, y: 400 });
    expect(center.x).toBeCloseTo(500);
    expect(center.y).toBeCloseTo(300);
    expect(camera.zoom).toBeCloseTo(Math.min(900 / 1400, 500 / 800));
    expect(fitBounds({ x: 0, y: 0, width: 10, height: 10 }, viewport).zoom).toBe(1);
    expect(boundsOf([])).toBeNull();
  });
});
