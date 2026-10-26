// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Rect } from "../bindings/Rect";
import type { TerminalWindow } from "../bindings/TerminalWindow";
import { viewport } from "../canvas/viewport";
import { api } from "../lib/api";
import { DEFAULT_TERMINAL_HEIGHT, DEFAULT_TERMINAL_WIDTH, useAppStore } from "../store/appStore";
import { useDialogStore } from "../store/dialogStore";
import { installShortcuts } from "./shortcuts";
import { focusTerminalView } from "./TerminalView";

vi.mock("./TerminalView", () => ({ focusTerminalView: vi.fn() }));
vi.mock("../lib/api", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../lib/api")>()),
  api: {
    createTerminal: vi.fn(
      async (_workspaceId: string, rect: Rect): Promise<TerminalWindow> => ({ id: "new", name: "Terminal 2", ...rect, z: 2 }),
    ),
    closeTerminal: vi.fn(async () => null),
    duplicateTerminal: vi.fn(async (_id: string, rect: Rect): Promise<TerminalWindow> => ({ id: "copy", name: "Terminal 1 2", ...rect, z: 3 })),
    terminalHasForegroundProcess: vi.fn(async () => false),
  },
}));

const existing = (x: number, y: number): TerminalWindow => ({ id: "old", name: "Terminal 1", x, y, width: 720, height: 440, z: 1 });

function setTerminals(terminals: TerminalWindow[]) {
  useAppStore.setState({
    activeWorkspaceId: "w1",
    workspaces: [{ id: "w1", name: "Main", camera: { x: 100, y: 50, zoom: 2 }, terminals, links: [], knowledgeBases: [] }],
  });
}

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

function pressNewTerminal(init: KeyboardEventInit = {}) {
  const event = new KeyboardEvent("keydown", { key: "T", ctrlKey: true, shiftKey: true, cancelable: true, ...init });
  window.dispatchEvent(event);
  return event;
}

let uninstall: () => void;

beforeEach(() => {
  // A 1000×800 canvas looking at canvas point (200, 175) in its center.
  viewport.setBounds({ left: 0, top: 0, width: 1000, height: 800 });
  viewport.load("w1", { x: 100, y: 50, zoom: 2 });
  setTerminals([]);
  uninstall = installShortcuts();
});

afterEach(() => {
  uninstall();
  vi.clearAllMocks();
});

describe("new terminal shortcut", () => {
  it("Ctrl+Shift+T creates a terminal centered in the view and focuses it", async () => {
    const event = pressNewTerminal();
    await flush();

    expect(event.defaultPrevented).toBe(true);
    expect(api.createTerminal).toHaveBeenCalledWith("w1", {
      x: 200 - DEFAULT_TERMINAL_WIDTH / 2,
      y: 175 - DEFAULT_TERMINAL_HEIGHT / 2,
      width: DEFAULT_TERMINAL_WIDTH,
      height: DEFAULT_TERMINAL_HEIGHT,
    });
    expect(focusTerminalView).toHaveBeenCalledWith("new");
  });

  it("works with Cmd on macOS", async () => {
    pressNewTerminal({ ctrlKey: false, metaKey: true });
    await flush();
    expect(api.createTerminal).toHaveBeenCalledTimes(1);
  });

  it("cascades instead of stacking on a terminal already in the center", async () => {
    setTerminals([existing(200 - DEFAULT_TERMINAL_WIDTH / 2, 175 - DEFAULT_TERMINAL_HEIGHT / 2)]);
    pressNewTerminal();
    await flush();

    const rect = vi.mocked(api.createTerminal).mock.calls[0]![1];
    expect(rect.x).toBe(200 - DEFAULT_TERMINAL_WIDTH / 2 + 32);
    expect(rect.y).toBe(175 - DEFAULT_TERMINAL_HEIGHT / 2 + 32);
  });

  it("opens one terminal while the keys are held down", async () => {
    pressNewTerminal();
    const repeat = pressNewTerminal({ repeat: true });
    await flush();

    expect(repeat.defaultPrevented).toBe(true);
    expect(api.createTerminal).toHaveBeenCalledTimes(1);
  });
});

describe("selected terminals", () => {
  const ids = () => useAppStore.getState().workspaces[0]!.terminals.map((t) => t.id);
  const press = (key: string) => window.dispatchEvent(new KeyboardEvent("keydown", { key, cancelable: true }));

  beforeEach(() => {
    setTerminals([existing(0, 0), { ...existing(800, 0), id: "other" }]);
    useAppStore.setState({ selectedTerminalIds: ["old", "other"] });
  });

  afterEach(() => useDialogStore.setState({ pending: null }));

  it("Delete closes them all at once when nothing is running", async () => {
    press("Delete");
    await flush();

    expect(api.closeTerminal).toHaveBeenCalledTimes(2);
    expect(ids()).toEqual([]);
    expect(useDialogStore.getState().pending).toBeNull();
  });

  it("asks once when a program is running, and keeps them all if cancelled", async () => {
    vi.mocked(api.terminalHasForegroundProcess).mockResolvedValueOnce(true);
    press("Delete");
    await flush();

    expect(useDialogStore.getState().pending?.title).toBe("Close 2 terminals?");
    useDialogStore.getState().close(false);
    await flush();
    expect(api.closeTerminal).not.toHaveBeenCalled();
    expect(ids()).toEqual(["old", "other"]);
  });

  it("Escape clears the selection", () => {
    press("Escape");
    expect(useAppStore.getState().selectedTerminalIds).toEqual([]);
  });
});

describe("duplicate shortcut", () => {
  const pressDuplicate = (target: EventTarget = window) => {
    const event = new KeyboardEvent("keydown", { key: "D", ctrlKey: true, shiftKey: true, bubbles: true, cancelable: true });
    target.dispatchEvent(event);
    return event;
  };

  beforeEach(() => {
    setTerminals([existing(0, 0)]);
    useAppStore.setState({ selectedTerminalIds: [] });
  });

  it("Ctrl+Shift+D duplicates the terminal that has the keyboard and focuses the copy", async () => {
    const win = document.createElement("div");
    win.className = "term-window";
    win.dataset["terminalId"] = "old";
    const shell = document.createElement("textarea");
    win.append(shell);
    document.body.append(win);
    shell.focus();

    const event = pressDuplicate(shell);
    await flush();

    expect(event.defaultPrevented).toBe(true);
    expect(api.duplicateTerminal).toHaveBeenCalledWith("old", expect.objectContaining({ width: 720, height: 440 }));
    expect(focusTerminalView).toHaveBeenCalledWith("copy");
    win.remove();
  });

  it("works on a single selected terminal too", async () => {
    useAppStore.setState({ selectedTerminalIds: ["old"] });
    pressDuplicate();
    await flush();

    expect(api.duplicateTerminal).toHaveBeenCalledWith("old", expect.anything());
  });

  it("leaves the keys alone when no terminal is current", () => {
    const event = pressDuplicate();

    expect(event.defaultPrevented).toBe(false);
    expect(api.duplicateTerminal).not.toHaveBeenCalled();
  });
});
