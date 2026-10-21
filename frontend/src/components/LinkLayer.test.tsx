// @vitest-environment jsdom
import { Profiler, act } from "react";
import { type Root, createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Link } from "../bindings/Link";
import type { Rect } from "../bindings/Rect";
import type { TerminalWindow } from "../bindings/TerminalWindow";
import { linkGeometry } from "../canvas/geometry";
import { layout } from "../canvas/layout";
import { linkDraft } from "../canvas/linkDraft";
import { ACTIVE_LINK_MS, BOUNDS_MARGIN } from "../canvas/linkLayout";
import { LinkLayer } from "./LinkLayer";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const A: Rect = { x: 0, y: 0, width: 400, height: 300 };
const B: Rect = { x: 600, y: 0, width: 400, height: 300 };
const terminals: TerminalWindow[] = [
  { id: "a", name: "Alpha", ...A, z: 1 },
  { id: "b", name: "Beta", ...B, z: 2 },
];
const link = (overrides: Partial<Link> = {}): Link => ({
  id: "ab",
  sourceId: "a",
  targetId: "b",
  kind: "manual",
  lastActivityAt: 0,
  ...overrides,
});

// Animation frames run only when the test flushes them.
let frames: FrameRequestCallback[] = [];
const flushFrames = () => {
  const pending = frames;
  frames = [];
  for (const callback of pending) callback(0);
};

let container: HTMLDivElement;
let root: Root;
let commits = 0;
const onSelect = vi.fn();

function render(links: Link[], selectedId: string | null = null, visible = true) {
  act(() =>
    root.render(
      <Profiler id="links" onRender={() => commits++}>
        <LinkLayer links={links} terminals={terminals} visible={visible} selectedId={selectedId} onSelect={onSelect} />
      </Profiler>,
    ),
  );
}

const query = <T extends Element>(selector: string) => container.querySelector<T>(selector)!;

beforeEach(() => {
  vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => frames.push(callback));
  vi.stubGlobal("cancelAnimationFrame", () => undefined);
  layout.set("a", A);
  layout.set("b", B);
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
  commits = 0;
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
  layout.delete("a");
  layout.delete("b");
  linkDraft.clear();
  frames = [];
  onSelect.mockReset();
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

describe("LinkLayer", () => {
  it("draws each link between the closest edges of its terminals", () => {
    render([link()]);
    const { d } = linkGeometry(A, B);
    for (const part of ["glow", "line", "hit"]) expect(query(`.link-${part}`).getAttribute("d")).toBe(d);
    expect(query(".link-line").getAttribute("marker-end")).toBe("url(#link-arrow-manual)");
    expect(query(".link").classList).toContain("link-manual");
  });

  it("follows a moving window through the DOM, without a React render", () => {
    render([link()]);
    const before = commits;
    const moved = { ...B, x: 900, y: 400 };
    layout.set("b", moved);
    flushFrames();
    const curve = linkGeometry(A, moved);
    expect(query(".link-line").getAttribute("d")).toBe(curve.d);
    // The SVG box follows the curve, so its whole length stays hit-testable.
    const m = BOUNDS_MARGIN;
    const { x, y, width, height } = curve.bounds;
    expect(query("svg").getAttribute("viewBox")).toBe(`${x - m} ${y - m} ${width + 2 * m} ${height + 2 * m}`);
    expect(query<SVGSVGElement>("svg").style.left).toBe(`${x - m}px`);
    expect(commits).toBe(before);
  });

  it("hides a link whose terminal is not on the canvas", () => {
    render([link({ targetId: "gone" })]);
    expect(query<SVGGElement>(".link").style.display).toBe("none");
  });

  it("marks recent activity and dims the link once idle", () => {
    vi.useFakeTimers({ toFake: ["Date", "setInterval", "clearInterval"] });
    const now = Date.now();
    render([link({ lastActivityAt: now })]);
    expect(query<SVGGElement>(".link").dataset["activity"]).toBe("active");
    vi.setSystemTime(now + ACTIVE_LINK_MS);
    vi.advanceTimersByTime(500);
    expect(query<SVGGElement>(".link").dataset["activity"]).toBe("idle");
  });

  it("keeps the activity state when React re-renders the link", () => {
    render([link({ lastActivityAt: Date.now() })]);
    render([link({ lastActivityAt: Date.now() })], "ab");
    const group = query<SVGGElement>(".link");
    expect(group.classList).toContain("is-selected");
    expect(group.dataset["activity"]).toBe("active");
    expect(query(".link-line").getAttribute("marker-end")).toBe("url(#link-arrow-selected)");
  });

  it("labels the hovered link source → target and selects it on click", () => {
    render([link({ kind: "detected" })]);
    const hit = query(".link-hit");
    act(() => {
      hit.dispatchEvent(new MouseEvent("pointermove", { bubbles: true, clientX: 50, clientY: 60 }));
    });
    const tooltip = document.querySelector(".link-tooltip")!;
    expect(tooltip.textContent).toContain("Alpha→Beta");
    expect(tooltip.textContent).toContain("detected");
    act(() => {
      hit.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    expect(onSelect).toHaveBeenCalledWith("ab");
    act(() => {
      hit.dispatchEvent(new MouseEvent("pointerout", { bubbles: true }));
    });
    expect(document.querySelector(".link-tooltip")).toBeNull();
  });

  it("previews the link being drawn, snapped to the target under the pointer", () => {
    render([]);
    const draft = query<SVGPathElement>(".link-draft");
    expect(draft.style.display).toBe("none");
    linkDraft.update("a", { x: 500, y: 500 }, null);
    flushFrames();
    expect(draft.style.display).toBe("");
    expect(draft.getAttribute("d")).toBe("M 200 300 L 500 500");
    linkDraft.update("a", { x: 700, y: 100 }, "b");
    flushFrames();
    expect(draft.getAttribute("d")).toBe(linkGeometry(A, B).d);
    linkDraft.clear();
    flushFrames();
    expect(draft.style.display).toBe("none");
  });

  it("hides every link and its label when toggled off", () => {
    render([link()], null, false);
    expect(query("svg").classList).toContain("links-hidden");
    act(() => {
      query(".link-hit").dispatchEvent(new MouseEvent("pointermove", { bubbles: true }));
    });
    expect(document.querySelector(".link-tooltip")).toBeNull();
  });
});
