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
  // Pointer capture keeps the cursor and xterm behaving, but the drag must not
  // depend on it: if capture is lost or the handle's node is replaced by a
  // re-render, a listener bound to `element` would never see the release and the
  // interaction shield would stay up, freezing the whole canvas. So the move and
  // end listeners live on `window`, filtered by pointerId, and run exactly once.
  try {
    element.setPointerCapture(pointerId);
  } catch {
    // Capture is best-effort; the window listeners below still end the drag.
  }
  const end = beginInteraction(cursor);

  let ended = false;
  const move = (e: PointerEvent) => {
    if (e.pointerId !== pointerId) return;
    onMove(e.clientX - startX, e.clientY - startY, e);
  };
  const up = (e: PointerEvent) => {
    if (e.pointerId !== pointerId || ended) return;
    ended = true;
    window.removeEventListener("pointermove", move);
    window.removeEventListener("pointerup", up);
    window.removeEventListener("pointercancel", up);
    element.removeEventListener("lostpointercapture", up);
    window.removeEventListener("blur", onBlur);
    if (element.hasPointerCapture(pointerId)) element.releasePointerCapture(pointerId);
    end();
    onEnd(e);
  };
  // Window losing focus (alt-tab, OS dialog) drops the pointer stream; end the
  // drag so the shield never gets stuck.
  const onBlur = () => up(new PointerEvent("pointercancel", { pointerId }));
  window.addEventListener("pointermove", move);
  window.addEventListener("pointerup", up);
  window.addEventListener("pointercancel", up);
  element.addEventListener("lostpointercapture", up);
  window.addEventListener("blur", onBlur);
}

/** True when keyboard focus is in a text field or a terminal. */
export function isTypingTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  return target.isContentEditable || target.tagName === "INPUT" || target.tagName === "TEXTAREA";
}
