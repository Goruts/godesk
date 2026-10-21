import { useEffect, useRef, useState } from "react";
import { type ITerminalOptions, Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebglAddon } from "@xterm/addon-webgl";
import "@xterm/xterm/css/xterm.css";
import type { PtyOutputEvent } from "../bindings/PtyOutputEvent";
import { api, errorMessage, events } from "../lib/api";
import { decodeBase64, encodeBinaryString, encodeText } from "../lib/base64";
import { readClipboardText, writeClipboardText } from "../lib/clipboard";
import { PLATFORM, type Platform } from "../lib/platform";
import { useAppStore } from "../store/appStore";
import { viewport } from "../canvas/viewport";

/** WebGL is faster with many busy terminals; xterm falls back to its DOM renderer. */
const USE_WEBGL = true;
const RESIZE_DEBOUNCE_MS = 60;

/** `background` matches `--window-bg` in styles.css. */
const THEME = {
  background: "#141415",
  foreground: "#dcdcde",
  cursor: "#ededee",
  cursorAccent: "#141415",
  selectionBackground: "#3a3a3e",
  black: "#26262b",
  red: "#f7768e",
  green: "#9ece6a",
  yellow: "#e0af68",
  blue: "#7aa2f7",
  magenta: "#bb9af7",
  cyan: "#7dcfff",
  white: "#c0caf5",
  brightBlack: "#565f89",
  brightRed: "#ff899d",
  brightGreen: "#addb7a",
  brightYellow: "#f0c07a",
  brightBlue: "#8db0ff",
  brightMagenta: "#c7a9ff",
  brightCyan: "#a4daff",
  brightWhite: "#e5e9f5",
};

/**
 * xterm settings for this OS. On Windows the shell runs behind ConPTY, which
 * redraws on resize itself; xterm must know, or reflow garbles the screen.
 */
export function terminalOptions(platform: Platform): ITerminalOptions {
  return {
    fontFamily: '"JetBrains Mono", "Fira Code", "Cascadia Mono", Menlo, Consolas, "DejaVu Sans Mono", monospace',
    fontSize: 13,
    lineHeight: 1.15,
    cursorBlink: true,
    scrollback: 5000,
    allowProposedApi: false,
    theme: THEME,
    ...(platform === "windows" ? { windowsPty: { backend: "conpty" as const } } : {}),
  };
}

/** Mounted xterm instances, so windows can move keyboard focus into them. */
export const terminals = new Map<string, Terminal>();

/** Terminals that asked for focus before their xterm existed. */
const pendingFocus = new Set<string>();

/** Focuses a terminal now, or as soon as its view mounts (e.g. a new one). */
export function focusTerminalView(terminalId: string): void {
  const term = terminals.get(terminalId);
  if (term) term.focus();
  else pendingFocus.add(terminalId);
}

const MOUSE_EVENTS = ["mousedown", "mousemove", "mouseup", "click", "dblclick", "contextmenu", "auxclick"] as const;
const corrected = new WeakSet<Event>();

/**
 * xterm maps the mouse with `clientX - rect.left` (scaled by the canvas zoom)
 * divided by its unscaled cell size, so selection is off at any zoom ≠ 100%.
 * Re-dispatch mouse events over the terminal with coordinates divided by the
 * zoom, relative to the xterm screen element.
 */
function installZoomCorrection(container: HTMLElement): () => void {
  const handler = (event: Event) => {
    const e = event as MouseEvent;
    const zoom = viewport.zoom;
    if (corrected.has(e) || Math.abs(zoom - 1) < 1e-3 || !e.target) return;
    const screen = container.querySelector(".xterm-screen");
    if (!screen) return;
    const rect = screen.getBoundingClientRect();
    e.stopImmediatePropagation();
    const fixed = new MouseEvent(e.type, {
      bubbles: e.bubbles,
      cancelable: e.cancelable,
      composed: e.composed,
      view: e.view,
      detail: e.detail,
      screenX: e.screenX,
      screenY: e.screenY,
      clientX: rect.left + (e.clientX - rect.left) / zoom,
      clientY: rect.top + (e.clientY - rect.top) / zoom,
      ctrlKey: e.ctrlKey,
      shiftKey: e.shiftKey,
      altKey: e.altKey,
      metaKey: e.metaKey,
      button: e.button,
      buttons: e.buttons,
      relatedTarget: e.relatedTarget,
    });
    corrected.add(fixed);
    e.target.dispatchEvent(fixed);
    if (fixed.defaultPrevented) e.preventDefault();
  };
  for (const type of MOUSE_EVENTS) container.addEventListener(type, handler, true);
  return () => {
    for (const type of MOUSE_EVENTS) container.removeEventListener(type, handler, true);
  };
}

function loadRenderer(term: Terminal): void {
  if (!USE_WEBGL) return;
  try {
    const webgl = new WebglAddon();
    // Browsers cap live WebGL contexts; when one is lost, fall back to DOM.
    webgl.onContextLoss(() => webgl.dispose());
    term.loadAddon(webgl);
  } catch {
    // No WebGL2 in this webview: keep the default DOM renderer.
  }
}

/**
 * Ctrl/Cmd+Shift+C copies the selection, Ctrl/Cmd+Shift+V pastes. The
 * default is prevented because WebView2 and WKWebView bind these keys to a
 * native paste of their own, which would paste twice.
 */
function clipboardKeys(term: Terminal) {
  const fail = (action: "copy" | "paste") => (error: unknown) => {
    // WebKitGTK's wording for a refused clipboard is long and vague.
    const blocked = error instanceof DOMException && error.name === "NotAllowedError";
    useAppStore
      .getState()
      .notify(blocked ? `Can't ${action}: this webview blocks clipboard access.` : `Can't ${action}: ${errorMessage(error)}`);
  };
  return (e: KeyboardEvent): boolean => {
    if (e.type !== "keydown" || !(e.ctrlKey || e.metaKey) || !e.shiftKey) return true;
    const key = e.key.toLowerCase();
    if (key === "c" && term.hasSelection()) {
      e.preventDefault();
      writeClipboardText(term.getSelection()).catch(fail("copy"));
      return false;
    }
    if (key === "v") {
      e.preventDefault();
      readClipboardText()
        .then((text) => term.paste(text))
        .catch(fail("paste"));
      return false;
    }
    return true;
  };
}

type Props = { terminalId: string };

type SessionState = { status: "running" } | { status: "exited"; code: number | null } | { status: "error"; message: string };

export function TerminalView({ terminalId }: Props) {
  const containerRef = useRef<HTMLDivElement>(null);
  const [generation, setGeneration] = useState(0);
  const [session, setSession] = useState<SessionState>({ status: "running" });
  const sizeRef = useRef({ cols: 80, rows: 24 });

  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;
    let disposed = false;
    const cleanups: Array<() => void> = [];

    const term = new Terminal(terminalOptions(PLATFORM));
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(container);
    loadRenderer(term);
    term.attachCustomKeyEventHandler(clipboardKeys(term));
    terminals.set(terminalId, term);
    if (pendingFocus.delete(terminalId)) term.focus();
    cleanups.push(installZoomCorrection(container));

    const safeFit = () => {
      try {
        fit.fit();
      } catch {
        // The element is not laid out yet.
      }
    };
    safeFit();

    // Input → PTY.
    const dataSub = term.onData((data) => void api.writeTerminal(terminalId, encodeText(data)).catch(() => undefined));
    const binarySub = term.onBinary(
      (data) => void api.writeTerminal(terminalId, encodeBinaryString(data)).catch(() => undefined),
    );
    cleanups.push(() => dataSub.dispose(), () => binarySub.dispose());

    // Size → PTY (debounced so a resize drag does not flood the shell with SIGWINCH).
    let resizeTimer: number | undefined;
    const resizeSub = term.onResize(({ cols, rows }) => {
      sizeRef.current = { cols, rows };
      window.clearTimeout(resizeTimer);
      resizeTimer = window.setTimeout(() => {
        void api.resizeTerminal(terminalId, cols, rows).catch(() => undefined);
      }, RESIZE_DEBOUNCE_MS);
    });
    cleanups.push(() => resizeSub.dispose(), () => window.clearTimeout(resizeTimer));

    // Refit whenever the window body changes size (window resize, layout).
    let frame = 0;
    const observer = new ResizeObserver(() => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(safeFit);
    });
    observer.observe(container);
    cleanups.push(() => observer.disconnect(), () => cancelAnimationFrame(frame));

    // Output: listen first, then attach. Events that arrive before the replay
    // are queued; anything the replay already contains (seq ≤ snapshot) is
    // dropped, so nothing is lost or written twice.
    let replayedUpTo: number | null = null;
    const queued: PtyOutputEvent[] = [];
    const write = (e: PtyOutputEvent) => {
      if (replayedUpTo !== null && e.seq > replayedUpTo) term.write(decodeBase64(e.data));
    };

    // Events and command replies travel separately: an exit seen while the
    // attach is in flight must not be undone by its older `alive: true`.
    let exited = false;

    void (async () => {
      const unlistenOutput = await events.onPtyOutput(terminalId, (e) => {
        if (replayedUpTo === null) queued.push(e);
        else write(e);
      });
      const unlistenExit = await events.onPtyExit(terminalId, (e) => {
        exited = true;
        setSession({ status: "exited", code: e.exitCode });
      });
      if (disposed) {
        unlistenOutput();
        unlistenExit();
        return;
      }
      cleanups.push(unlistenOutput, unlistenExit);
      try {
        const { cols, rows } = term;
        sizeRef.current = { cols, rows };
        const result = await api.attachTerminal(terminalId, cols, rows);
        if (disposed) return;
        term.write(decodeBase64(result.data));
        replayedUpTo = result.seq;
        queued.splice(0).forEach(write);
        if (!exited) setSession(result.alive ? { status: "running" } : { status: "exited", code: null });
      } catch (error) {
        if (!disposed) setSession({ status: "error", message: errorMessage(error) });
      }
    })();

    return () => {
      disposed = true;
      for (const cleanup of cleanups.reverse()) cleanup();
      if (terminals.get(terminalId) === term) terminals.delete(terminalId);
      term.dispose();
    };
  }, [terminalId, generation]);

  const restart = async () => {
    const { cols, rows } = sizeRef.current;
    try {
      await api.restartTerminal(terminalId, cols, rows);
      setSession({ status: "running" });
      setGeneration((g) => g + 1);
    } catch (error) {
      setSession({ status: "error", message: errorMessage(error) });
    }
  };

  return (
    <div className="terminal-view">
      <div ref={containerRef} className="terminal-xterm" />
      {session.status !== "running" && (
        <div className="terminal-overlay">
          <p>
            {session.status === "exited"
              ? `Process exited${session.code === null ? "" : ` with code ${session.code}`}.`
              : session.message}
          </p>
          <button type="button" className="btn btn-primary" onClick={() => void restart()}>
            Restart shell
          </button>
        </div>
      )}
    </div>
  );
}
