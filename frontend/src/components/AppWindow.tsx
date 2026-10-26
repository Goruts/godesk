import { memo, useLayoutEffect, useRef } from "react";
import type { PointerEvent as ReactPointerEvent } from "react";
import type { Rect } from "../bindings/Rect";
import { focusRect } from "../canvas/actions";
import { dragOffset } from "../canvas/camera";
import { RESIZE_DIRECTIONS, type ResizeDirection, resizeRect } from "../canvas/geometry";
import { trackDrag } from "../canvas/interaction";
import { layout } from "../canvas/layout";
import { viewport } from "../canvas/viewport";
import { type CanvasApp, useAppStore } from "../store/appStore";
import { PopOutIcon } from "./icons";

const CURSORS: Record<ResizeDirection, string> = {
  n: "ns-resize",
  s: "ns-resize",
  e: "ew-resize",
  w: "ew-resize",
  ne: "nesw-resize",
  sw: "nesw-resize",
  nw: "nwse-resize",
  se: "nwse-resize",
};

function rectOf(a: CanvasApp): Rect {
  return { x: a.x, y: a.y, width: a.width, height: a.height };
}

function applyRect(el: HTMLElement, rect: Rect): void {
  el.style.left = `${rect.x}px`;
  el.style.top = `${rect.y}px`;
  el.style.width = `${rect.width}px`;
  el.style.height = `${rect.height}px`;
}

/** "Code.exe" → "Code". */
function programLabel(program: string): string {
  return program.replace(/\.exe$/i, "") || "Program";
}

type Props = { app: CanvasApp };

/**
 * The card of another program's window. Godesk draws the frame (title bar,
 * resize handles); the program's own window is laid over the body by the
 * backend (see canvas/appPlacement.ts). Zoomed too far out to use, the body
 * shows the window's title instead.
 */
export const AppWindow = memo(function AppWindow({ app }: Props) {
  const ref = useRef<HTMLDivElement>(null);
  const focusApp = useAppStore((s) => s.focusApp);
  const moveApp = useAppStore((s) => s.moveApp);
  const closeApp = useAppStore((s) => s.closeApp);
  const releaseApp = useAppStore((s) => s.releaseApp);
  const preview = useAppStore((s) => s.appPreviews[app.id]);
  const { id } = app;
  const label = programLabel(app.program);

  // Publish the rectangle so placement follows drags without a render.
  useLayoutEffect(() => {
    layout.set(id, rectOf(app));
  }, [id, app.x, app.y, app.width, app.height]);
  useLayoutEffect(() => () => layout.delete(id), [id]);

  /** Moves/resizes the DOM node directly while dragging; commits on release. */
  const startGeometryDrag = (e: ReactPointerEvent, cursor: string, next: (start: Rect, dx: number, dy: number) => Rect) => {
    const el = ref.current;
    if (!el || e.button !== 0) return;
    e.preventDefault();
    e.stopPropagation();
    focusApp(id);
    const start = rectOf(app);
    let current = start;
    const origin = viewport.clientToWorld(e.clientX, e.clientY);
    let clientX = e.clientX;
    let clientY = e.clientY;
    const update = () => {
      const d = dragOffset(viewport.get(), origin, viewport.clientToScreen(clientX, clientY));
      current = next(start, d.x, d.y);
      applyRect(el, current);
      layout.set(id, current);
    };
    const unsubscribe = viewport.subscribe(update);
    trackDrag(
      e,
      cursor,
      (_dx, _dy, ev) => {
        clientX = ev.clientX;
        clientY = ev.clientY;
        update();
      },
      () => {
        unsubscribe();
        if (current !== start) moveApp(id, current);
      },
    );
  };

  const onTitlePointerDown = (e: ReactPointerEvent) => {
    if ((e.target as HTMLElement).closest("button")) return;
    startGeometryDrag(e, "grabbing", (start, dx, dy) => ({ ...start, x: start.x + dx, y: start.y + dy }));
  };

  const onResizePointerDown = (direction: ResizeDirection) => (e: ReactPointerEvent) =>
    startGeometryDrag(e, CURSORS[direction], (start, dx, dy) => resizeRect(start, direction, dx, dy));

  return (
    <div
      ref={ref}
      className="term-window app-window"
      role="group"
      aria-label={app.title}
      data-app-id={id}
      style={{ left: app.x, top: app.y, width: app.width, height: app.height, zIndex: app.z }}
      onPointerDown={(e) => {
        if (e.button === 0) focusApp(id);
      }}
    >
      <div className="term-titlebar" onPointerDown={onTitlePointerDown}>
        <span className="app-program">{label}</span>
        <span className="term-title" title={app.title}>
          {app.title}
        </span>
        <button
          type="button"
          className="term-action"
          aria-label={`Give ${label} back to the desktop`}
          title="Back to the desktop"
          onClick={() => void releaseApp(id)}
        >
          <PopOutIcon />
        </button>
        <button
          type="button"
          className="term-close"
          aria-label={`Close ${app.title}`}
          title={`Close ${label}`}
          onClick={() => void closeApp(id)}
        >
          ×
        </button>
      </div>
      <div className="term-body app-body">
        {/* Shown while the window is hidden (zoomed out): a click brings it back. */}
        <button
          type="button"
          className="app-placeholder"
          title={`Zoom to ${label}`}
          onClick={() => {
            focusRect(rectOf(app));
            focusApp(id);
          }}
        >
          {preview ? (
            <img className="app-preview" src={preview} alt="" draggable={false} />
          ) : (
            <>
              <span className="app-placeholder-program">{label}</span>
              <span className="app-placeholder-title">{app.title}</span>
            </>
          )}
        </button>
      </div>
      {RESIZE_DIRECTIONS.map((direction) => (
        <div
          key={direction}
          className={`term-resize term-resize-${direction}`}
          aria-hidden="true"
          onPointerDown={onResizePointerDown(direction)}
        />
      ))}
    </div>
  );
});
