// @vitest-environment jsdom
import { act } from "react";
import { type Root, createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Camera } from "../bindings/Camera";
import type { TerminalWindow as TerminalModel } from "../bindings/TerminalWindow";
import { viewport } from "../canvas/viewport";
import { api } from "../lib/api";
import { useAppStore } from "../store/appStore";
import { Canvas } from "./Canvas";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

/** Every mount and unmount of a terminal window, to tell a re-attach from a restart. */
const lifecycle: string[] = [];

vi.mock("./TerminalWindow", async () => {
  const { useEffect } = await import("react");
  return {
    TerminalWindow: ({ terminal }: { terminal: TerminalModel }) => {
      useEffect(() => {
        lifecycle.push(`mount ${terminal.id}`);
        return () => void lifecycle.push(`unmount ${terminal.id}`);
      }, [terminal.id]);
      return (
        <div className="mock-terminal" data-terminal-id={terminal.id}>
          <div className="term-body">
            <textarea className="mock-shell" />
          </div>
        </div>
      );
    },
  };
});
vi.mock("./LinkLayer", () => ({ LinkLayer: () => null }));
vi.mock("../lib/api", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../lib/api")>()),
  api: {
    setActiveWorkspace: vi.fn(async () => null),
    updateCamera: vi.fn(async () => null),
    closeTerminal: vi.fn(async () => null),
  },
  events: { onCanvasPinch: vi.fn(async () => () => undefined) },
}));

const CAMERA_1: Camera = { x: 10, y: 20, zoom: 1 };
const CAMERA_2: Camera = { x: -300, y: 40, zoom: 0.5 };

function terminal(id: string): TerminalModel {
  return { id, name: id, x: 0, y: 0, width: 600, height: 400, z: 1 };
}

let container: HTMLDivElement;
let root: Root;

const shownTerminals = () =>
  [...container.querySelectorAll<HTMLElement>(".workspace-layer:not([inert]) .mock-terminal")].map(
    (el) => el.dataset["terminalId"],
  );
const savedCamera = (workspaceId: string) =>
  useAppStore.getState().workspaces.find((ws) => ws.id === workspaceId)?.camera;

async function switchTo(workspaceId: string) {
  await act(() => useAppStore.getState().setActiveWorkspace(workspaceId));
}

beforeEach(() => {
  vi.useFakeTimers();
  vi.stubGlobal(
    "ResizeObserver",
    class {
      observe() {}
      disconnect() {}
    },
  );
  useAppStore.setState({
    ready: true,
    activeWorkspaceId: "w1",
    workspaces: [
      { id: "w1", name: "Main", camera: CAMERA_1, terminals: [terminal("t1")], links: [], knowledgeBases: [] },
      { id: "w2", name: "Side", camera: CAMERA_2, terminals: [terminal("t2")], links: [], knowledgeBases: [] },
    ],
  });
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
  act(() => root.render(<Canvas />));
  lifecycle.length = 0;
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
  vi.useRealTimers();
  vi.unstubAllGlobals();
  vi.clearAllMocks();
});

describe("Canvas focus follows the mouse", () => {
  const canvas = () => container.querySelector<HTMLElement>(".canvas")!;
  const shell = () => container.querySelector<HTMLTextAreaElement>(".mock-shell")!;

  function leaveTerminalForBackground(init: PointerEventInit = {}) {
    const from = container.querySelector(".mock-terminal");
    act(() => canvas().dispatchEvent(new PointerEvent("pointerover", { bubbles: true, pointerType: "mouse", relatedTarget: from, ...init })));
  }

  it("takes the keyboard back from a terminal when the mouse moves onto the background", () => {
    shell().focus();
    leaveTerminalForBackground();

    expect(document.activeElement).toBe(document.body);
  });

  it("keeps the terminal focused mid-drag, for touch, and over another window", () => {
    shell().focus();
    leaveTerminalForBackground({ buttons: 1 });
    leaveTerminalForBackground({ pointerType: "touch" });
    act(() => shell().dispatchEvent(new PointerEvent("pointerover", { bubbles: true, pointerType: "mouse" })));

    expect(document.activeElement).toBe(shell());
  });

  it("leaves a text field outside the terminals alone", () => {
    const field = document.createElement("input");
    document.body.append(field);
    field.focus();
    leaveTerminalForBackground();

    expect(document.activeElement).toBe(field);
    field.remove();
  });
});

describe("Canvas selection box", () => {
  const canvas = () => container.querySelector<HTMLElement>(".canvas")!;
  const pointer = (type: string, x: number, y: number, init: PointerEventInit = {}) =>
    act(() =>
      canvas().dispatchEvent(
        new PointerEvent(type, { bubbles: true, cancelable: true, button: 0, pointerId: 1, clientX: x, clientY: y, ...init }),
      ),
    );

  beforeEach(() => {
    // jsdom has no pointer capture.
    Object.assign(HTMLElement.prototype, {
      setPointerCapture() {},
      releasePointerCapture() {},
      hasPointerCapture: () => false,
    });
    useAppStore.setState({
      selectedTerminalIds: [],
      workspaces: [
        {
          id: "w1",
          name: "Main",
          camera: { x: 0, y: 0, zoom: 1 },
          terminals: [terminal("t1"), { ...terminal("t3"), x: 1000, y: 0 }],
          links: [],
          knowledgeBases: [],
        },
        { id: "w2", name: "Side", camera: CAMERA_2, terminals: [terminal("t2")], links: [], knowledgeBases: [] },
      ],
    });
    act(() => viewport.load("w1", { x: 0, y: 0, zoom: 1 }));
  });

  it("Shift+drag on the background selects every window the box touches, and only those", () => {
    pointer("pointerdown", 500, 300, { shiftKey: true });
    pointer("pointermove", 700, 450, { shiftKey: true });

    expect(container.querySelector<HTMLElement>(".marquee")!.hidden).toBe(false);
    expect(useAppStore.getState().selectedTerminalIds).toEqual(["t1"]);

    pointer("pointerup", 700, 450);
    expect(container.querySelector<HTMLElement>(".marquee")!.hidden).toBe(true);
  });

  it("a plain click on the background clears the selection", () => {
    act(() => useAppStore.setState({ selectedTerminalIds: ["t1"] }));
    pointer("pointerdown", 900, 700);

    expect(useAppStore.getState().selectedTerminalIds).toEqual([]);
  });
});

describe("Canvas workspace switching", () => {
  it("shows each workspace's own camera and terminals", async () => {
    expect(viewport.get()).toEqual(CAMERA_1);
    expect(shownTerminals()).toEqual(["t1"]);

    await switchTo("w2");
    expect(viewport.get()).toEqual(CAMERA_2);
    expect(shownTerminals()).toEqual(["t2"]);

    await switchTo("w1");
    expect(viewport.get()).toEqual(CAMERA_1);
    expect(shownTerminals()).toEqual(["t1"]);
  });

  it("keeps the windows of a hidden workspace mounted, so switching back rebuilds nothing", async () => {
    await switchTo("w2");
    await switchTo("w1");

    expect(lifecycle).toEqual([]);
    expect(api.closeTerminal).not.toHaveBeenCalled();
  });

  it("makes a hidden workspace inert and takes the keyboard away from its terminals", async () => {
    const shell = container.querySelector<HTMLTextAreaElement>('[data-terminal-id="t1"] .mock-shell')!;
    shell.focus();
    await switchTo("w2");

    expect(shell.closest(".workspace-layer")!.hasAttribute("inert")).toBe(true);
    expect(document.activeElement).not.toBe(shell);
  });

  it("keeps a camera moved right before switching away, even when the next one moves at once", async () => {
    const moved: Camera = { x: 500, y: -80, zoom: 2 };
    act(() => viewport.set(moved));
    await switchTo("w2");
    act(() => viewport.set({ ...CAMERA_2, x: CAMERA_2.x + 15 }));
    act(() => vi.runAllTimers());

    expect(api.updateCamera).toHaveBeenCalledWith("w1", moved);
    expect(savedCamera("w1")).toEqual(moved);

    await switchTo("w1");
    expect(viewport.get()).toEqual(moved);
  });

  it("returns to a camera moved less than the save delay before switching", async () => {
    const moved: Camera = { x: 1, y: 2, zoom: 3 };
    act(() => viewport.set(moved));
    await switchTo("w2");
    await switchTo("w1");

    expect(viewport.get()).toEqual(moved);
  });
});
