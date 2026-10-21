// @vitest-environment jsdom
import { act } from "react";
import { type Root, createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Terminal } from "@xterm/xterm";
import type { TerminalWindow as TerminalModel } from "../bindings/TerminalWindow";
import { viewport } from "../canvas/viewport";
import { api } from "../lib/api";
import { useAppStore } from "../store/appStore";
import { useDialogStore } from "../store/dialogStore";
import { TerminalWindow } from "./TerminalWindow";
import { focusTerminalView, terminals } from "./TerminalView";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

vi.mock("./TerminalView", () => ({
  TerminalView: () => null,
  terminals: new Map(),
  focusTerminalView: vi.fn(),
}));
vi.mock("../lib/api", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../lib/api")>()),
  api: {
    renameTerminal: vi.fn(async () => null),
    closeTerminal: vi.fn(async () => null),
    duplicateTerminal: vi.fn(async () => ({ ...TERMINAL, id: "t2", name: "Terminal 1 2", x: 624, z: 2 })),
    focusTerminal: vi.fn(async () => 1),
    terminalHasForegroundProcess: vi.fn(async () => false),
  },
}));

const TERMINAL: TerminalModel = { id: "t1", name: "Terminal 1", x: 0, y: 0, width: 600, height: 400, z: 1 };

let container: HTMLDivElement;
let root: Root;
/** Stands in for xterm's hidden textarea, so focusing the shell really moves focus. */
let shellInput: HTMLTextAreaElement;
const shellFocus = vi.fn();

const query = <T extends Element>(selector: string) => container.querySelector<T>(selector);
const titleInput = () => query<HTMLInputElement>(".term-title-input");

function startRename() {
  act(() => query(".term-title")!.dispatchEvent(new MouseEvent("dblclick", { bubbles: true })));
  return titleInput()!;
}

function press(el: Element, key: string) {
  act(() => el.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true })));
}

beforeEach(() => {
  useAppStore.setState({
    ready: true,
    toast: null,
    selectedTerminalIds: [],
    activeWorkspaceId: "w1",
    workspaces: [{ id: "w1", name: "Main", camera: { x: 0, y: 0, zoom: 1 }, terminals: [TERMINAL], links: [] }],
  });
  shellInput = document.createElement("textarea");
  document.body.append(shellInput);
  terminals.set("t1", {
    focus: () => {
      shellFocus();
      shellInput.focus();
    },
  } as unknown as Terminal);
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
  act(() => root.render(<TerminalWindow terminal={TERMINAL} />));
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
  shellInput.remove();
  terminals.clear();
  useDialogStore.setState({ pending: null });
  vi.clearAllMocks();
});

describe("TerminalWindow rename", () => {
  it("opens on double-click; Enter saves the trimmed name and gives the shell its focus back", () => {
    const input = startRename();
    expect(input.value).toBe("Terminal 1");
    expect(document.activeElement).toBe(input);

    input.value = "  Planner  ";
    press(input, "Enter");

    expect(api.renameTerminal).toHaveBeenCalledTimes(1);
    expect(api.renameTerminal).toHaveBeenCalledWith("t1", "Planner");
    expect(titleInput()).toBeNull();
    expect(document.activeElement).toBe(shellInput);
  });

  it("Escape keeps the old name", () => {
    const input = startRename();
    input.value = "Discarded";
    press(input, "Escape");

    expect(api.renameTerminal).not.toHaveBeenCalled();
    expect(titleInput()).toBeNull();
    expect(document.activeElement).toBe(shellInput);
  });

  it("saves when focus leaves the field, without pulling focus back to the shell", () => {
    const elsewhere = document.createElement("button");
    document.body.append(elsewhere);
    const input = startRename();
    input.value = "Coder";
    act(() => elsewhere.focus());

    expect(api.renameTerminal).toHaveBeenCalledWith("t1", "Coder");
    expect(shellFocus).not.toHaveBeenCalled();
    elsewhere.remove();
  });

  it("stays open when the field itself is clicked", () => {
    const input = startRename();
    act(() => input.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true, button: 0 })));

    expect(titleInput()).toBe(input);
    expect(document.activeElement).toBe(input);
    expect(api.renameTerminal).not.toHaveBeenCalled();
  });
});

describe("TerminalWindow hover focus", () => {
  const OTHER: TerminalModel = { ...TERMINAL, id: "t2", name: "Terminal 2", z: 2 };

  /** React builds onPointerEnter from pointerover, coming from outside the window. */
  function hover(init: PointerEventInit = {}) {
    const event = new PointerEvent("pointerover", { bubbles: true, pointerType: "mouse", relatedTarget: document.body, ...init });
    act(() => query(".term-window")!.dispatchEvent(event));
  }

  afterEach(() => viewport.load("w1", { x: 0, y: 0, zoom: 1 }));

  it("gives the shell the keyboard when the mouse enters, without raising the window", () => {
    act(() => useAppStore.setState({ workspaces: [{ ...useAppStore.getState().workspaces[0]!, terminals: [TERMINAL, OTHER] }] }));
    hover();

    expect(document.activeElement).toBe(shellInput);
    expect(api.focusTerminal).not.toHaveBeenCalled();
    expect(useAppStore.getState().workspaces[0]!.terminals.map((t) => t.z)).toEqual([1, 2]);
  });

  it("ignores touch, and a mouse that enters mid-drag", () => {
    hover({ pointerType: "touch" });
    hover({ buttons: 1 });

    expect(shellFocus).not.toHaveBeenCalled();
  });

  it("leaves a text field being edited elsewhere alone", () => {
    const field = document.createElement("input");
    document.body.append(field);
    field.focus();
    hover();

    expect(document.activeElement).toBe(field);
    field.remove();
  });

  it("leaves the keyboard with the canvas while windows are selected, so Delete acts on them", () => {
    act(() => useAppStore.setState({ selectedTerminalIds: ["t1"] }));
    hover();

    expect(shellFocus).not.toHaveBeenCalled();
  });

  it("does nothing while Space pans the canvas or when zoomed too far out to read", () => {
    container.dataset["spacePan"] = "true";
    hover();
    container.dataset["spacePan"] = "false";
    viewport.load("w1", { x: 0, y: 0, zoom: 0.2 });
    hover();

    expect(shellFocus).not.toHaveBeenCalled();
  });
});

describe("TerminalWindow selection", () => {
  const shiftDown = (el: Element) =>
    act(() => el.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true, cancelable: true, button: 0, shiftKey: true })));

  it("Shift+click on the title picks the window, without moving it or focusing the shell", () => {
    shiftDown(query(".term-title")!);

    expect(useAppStore.getState().selectedTerminalIds).toEqual(["t1"]);
    expect(query(".term-window")!.hasAttribute("data-selected")).toBe(true);
    expect(shellFocus).not.toHaveBeenCalled();
    expect(document.documentElement.dataset["interacting"]).toBeUndefined();

    shiftDown(query(".term-title")!);
    expect(useAppStore.getState().selectedTerminalIds).toEqual([]);
  });

  it("a plain click on a window clears the selection and goes back to its shell", () => {
    act(() => useAppStore.setState({ selectedTerminalIds: ["t1", "t2"] }));
    act(() => query(".term-body")!.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true, button: 0 })));

    expect(useAppStore.getState().selectedTerminalIds).toEqual([]);
    expect(document.activeElement).toBe(shellInput);
  });

  it("a Shift+double-click does not start renaming", () => {
    act(() => query(".term-title")!.dispatchEvent(new MouseEvent("dblclick", { bubbles: true, shiftKey: true })));
    expect(titleInput()).toBeNull();
  });
});

describe("TerminalWindow duplicate", () => {
  it("its title bar button opens a copy beside it and gives the copy the keyboard", async () => {
    await act(async () => query<HTMLButtonElement>('[aria-label="Duplicate Terminal 1"]')!.click());

    expect(api.duplicateTerminal).toHaveBeenCalledWith("t1", expect.objectContaining({ width: 600, height: 400 }));
    expect(useAppStore.getState().workspaces[0]!.terminals.map((t) => t.id)).toEqual(["t1", "t2"]);
    expect(focusTerminalView).toHaveBeenCalledWith("t2");
  });
});

describe("TerminalWindow close", () => {
  const closeButton = () => query<HTMLButtonElement>(".term-close")!;
  const shownTerminals = () => useAppStore.getState().workspaces[0]!.terminals.map((t) => t.id);

  it("closes right away when nothing runs in the foreground", async () => {
    await act(async () => closeButton().click());

    expect(useDialogStore.getState().pending).toBeNull();
    expect(api.closeTerminal).toHaveBeenCalledWith("t1");
    expect(shownTerminals()).toEqual([]);
  });

  it("stays open and explains why when the backend cannot close it", async () => {
    vi.mocked(api.closeTerminal).mockRejectedValueOnce("terminal t1 not found");
    await act(async () => closeButton().click());

    expect(shownTerminals()).toEqual(["t1"]);
    expect(useAppStore.getState().toast).toBe("terminal t1 not found");
  });

  it("asks first when a program is running, and keeps the terminal if cancelled", async () => {
    vi.mocked(api.terminalHasForegroundProcess).mockResolvedValue(true);
    await act(async () => closeButton().click());
    expect(useDialogStore.getState().pending?.title).toBe("Close “Terminal 1”?");

    await act(async () => useDialogStore.getState().close(false));

    expect(api.closeTerminal).not.toHaveBeenCalled();
    expect(document.activeElement).toBe(shellInput);
  });

  it("closes after the confirmation is accepted", async () => {
    vi.mocked(api.terminalHasForegroundProcess).mockResolvedValue(true);
    await act(async () => closeButton().click());
    await act(async () => useDialogStore.getState().close(true));

    expect(api.closeTerminal).toHaveBeenCalledWith("t1");
  });

  it("a double-click on × asks once and does not start renaming", async () => {
    let answer!: (busy: boolean) => void;
    vi.mocked(api.terminalHasForegroundProcess).mockReturnValue(new Promise((resolve) => (answer = resolve)));
    act(() => {
      closeButton().click();
      closeButton().click();
      closeButton().dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
    });
    await act(async () => answer(true));
    await act(async () => useDialogStore.getState().close(true));

    expect(titleInput()).toBeNull();
    expect(api.terminalHasForegroundProcess).toHaveBeenCalledTimes(1);
    expect(api.closeTerminal).toHaveBeenCalledTimes(1);
  });
});
