import type { Rect } from "../bindings/Rect";
import { containsPoint } from "./geometry";
import { layout } from "./layout";
import { activeWorkspace, useAppStore } from "../store/appStore";

/**
 * Topmost node of the active workspace under a canvas point: a terminal or a
 * knowledge base. Used when drawing an arrow, so either kind can be an
 * endpoint. The live `layout` rect wins over the committed model rect, so a
 * node being dragged is hit where it is drawn.
 */
export function nodeAt(point: { x: number; y: number }, exceptId: string): string | null {
  const ws = activeWorkspace(useAppStore.getState());
  if (!ws) return null;
  const nodes: Array<{ id: string; z: number; rect: Rect }> = [
    ...ws.terminals.map((t) => ({ id: t.id, z: t.z, rect: { x: t.x, y: t.y, width: t.width, height: t.height } })),
    ...(ws.knowledgeBases ?? []).map((k) => ({
      id: k.id,
      z: k.z,
      rect: { x: k.x, y: k.y, width: k.width, height: k.height },
    })),
  ];
  const hit = nodes
    .filter((n) => n.id !== exceptId)
    .filter((n) => containsPoint(layout.get(n.id) ?? n.rect, point))
    .sort((a, b) => b.z - a.z)[0];
  return hit?.id ?? null;
}
