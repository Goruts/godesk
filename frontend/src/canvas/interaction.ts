import type { PointerEvent as ReactPointerEvent } from "react";

/**
 * While dragging, resizing, panning or drawing a link, a full-screen shield is
 * shown and terminals stop receiving the mouse, so xterm never swallows the
 * pointer stream and no event is lost.
 */
export function beginInteraction(cursor: string): () => void {
  const root = document.documentElement;
  root.dataset["interacting"] = "true";
  root.style.setProperty("--interaction-cursor", cursor);
  return () => {
    delete root.dataset["interacting"];
    root.style.removeProperty("--interaction-cursor");
  };
}

/**
 * Tracks a pointer drag on `element` with pointer capture and the
 * interaction shield. `onMove` receives the screen delta from the start.
 */
export function trackDrag(
  event: PointerEvent | ReactPointerEvent,
  cursor: string,
  onMove: (dx: number, dy: number, e: PointerEvent) => void,
  onEnd: (e: PointerEvent) => void,
): void {
  const element = event.currentTarget as HTMLElement;
  const pointerId = event.pointerId;
  const startX = event.clientX;
  const startY = event.clientY;
  element.setPointerCapture(pointerId);
  const end = beginInteraction(cursor);

  const move = (e: PointerEvent) => {
    if (e.pointerId !== pointerId) return;
    onMove(e.clientX - startX, e.clientY - startY, e);
  };
  const up = (e: PointerEvent) => {
    if (e.pointerId !== pointerId) return;
    element.removeEventListener("pointermove", move);
    element.removeEventListener("pointerup", up);
    element.removeEventListener("pointercancel", up);
    if (element.hasPointerCapture(pointerId)) element.releasePointerCapture(pointerId);
    end();
    onEnd(e);
  };
  element.addEventListener("pointermove", move);
  element.addEventListener("pointerup", up);
  element.addEventListener("pointercancel", up);
}

/** True when keyboard focus is in a text field or a terminal. */
export function isTypingTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  return target.isContentEditable || target.tagName === "INPUT" || target.tagName === "TEXTAREA";
}
