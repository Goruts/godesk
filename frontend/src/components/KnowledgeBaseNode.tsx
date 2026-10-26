import { memo, useEffect, useLayoutEffect, useRef, useState } from "react";
import type { MouseEvent as ReactMouseEvent, PointerEvent as ReactPointerEvent } from "react";
import type { Rect } from "../bindings/Rect";
import type { KnowledgeBase } from "../bindings/KnowledgeBase";
import { dragOffset } from "../canvas/camera";
import { RESIZE_DIRECTIONS, type ResizeDirection, resizeRect } from "../canvas/geometry";
import { layout } from "../canvas/layout";
import { nodeAt } from "../canvas/nodes";
import { trackDrag } from "../canvas/interaction";
import { viewport } from "../canvas/viewport";
import { linkDraft } from "../canvas/linkDraft";
import { useAppStore } from "../store/appStore";
import { KnowledgeIcon } from "./icons";

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

function rectOf(k: KnowledgeBase): Rect {
  return { x: k.x, y: k.y, width: k.width, height: k.height };
}

function applyRect(el: HTMLElement, rect: Rect): void {
  el.style.left = `${rect.x}px`;
  el.style.top = `${rect.y}px`;
  el.style.width = `${rect.width}px`;
  el.style.height = `${rect.height}px`;
}

type Props = { knowledgeBase: KnowledgeBase };

export const KnowledgeBaseNode = memo(function KnowledgeBaseNode({ knowledgeBase }: Props) {
  const ref = useRef<HTMLDivElement>(null);
  const [editing, setEditing] = useState(false);
  const commitRect = useAppStore((s) => s.commitKnowledgeBaseRect);
  const rename = useAppStore((s) => s.renameKnowledgeBase);
  const remove = useAppStore((s) => s.removeKnowledgeBase);
  const createLink = useAppStore((s) => s.createLink);
  const { id } = knowledgeBase;

  // Publish the committed rectangle for the link layer and drop it on unmount.
  useLayoutEffect(() => {
    layout.set(id, rectOf(knowledgeBase));
  }, [id, knowledgeBase.x, knowledgeBase.y, knowledgeBase.width, knowledgeBase.height]);
  useLayoutEffect(() => () => layout.delete(id), [id]);

  // Highlight this node while a link being drawn would connect to it.
  useEffect(
    () => linkDraft.subscribe((draft) => ref.current?.toggleAttribute("data-link-target", draft?.targetId === id)),
    [id],
  );

  /** Moves/resizes the DOM node directly while dragging; commits on release. */
  const startGeometryDrag = (
    e: ReactPointerEvent,
    cursor: string,
    next: (start: Rect, dx: number, dy: number) => Rect,
  ) => {
    const el = ref.current;
    if (!el || e.button !== 0) return;
    e.preventDefault();
    e.stopPropagation();
    const start = rectOf(knowledgeBase);
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
        if (current !== start) void commitRect(id, current);
      },
    );
  };

  const startMove = (e: ReactPointerEvent) =>
    startGeometryDrag(e, "grabbing", (start, dx, dy) => ({ ...start, x: start.x + dx, y: start.y + dy }));

  const onResizePointerDown = (direction: ResizeDirection) => (e: ReactPointerEvent) =>
    startGeometryDrag(e, CURSORS[direction], (start, dx, dy) => resizeRect(start, direction, dx, dy));

  /** Draws a link from this node; releasing over a terminal creates it. */
  const onLinkHandlePointerDown = (e: ReactPointerEvent) => {
    if (e.button !== 0) return;
    e.preventDefault();
    e.stopPropagation();
    let clientX = e.clientX;
    let clientY = e.clientY;
    let cancelled = false;
    const update = () => {
      if (cancelled) return;
      const to = viewport.clientToWorld(clientX, clientY);
      linkDraft.update(id, to, nodeAt(to, id));
    };
    const onKeyDown = (ev: KeyboardEvent) => {
      if (ev.key !== "Escape") return;
      ev.preventDefault();
      ev.stopPropagation();
      cancelled = true;
      linkDraft.clear();
    };
    update();
    const unsubscribe = viewport.subscribe(update);
    window.addEventListener("keydown", onKeyDown, true);
    trackDrag(
      e,
      "crosshair",
      (_dx, _dy, ev) => {
        clientX = ev.clientX;
        clientY = ev.clientY;
        update();
      },
      (ev) => {
        unsubscribe();
        window.removeEventListener("keydown", onKeyDown, true);
        linkDraft.clear();
        if (cancelled || ev.type !== "pointerup") return;
        const target = nodeAt(viewport.clientToWorld(ev.clientX, ev.clientY), id);
        if (target) void createLink(id, target);
      },
    );
  };

  const onTitleDoubleClick = (e: ReactMouseEvent) => {
    if (!(e.target as HTMLElement).closest("button")) setEditing(true);
  };

  const finishRename = (value: string | null) => {
    setEditing(false);
    const name = value?.trim();
    if (name && name !== knowledgeBase.name) void rename(id, name);
  };

  return (
    <div
      ref={ref}
      className="kb-node"
      role="group"
      aria-label={knowledgeBase.name}
      data-kb-id={id}
      style={{
        left: knowledgeBase.x,
        top: knowledgeBase.y,
        width: knowledgeBase.width,
        height: knowledgeBase.height,
        zIndex: knowledgeBase.z,
      }}
    >
      <div className="kb-titlebar" onPointerDown={startMove} onDoubleClick={onTitleDoubleClick}>
        <span className="kb-icon" aria-hidden="true">
          <KnowledgeIcon />
        </span>
        {editing ? (
          <input
            className="term-title-input"
            defaultValue={knowledgeBase.name}
            autoFocus
            maxLength={64}
            onFocus={(e) => e.currentTarget.select()}
            onBlur={(e) => finishRename(e.currentTarget.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") finishRename(e.currentTarget.value);
              if (e.key === "Escape") finishRename(null);
              e.stopPropagation();
            }}
          />
        ) : (
          <span className="kb-title" title="Double-click to rename">
            <span className="kb-title-text">{knowledgeBase.name}</span>
          </span>
        )}
        <button
          type="button"
          className="term-close"
          aria-label={`Remove ${knowledgeBase.name}`}
          title="Remove this knowledge base (the folder is left untouched)"
          onClick={() => void remove(id)}
        >
          ×
        </button>
      </div>
      <div className="kb-body">
        <KnowledgeIcon />
        <p className="kb-path" title={knowledgeBase.path}>
          {knowledgeBase.path}
        </p>
        <p className="kb-hint">Connect an arrow to a terminal to share this with its agent.</p>
      </div>
      {RESIZE_DIRECTIONS.map((direction) => (
        <div
          key={direction}
          className={`term-resize term-resize-${direction}`}
          aria-hidden="true"
          onPointerDown={onResizePointerDown(direction)}
        />
      ))}
      <div
        className="term-link-handle"
        aria-hidden="true"
        title="Drag to a terminal to give its agent this knowledge base"
        onPointerDown={onLinkHandlePointerDown}
      />
    </div>
  );
});
