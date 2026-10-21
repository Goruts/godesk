import { useSyncExternalStore } from "react";
import type { Rect } from "../bindings/Rect";
import { fitToContent, resetZoom, zoomBy } from "../canvas/actions";
import { layout } from "../canvas/layout";
import { viewport } from "../canvas/viewport";
import { errorMessage } from "../lib/api";
import { MOD_KEY } from "../lib/platform";
import { appWindow } from "../lib/appWindow";
import { activeWorkspace, useAppStore } from "../store/appStore";
import { FitIcon, GripIcon, LinkIcon, MoreIcon, PlusIcon } from "./icons";
import { closeSelectedTerminals, openNewTerminal, terminalCount } from "./shortcuts";
import { usePopover } from "./usePopover";
import { WorkspaceMenu } from "./WorkspaceMenu";

const ZOOM_STEP = 1.2;

const SHORTCUTS: Array<[action: string, keys: string]> = [
  ["New terminal", `${MOD_KEY} + Shift + T`],
  ["Duplicate a terminal (folder and program)", `${MOD_KEY} + Shift + D`],
  ["Pan", "Scroll, middle-drag or Space + drag"],
  ["Zoom", `${MOD_KEY} + scroll, or pinch`],
  ["Link two terminals", "Drag a window's ● handle onto another"],
  ["Select several terminals", "Shift + click titles, or Shift + drag"],
  ["Close the selected terminals", "Delete"],
  ["Delete the selected link", "Delete"],
  ["Rename", "Double-click a title, F2 on a workspace"],
];

/** Re-renders only when the rounded percentage changes. */
function useZoomPercent(): number {
  return useSyncExternalStore(
    (onChange) => viewport.subscribe(onChange),
    () => Math.round(viewport.zoom * 100),
  );
}

/** View, window and help options that do not need to be one click away. */
function MoreMenu() {
  const zoom = useZoomPercent();
  const notify = useAppStore((s) => s.notify);
  const transparentCanvas = useAppStore((s) => s.transparentCanvas);
  const toggleTransparentCanvas = useAppStore((s) => s.toggleTransparentCanvas);
  const { open, toggle, buttonRef, panelRef } = usePopover();
  const run = (action: () => Promise<void>) => () => void action().catch((e) => notify(errorMessage(e)));

  return (
    <div className="popover-anchor">
      <button
        ref={buttonRef}
        type="button"
        className="dock-btn"
        aria-label="More options"
        aria-expanded={open}
        aria-controls="more-panel"
        data-tip={open ? undefined : "More"}
        onClick={toggle}
      >
        <MoreIcon />
      </button>
      {open && (
        <div ref={panelRef} id="more-panel" className="popover popover-end" role="dialog" aria-label="More options">
          <h2 className="popover-title">Canvas</h2>
          <button
            type="button"
            className="menu-row"
            role="switch"
            aria-checked={transparentCanvas}
            onClick={toggleTransparentCanvas}
          >
            Transparent background
            <span className="switch" aria-hidden="true" />
          </button>
          <div className="menu-row" role="group" aria-label="Zoom">
            Zoom
            <span className="segmented">
              <button type="button" aria-label="Zoom out" onClick={() => zoomBy(1 / ZOOM_STEP)}>
                −
              </button>
              <button type="button" aria-label={`Zoom ${zoom}%. Reset to 100%`} title="Reset to 100%" onClick={resetZoom}>
                {zoom}%
              </button>
              <button type="button" aria-label="Zoom in" onClick={() => zoomBy(ZOOM_STEP)}>
                +
              </button>
            </span>
          </div>

          <h2 className="popover-title">Window</h2>
          <div className="menu-actions">
            <button type="button" onClick={run(appWindow.minimize)}>
              Minimize
            </button>
            <button type="button" onClick={run(appWindow.toggleMaximize)}>
              Maximize
            </button>
            <button type="button" className="is-danger" onClick={run(appWindow.close)}>
              Quit
            </button>
          </div>

          <h2 className="popover-title">Shortcuts</h2>
          <dl className="shortcuts">
            {SHORTCUTS.map(([action, keys]) => (
              <div key={action} className="shortcut">
                <dt>{action}</dt>
                <dd>{keys}</dd>
              </div>
            ))}
          </dl>
        </div>
      )}
    </div>
  );
}

/**
 * The only chrome: a floating dock at the bottom of the canvas. The grip is a
 * window drag region, since the native frame is off; Tauri only drags when
 * the pressed element itself carries `data-tauri-drag-region`.
 */
export function Dock() {
  const ready = useAppStore((s) => s.ready);
  const linksVisible = useAppStore((s) => s.linksVisible);
  const toggleLinks = useAppStore((s) => s.toggleLinks);
  const selectedLinkId = useAppStore((s) => s.selectedLinkId);
  const deleteLink = useAppStore((s) => s.deleteLink);
  const selectedTerminals = useAppStore((s) => s.selectedTerminalIds.length);

  const fit = () => {
    const ws = activeWorkspace(useAppStore.getState());
    if (!ws) return;
    const rects: Rect[] = ws.terminals.map((t) => layout.get(t.id) ?? t);
    fitToContent(rects);
  };

  return (
    <div className="dock" role="toolbar" aria-label="Godesk">
      <div className="dock-grip" data-tauri-drag-region title="Drag to move the window">
        <GripIcon />
      </div>
      <button
        type="button"
        className="dock-btn dock-btn-primary"
        aria-label="New terminal"
        aria-keyshortcuts="Control+Shift+T Meta+Shift+T"
        data-tip={`New terminal · ${MOD_KEY}+Shift+T`}
        disabled={!ready}
        onClick={() => void openNewTerminal()}
      >
        <PlusIcon />
      </button>
      <WorkspaceMenu />
      <span className="dock-divider" aria-hidden="true" />
      <button
        type="button"
        className="dock-btn"
        aria-label="Show links"
        aria-pressed={linksVisible}
        data-tip={linksVisible ? "Hide links" : "Show links"}
        onClick={toggleLinks}
      >
        <LinkIcon />
      </button>
      <button type="button" className="dock-btn" aria-label="Fit to content" data-tip="Frame every terminal" onClick={fit}>
        <FitIcon />
      </button>
      {selectedTerminals > 0 && (
        <button
          type="button"
          className="dock-btn dock-btn-danger"
          onClick={() => void closeSelectedTerminals()}
          title="Close the selected terminals (Delete)"
        >
          Close {terminalCount(selectedTerminals)}
        </button>
      )}
      {selectedLinkId && linksVisible && (
        <button
          type="button"
          className="dock-btn dock-btn-danger"
          onClick={() => void deleteLink(selectedLinkId)}
          title="Delete the selected link (Delete)"
        >
          Delete link
        </button>
      )}
      <MoreMenu />
    </div>
  );
}
