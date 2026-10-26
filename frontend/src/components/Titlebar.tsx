import { useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { appWindow } from "../lib/appWindow";
import { CloseIcon, MaximizeIcon, MinimizeIcon, RestoreIcon } from "./icons";

/**
 * The window's own title bar, across the top of the whole board. The native
 * frame is off (`decorations: false`), so this is what gives Godesk its
 * minimize/maximize/close controls and a grab region to move the window, like a
 * desktop application. The bar itself is `data-tauri-drag-region`, so dragging
 * it moves the window (and double-clicking it maximizes, as Windows does).
 */
export function Titlebar() {
  const [maximized, setMaximized] = useState(false);

  useEffect(() => {
    const win = getCurrentWindow();
    let unlisten: (() => void) | undefined;
    void win.isMaximized().then(setMaximized);
    void win
      .onResized(() => {
        void win.isMaximized().then(setMaximized);
      })
      .then((fn) => {
        unlisten = fn;
      });
    return () => unlisten?.();
  }, []);

  // Tell the shell whether the window is maximized, so the frame can drop its
  // rounded corners and border when it fills the screen, as desktop windows do.
  useEffect(() => {
    document.documentElement.dataset.winMaximized = String(maximized);
  }, [maximized]);

  return (
    <div className="app-titlebar" data-tauri-drag-region>
      <span className="app-titlebar-title" data-tauri-drag-region>
        Godesk
      </span>
      <div className="app-titlebar-controls">
        <button
          type="button"
          className="win-btn"
          aria-label="Minimize"
          title="Minimize"
          onClick={() => void appWindow.minimize()}
        >
          <MinimizeIcon />
        </button>
        <button
          type="button"
          className="win-btn"
          aria-label={maximized ? "Restore" : "Maximize"}
          title={maximized ? "Restore" : "Maximize"}
          onClick={() => void appWindow.toggleMaximize()}
        >
          {maximized ? <RestoreIcon /> : <MaximizeIcon />}
        </button>
        <button
          type="button"
          className="win-btn win-close"
          aria-label="Close"
          title="Close"
          onClick={() => void appWindow.close()}
        >
          <CloseIcon />
        </button>
      </div>
    </div>
  );
}
