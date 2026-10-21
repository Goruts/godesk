import type { Rect } from "../bindings/Rect";
import type { Point } from "./camera";

export const MIN_TERMINAL_WIDTH = 320;
export const MIN_TERMINAL_HEIGHT = 200;

export type ResizeDirection = "n" | "s" | "e" | "w" | "ne" | "nw" | "se" | "sw";

export const RESIZE_DIRECTIONS: readonly ResizeDirection[] = ["n", "s", "e", "w", "ne", "nw", "se", "sw"];

/**
 * Resizes `start` by a canvas-space delta from the given edge/corner, keeping
 * the opposite edges fixed and respecting the minimum size.
 */
export function resizeRect(start: Rect, direction: ResizeDirection, dx: number, dy: number): Rect {
  let { x, y, width, height } = start;
  if (direction.includes("e")) {
    width = Math.max(MIN_TERMINAL_WIDTH, start.width + dx);
  }
  if (direction.includes("s")) {
    height = Math.max(MIN_TERMINAL_HEIGHT, start.height + dy);
  }
  if (direction.includes("w")) {
    width = Math.max(MIN_TERMINAL_WIDTH, start.width - dx);
    x = start.x + start.width - width;
  }
  if (direction.includes("n")) {
    height = Math.max(MIN_TERMINAL_HEIGHT, start.height - dy);
    y = start.y + start.height - height;
  }
  return { x, y, width, height };
}

export function containsPoint(rect: Rect, p: Point): boolean {
  return p.x >= rect.x && p.x <= rect.x + rect.width && p.y >= rect.y && p.y <= rect.y + rect.height;
}

/** The rectangle spanned by two opposite corners, given in any order. */
export function rectFromCorners(a: Point, b: Point): Rect {
  return { x: Math.min(a.x, b.x), y: Math.min(a.y, b.y), width: Math.abs(a.x - b.x), height: Math.abs(a.y - b.y) };
}

/** True when `inner` lies entirely inside `outer`. */
export function containsRect(outer: Rect, inner: Rect): boolean {
  return (
    inner.x >= outer.x &&
    inner.y >= outer.y &&
    inner.x + inner.width <= outer.x + outer.width &&
    inner.y + inner.height <= outer.y + outer.height
  );
}

/** True when the rectangles overlap; edges that only touch do not count. */
export function intersects(a: Rect, b: Rect): boolean {
  return a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height;
}

type Side = "top" | "right" | "bottom" | "left";
type Anchor = Point & { side: Side };

const NORMALS: Record<Side, Point> = {
  top: { x: 0, y: -1 },
  right: { x: 1, y: 0 },
  bottom: { x: 0, y: 1 },
  left: { x: -1, y: 0 },
};

function anchors(r: Rect): Anchor[] {
  return [
    { x: r.x + r.width / 2, y: r.y, side: "top" },
    { x: r.x + r.width, y: r.y + r.height / 2, side: "right" },
    { x: r.x + r.width / 2, y: r.y + r.height, side: "bottom" },
    { x: r.x, y: r.y + r.height / 2, side: "left" },
  ];
}

export type LinkGeometry = {
  /** SVG path data of the curve. */
  d: string;
  start: Point;
  end: Point;
  /** Point halfway along the curve, for labels. */
  mid: Point;
  /** Box containing the whole curve (its control points' hull). */
  bounds: Rect;
};

function boxOf(points: Point[]): Rect {
  const xs = points.map((p) => p.x);
  const ys = points.map((p) => p.y);
  const x = Math.min(...xs);
  const y = Math.min(...ys);
  return { x, y, width: Math.max(...xs) - x, height: Math.max(...ys) - y };
}

function closestAnchor(r: Rect, to: Point): Anchor {
  let best = anchors(r)[0]!;
  let bestDistance = Infinity;
  for (const a of anchors(r)) {
    const distance = Math.hypot(a.x - to.x, a.y - to.y);
    if (distance < bestDistance) {
      bestDistance = distance;
      best = a;
    }
  }
  return best;
}

/**
 * Curve joining the closest pair of edge midpoints of two rectangles, leaving
 * and entering perpendicular to those edges. `sideways` bows the curve to the
 * right of its direction; since the direction flips, A→B and B→A drawn with
 * the same value separate instead of overlapping.
 */
export function linkGeometry(source: Rect, target: Rect, sideways = 0): LinkGeometry {
  let best: [Anchor, Anchor] | null = null;
  let bestDistance = Infinity;
  for (const a of anchors(source)) {
    for (const b of anchors(target)) {
      const distance = Math.hypot(a.x - b.x, a.y - b.y);
      if (distance < bestDistance) {
        bestDistance = distance;
        best = [a, b];
      }
    }
  }
  const [a, b] = best!;
  const bend = Math.max(40, bestDistance * 0.4);
  // Unit vector to the right of the a→b direction (screen y points down).
  const length = Math.max(1, bestDistance);
  const right = { x: -(b.y - a.y) / length, y: (b.x - a.x) / length };
  const c1 = {
    x: a.x + NORMALS[a.side].x * bend + right.x * sideways,
    y: a.y + NORMALS[a.side].y * bend + right.y * sideways,
  };
  const c2 = {
    x: b.x + NORMALS[b.side].x * bend + right.x * sideways,
    y: b.y + NORMALS[b.side].y * bend + right.y * sideways,
  };
  // Cubic Bézier at t = 0.5.
  const mid = {
    x: 0.125 * a.x + 0.375 * c1.x + 0.375 * c2.x + 0.125 * b.x,
    y: 0.125 * a.y + 0.375 * c1.y + 0.375 * c2.y + 0.125 * b.y,
  };
  const d = `M ${a.x} ${a.y} C ${c1.x} ${c1.y}, ${c2.x} ${c2.y}, ${b.x} ${b.y}`;
  return { d, start: a, end: b, mid, bounds: boxOf([a, c1, c2, b]) };
}

/** Straight line from the closest edge midpoint of `source` to a free point. */
export function draftGeometry(source: Rect, to: Point): LinkGeometry {
  const a = closestAnchor(source, to);
  const mid = { x: (a.x + to.x) / 2, y: (a.y + to.y) / 2 };
  return { d: `M ${a.x} ${a.y} L ${to.x} ${to.y}`, start: a, end: to, mid, bounds: boxOf([a, to]) };
}
