import { useEffect, useState, useSyncExternalStore } from "react";
import type { Rect } from "../bindings/Rect";
import { fitToContent, resetZoom, zoomBy } from "../canvas/actions";
import { layout } from "../canvas/layout";
import { viewport } from "../canvas/viewport";
import { api, errorMessage } from "../lib/api";
import { MOD_KEY, PLATFORM } from "../lib/platform";
import { appWindow } from "../lib/appWindow";
import { activeWorkspace, useAppStore } from "../store/appStore";
import { formatTokens } from "../canvas/subagents";
import { AppsMenu } from "./AppsMenu";
import { FitIcon, GripIcon, KnowledgeIcon, LinkIcon, MoreIcon, PlusIcon } from "./icons";
import { closeSelectedTerminals, openNewTerminal, terminalCount } from "./shortcuts";
import { usePopover } from "./usePopover";
import { WorkspaceMenu } from "./WorkspaceMenu";

const ZOOM_STEP = 1.2;

const SHORTCUTS: Array<[action: string, keys: string]> = [
  ["New terminal", `${MOD_KEY} + Shift + T`],
  ["Duplicate a terminal (folder and program)", `${MOD_KEY} + Shift + D`],
  ...(PLATFORM === "windows"
    ? [
        ["Show Godesk, or minimize it", "Ctrl + Alt + F"] as [string, string],
        ["Open a program on the canvas", "Window button in the dock"] as [string, string],
      ]
    : []),
  ["Pan", "Scroll, middle-drag or Space + drag"],
  ["Zoom", `${MOD_KEY} + scroll, or pinch`],
  ["Link two terminals", "Drag a window's ● handle onto another"],
  ["Let an agent message another terminal (links them)", "godesk send <terminal> <message>"],
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
  const captureLaunches = useAppStore((s) => s.captureLaunches);
  const toggleCaptureLaunches = useAppStore((s) => s.toggleCaptureLaunches);
  const agentView = useAppStore((s) => s.agentView);
  const setAgentView = useAppStore((s) => s.setAgentView);
  const { open, toggle, buttonRef, panelRef } = usePopover();
  const [agentNotes, setAgentNotes] = useState<boolean | null>(null);

  // Read each time the menu opens: the files may have been edited by hand.
  useEffect(() => {
    if (!open || PLATFORM !== "windows") return;
    let current = true;
    Promise.resolve()
      .then(() => api.agentNotesInstalled())
      .then((on) => current && setAgentNotes(on))
      .catch(() => current && setAgentNotes(null));
    return () => {
      current = false;
    };
  }, [open]);

  const toggleAgentNotes = () => {
    const on = !agentNotes;
    api
      .setAgentNotes(on)
      .then(() => setAgentNotes(on))
      .catch((e) => notify(errorMessage(e)));
  };
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
          {PLATFORM === "windows" && (
            <button
              type="button"
              className="menu-row"
              role="switch"
              aria-checked={captureLaunches}
              title="code ., explorer . or notepad typed in a terminal opens beside it"
              onClick={toggleCaptureLaunches}
            >
              Open programs from terminals here
              <span className="switch" aria-hidden="true" />
            </button>
          )}
          <div className="menu-row menu-choice-row">
            <span id="agent-view-label">Agents open in</span>
            <div className="menu-choice" role="radiogroup" aria-labelledby="agent-view-label">
              <button
                type="button"
                role="radio"
                aria-checked={agentView}
                title="claude, codex, opencode or mimo run in a terminal show Godesk's view: live thinking, diffs, commands and permissions"
                onClick={() => setAgentView(true)}
              >
                Godesk view
              </button>
              <button
                type="button"
                role="radio"
                aria-checked={!agentView}
                title="Agents show their own screen in the terminal, as before. Open ones keep their conversation."
                onClick={() => setAgentView(false)}
              >
                Classic terminal
              </button>
            </div>
          </div>
          {PLATFORM === "windows" && agentNotes !== null && (
            <button
              type="button"
              className="menu-row"
              role="switch"
              aria-checked={agentNotes}
              title="Adds a note to ~/.claude/CLAUDE.md and ~/.codex/AGENTS.md so agents know the terminals by name and the godesk command"
              onClick={toggleAgentNotes}
            >
              Agents know the terminals by name
              <span className="switch" aria-hidden="true" />
            </button>
          )}
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

/** Every agent's tokens together; the breakdown per terminal opens from it. */
function TokensMenu() {
  const agents = useAppStore((s) => s.agents);
  const total = useAppStore((s) => s.tokenTotal);
  const workspaces = useAppStore((s) => s.workspaces);
  const { open, toggle, buttonRef, panelRef } = usePopover();
  if (agents.length === 0) return null;
  const names = new Map(workspaces.flatMap((w) => w.terminals.map((t) => [t.id, t.name] as const)));
  const sum = total.input + total.output;

  return (
    <div className="popover-anchor">
      <button
        ref={buttonRef}
        type="button"
        className="dock-btn dock-btn-tokens"
        aria-label={`${sum} tokens used by the agents`}
        aria-expanded={open}
        aria-controls="tokens-panel"
        data-tip={open ? undefined : "Tokens of every agent"}
        onClick={toggle}
      >
        {formatTokens(sum)}
        <span className="dock-btn-unit">tokens</span>
      </button>
      {open && (
        <div ref={panelRef} id="tokens-panel" className="popover popover-end" role="dialog" aria-label="Tokens">
          <h2 className="popover-title">Tokens of the agents</h2>
          <dl className="shortcuts">
            {agents.map((a) => {
              const working = a.subagents.filter((s) => !s.finished).length;
              return (
                <div key={a.terminalId} className="shortcut">
                  <dt>
                    {names.get(a.terminalId) ?? "Terminal"} · {a.agent}
                    {working > 0 && ` · ${working} subagent${working === 1 ? "" : "s"} working`}
                  </dt>
                  <dd title={`${a.tokens.input} read · ${a.tokens.output} written`}>
                    {formatTokens(a.tokens.input + a.tokens.output)}
                  </dd>
                </div>
              );
            })}
            <div className="shortcut">
              <dt>
                <strong>All agents</strong>
              </dt>
              <dd>
                <strong>{formatTokens(sum)}</strong> ({formatTokens(total.input)} read · {formatTokens(total.output)}{" "}
                written)
              </dd>
            </div>
          </dl>
          <p className="menu-note">
            Read counts the prompt, context and cache an agent sends each turn; written, its answers. Subagents count
            toward their agent.
          </p>
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
  const createKnowledgeBase = useAppStore((s) => s.createKnowledgeBase);
  const notify = useAppStore((s) => s.notify);

  const fit = () => {
    const ws = activeWorkspace(useAppStore.getState());
    if (!ws) return;
    const apps = useAppStore.getState().apps.filter((a) => a.workspaceId === ws.id);
    const rects: Rect[] = [...ws.terminals, ...apps].map((w) => layout.get(w.id) ?? w);
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
      <button
        type="button"
        className="dock-btn"
        aria-label="Add knowledge base"
        data-tip="Add a knowledge base (a folder of documents)"
        disabled={!ready}
        onClick={() => void createKnowledgeBase().catch((e) => notify(errorMessage(e)))}
      >
        <KnowledgeIcon />
      </button>
      {PLATFORM === "windows" && <AppsMenu />}
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
      <TokensMenu />
      <MoreMenu />
    </div>
  );
}
