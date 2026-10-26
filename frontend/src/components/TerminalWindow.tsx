import { memo, useEffect, useLayoutEffect, useRef, useState } from "react";
import type { MouseEvent as ReactMouseEvent, PointerEvent as ReactPointerEvent } from "react";
import type { Rect } from "../bindings/Rect";
import type { TerminalWindow as TerminalModel } from "../bindings/TerminalWindow";
import { api, errorMessage } from "../lib/api";
import { LOW_ZOOM, dragOffset } from "../canvas/camera";
import { RESIZE_DIRECTIONS, type ResizeDirection, resizeRect } from "../canvas/geometry";
import { layout } from "../canvas/layout";
import { nodeAt } from "../canvas/nodes";
import { isTypingTarget, trackDrag } from "../canvas/interaction";
import { viewport } from "../canvas/viewport";
import { linkDraft } from "../canvas/linkDraft";
import { useAppStore } from "../store/appStore";
import { confirmDialog } from "../store/dialogStore";
import { MOD_KEY, PLATFORM } from "../lib/platform";
import { CodeIcon, DuplicateIcon, MaximizeIcon, MinimizeIcon, RestoreIcon } from "./icons";
import { openDuplicate } from "./shortcuts";
import { AgentTerminal } from "./AgentTerminal";
import { TerminalView, focusTerminalView } from "./TerminalView";

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

/** Height a collapsed (minimized) window shows: just its title bar (--titlebar-h). */
const COLLAPSED_HEIGHT = 26;
/** Fraction of the visible area left as a margin when a window is maximized. */
const MAXIMIZE_INSET = 0.03;

type Props = { terminal: TerminalModel };

export const TerminalWindow = memo(function TerminalWindow({ terminal }: Props) {
  const ref = useRef<HTMLDivElement>(null);
  const [editing, setEditing] = useState(false);
  const [collapsed, setCollapsed] = useState(false);
  const [maximized, setMaximized] = useState(false);
  // The rectangle to return to when a maximized window is restored.
  const restoreRect = useRef<Rect | null>(null);
  const refocusAfterRename = useRef(false);
  const closing = useRef(false);
  const focusTerminal = useAppStore((s) => s.focusTerminal);
  const commitTerminalRect = useAppStore((s) => s.commitTerminalRect);
  const renameTerminal = useAppStore((s) => s.renameTerminal);
  const closeTerminal = useAppStore((s) => s.closeTerminal);
  const openApp = useAppStore((s) => s.openApp);
  const notify = useAppStore((s) => s.notify);
  const createLink = useAppStore((s) => s.createLink);
  const toggleTerminalSelection = useAppStore((s) => s.toggleTerminalSelection);
  const { id } = terminal;
  const selected = useAppStore((s) => s.selectedTerminalIds.includes(id));

  // Publish the committed rectangle for the link layer and drop it on unmount.
  // A collapsed window reports only its title bar, so arrows meet its visible
  // edge rather than the hidden body.
  useLayoutEffect(() => {
    const rect = rectOf(terminal);
    layout.set(id, collapsed ? { ...rect, height: COLLAPSED_HEIGHT } : rect);
  }, [id, terminal.x, terminal.y, terminal.width, terminal.height, collapsed]);
  useLayoutEffect(() => () => layout.delete(id), [id]);

  // Highlight this window while a link being drawn would connect to it.
  useEffect(
    () => linkDraft.subscribe((draft) => ref.current?.toggleAttribute("data-link-target", draft?.targetId === id)),
    [id],
  );

  const focus = () => {
    focusTerminal(id);
    focusTerminalView(id);
  };

  /** Moves/resizes the DOM node directly while dragging; commits on release. */
  const startGeometryDrag = (e: ReactPointerEvent, cursor: string, next: (start: Rect, dx: number, dy: number) => Rect) => {
    const el = ref.current;
    if (!el || e.button !== 0) return;
    e.preventDefault();
    e.stopPropagation();
    focus();
    // Taking manual control of the geometry ends the maximized state, so the
    // control toggles to "maximize" again and no stale restore rect lingers.
    if (maximized) {
      setMaximized(false);
      restoreRect.current = null;
    }
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
    focusTerminalView(id);
  };

  const onResizePointerDown = (direction: ResizeDirection) => (e: ReactPointerEvent) =>
    startGeometryDrag(e, CURSORS[direction], (start, dx, dy) => resizeRect(start, direction, dx, dy));

  // Minimize: collapse to the title bar, or expand again. Local to the view —
  // the window keeps its real size, only its body is hidden.
  const onToggleCollapse = () => {
    focus();
    setCollapsed((value) => !value);
  };

  // Maximize to fill the visible canvas, or restore the previous rectangle.
  const onToggleMaximize = () => {
    const el = ref.current;
    if (!el) return;
    focus();
    if (maximized) {
      const previous = restoreRect.current;
      restoreRect.current = null;
      setMaximized(false);
      if (previous) {
        applyRect(el, previous);
        layout.set(id, previous);
        void commitTerminalRect(id, previous);
      }
      return;
    }
    const area = viewport.visibleRect();
    const insetX = area.width * MAXIMIZE_INSET;
    const insetY = area.height * MAXIMIZE_INSET;
    const target = {
      x: area.x + insetX,
      y: area.y + insetY,
      width: area.width - insetX * 2,
      height: area.height - insetY * 2,
    };
    restoreRect.current = rectOf(terminal);
    setMaximized(true);
    setCollapsed(false);
    applyRect(el, target);
    layout.set(id, target);
    void commitTerminalRect(id, target);
  };

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
      linkDraft.update(id, to, nodeAt(to, id));
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
        const target = nodeAt(viewport.clientToWorld(ev.clientX, ev.clientY), id);
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
          focusTerminalView(id);
          return;
        }
      }
      await closeTerminal(id);
    } finally {
      closing.current = false;
    }
  };

  /** VS Code on the folder the terminal works in, beside it on the canvas. */
  const onOpenInVsCode = async () => {
    try {
      const folder = await api.terminalFolder(id);
      await openApp({ kind: "vsCode", folder }, "VS Code", id);
    } catch (e) {
      notify(errorMessage(e));
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
    focusTerminalView(id);
  }, [editing, id]);

  return (
    <div
      ref={ref}
      className="term-window"
      role="group"
      aria-label={terminal.name}
      data-terminal-id={id}
      data-selected={selected || undefined}
      data-collapsed={collapsed || undefined}
      data-maximized={maximized || undefined}
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
            <span className="term-title-text">{terminal.name}</span>
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
        {PLATFORM === "windows" && (
          <button
            type="button"
            className="term-action"
            aria-label={`Open the folder of ${terminal.name} in VS Code`}
            title="Open this folder in VS Code, beside the terminal"
            onClick={() => void onOpenInVsCode()}
          >
            <CodeIcon />
          </button>
        )}
        <button
          type="button"
          className="term-action"
          aria-label={collapsed ? `Expand ${terminal.name}` : `Minimize ${terminal.name}`}
          title={collapsed ? "Expand" : "Minimize to the title bar"}
          onClick={onToggleCollapse}
        >
          <MinimizeIcon />
        </button>
        <button
          type="button"
          className="term-action"
          aria-label={maximized ? `Restore ${terminal.name}` : `Maximize ${terminal.name}`}
          title={maximized ? "Restore size" : "Maximize to fill the view"}
          onClick={onToggleMaximize}
        >
          {maximized ? <RestoreIcon /> : <MaximizeIcon />}
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
        {terminal.agent ? <AgentTerminal terminalId={id} agent={terminal.agent} /> : <TerminalView terminalId={id} />}
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
