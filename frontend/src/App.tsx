import { useEffect } from "react";
import { installAppPlacement } from "./canvas/appPlacement";
import { PLATFORM } from "./lib/platform";
import { useAppStore } from "./store/appStore";
import { Canvas } from "./components/Canvas";
import { ConfirmDialog } from "./components/ConfirmDialog";
import { Dock } from "./components/Dock";
import { FolderPalette } from "./components/FolderPalette";
import { installCloseGuard } from "./components/closeGuard";
import { installShortcuts } from "./components/shortcuts";
import { Titlebar } from "./components/Titlebar";

const TOAST_MS = 4_000;

function Toast() {
  const toast = useAppStore((s) => s.toast);
  const notify = useAppStore((s) => s.notify);
  useEffect(() => {
    if (!toast) return;
    const timer = window.setTimeout(() => notify(null), TOAST_MS);
    return () => window.clearTimeout(timer);
  }, [toast, notify]);
  if (!toast) return null;
  return (
    <div className="toast" role="status">
      {toast}
    </div>
  );
}

export function App() {
  const ready = useAppStore((s) => s.ready);
  const init = useAppStore((s) => s.init);
  const transparentCanvas = useAppStore((s) => s.transparentCanvas);

  useEffect(() => {
    void init();
  }, [init]);
  useEffect(installShortcuts, []);
  useEffect(installCloseGuard, []);
  // Other programs' windows only exist on Windows.
  useEffect(() => (PLATFORM === "windows" ? installAppPlacement() : undefined), []);

  return (
    <div className="app" data-canvas={transparentCanvas ? "transparent" : "solid"}>
      <Titlebar />
      <main className="main">
        {ready ? <Canvas /> : <div className="loading">Loading workspaces…</div>}
        <Dock />
      </main>
      <FolderPalette />
      <ConfirmDialog />
      <Toast />
    </div>
  );
}
