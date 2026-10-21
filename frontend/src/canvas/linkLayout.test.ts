import { describe, expect, it } from "vitest";
import type { Link } from "../bindings/Link";
import type { Rect } from "../bindings/Rect";
import { ACTIVE_LINK_MS, BOUNDS_MARGIN, isLinkActive, planLinks } from "./linkLayout";
import { linkGeometry } from "./geometry";

const rects: Record<string, Rect> = {
  a: { x: 0, y: 0, width: 400, height: 300 },
  b: { x: 600, y: 0, width: 400, height: 300 },
  c: { x: 0, y: 600, width: 400, height: 300 },
};
const rectOf = (id: string) => rects[id];

function link(id: string, sourceId: string, targetId: string, kind: Link["kind"] = "manual"): Link {
  return { id, sourceId, targetId, kind, lastActivityAt: 0 };
}

describe("planLinks", () => {
  it("draws one curve per link between the closest edges", () => {
    const plan = planLinks([link("ab", "a", "b"), link("ac", "a", "c", "detected")], rectOf, null);
    expect(plan.links.get("ab")).toEqual(linkGeometry(rects.a!, rects.b!));
    expect(plan.links.get("ac")?.start).toMatchObject({ x: 200, y: 300 });
    expect(plan.draft).toBeNull();
  });

  it("leaves out links whose terminal is not on the canvas", () => {
    const plan = planLinks([link("ax", "a", "gone")], rectOf, null);
    expect(plan.links.get("ax")).toBeNull();
    expect(plan.bounds).toBeNull();
  });

  it("bows only pairs linked in both directions, to opposite sides", () => {
    const plan = planLinks([link("ab", "a", "b"), link("ba", "b", "a"), link("ac", "a", "c")], rectOf, null);
    expect(plan.links.get("ab")!.mid.y).toBeGreaterThan(150);
    expect(plan.links.get("ba")!.mid.y).toBeLessThan(150);
    expect(plan.links.get("ac")).toEqual(linkGeometry(rects.a!, rects.c!));
  });

  it("draws the link being dragged straight to the pointer", () => {
    const plan = planLinks([], rectOf, { sourceId: "a", to: { x: 500, y: 150 }, targetId: null });
    expect(plan.draft?.d).toBe("M 400 150 L 500 150");
  });

  it("snaps the link being dragged to its final curve over a target", () => {
    const draft = { sourceId: "a", to: { x: 700, y: 100 }, targetId: "b" };
    expect(planLinks([], rectOf, draft).draft).toEqual(linkGeometry(rects.a!, rects.b!));
    // Dropping it would make the pair bidirectional, so the preview bows too.
    expect(planLinks([link("ba", "b", "a")], rectOf, draft).draft!.mid.y).toBeGreaterThan(150);
  });

  it("bounds every curve and the draft, with room for strokes", () => {
    const draft = { sourceId: "c", to: { x: -300, y: 1200 }, targetId: null };
    const { bounds, links } = planLinks([link("ab", "a", "b")], rectOf, draft);
    const curve = links.get("ab")!.bounds;
    expect(bounds!.x).toBe(-300 - BOUNDS_MARGIN);
    expect(bounds!.y).toBe(curve.y - BOUNDS_MARGIN);
    expect(bounds!.x + bounds!.width).toBe(curve.x + curve.width + BOUNDS_MARGIN);
    expect(bounds!.y + bounds!.height).toBe(1200 + BOUNDS_MARGIN);
  });
});

describe("isLinkActive", () => {
  it("is active only within the recent-activity window", () => {
    const now = 1_000_000;
    expect(isLinkActive({ lastActivityAt: now - 1 }, now)).toBe(true);
    expect(isLinkActive({ lastActivityAt: now - ACTIVE_LINK_MS + 1 }, now)).toBe(true);
    expect(isLinkActive({ lastActivityAt: now - ACTIVE_LINK_MS }, now)).toBe(false);
  });
});
