import type { Rect } from "../bindings/Rect";

type Listener = () => void;

/**
 * Live canvas rectangles of the mounted terminal windows. Drag and resize
 * update it every pointer move; the link layer subscribes to redraw lines
 * without a React render.
 */
class LayoutRegistry {
  private readonly rects = new Map<string, Rect>();
  private readonly listeners = new Set<Listener>();

  get(id: string): Rect | undefined {
    return this.rects.get(id);
  }

  set(id: string, rect: Rect): void {
    this.rects.set(id, rect);
    this.notify();
  }

  delete(id: string): void {
    if (this.rects.delete(id)) this.notify();
  }

  subscribe(listener: Listener): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  private notify(): void {
    for (const listener of this.listeners) listener();
  }
}

export const layout = new LayoutRegistry();
