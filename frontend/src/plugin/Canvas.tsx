import { useEffect, useRef } from "react";
import { useShallow } from "zustand/react/shallow";
import { LinkLayer } from "../components/LinkLayer";
import { panBy, wheelPixels, wheelZoomFactor, zoomAt } from "../canvas/camera";
import { viewport } from "../canvas/viewport";
import { TerminalNode } from "./TerminalNode";
import { allLinks, useCanvas } from "./store";

const CAMERA_PERSIST_MS = 400;

/**
 * The infinite board. Pan and zoom live in the `viewport` singleton and are
 * written straight to the world layer's transform, outside React — the same
 * design godesk's own canvas uses, so dragging and zooming never re-render the
 * nodes. The store hears the camera only occasionally, to persist it.
 */
export function Canvas() {
  const rootRef = useRef<HTMLDivElement>(null);
  const worldRef = useRef<HTMLDivElement>(null);
  const persistTimer = useRef(0);

  const { nodes, logs, focusedId, manualLinks, detectedLinks, selectedLinkId } = useCanvas(
    useShallow((s) => ({
      nodes: s.nodes,
      logs: s.logs,
      focusedId: s.focusedId,
      manualLinks: s.manualLinks,
      detectedLinks: s.detectedLinks,
      selectedLinkId: s.selectedLinkId,
    })),
  );
  const setCamera = useCanvas((s) => s.setCamera);
  const selectLink = useCanvas((s) => s.selectLink);
  const deleteLink = useCanvas((s) => s.deleteLink);

  // Camera → transform, outside React; the store is told (debounced) to persist.
  useEffect(() => {
    const applyBounds = () => {
      const el = rootRef.current;
      if (!el) return;
      const r = el.getBoundingClientRect();
      viewport.setBounds({ left: r.left, top: r.top, width: r.width, height: r.height });
    };
    applyBounds();
    const unsub = viewport.subscribe(({ camera, fromUser }) => {
      const world = worldRef.current;
      if (world) {
        world.style.transform = `translate(${camera.x}px, ${camera.y}px) scale(${camera.zoom})`;
        // So a stroke of `calc(2px * var(--inv-zoom))` keeps a constant width on
        // screen however far the world is zoomed (the link layer relies on this).
        world.style.setProperty("--inv-zoom", String(1 / camera.zoom));
      }
      if (fromUser) {
        window.clearTimeout(persistTimer.current);
        persistTimer.current = window.setTimeout(() => setCamera(viewport.get()), CAMERA_PERSIST_MS);
      }
    });
    // Load the saved camera once, after bounds are known.
    viewport.load("plugin", useCanvas.getState().camera);
    const onResize = () => applyBounds();
    window.addEventListener("resize", onResize);
    return () => {
      unsub();
      window.removeEventListener("resize", onResize);
    };
  }, [setCamera]);

  const onWheel = (e: React.WheelEvent) => {
    e.preventDefault();
    const screen = viewport.clientToScreen(e.clientX, e.clientY);
    if (e.ctrlKey || e.metaKey) {
      const factor = wheelZoomFactor(wheelPixels(e.deltaY, e.deltaMode));
      viewport.set(zoomAt(viewport.get(), screen, viewport.zoom * factor));
    } else {
      viewport.set(panBy(viewport.get(), -wheelPixels(e.deltaX, e.deltaMode), -wheelPixels(e.deltaY, e.deltaMode)));
    }
  };

  // Middle-button (or background left) drag pans the canvas.
  const onPointerDown = (e: React.PointerEvent) => {
    const background = e.target === rootRef.current || e.target === worldRef.current;
    if (e.button !== 1 && !(e.button === 0 && background)) return;
    if (background && e.button === 0) selectLink(null);
    const startCam = viewport.get();
    const startX = e.clientX;
    const startY = e.clientY;
    const move = (ev: PointerEvent) => {
      viewport.set({ ...startCam, x: startCam.x + (ev.clientX - startX), y: startCam.y + (ev.clientY - startY) });
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };

  // Delete removes the selected manual link (detected links are not the user's).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.key === "Delete" || e.key === "Backspace") && selectedLinkId) {
        deleteLink(selectedLinkId);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [selectedLinkId, deleteLink]);

  const links = allLinks({ manualLinks, detectedLinks });

  return (
    <div ref={rootRef} className="canvas-root" onWheel={onWheel} onPointerDown={onPointerDown}>
      <div ref={worldRef} className="canvas-world">
        <LinkLayer
          links={links}
          terminals={nodes}
          visible
          selectedId={selectedLinkId}
          onSelect={selectLink}
        />
        {nodes.map((node) => (
          <TerminalNode key={node.id} node={node} focused={node.id === focusedId} logs={logs[node.id]} />
        ))}
      </div>
    </div>
  );
}
