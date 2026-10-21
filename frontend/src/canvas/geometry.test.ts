import { describe, expect, it } from "vitest";
import {
  MIN_TERMINAL_HEIGHT,
  MIN_TERMINAL_WIDTH,
  containsPoint,
  draftGeometry,
  intersects,
  linkGeometry,
  rectFromCorners,
  resizeRect,
} from "./geometry";

const start = { x: 100, y: 100, width: 600, height: 400 };

describe("resizeRect", () => {
  it("grows from the south-east corner", () => {
    expect(resizeRect(start, "se", 50, 20)).toEqual({ x: 100, y: 100, width: 650, height: 420 });
  });

  it("keeps the opposite edge fixed when resizing from the north-west", () => {
    const r = resizeRect(start, "nw", 30, 40);
    expect(r).toEqual({ x: 130, y: 140, width: 570, height: 360 });
    expect(r.x + r.width).toBe(start.x + start.width);
  });

  it("enforces the minimum size without moving the fixed edge", () => {
    const r = resizeRect(start, "w", 10_000, 0);
    expect(r.width).toBe(MIN_TERMINAL_WIDTH);
    expect(r.x + r.width).toBe(start.x + start.width);
    expect(resizeRect(start, "s", 0, -10_000).height).toBe(MIN_TERMINAL_HEIGHT);
  });
});

describe("linkGeometry", () => {
  it("joins the facing edges of side-by-side terminals", () => {
    const left = { x: 0, y: 0, width: 400, height: 300 };
    const right = { x: 600, y: 0, width: 400, height: 300 };
    const g = linkGeometry(left, right);
    expect(g.start).toMatchObject({ x: 400, y: 150 });
    expect(g.end).toMatchObject({ x: 600, y: 150 });
    expect(g.mid.x).toBeCloseTo(500);
    expect(g.d.startsWith("M 400 150 C")).toBe(true);
  });

  it("bows opposite directions of the same pair to opposite sides", () => {
    const left = { x: 0, y: 0, width: 400, height: 300 };
    const right = { x: 600, y: 0, width: 400, height: 300 };
    const forward = linkGeometry(left, right, 30);
    const backward = linkGeometry(right, left, 30);
    expect(forward.mid.y).toBeGreaterThan(150);
    expect(backward.mid.y).toBeLessThan(150);
    expect(linkGeometry(left, right).mid.y).toBeCloseTo(150);
  });

  it("joins bottom to top for stacked terminals", () => {
    const top = { x: 0, y: 0, width: 400, height: 300 };
    const bottom = { x: 0, y: 500, width: 400, height: 300 };
    const g = linkGeometry(top, bottom);
    expect(g.start).toMatchObject({ x: 200, y: 300 });
    expect(g.end).toMatchObject({ x: 200, y: 500 });
  });

  it("bounds the whole curve, bend included", () => {
    const left = { x: 0, y: 0, width: 400, height: 300 };
    const right = { x: 600, y: 0, width: 400, height: 300 };
    const { bounds, start, end, mid } = linkGeometry(left, right, 30);
    for (const p of [start, end, mid]) expect(containsPoint(bounds, p)).toBe(true);
    // The sideways bow pushes the control points below the straight line.
    expect(bounds.y + bounds.height).toBeGreaterThan(150);
  });
});

describe("draftGeometry", () => {
  it("draws a straight line from the edge closest to the pointer", () => {
    const source = { x: 0, y: 0, width: 400, height: 300 };
    const g = draftGeometry(source, { x: 200, y: 700 });
    expect(g.start).toMatchObject({ x: 200, y: 300 });
    expect(g.d).toBe("M 200 300 L 200 700");
    expect(g.bounds).toEqual({ x: 200, y: 300, width: 0, height: 400 });
  });
});

describe("containsPoint", () => {
  it("includes edges", () => {
    expect(containsPoint(start, { x: 100, y: 500 })).toBe(true);
    expect(containsPoint(start, { x: 99, y: 200 })).toBe(false);
  });
});

describe("marquee geometry", () => {
  it("spans two corners dragged in any direction", () => {
    expect(rectFromCorners({ x: 300, y: 50 }, { x: 100, y: 200 })).toEqual({ x: 100, y: 50, width: 200, height: 150 });
  });

  it("catches a window the box only partly covers, but not one it merely touches", () => {
    const window = { x: 100, y: 100, width: 600, height: 400 };

    expect(intersects({ x: 650, y: 450, width: 100, height: 100 }, window)).toBe(true);
    expect(intersects({ x: 700, y: 100, width: 50, height: 50 }, window)).toBe(false);
    expect(intersects({ x: 0, y: 0, width: 50, height: 50 }, window)).toBe(false);
  });
});
