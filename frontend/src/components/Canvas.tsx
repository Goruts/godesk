import { useEffect, useLayoutEffect, useRef } from "react";
import type { PointerEvent as ReactPointerEvent } from "react";
import { LOW_ZOOM, panBy, wheelPixels, wheelZoomFactor, zoomAt } from "../canvas/camera";
import { createCameraSaver } from "../canvas/cameraSaver";
import { intersects, rectFromCorners } from "../canvas/geometry";
import { isTypingTarget, trackDrag } from "../canvas/interaction";
import { layout } from "../canvas/layout";
import { viewport } from "../canvas/viewport";
import { events } from "../lib/api";
import { MOD_KEY } from "../lib/platform";
import { activeWorkspace, useAppStore } from "../store/appStore";
import { LinkLayer } from "./LinkLayer";
import { TerminalWindow } from "./TerminalWindow";

const CAMERA_SAVE_DEBOUNCE_MS = 300;
const EMPTY: never[] = [];

/** True for pointer targets that are the canvas itself rather than a window or link. */
function isBackground(target: EventTarget | null, root: HTMLElement): boolean {
  if (!(target instanceof Element)) return false;
  return target === root || target.classList.contains("world") || target.classList.contains("link-layer");
}

/**
 * Infinite canvas: a container layer transformed with
 * `translate(x, y) scale(zoom)` holding the link layer and the windows.
 */
export function Canvas() {
  const rootRef = useRef<HTMLDivElement>(null);
  const worldRef = useRef<HTMLDivElement>(null);
  const marqueeRef = useRef<HTMLDivElement>(null);
  const spaceHeld = useRef(false);

  const workspaces = useAppStore((s) => s.workspaces);
  const ws = useAppStore(activeWorkspace);
  const terminals = ws?.terminals ?? EMPTY;
  const links = ws?.links ?? EMPTY;
  const workspaceId = ws?.id ?? "";
  const linksVisible = useAppStore((s) => s.linksVisible);
  const selectedLinkId = useAppStore((s) => s.selectedLinkId);
  const selectLink = useAppStore((s) => s.selectLink);
  const selectTerminals = useAppStore((s) => s.selectTerminals);
  const saveCamera = useAppStore((s) => s.saveCamera);

  // Camera → DOM, with no React render.
  useLayoutEffect(() => {
    const world = worldRef.current;
    if (!world) return;
    const saver = createCameraSaver(saveCamera, CAMERA_SAVE_DEBOUNCE_MS);
    const unsubscribe = viewport.subscribe((change) => {
      const { camera } = change;
      world.style.transform = `translate(${camera.x}px, ${camera.y}px) scale(${camera.zoom})`;
      world.style.setProperty("--inv-zoom", String(1 / camera.zoom));
      world.dataset["lowZoom"] = String(camera.zoom < LOW_ZOOM);
      saver.onChange(change);
    });
    return () => {
      unsubscribe();
      saver.flush();
    };
  }, [saveCamera]);

  // Show the workspace's own camera when switching. A terminal left focused
  // in the workspace just hidden would otherwise keep receiving keys.
  useLayoutEffect(() => {
    const current = activeWorkspace(useAppStore.getState());
    if (current) viewport.load(current.id, current.camera);
    const focused = document.activeElement;
    if (focused instanceof HTMLElement && focused.closest(".workspace-layer[inert]")) focused.blur();
  }, [workspaceId]);

  // Keep the viewport's screen bounds current.
  useLayoutEffect(() => {
    const root = rootRef.current;
    if (!root) return;
    const update = () => {
      const r = root.getBoundingClientRect();
      viewport.setBounds({ left: r.left, top: r.top, width: r.width, height: r.height });
    };
    update();
    const observer = new ResizeObserver(update);
    observer.observe(root);
    window.addEventListener("resize", update);
    return () => {
      observer.disconnect();
      window.removeEventListener("resize", update);
    };
  }, []);

  // Wheel: Ctrl/Cmd (and trackpad pinch, which arrives as ctrl+wheel) zooms
  // anywhere; plain scrolling pans, except over a terminal, which scrolls its
  // own history.
  useEffect(() => {
    const root = rootRef.current;
    if (!root) return;
    const onWheel = (e: WheelEvent) => {
      if (e.ctrlKey || e.metaKey) {
        e.preventDefault();
        e.stopPropagation();
        const anchor = viewport.clientToScreen(e.clientX, e.clientY);
        const factor = wheelZoomFactor(wheelPixels(e.deltaY, e.deltaMode));
        viewport.set(zoomAt(viewport.get(), anchor, viewport.zoom * factor));
        return;
      }
      if (e.target instanceof Element && e.target.closest(".term-body")) return;
      e.preventDefault();
      let dx = wheelPixels(e.deltaX, e.deltaMode);
      let dy = wheelPixels(e.deltaY, e.deltaMode);
      if (e.shiftKey && dx === 0) [dx, dy] = [dy, 0];
      viewport.set(panBy(viewport.get(), -dx, -dy));
    };

    // Pinch gestures report their scale relative to where they began.
    let pinchStartZoom = 1;
    const pinchTo = (scale: number, clientX: number, clientY: number) => {
      const anchor = viewport.clientToScreen(clientX, clientY);
      viewport.set(zoomAt(viewport.get(), anchor, pinchStartZoom * scale));
    };

    // Safari/WebKit (macOS) pinch gestures.
    type GestureEvent = UIEvent & { scale: number; clientX: number; clientY: number };
    const onGestureStart = (e: Event) => {
      e.preventDefault();
      pinchStartZoom = viewport.zoom;
    };
    const onGestureChange = (e: Event) => {
      e.preventDefault();
      const g = e as GestureEvent;
      pinchTo(g.scale, g.clientX, g.clientY);
    };

    // Linux: WebKitGTK keeps pinches to itself, so the backend forwards them.
    const unlistenPinch = events
      .onCanvasPinch((e) => {
        if (e.phase === "begin") pinchStartZoom = viewport.zoom;
        if (e.phase !== "end") pinchTo(e.scale, e.x, e.y);
      })
      .catch(() => undefined);

    // Middle-button panning must not reach xterm (paste / mouse reporting).
    const swallowMiddle = (e: MouseEvent) => {
      if (e.button === 1) {
        e.preventDefault();
        e.stopPropagation();
      }
    };

    root.addEventListener("wheel", onWheel, { passive: false, capture: true });
    root.addEventListener("gesturestart", onGestureStart);
    root.addEventListener("gesturechange", onGestureChange);
    for (const type of ["mousedown", "mouseup", "auxclick"] as const) {
      root.addEventListener(type, swallowMiddle, true);
    }
    return () => {
      root.removeEventListener("wheel", onWheel, { capture: true });
      root.removeEventListener("gesturestart", onGestureStart);
      root.removeEventListener("gesturechange", onGestureChange);
      void unlistenPinch.then((unlisten) => unlisten?.());
      for (const type of ["mousedown", "mouseup", "auxclick"] as const) {
        root.removeEventListener(type, swallowMiddle, true);
      }
    };
  }, []);

  // Space + drag pans; track the key outside text fields and terminals.
  useEffect(() => {
    const root = rootRef.current;
    const setSpace = (held: boolean) => {
      spaceHeld.current = held;
      if (root) root.dataset["spacePan"] = String(held);
    };
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.code === "Space" && !isTypingTarget(e.target)) {
        e.preventDefault();
        setSpace(true);
      }
    };
    const onKeyUp = (e: KeyboardEvent) => {
      if (e.code === "Space") setSpace(false);
    };
    const onBlur = () => setSpace(false);
    window.addEventListener("keydown", onKeyDown);
    window.addEventListener("keyup", onKeyUp);
    window.addEventListener("blur", onBlur);
    return () => {
      window.removeEventListener("keydown", onKeyDown);
      window.removeEventListener("keyup", onKeyUp);
      window.removeEventListener("blur", onBlur);
    };
  }, []);

  const startPan = (e: ReactPointerEvent) => {
    e.preventDefault();
    e.stopPropagation();
    // Pan by increments of the live camera so a zoom made mid-pan (wheel,
    // pinch) is kept instead of being reset to the zoom the pan started at.
    let lastX = 0;
    let lastY = 0;
    trackDrag(
      e,
      "grabbing",
      (dx, dy) => {
        viewport.set(panBy(viewport.get(), dx - lastX, dy - lastY));
        lastX = dx;
        lastY = dy;
      },
      () => undefined,
    );
  };

  // Middle button, or space + left button, pans from anywhere, windows included.
  const onPointerDownCapture = (e: ReactPointerEvent) => {
    if (e.button === 1 || (e.button === 0 && spaceHeld.current)) startPan(e);
  };

  /** Shift+drag on the background selects every window the box touches. */
  const startMarquee = (e: ReactPointerEvent) => {
    const root = rootRef.current;
    const box = marqueeRef.current;
    if (!root || !box) return;
    e.preventDefault();
    const bounds = root.getBoundingClientRect();
    const from = { x: e.clientX, y: e.clientY };
    const before = useAppStore.getState().selectedTerminalIds;
    const update = (to: { x: number; y: number }) => {
      const screen = rectFromCorners(from, to);
      box.style.left = `${screen.x - bounds.left}px`;
      box.style.top = `${screen.y - bounds.top}px`;
      box.style.width = `${screen.width}px`;
      box.style.height = `${screen.height}px`;
      box.hidden = false;
      const area = rectFromCorners(viewport.clientToWorld(from.x, from.y), viewport.clientToWorld(to.x, to.y));
      const hits = terminals.filter((t) => intersects(layout.get(t.id) ?? t, area)).map((t) => t.id);
      selectTerminals([...new Set([...before, ...hits])]);
    };
    trackDrag(
      e,
      "crosshair",
      (_dx, _dy, ev) => update({ x: ev.clientX, y: ev.clientY }),
      () => void (box.hidden = true),
    );
  };

  const onPointerDown = (e: ReactPointerEvent) => {
    const root = rootRef.current;
    if (!root || e.button !== 0 || !isBackground(e.target, root)) return;
    // Leave terminals so the space bar and shortcuts reach the canvas.
    if (document.activeElement instanceof HTMLElement) document.activeElement.blur();
    selectLink(null);
    if (e.shiftKey) startMarquee(e);
    else selectTerminals([]);
  };

  // Focus follows the mouse back out of a terminal (see TerminalWindow): over
  // the background the canvas takes the keyboard, so Space-pan and the link
  // keys work where the mouse is. A rename field keeps its focus.
  const onPointerOver = (e: ReactPointerEvent) => {
    const root = rootRef.current;
    if (!root || e.pointerType !== "mouse" || e.buttons !== 0 || !isBackground(e.target, root)) return;
    const active = document.activeElement;
    if (active instanceof HTMLElement && active.closest(".term-body")) active.blur();
  };

  return (
    <div
      ref={rootRef}
      className="canvas"
      onPointerDownCapture={onPointerDownCapture}
      onPointerDown={onPointerDown}
      onPointerOver={onPointerOver}
    >
      <div ref={worldRef} className="world" data-links-hidden={linksVisible ? undefined : ""}>
        <LinkLayer
          links={links}
          terminals={terminals}
          visible={linksVisible}
          selectedId={selectedLinkId}
          onSelect={selectLink}
        />
        {/* Every workspace stays mounted and only the active one shows:
            rebuilding xterm (renderer, scrollback replay) on each switch
            cost ~50 ms per terminal in a single frame. */}
        {workspaces.map((w) => (
          <div key={w.id} className="workspace-layer" inert={w.id !== workspaceId}>
            {w.terminals.map((t) => (
              <TerminalWindow key={t.id} terminal={t} />
            ))}
          </div>
        ))}
      </div>
      {ws && terminals.length === 0 && (
        <div className="canvas-empty">
          <p>This workspace is empty.</p>
          <p>
            Press <kbd>{MOD_KEY}</kbd> + <kbd>Shift</kbd> + <kbd>T</kbd> or the <strong>+</strong> button below.
          </p>
        </div>
      )}
      <div ref={marqueeRef} className="marquee" hidden />
      <div className="interaction-shield" />
    </div>
  );
}
