// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Link } from "../bindings/Link";
import type { Rect } from "../bindings/Rect";
import type { TerminalWindow } from "../bindings/TerminalWindow";
import type { Workspace } from "../bindings/Workspace";
import { viewport } from "../canvas/viewport";
import { api } from "../lib/api";
import { useAppStore } from "./appStore";

// Like Tauri 2, commands that return `()` in Rust resolve to null.
vi.mock("../lib/api", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../lib/api")>()),
  api: {
    renameWorkspace: vi.fn(async () => null),
    deleteWorkspace: vi.fn(async () => "w2"),
    createTerminal: vi.fn(),
    duplicateTerminal: vi.fn(async (_id: string, rect: Rect) => ({ id: "copy", name: "t1 2", ...rect, z: 2 })),
    closeTerminal: vi.fn(async () => null),
    updateTerminalRect: vi.fn(),
  },
}));

function terminal(id: string): TerminalWindow {
  return { id, name: id, x: 0, y: 0, width: 600, height: 400, z: 1 };
}

function workspace(id: string, name: string, terminals: TerminalWindow[] = [], links: Link[] = []): Workspace {
  return { id, name, camera: { x: 0, y: 0, zoom: 1 }, terminals, links };
}

const LINK: Link = { id: "l1", sourceId: "t1", targetId: "t2", kind: "manual", lastActivityAt: 0 };

const state = () => useAppStore.getState();
const main = () => state().workspaces.find((ws) => ws.id === "w1")!;

beforeEach(() => {
  useAppStore.setState({
    ready: true,
    toast: null,
    activeWorkspaceId: "w1",
    selectedLinkId: null,
    selectedTerminalIds: [],
    workspaces: [workspace("w1", "Main", [terminal("t1"), terminal("t2")], [LINK]), workspace("w2", "Agents")],
  });
});

afterEach(() => {
  vi.clearAllMocks();
});

describe("closeTerminal", () => {
  it("removes the window and its links once the backend closes it", async () => {
    await state().closeTerminal("t1");

    expect(api.closeTerminal).toHaveBeenCalledWith("t1");
    expect(main().terminals.map((t) => t.id)).toEqual(["t2"]);
    expect(main().links).toEqual([]);
    expect(state().toast).toBeNull();
  });

  it("keeps the window and explains why when the backend refuses", async () => {
    vi.mocked(api.closeTerminal).mockRejectedValueOnce("terminal t1 not found");
    await state().closeTerminal("t1");

    expect(main().terminals.map((t) => t.id)).toEqual(["t1", "t2"]);
    expect(main().links).toEqual([LINK]);
    expect(state().toast).toBe("terminal t1 not found");
  });
});

describe("renameWorkspace", () => {
  it("keeps the new name once the backend saves it", async () => {
    await state().renameWorkspace("w1", "  Planning  ");

    expect(api.renameWorkspace).toHaveBeenCalledWith("w1", "Planning");
    expect(main().name).toBe("Planning");
    expect(state().toast).toBeNull();
  });

  it("puts the old name back and explains why when the backend refuses", async () => {
    vi.mocked(api.renameWorkspace).mockRejectedValueOnce("workspace not found");
    await state().renameWorkspace("w1", "Gone");

    expect(main().name).toBe("Main");
    expect(state().toast).toBe("workspace not found");
  });

  it("a refused rename does not undo a later one", async () => {
    let refuse!: (reason: string) => void;
    vi.mocked(api.renameWorkspace).mockReturnValueOnce(new Promise((_, reject) => (refuse = reject)));
    const first = state().renameWorkspace("w1", "First");
    await state().renameWorkspace("w1", "Second");
    refuse("busy");
    await first;

    expect(main().name).toBe("Second");
    expect(state().toast).toBe("busy");
  });
});

describe("deleteWorkspace", () => {
  it("removes it and moves to the workspace the backend picks", async () => {
    await expect(state().deleteWorkspace("w1")).resolves.toBe(true);

    expect(state().workspaces.map((ws) => ws.id)).toEqual(["w2"]);
    expect(state().activeWorkspaceId).toBe("w2");
  });

  it("keeps it when the backend refuses", async () => {
    vi.mocked(api.deleteWorkspace).mockRejectedValueOnce("cannot delete the last workspace");

    await expect(state().deleteWorkspace("w1")).resolves.toBe(false);
    expect(state().workspaces.map((ws) => ws.id)).toEqual(["w1", "w2"]);
    expect(state().activeWorkspaceId).toBe("w1");
    expect(state().toast).toBe("cannot delete the last workspace");
  });
});

describe("createTerminal", () => {
  it("adds the terminal the backend created", async () => {
    vi.mocked(api.createTerminal).mockResolvedValueOnce(terminal("t3"));

    await expect(state().createTerminal()).resolves.toEqual(terminal("t3"));
    expect(main().terminals.map((t) => t.id)).toEqual(["t1", "t2", "t3"]);
  });

  it("resolves to null and adds nothing when the backend refuses", async () => {
    vi.mocked(api.createTerminal).mockRejectedValueOnce("pty unavailable");

    await expect(state().createTerminal()).resolves.toBeNull();
    expect(main().terminals.map((t) => t.id)).toEqual(["t1", "t2"]);
    expect(state().toast).toBe("pty unavailable");
  });
});

describe("commitTerminalRect", () => {
  it("applies the rect the backend saved", async () => {
    vi.mocked(api.updateTerminalRect).mockResolvedValueOnce({ x: 10, y: 20, width: 320, height: 200 });
    await state().commitTerminalRect("t1", { x: 10, y: 20, width: 100, height: 50 });

    expect(main().terminals[0]).toMatchObject({ x: 10, y: 20, width: 320, height: 200 });
  });
});

describe("terminal selection", () => {
  it("selecting terminals drops the selected link, and selecting a link drops them", () => {
    useAppStore.setState({ selectedLinkId: "l1" });
    state().selectTerminals(["t1", "t2"]);
    expect(state().selectedLinkId).toBeNull();

    state().selectLink("l1");
    expect(state().selectedTerminalIds).toEqual([]);
  });

  it("toggles one window in and out", () => {
    state().toggleTerminalSelection("t1");
    state().toggleTerminalSelection("t2");
    state().toggleTerminalSelection("t1");

    expect(state().selectedTerminalIds).toEqual(["t2"]);
  });

  it("closes every selected terminal and forgets them", async () => {
    state().selectTerminals(["t1", "t2"]);
    await state().closeTerminals(["t1", "t2"]);

    expect(api.closeTerminal).toHaveBeenCalledTimes(2);
    expect(main().terminals).toEqual([]);
    expect(state().selectedTerminalIds).toEqual([]);
  });

  it("keeps a window the backend refused to close selected", async () => {
    vi.mocked(api.closeTerminal).mockRejectedValueOnce("terminal t1 not found");
    state().selectTerminals(["t1", "t2"]);
    await state().closeTerminals(["t1", "t2"]);

    expect(main().terminals.map((t) => t.id)).toEqual(["t1"]);
    expect(state().selectedTerminalIds).toEqual(["t1"]);
  });
});

describe("duplicateTerminal", () => {
  beforeEach(() => {
    // 1000×800 on screen at zoom 1: the canvas area (0, 0)–(1000, 800).
    viewport.setBounds({ left: 0, top: 0, width: 1000, height: 800 });
    viewport.load("w1", { x: 0, y: 0, zoom: 1 });
  });

  it("opens the copy right beside the source, same size, when that spot is free and on screen", async () => {
    useAppStore.setState({ workspaces: [workspace("w1", "Main", [{ ...terminal("t1"), width: 400, height: 300 }])] });
    const copy = await state().duplicateTerminal("t1");

    expect(api.duplicateTerminal).toHaveBeenCalledWith("t1", { x: 424, y: 0, width: 400, height: 300 });
    expect(copy?.id).toBe("copy");
    expect(main().terminals.map((t) => t.id)).toEqual(["t1", "copy"]);
  });

  it("falls back to the center of the view, same size, when the spot beside is off screen", async () => {
    await state().duplicateTerminal("t1");

    // Right of t1 would end at 624 + 600 > 1000.
    expect(api.duplicateTerminal).toHaveBeenCalledWith("t1", { x: 200, y: 200, width: 600, height: 400 });
  });

  it("falls back to the center of the view when another window already sits beside it", async () => {
    const small = { ...terminal("t1"), width: 300, height: 200 };
    useAppStore.setState({ workspaces: [workspace("w1", "Main", [small, { ...small, id: "t2", x: 400 }])] });
    await state().duplicateTerminal("t1");

    expect(api.duplicateTerminal).toHaveBeenCalledWith("t1", { x: 350, y: 300, width: 300, height: 200 });
  });

  it("resolves to null and explains why when the backend refuses", async () => {
    vi.mocked(api.duplicateTerminal).mockRejectedValueOnce("terminal t1 not found");

    await expect(state().duplicateTerminal("t1")).resolves.toBeNull();
    expect(main().terminals.map((t) => t.id)).toEqual(["t1", "t2"]);
    expect(state().toast).toBe("terminal t1 not found");
  });
});
