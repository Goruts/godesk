// @vitest-environment jsdom
import { act } from "react";
import { type Root, createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { AgentTerminal } from "./AgentTerminal";

// What the agent's stream handler receives, so a test can push lines into it.
const lineHandlers: Array<(line: string, seq: number) => void> = [];
let historyText = "";
let historyLines = 0;

vi.mock("./TerminalView", () => ({ agentInputs: new Map(), takePendingFocus: () => false }));

vi.mock("../lib/api", async (importOriginal) => {
  const original = await importOriginal<typeof import("../lib/api")>();
  // Every call the view makes resolves to nothing, except the ones these tests read.
  const api = new Proxy(
    {
      agentOutput: async () => ({ text: historyText, lines: historyLines }),
      agentFolder: async () => null,
    } as Record<string, unknown>,
    { get: (target, key: string) => target[key] ?? (async () => null) },
  );
  const events = new Proxy({} as Record<string, unknown>, {
    get: (_, key: string) =>
      key === "onAgentLine"
        ? async (_id: string, handler: (line: string, seq: number) => void) => {
            lineHandlers.push(handler);
            return () => {};
          }
        : async () => () => {},
  });
  return { ...original, api, events };
});

/** A conversation of `count` prompts, as the agent's history file holds it. */
const prompts = (count: number, from = 0) =>
  Array.from({ length: count }, (_, i) => JSON.stringify({ type: "godesk.prompt", text: `message ${from + i}`, at: 1 }));

const frame = () => new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));

let container: HTMLDivElement;
let root: Root;
const scrolled: Element[] = [];

beforeEach(() => {
  lineHandlers.length = 0;
  scrolled.length = 0;
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  // jsdom has no layout: record which element a jump scrolls into view.
  Element.prototype.scrollIntoView = function (this: Element) {
    scrolled.push(this);
  };
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
});

async function open(history: string[]) {
  historyText = history.join("\n");
  historyLines = history.length;
  await act(async () => {
    root.render(<AgentTerminal terminalId="t1" agent="claude" />);
  });
  // Let the history load and render.
  await act(async () => {
    await frame();
  });
}

const drawn = () => container.querySelectorAll("[data-item]").length;

describe("AgentTerminal with a long conversation", () => {
  it("draws only the newest items and loads earlier ones on demand", async () => {
    await open(prompts(200));
    expect(drawn()).toBe(150);
    const earlier = container.querySelector<HTMLButtonElement>(".agent-earlier");
    expect(earlier?.textContent).toContain("50 hidden");
    await act(async () => earlier?.click());
    expect(drawn()).toBe(200);
    expect(container.querySelector(".agent-earlier")).toBeNull();
  });

  it("applies streamed lines once per frame, not once per line", async () => {
    await open(prompts(3));
    expect(drawn()).toBe(3);
    const handler = lineHandlers[0];
    if (!handler) throw new Error("the view did not listen to the agent");
    act(() => {
      prompts(20, 3).forEach((line, i) => handler(line, 10 + i));
    });
    // Nothing is drawn until the frame comes; then every line at once.
    expect(drawn()).toBe(3);
    await act(async () => {
      await frame();
    });
    expect(drawn()).toBe(23);
  });

  it("jumps to a message from the map, drawing it first when it is not yet", async () => {
    await open(prompts(200));
    expect(container.querySelector('[data-item="3"]')).toBeNull();
    const marks = container.querySelectorAll<HTMLButtonElement>(".agent-rail-mark");
    expect(marks.length).toBe(200);
    await act(async () => marks[3]?.click());
    const target = container.querySelector('[data-item="3"]');
    expect(target).not.toBeNull();
    expect(scrolled).toContain(target);
  });

  it("goes to the start with Alt+Home", async () => {
    await open(prompts(200));
    const view = container.querySelector(".agent-view") as HTMLElement;
    await act(async () => {
      view.dispatchEvent(new KeyboardEvent("keydown", { key: "Home", altKey: true, bubbles: true }));
    });
    expect(container.querySelector('[data-item="0"]')).not.toBeNull();
    expect(drawn()).toBe(200);
  });
});
