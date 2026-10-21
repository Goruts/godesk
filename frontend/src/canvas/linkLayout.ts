import type { Link } from "../bindings/Link";
import type { Rect } from "../bindings/Rect";
import { MIN_ZOOM, boundsOf } from "./camera";
import { type LinkGeometry, draftGeometry, linkGeometry } from "./geometry";
import type { Draft } from "./linkDraft";

/** A link with activity this recent is drawn animated and highlighted. */
export const ACTIVE_LINK_MS = 5_000;
/** Sideways offset (canvas px) separating A→B from B→A. */
export const BIDIRECTIONAL_BEND = 28;
/**
 * Room around the curves for what is drawn past their centerline (hit
 * stroke, glow, arrowheads): 16 screen px, in canvas px at the lowest zoom.
 */
export const BOUNDS_MARGIN = 16 / MIN_ZOOM;

export function isLinkActive(link: Pick<Link, "lastActivityAt">, now: number): boolean {
  return now - link.lastActivityAt < ACTIVE_LINK_MS;
}

export type LinkPlan = {
  /** Curve of each link by id; null when an endpoint is not on the canvas. */
  links: Map<string, LinkGeometry | null>;
  draft: LinkGeometry | null;
  /** Canvas area holding everything drawn, plus margin; null when empty. */
  bounds: Rect | null;
};

const pairKey = (sourceId: string, targetId: string) => `${sourceId}→${targetId}`;

/**
 * Lays out the link layer: one curve per link between the closest edges of
 * its terminals (bowed apart when both directions exist), and the link being
 * drawn, which snaps to its final curve while over a target.
 */
export function planLinks(links: Link[], rectOf: (terminalId: string) => Rect | undefined, draft: Draft): LinkPlan {
  const pairs = new Set(links.map((l) => pairKey(l.sourceId, l.targetId)));
  const curve = (sourceId: string, targetId: string, source: Rect, target: Rect) =>
    linkGeometry(source, target, pairs.has(pairKey(targetId, sourceId)) ? BIDIRECTIONAL_BEND : 0);

  const drawn: Rect[] = [];
  const geometries = new Map<string, LinkGeometry | null>();
  for (const link of links) {
    const source = rectOf(link.sourceId);
    const target = rectOf(link.targetId);
    const geometry = source && target ? curve(link.sourceId, link.targetId, source, target) : null;
    geometries.set(link.id, geometry);
    if (geometry) drawn.push(geometry.bounds);
  }

  let draftCurve: LinkGeometry | null = null;
  const draftSource = draft && rectOf(draft.sourceId);
  if (draft && draftSource) {
    const target = draft.targetId === null ? undefined : rectOf(draft.targetId);
    draftCurve =
      draft.targetId !== null && target
        ? curve(draft.sourceId, draft.targetId, draftSource, target)
        : draftGeometry(draftSource, draft.to);
    drawn.push(draftCurve.bounds);
  }

  const box = boundsOf(drawn);
  const bounds = box && {
    x: box.x - BOUNDS_MARGIN,
    y: box.y - BOUNDS_MARGIN,
    width: box.width + 2 * BOUNDS_MARGIN,
    height: box.height + 2 * BOUNDS_MARGIN,
  };
  return { links: geometries, draft: draftCurve, bounds };
}
