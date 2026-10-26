import { useEffect, useState } from "react";
import type { AppLaunch } from "../bindings/AppLaunch";
import type { NativeWindow } from "../bindings/NativeWindow";
import { api, errorMessage } from "../lib/api";
import { useAppStore } from "../store/appStore";
import { AppsIcon } from "./icons";
import { usePopover } from "./usePopover";

/**
 * Brings other programs onto the canvas (Windows only): VS Code on a folder,
 * File Explorer, a document with its own program, or a window already open.
 */
export function AppsMenu() {
  const ready = useAppStore((s) => s.ready);
  const openApp = useAppStore((s) => s.openApp);
  const notify = useAppStore((s) => s.notify);
  const { open, toggle, close, buttonRef, panelRef } = usePopover();
  const [windows, setWindows] = useState<NativeWindow[] | null>(null);

  // The list is read each time the menu opens: windows come and go.
  useEffect(() => {
    if (!open) return;
    let current = true;
    setWindows(null);
    api
      .listWindows()
      .then((list) => current && setWindows(list))
      .catch((e) => current && (setWindows([]), notify(errorMessage(e))));
    return () => {
      current = false;
    };
  }, [open, notify]);

  const launch = (launch: AppLaunch, label: string) => {
    close(false);
    void openApp(launch, label);
  };

  /** Asks for a folder or file first; cancelling opens nothing. */
  const launchPicked = async (folder: boolean, make: (path: string) => AppLaunch, label: string) => {
    close(false);
    try {
      const path = await api.pickPath(folder);
      if (path) await openApp(make(path), label);
    } catch (e) {
      notify(errorMessage(e));
    }
  };

  return (
    <div className="popover-anchor">
      <button
        ref={buttonRef}
        type="button"
        className="dock-btn"
        aria-label="Open a program on the canvas"
        aria-expanded={open}
        aria-controls="apps-panel"
        data-tip={open ? undefined : "Open a program here"}
        disabled={!ready}
        onClick={toggle}
      >
        <AppsIcon />
      </button>
      {open && (
        <div ref={panelRef} id="apps-panel" className="popover popover-start" role="dialog" aria-label="Open a program">
          <h2 className="popover-title">Open on the canvas</h2>
          <button
            type="button"
            className="menu-row"
            onClick={() => void launchPicked(true, (folder) => ({ kind: "vsCode", folder }), "VS Code")}
          >
            VS Code…
            <span className="menu-hint">pick a folder</span>
          </button>
          <button type="button" className="menu-row" onClick={() => launch({ kind: "explorer", folder: null }, "File Explorer")}>
            File Explorer
          </button>
          <button
            type="button"
            className="menu-row"
            onClick={() => void launchPicked(false, (path) => ({ kind: "open", path }), "the document")}
          >
            Document…
            <span className="menu-hint">opens with its program</span>
          </button>

          <h2 className="popover-title">Windows already open</h2>
          {windows === null ? (
            <p className="menu-note">Looking…</p>
          ) : windows.length === 0 ? (
            <p className="menu-note">No other windows are open.</p>
          ) : (
            <ul className="app-list">
              {windows.map((w) => (
                <li key={w.handle}>
                  <button
                    type="button"
                    className="menu-row app-row"
                    title={w.title}
                    onClick={() => launch({ kind: "window", handle: w.handle }, w.title)}
                  >
                    <span className="app-row-title">{w.title}</span>
                    <span className="menu-hint">{w.program.replace(/\.exe$/i, "")}</span>
                  </button>
                </li>
              ))}
            </ul>
          )}
        </div>
      )}
    </div>
  );
}
