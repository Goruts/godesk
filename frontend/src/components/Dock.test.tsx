// @vitest-environment jsdom
import { act } from "react";
import { type Root, createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Link } from "../bindings/Link";
import type { TerminalWindow } from "../bindings/TerminalWindow";
import type { Workspace } from "../bindings/Workspace";
import { api } from "../lib/api";
import { appWindow } from "../lib/appWindow";
import { useAppStore } from "../store/appStore";
import { Dock } from "./Dock";
import { closeSelectedTerminals, openNewTerminal } from "./shortcuts";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

// The real one pulls in xterm, which needs a canvas jsdom does not have.
vi.mock("./shortcuts", async (importOriginal) => ({
  ...(await importOriginal<typeof import("./shortcuts")>()),
  openNewTerminal: vi.fn(async () => undefined),
  closeSelectedTerminals: vi.fn(async () => undefined),
}));
vi.mock("../lib/api", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../lib/api")>()),
  api: { deleteLink: vi.fn(async () => null) },
}));
vi.mock("../lib/appWindow", () => ({
  appWindow: {
    minimize: vi.fn(async () => undefined),
    toggleMaximize: vi.fn(async () => undefined),
    close: vi.fn(async () => undefined),
  },
}));

function terminal(id: string): TerminalWindow {
  return { id, name: id, x: 0, y: 0, width: 600, height: 400, z: 1 };
}

const LINK: Link = { id: "l1", sourceId: "t1", targetId: "t2", kind: "manual", lastActivityAt: 0 };

const WORKSPACE: Workspace = {
  id: "w1",
  name: "Main",
  camera: { x: 0, y: 0, zoom: 1 },
  terminals: [terminal("t1"), terminal("t2")],
  links: [LINK],
  knowledgeBases: [],
};

let container: HTMLDivElement;
let root: Root;

const button = (label: string) => container.querySelector<HTMLButtonElement>(`[aria-label="${label}"]`)!;
const buttonByText = (text: string) =>
  [...container.querySelectorAll<HTMLButtonElement>("button")].find((b) => b.textContent?.trim() === text);
const morePanel = () => document.getElementById("more-panel");

beforeEach(() => {
  window.localStorage.clear();
  useAppStore.setState({
    ready: true,
    toast: null,
    activeWorkspaceId: "w1",
    workspaces: [WORKSPACE],
    linksVisible: true,
    transparentCanvas: true,
    selectedLinkId: null,
    selectedTerminalIds: [],
  });
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
  act(() => root.render(<Dock />));
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
  vi.clearAllMocks();
});

describe("Dock", () => {
  it("opens a terminal like the keyboard shortcut does", () => {
    act(() => button("New terminal").click());
    expect(openNewTerminal).toHaveBeenCalledTimes(1);
  });

  it("cannot create a terminal before the state is loaded", () => {
    act(() => useAppStore.setState({ ready: false }));
    expect(button("New terminal").disabled).toBe(true);
  });

  it("shows and hides the links, reflecting the state in aria-pressed", () => {
    expect(button("Show links").getAttribute("aria-pressed")).toBe("true");
    act(() => button("Show links").click());

    expect(useAppStore.getState().linksVisible).toBe(false);
    expect(button("Show links").getAttribute("aria-pressed")).toBe("false");
  });

  it("offers to delete the selected link, only while links are shown", async () => {
    expect(buttonByText("Delete link")).toBeUndefined();
    act(() => useAppStore.setState({ selectedLinkId: "l1" }));
    await act(async () => buttonByText("Delete link")!.click());

    expect(api.deleteLink).toHaveBeenCalledWith("l1");
    act(() => useAppStore.setState({ selectedLinkId: "l1", linksVisible: false }));
    expect(buttonByText("Delete link")).toBeUndefined();
  });

  it("offers to close the selected terminals, counting them", () => {
    expect(buttonByText("Close 2 terminals")).toBeUndefined();
    act(() => useAppStore.setState({ selectedTerminalIds: ["t1", "t2"] }));
    act(() => buttonByText("Close 2 terminals")!.click());

    expect(closeSelectedTerminals).toHaveBeenCalledTimes(1);
  });

  it("has a window drag region, since the native frame is off", () => {
    expect(container.querySelector(".dock-grip")!.hasAttribute("data-tauri-drag-region")).toBe(true);
  });
});

describe("Dock more options", () => {
  beforeEach(() => act(() => button("More options").click()));

  it("switches the canvas to solid and remembers the choice", () => {
    const toggle = morePanel()!.querySelector<HTMLButtonElement>('[role="switch"]')!;
    expect(toggle.getAttribute("aria-checked")).toBe("true");
    act(() => toggle.click());

    expect(useAppStore.getState().transparentCanvas).toBe(false);
    expect(toggle.getAttribute("aria-checked")).toBe("false");
    expect(window.localStorage.getItem("godesk.transparentCanvas")).toBe("false");
  });

  it("lists the shortcuts", () => {
    expect(morePanel()!.textContent).toContain("Ctrl + Shift + T");
  });

  it("drives the window", async () => {
    await act(async () => buttonByText("Minimize")!.click());
    await act(async () => buttonByText("Maximize")!.click());
    await act(async () => buttonByText("Quit")!.click());

    expect(appWindow.minimize).toHaveBeenCalledTimes(1);
    expect(appWindow.toggleMaximize).toHaveBeenCalledTimes(1);
    expect(appWindow.close).toHaveBeenCalledTimes(1);
  });

  it("explains a refused window action instead of failing silently", async () => {
    vi.mocked(appWindow.close).mockRejectedValueOnce("window.close not allowed");
    await act(async () => buttonByText("Quit")!.click());

    expect(useAppStore.getState().toast).toBe("window.close not allowed");
  });

  it("Escape closes it and returns focus to its button", () => {
    act(() => window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" })));
    expect(morePanel()).toBeNull();
    expect(document.activeElement).toBe(button("More options"));
  });
});
