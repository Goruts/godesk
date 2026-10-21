import { memo, useEffect, useLayoutEffect, useRef, useState } from "react";
import type { MouseEvent as ReactMouseEvent, PointerEvent as ReactPointerEvent } from "react";
import type { Rect } from "../bindings/Rect";
import type { TerminalWindow as TerminalModel } from "../bindings/TerminalWindow";
import { api } from "../lib/api";
import { LOW_ZOOM, dragOffset } from "../canvas/camera";
import { RESIZE_DIRECTIONS, type ResizeDirection, containsPoint, resizeRect } from "../canvas/geometry";
import { layout } from "../canvas/layout";
import { isTypingTarget, trackDrag } from "../canvas/interaction";
import { viewport } from "../canvas/viewport";
import { linkDraft } from "../canvas/linkDraft";
import { activeWorkspace, useAppStore } from "../store/appStore";
import { confirmDialog } from "../store/dialogStore";
import { MOD_KEY } from "../lib/platform";
import { DuplicateIcon } from "./icons";
import { openDuplicate } from "./shortcuts";
import { TerminalView, terminals } from "./TerminalView";

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

function rectOf(t: TerminalModel): Rect {
  return { x: t.x, y: t.y, width: t.width, height: t.height };
}

function applyRect(el: HTMLElement, rect: Rect): void {
  el.style.left = `${rect.x}px`;
  el.style.top = `${rect.y}px`;
  el.style.width = `${rect.width}px`;
  el.style.height = `${rect.height}px`;
}

/** Topmost terminal of the active workspace under a canvas point. */
function terminalAt(point: { x: number; y: number }, exceptId: string): string | null {
  const ws = activeWorkspace(useAppStore.getState());
  if (!ws) return null;
  const hits = ws.terminals
    .filter((t) => t.id !== exceptId)
    .filter((t) => containsPoint(layout.get(t.id) ?? rectOf(t), point))
    .sort((a, b) => b.z - a.z);
  return hits[0]?.id ?? null;
}

type Props = { terminal: TerminalModel };

export const TerminalWindow = memo(function TerminalWindow({ terminal }: Props) {
  const ref = useRef<HTMLDivElement>(null);
  const [editing, setEditing] = useState(false);
  const refocusAfterRename = useRef(false);
  const closing = useRef(false);
  const focusTerminal = useAppStore((s) => s.focusTerminal);
  const commitTerminalRect = useAppStore((s) => s.commitTerminalRect);
  const renameTerminal = useAppStore((s) => s.renameTerminal);
  const closeTerminal = useAppStore((s) => s.closeTerminal);
  const createLink = useAppStore((s) => s.createLink);
  const toggleTerminalSelection = useAppStore((s) => s.toggleTerminalSelection);
  const { id } = terminal;
  const selected = useAppStore((s) => s.selectedTerminalIds.includes(id));

  // Publish the committed rectangle for the link layer and drop it on unmount.
  useLayoutEffect(() => {
    layout.set(id, rectOf(terminal));
  }, [id, terminal.x, terminal.y, terminal.width, terminal.height]);
  useLayoutEffect(() => () => layout.delete(id), [id]);

  // Highlight this window while a link being drawn would connect to it.
  useEffect(
    () => linkDraft.subscribe((draft) => ref.current?.toggleAttribute("data-link-target", draft?.targetId === id)),
    [id],
  );

  const focus = () => {
    focusTerminal(id);
    terminals.get(id)?.focus();
  };

  /** Moves/resizes the DOM node directly while dragging; commits on release. */
  const startGeometryDrag = (e: ReactPointerEvent, cursor: string, next: (start: Rect, dx: number, dy: number) => Rect) => {
    const el = ref.current;
    if (!el || e.button !== 0) return;
    e.preventDefault();
    e.stopPropagation();
    focus();
    const start = rectOf(terminal);
    let current = start;
    // Geometry is in canvas units; re-run on camera changes too, since the
    // wheel still pans/zooms over the shield mid-drag.
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
        if (current !== start) void commitTerminalRect(id, current);
      },
    );
  };

  const startMove = (e: ReactPointerEvent) =>
    startGeometryDrag(e, "grabbing", (start, dx, dy) => ({ ...start, x: start.x + dx, y: start.y + dy }));

  const onTitlePointerDown = (e: ReactPointerEvent) => {
    if ((e.target as HTMLElement).closest("button, input")) return;
    if (e.shiftKey && e.button === 0) {
      // Picks the window for a bulk action; the keyboard stays with the
      // canvas so Delete and Escape act on the selection.
      e.preventDefault();
      if (document.activeElement instanceof HTMLElement) document.activeElement.blur();
      toggleTerminalSelection(id);
      return;
    }
    startMove(e);
  };

  // Zoomed far out the terminal body ignores the mouse (see styles.css), so a
  // press there lands on the window itself and moves it.
  const onWindowPointerDown = (e: ReactPointerEvent) => {
    if (e.target === e.currentTarget && viewport.zoom < LOW_ZOOM) startMove(e);
  };

  // Focus follows the mouse: entering a window hands it the keyboard without
  // raising it. Not mid-drag, not away from a text field being edited, not
  // while Space pans (its key repeat would type into the shell) and not when
  // zoomed too far out to read.
  const onPointerEnter = (e: ReactPointerEvent) => {
    if (e.pointerType !== "mouse" || e.buttons !== 0 || viewport.zoom < LOW_ZOOM) return;
    if (useAppStore.getState().selectedTerminalIds.length > 0) return;
    if (ref.current?.closest('[data-space-pan="true"]')) return;
    const active = document.activeElement;
    if (active && isTypingTarget(active) && !active.closest(".term-body")) return;
    terminals.get(id)?.focus();
  };

  const onResizePointerDown = (direction: ResizeDirection) => (e: ReactPointerEvent) =>
    startGeometryDrag(e, CURSORS[direction], (start, dx, dy) => resizeRect(start, direction, dx, dy));

  /** Draws a link from this window; releasing over another one creates it. */
  const onLinkHandlePointerDown = (e: ReactPointerEvent) => {
    if (e.button !== 0) return;
    e.preventDefault();
    e.stopPropagation();
    let clientX = e.clientX;
    let clientY = e.clientY;
    let cancelled = false;
    // Re-run on camera changes too, since the wheel still pans/zooms mid-drag.
    const update = () => {
      if (cancelled) return;
      const to = viewport.clientToWorld(clientX, clientY);
      linkDraft.update(id, to, terminalAt(to, id));
    };
    const onKeyDown = (ev: KeyboardEvent) => {
      if (ev.key !== "Escape") return;
      // Keep the Escape away from the focused shell.
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
        const target = terminalAt(viewport.clientToWorld(ev.clientX, ev.clientY), id);
        if (target) void createLink(id, target);
      },
    );
  };

  const onClose = async () => {
    // A double click on × must not ask twice or close twice.
    if (closing.current) return;
    closing.current = true;
    try {
      const busy = await api.terminalHasForegroundProcess(id).catch(() => false);
      if (busy) {
        const ok = await confirmDialog({
          title: `Close “${terminal.name}”?`,
          message: "A program is still running in this terminal. Closing it will stop that program.",
          confirmLabel: "Close terminal",
          danger: true,
        });
        if (!ok) {
          terminals.get(id)?.focus();
          return;
        }
      }
      await closeTerminal(id);
    } finally {
      closing.current = false;
    }
  };

  const onTitleDoubleClick = (e: ReactMouseEvent) => {
    if (!e.shiftKey && !(e.target as HTMLElement).closest("button")) setEditing(true);
  };

  /** Ends renaming; `null` cancels. Enter and Escape give the shell its focus back. */
  const finishRename = (value: string | null, refocus: boolean) => {
    refocusAfterRename.current = refocus;
    setEditing(false);
    const name = value?.trim();
    if (name && name !== terminal.name) void renameTerminal(id, name);
  };

  // Refocus once the field is gone, so its blur cannot commit a second time.
  useEffect(() => {
    if (editing || !refocusAfterRename.current) return;
    refocusAfterRename.current = false;
    terminals.get(id)?.focus();
  }, [editing, id]);

  return (
    <div
      ref={ref}
      className="term-window"
      role="group"
      aria-label={terminal.name}
      data-terminal-id={id}
      data-selected={selected || undefined}
      style={{ left: terminal.x, top: terminal.y, width: terminal.width, height: terminal.height, zIndex: terminal.z }}
      onPointerDownCapture={(e) => {
        if (e.button !== 0) return;
        const target = e.target as HTMLElement;
        if (e.shiftKey && target.closest(".term-titlebar") && !target.closest("button, input")) return;
        // A plain click goes back to working in one terminal.
        if (useAppStore.getState().selectedTerminalIds.length > 0) useAppStore.getState().selectTerminals([]);
        // Moving the focus into the shell would blur (and commit) the rename field.
        if (target.closest("input")) focusTerminal(id);
        else focus();
      }}
      onPointerDown={onWindowPointerDown}
      onPointerEnter={onPointerEnter}
    >
      <div className="term-titlebar" onPointerDown={onTitlePointerDown} onDoubleClick={onTitleDoubleClick}>
        {editing ? (
          <input
            className="term-title-input"
            defaultValue={terminal.name}
            autoFocus
            maxLength={64}
            onFocus={(e) => e.currentTarget.select()}
            onBlur={(e) => finishRename(e.currentTarget.value, false)}
            onKeyDown={(e) => {
              if (e.key === "Enter") finishRename(e.currentTarget.value, true);
              if (e.key === "Escape") finishRename(null, true);
              e.stopPropagation();
            }}
          />
        ) : (
          <span className="term-title" title="Double-click to rename">
            {terminal.name}
          </span>
        )}
        <button
          type="button"
          className="term-action"
          aria-label={`Duplicate ${terminal.name}`}
          title={`Duplicate: same folder and program (${MOD_KEY}+Shift+D)`}
          onClick={() => void openDuplicate(id)}
        >
          <DuplicateIcon />
        </button>
        <button
          type="button"
          className="term-close"
          aria-label={`Close ${terminal.name}`}
          title="Close terminal"
          onClick={() => void onClose()}
        >
          ×
        </button>
      </div>
      <div className="term-body">
        <TerminalView terminalId={id} />
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
        title="Drag to another terminal to connect them"
        onPointerDown={onLinkHandlePointerDown}
      />
    </div>
  );
});
