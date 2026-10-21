import type { Point } from "./camera";

/**
 * `to` is the canvas point under the pointer; `targetId` is the terminal the
 * link would connect to if released there.
 */
export type Draft = { sourceId: string; to: Point; targetId: string | null } | null;
type Listener = (draft: Draft) => void;

/** The link being dragged out of a terminal's handle, drawn by the link layer. */
class LinkDraft {
  private draft: Draft = null;
  private readonly listeners = new Set<Listener>();

  update(sourceId: string, to: Point, targetId: string | null): void {
    this.draft = { sourceId, to, targetId };
    this.notify();
  }

  clear(): void {
    this.draft = null;
    this.notify();
  }

  get(): Draft {
    return this.draft;
  }

  subscribe(listener: Listener): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  private notify(): void {
    for (const listener of this.listeners) listener(this.draft);
  }
}

export const linkDraft = new LinkDraft();
