import { getCurrentWindow } from "@tauri-apps/api/window";

/**
 * Controls for the custom title bar. The native frame is off
 * (`decorations: false` in tauri.conf.json) so the window can be transparent;
 * each call needs its `core:window:allow-*` permission in
 * src-tauri/capabilities/default.json.
 */
export const appWindow = {
  minimize: () => getCurrentWindow().minimize(),
  toggleMaximize: () => getCurrentWindow().toggleMaximize(),
  /** Same path as the native close button: the backend flushes state on exit. */
  close: () => getCurrentWindow().close(),
};
