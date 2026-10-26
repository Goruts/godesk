// @vitest-environment jsdom
import { act } from "react";
import { type Root, createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { TerminalWindow } from "../bindings/TerminalWindow";
import type { Workspace } from "../bindings/Workspace";
import { api } from "../lib/api";
import { useAppStore } from "../store/appStore";
import { useDialogStore } from "../store/dialogStore";
import { ConfirmDialog } from "./ConfirmDialog";
import { WorkspaceMenu } from "./WorkspaceMenu";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

vi.mock("../lib/api", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../lib/api")>()),
  api: {
    createWorkspace: vi.fn(),
    renameWorkspace: vi.fn(async () => null),
    deleteWorkspace: vi.fn(),
    setActiveWorkspace: vi.fn(async () => null),
    closeTerminal: vi.fn(async () => null),
  },
}));

function terminal(id: string): TerminalWindow {
  return { id, name: id, x: 0, y: 0, width: 600, height: 400, z: 1 };
}

function workspace(id: string, name: string, terminals: TerminalWindow[] = []): Workspace {
  return { id, name, camera: { x: 0, y: 0, zoom: 1 }, terminals, links: [], knowledgeBases: [] };
}

let container: HTMLDivElement;
let root: Root;

const query = <T extends Element>(selector: string) => container.querySelector<T>(selector);
const items = () => [...container.querySelectorAll<HTMLButtonElement>(".ws-select")];
const item = (name: string) => items().find((b) => b.querySelector(".ws-name")?.textContent === name)!;
const nameInput = () => query<HTMLInputElement>(".ws-input");
const names = () => useAppStore.getState().workspaces.map((ws) => ws.name);
const button = (label: string) => query<HTMLButtonElement>(`[aria-label="${label}"]`)!;
const dialog = () => useDialogStore.getState().pending;
const menuButton = () => query<HTMLButtonElement>('[aria-controls="workspace-panel"]')!;
const panel = () => query("#workspace-panel");

function press(el: Element, key: string) {
  act(() => el.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true })));
}

function startRename(name: string) {
  act(() => button(`Rename ${name}`).click());
  return nameInput()!;
}

beforeEach(() => {
  useAppStore.setState({
    ready: true,
    toast: null,
    activeWorkspaceId: "w1",
    workspaces: [workspace("w1", "Main", [terminal("t1"), terminal("t2")]), workspace("w2", "Agents")],
  });
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
  act(() =>
    root.render(
      <>
        <WorkspaceMenu />
        <ConfirmDialog />
      </>,
    ),
  );
  act(() => menuButton().click());
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
  useDialogStore.setState({ pending: null });
  vi.clearAllMocks();
});

describe("WorkspaceMenu panel", () => {
  it("names the workspace on the canvas", () => {
    expect(menuButton().getAttribute("aria-label")).toBe("Workspace: Main");
    expect(menuButton().getAttribute("aria-expanded")).toBe("true");
  });

  it("Escape closes it and returns focus to the dock button", () => {
    press(item("Main"), "Escape");
    expect(panel()).toBeNull();
    expect(document.activeElement).toBe(menuButton());
  });

  it("closes on a press outside, but not on one inside", () => {
    act(() => item("Agents").dispatchEvent(new Event("pointerdown", { bubbles: true })));
    expect(panel()).not.toBeNull();

    act(() => document.body.dispatchEvent(new Event("pointerdown", { bubbles: true })));
    expect(panel()).toBeNull();
  });
});

describe("WorkspaceMenu list", () => {
  it("lists every workspace with its terminal count and marks the active one", () => {
    expect(items().map((b) => b.getAttribute("aria-label"))).toEqual(["Main, 2 terminals", "Agents, 0 terminals"]);
    expect(item("Main").getAttribute("aria-current")).toBe("true");
    expect(item("Agents").hasAttribute("aria-current")).toBe(false);
  });

  it("switches workspace on click without closing any terminal, then closes", async () => {
    await act(async () => item("Agents").click());

    expect(useAppStore.getState().activeWorkspaceId).toBe("w2");
    expect(api.setActiveWorkspace).toHaveBeenCalledWith("w2");
    expect(api.closeTerminal).not.toHaveBeenCalled();
    expect(panel()).toBeNull();
    expect(menuButton().getAttribute("aria-label")).toBe("Workspace: Agents");
    expect(document.activeElement).toBe(menuButton());
  });
});

describe("WorkspaceMenu create", () => {
  it("creates a workspace, shows it and opens its name for editing", async () => {
    vi.mocked(api.createWorkspace).mockResolvedValue(workspace("w3", "Workspace 1"));
    await act(async () => query<HTMLButtonElement>(".ws-new")!.click());

    expect(useAppStore.getState().activeWorkspaceId).toBe("w3");
    const input = nameInput()!;
    expect(input.value).toBe("Workspace 1");
    expect(document.activeElement).toBe(input);

    input.value = "Review";
    await act(async () => press(input, "Enter"));
    expect(api.renameWorkspace).toHaveBeenCalledWith("w3", "Review");
    expect(names()).toEqual(["Main", "Agents", "Review"]);
  });

  it("a double click creates only one workspace", async () => {
    vi.mocked(api.createWorkspace).mockResolvedValue(workspace("w3", "Workspace 1"));
    const newButton = query<HTMLButtonElement>(".ws-new")!;
    await act(async () => {
      newButton.click();
      newButton.click();
    });

    expect(api.createWorkspace).toHaveBeenCalledTimes(1);
  });

  it("is disabled until the state is loaded", () => {
    act(() => useAppStore.setState({ ready: false }));
    expect(query<HTMLButtonElement>(".ws-new")!.disabled).toBe(true);
  });
});

describe("WorkspaceMenu rename", () => {
  it("the pencil opens it; Enter saves the trimmed name and returns focus to the item", async () => {
    const input = startRename("Main");
    expect(input.value).toBe("Main");
    expect(document.activeElement).toBe(input);

    input.value = "  Planning  ";
    await act(async () => press(input, "Enter"));

    expect(api.renameWorkspace).toHaveBeenCalledTimes(1);
    expect(api.renameWorkspace).toHaveBeenCalledWith("w1", "Planning");
    expect(names()).toEqual(["Planning", "Agents"]);
    expect(nameInput()).toBeNull();
    expect(document.activeElement).toBe(item("Planning"));
  });

  it("F2 on an item opens it too", () => {
    press(item("Agents"), "F2");
    expect(nameInput()?.value).toBe("Agents");
  });

  it("Escape keeps the old name, even if the field blurs while it closes, and keeps the panel open", () => {
    const input = startRename("Main");
    input.value = "Discarded";
    // Some engines blur a focused field as it is removed: before React unmounts it.
    act(() => {
      input.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
      input.dispatchEvent(new FocusEvent("focusout", { bubbles: true }));
    });

    expect(api.renameWorkspace).not.toHaveBeenCalled();
    expect(names()[0]).toBe("Main");
    expect(document.activeElement).toBe(item("Main"));
    expect(panel()).not.toBeNull();
  });

  it("saves when focus leaves the field", async () => {
    const input = startRename("Agents");
    input.value = "Bots";
    await act(async () => input.blur());

    expect(api.renameWorkspace).toHaveBeenCalledWith("w2", "Bots");
  });

  it("ignores an empty or unchanged name", async () => {
    let input = startRename("Main");
    input.value = "   ";
    await act(async () => press(input, "Enter"));
    input = startRename("Main");
    await act(async () => press(input, "Enter"));

    expect(api.renameWorkspace).not.toHaveBeenCalled();
    expect(names()[0]).toBe("Main");
  });

  it("puts the old name back and explains why when the backend refuses it", async () => {
    vi.mocked(api.renameWorkspace).mockRejectedValueOnce("workspace not found");
    const input = startRename("Main");
    input.value = "Gone";
    await act(async () => press(input, "Enter"));

    expect(names()[0]).toBe("Main");
    expect(useAppStore.getState().toast).toBe("workspace not found");
  });
});

describe("WorkspaceMenu delete", () => {
  it("asks first, focusing Cancel; cancelling keeps the workspace and its terminals", async () => {
    await act(async () => button("Delete Main").click());

    expect(dialog()?.title).toBe("Delete “Main”?");
    expect(dialog()?.message).toContain("2 terminals will be closed");
    expect(document.activeElement?.textContent).toBe("Cancel");

    await act(async () => (document.activeElement as HTMLButtonElement).click());
    expect(api.deleteWorkspace).not.toHaveBeenCalled();
    expect(names()).toEqual(["Main", "Agents"]);
    expect(document.activeElement).toBe(button("Delete Main"));
  });

  it("deletes after confirming and moves to the workspace the backend picks", async () => {
    vi.mocked(api.deleteWorkspace).mockResolvedValue("w2");
    await act(async () => button("Delete Main").click());
    await act(async () => query<HTMLButtonElement>(".dialog .btn-danger")!.click());

    expect(api.deleteWorkspace).toHaveBeenCalledWith("w1");
    expect(names()).toEqual(["Agents"]);
    expect(useAppStore.getState().activeWorkspaceId).toBe("w2");
    expect(document.activeElement).toBe(item("Agents"));
  });

  it("a press on the confirmation does not close the panel behind it", async () => {
    await act(async () => button("Delete Main").click());
    act(() => query(".dialog")!.dispatchEvent(new Event("pointerdown", { bubbles: true })));
    press(document.activeElement!, "Escape");

    expect(dialog()).toBeNull();
    expect(panel()).not.toBeNull();
  });

  it("Tab stays inside the confirmation", async () => {
    await act(async () => button("Delete Main").click());
    press(document.activeElement!, "Tab");
    expect(document.activeElement?.textContent).toBe("Delete workspace");
    press(document.activeElement!, "Tab");
    expect(document.activeElement?.textContent).toBe("Cancel");
  });

  it("cannot delete the last workspace", () => {
    act(() => useAppStore.setState({ workspaces: [workspace("w1", "Main")] }));
    expect(button("Delete Main").disabled).toBe(true);
  });
});
