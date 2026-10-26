import { useEffect, useLayoutEffect, useRef } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import type { TerminalWindow } from "../bindings/TerminalWindow";
import { layout } from "../canvas/layout";
import { linkDraft } from "../canvas/linkDraft";
import { viewport } from "../canvas/viewport";
import { sdk, type TermSpan } from "./sdk";
import { useCanvas } from "./store";

/** A rough ANSI-16 palette for mirror spans whose colour is `ansi-N`. Live
 *  xterm uses its own theme; this is only for the polled, read-only mirror. */
const ANSI: Record<string, string> = {
  "ansi-0": "#393939", "ansi-1": "#fa4d56", "ansi-2": "#42be65", "ansi-3": "#f1c21b",
  "ansi-4": "#78a9ff", "ansi-5": "#a56eff", "ansi-6": "#33b1ff", "ansi-7": "#c6c6c6",
  "ansi-8": "#8d8d8d", "ansi-9": "#ff8389", "ansi-10": "#6fdc8c", "ansi-11": "#f1c21b",
  "ansi-12": "#a6c8ff", "ansi-13": "#be95ff", "ansi-14": "#82cfff", "ansi-15": "#f4f4f4",
};

function colour(v: string | null | undefined): string | undefined {
  if (!v) return undefined;
  return v.startsWith("ansi-") ? ANSI[v] : v;
}

/** The read-only mirror: `session_logs`' styled spans, as a <pre> grid. */
function Mirror({ lines }: { lines: TermSpan[][] | undefined }) {
  if (!lines || lines.length === 0) {
    return <div className="node-empty">waiting for output…</div>;
  }
  return (
    <pre className="node-mirror">
      {lines.map((row, i) => (
        <div key={i}>
          {row.length === 0 ? (
            " "
          ) : (
            row.map((span, j) => (
              <span
                key={j}
                style={{
                  color: colour(span.fg),
                  background: colour(span.bg),
                  fontWeight: span.bold ? 700 : undefined,
                  opacity: span.dim ? 0.6 : undefined,
                  fontStyle: span.italic ? "italic" : undefined,
                  textDecoration: span.underline ? "underline" : undefined,
                }}
              >
                {span.text}
              </span>
            ))
          )}
        </div>
      ))}
    </pre>
  );
}

/** The focused node's live terminal: attach to the session's pty and stream it. */
function Live({ id }: { id: string }) {
  const hostRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;
    const term = new Terminal({
      fontSize: 12,
      fontFamily: "var(--mono, monospace)",
      theme: { background: "#161616", foreground: "#f4f4f4" },
      cursorBlink: true,
      convertEol: false,
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(host);
    fit.fit();

    // Listen before attaching so nothing emitted in between is lost.
    const offData = sdk.onPtyData(id, ({ data }) => {
      const bytes = Uint8Array.from(atob(data), (c) => c.charCodeAt(0));
      term.write(bytes);
    });
    const offExit = sdk.onPtyExit(id, () => term.write("\r\n\x1b[2m[session ended]\x1b[0m\r\n"));

    void sdk.ptyOpen(id, term.cols, term.rows).catch(() => {});

    const onData = (d: string) => void sdk.ptyWrite(id, btoa(unescape(encodeURIComponent(d)))).catch(() => {});
    const dataSub = term.onData(onData);

    const observer = new ResizeObserver(() => {
      fit.fit();
      void sdk.ptyResize(id, term.cols, term.rows).catch(() => {});
    });
    observer.observe(host);

    return () => {
      observer.disconnect();
      offData();
      offExit();
      dataSub.dispose();
      term.dispose();
      void sdk.ptyClose(id).catch(() => {});
    };
  }, [id]);

  return <div ref={hostRef} className="node-live" />;
}

interface Props {
  node: TerminalWindow;
  focused: boolean;
  logs: TermSpan[][] | undefined;
}

/**
 * One agent on the board. The whole window lives in the transformed world layer
 * (the Canvas applies the camera), so its position is world coordinates written
 * straight onto the element. It registers its rect in the shared `layout` so the
 * link layer can draw to it without a React render.
 */
export function TerminalNode({ node, focused, logs }: Props) {
  const moveNode = useCanvas((s) => s.moveNode);
  const focus = useCanvas((s) => s.focus);
  const addLink = useCanvas((s) => s.addLink);

  // Keep the shared layout registry in step with where this node is.
  useLayoutEffect(() => {
    layout.set(node.id, { x: node.x, y: node.y, width: node.width, height: node.height });
    return () => layout.delete(node.id);
  }, [node.id, node.x, node.y, node.width, node.height]);

  // Drag the titlebar to move the node. Reads the live camera each move so the
  // grabbed point stays under the pointer at any zoom.
  const onTitlePointerDown = (e: React.PointerEvent) => {
    if (e.button !== 0) return;
    e.stopPropagation();
    (e.target as Element).setPointerCapture(e.pointerId);
    const start = viewport.clientToWorld(e.clientX, e.clientY);
    const origin = { x: node.x, y: node.y };
    const move = (ev: PointerEvent) => {
      const p = viewport.clientToWorld(ev.clientX, ev.clientY);
      moveNode(node.id, {
        x: origin.x + (p.x - start.x),
        y: origin.y + (p.y - start.y),
        width: node.width,
        height: node.height,
      });
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };

  // Drag the handle to draw a link to another node.
  const onHandlePointerDown = (e: React.PointerEvent) => {
    e.stopPropagation();
    e.preventDefault();
    const move = (ev: PointerEvent) => {
      const to = viewport.clientToWorld(ev.clientX, ev.clientY);
      const target = hitTest(to.x, to.y, node.id);
      linkDraft.update(node.id, to, target);
    };
    const up = (ev: PointerEvent) => {
      const to = viewport.clientToWorld(ev.clientX, ev.clientY);
      const target = hitTest(to.x, to.y, node.id);
      if (target) addLink(node.id, target);
      linkDraft.clear();
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };

  return (
    <div
      className={`node${focused ? " is-focused" : ""}`}
      style={{ left: node.x, top: node.y, width: node.width, height: node.height, zIndex: node.z }}
      onPointerDown={() => focus(node.id)}
    >
      <div className="node-title" onPointerDown={onTitlePointerDown} onDoubleClick={() => focus(focused ? null : node.id)}>
        <span className="node-name">{node.name}</span>
        <span className="node-provider">{node.agent}</span>
        <span className="node-handle" title="Drag to link" onPointerDown={onHandlePointerDown}>
          ●
        </span>
      </div>
      <div className="node-body">{focused ? <Live id={node.id} /> : <Mirror lines={logs} />}</div>
    </div>
  );
}

/** The node whose rect contains a world point, other than `exceptId`. */
function hitTest(x: number, y: number, exceptId: string): string | null {
  for (const node of useCanvas.getState().nodes) {
    if (node.id === exceptId) continue;
    if (x >= node.x && x <= node.x + node.width && y >= node.y && y <= node.y + node.height) {
      return node.id;
    }
  }
  return null;
}
