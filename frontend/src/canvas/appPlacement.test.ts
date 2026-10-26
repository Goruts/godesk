// @vitest-environment jsdom
import { afterEach, describe, expect, it } from "vitest";
import type { Rect } from "../bindings/Rect";
import type { TerminalWindow } from "../bindings/TerminalWindow";
import type { Workspace } from "../bindings/Workspace";
import type { CanvasApp } from "../store/appStore";
import { collectPlacements } from "./appPlacement";
import { viewport } from "./viewport";

/** An element that reports `rect` as its client rectangle, as layout would. */
function element(html: string, rect: Rect, parent: Element = document.body): HTMLElement {
  const holder = document.createElement("div");
  holder.innerHTML = html;
  const el = holder.firstElementChild as HTMLElement;
  el.getBoundingClientRect = () => new DOMRect(rect.x, rect.y, rect.width, rect.height);
  parent.append(el);
  return el;
}

function app(id: string, workspaceId: string, z: number): CanvasApp {
  return { id, title: id, program: "notepad.exe", workspaceId, x: 0, y: 0, width: 800, height: 600, z };
}

function terminal(id: string, z: number): TerminalWindow {
  return { id, name: id, x: 0, y: 0, width: 600, height: 400, z };
}

function workspace(id: string, terminals: TerminalWindow[] = []): Workspace {
  return { id, name: id, camera: { x: 0, y: 0, zoom: 1 }, terminals, links: [], knowledgeBases: [] };
}

/** A card whose body sits at `body`. */
function card(id: string, body: Rect) {
  const el = element(`<div data-app-id="${id}"></div>`, { ...body, y: body.y - 30, height: body.height + 30 });
  element(`<div class="app-body"></div>`, body, el);
}

afterEach(() => {
  document.body.innerHTML = "";
  viewport.load("w1", { x: 0, y: 0, zoom: 1 });
});

describe("collectPlacements", () => {
  it("puts a window over its card's body, minus what is drawn in front of it", () => {
    card("a1", { x: 100, y: 100, width: 800, height: 600 });
    element(`<div data-terminal-id="front"></div>`, { x: 700, y: 50, width: 400, height: 300 });
    element(`<div data-terminal-id="behind"></div>`, { x: 0, y: 0, width: 300, height: 300 });
    element(`<div class="dock"></div>`, { x: 300, y: 650, width: 400, height: 60 });
    element(`<div class="toast"></div>`, { x: 2000, y: 2000, width: 100, height: 40 });

    const [placement] = collectPlacements({
      apps: [app("a1", "w1", 5)],
      workspaces: [workspace("w1", [terminal("front", 6), terminal("behind", 4)])],
      activeWorkspaceId: "w1",
    });

    expect(placement).toEqual({
      id: "a1",
      rect: { x: 100, y: 100, width: 800, height: 600 },
      visible: true,
      // The toast is elsewhere and the lower terminal is underneath.
      cutouts: [
        { x: 300, y: 650, width: 400, height: 60 },
        { x: 700, y: 50, width: 400, height: 300 },
      ],
    });
  });

  it("hides windows of other workspaces and windows too small to use", () => {
    card("tiny", { x: 0, y: 0, width: 120, height: 70 });
    card("away", { x: 0, y: 0, width: 800, height: 600 });

    const placements = collectPlacements({
      apps: [app("tiny", "w1", 1), app("away", "w2", 1)],
      workspaces: [workspace("w1"), workspace("w2")],
      activeWorkspaceId: "w1",
    });

    expect(placements.map((p) => [p.id, p.visible])).toEqual([
      ["tiny", false],
      ["away", false],
    ]);
  });

  it("hides a window zoomed out, where its content could not shrink with the canvas", () => {
    card("a1", { x: 0, y: 0, width: 800, height: 600 });
    const state = { apps: [app("a1", "w1", 1)], workspaces: [workspace("w1")], activeWorkspaceId: "w1" };
    viewport.load("w1", { x: 0, y: 0, zoom: 0.5 });
    expect(collectPlacements(state)[0]!.visible).toBe(false);
    viewport.load("w1", { x: 0, y: 0, zoom: 1 });
    expect(collectPlacements(state)[0]!.visible).toBe(true);
  });

  it("has nothing to do without programs", () => {
    expect(collectPlacements({ apps: [], workspaces: [workspace("w1")], activeWorkspaceId: "w1" })).toEqual([]);
  });
});
