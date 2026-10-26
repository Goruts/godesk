// @vitest-environment jsdom
import { act } from "react";
import { type Root, createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { AgentItem, ToolItem } from "../agent/protocol";
import { AgentEntry } from "./AgentEntry";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let host: HTMLDivElement;
let root: Root;

beforeEach(() => {
  host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
});

afterEach(() => {
  act(() => root.unmount());
  host.remove();
});

function show(item: AgentItem) {
  act(() => root.render(<AgentEntry item={item} agent="claude" cwd={"C:\\work"} />));
}

function tool(patch: Partial<ToolItem>): ToolItem {
  return {
    kind: "tool",
    id: "t1",
    name: "Edit",
    category: "edit",
    summary: "C:\\work\\src\\a.rs",
    status: "done",
    output: "",
    exitCode: null,
    diff: null,
    files: null,
    todos: null,
    ...patch,
  };
}

describe("what the agent did", () => {
  it("shows an edit's code open, as the lines it takes out and puts in", () => {
    show(
      tool({
        diff: [
          { kind: "del", text: "let a = 1;" },
          { kind: "add", text: "let a = 2;" },
        ],
      }),
    );
    expect(host.querySelector(".agent-tool-verb")?.textContent).toBe("Edited");
    expect(host.querySelector(".agent-tool-summary")?.textContent).toBe("src/a.rs");
    expect(host.querySelector(".agent-tool-delta")?.textContent).toBe("+1−1");
    const lines = [...host.querySelectorAll(".agent-diff-line")].map((l) => l.getAttribute("data-kind"));
    expect(lines).toEqual(["del", "add"]);
  });

  it("shows a command's output once the card is opened", () => {
    show(tool({ name: "Bash", category: "shell", summary: "npm test", output: "12 passed" }));
    expect(host.querySelector(".agent-tool-verb")?.textContent).toBe("Ran");
    expect(host.querySelector(".agent-console")).toBeNull();
    act(() => host.querySelector<HTMLButtonElement>(".agent-tool-head")?.click());
    expect(host.querySelector(".agent-console-command")?.textContent).toContain("npm test");
    expect(host.querySelector(".agent-console-output")?.textContent).toBe("12 passed");
  });

  it("shows a running tool as running", () => {
    show(tool({ name: "Bash", category: "shell", summary: "cargo build", status: "running" }));
    expect(host.querySelector(".agent-tool-verb")?.textContent).toBe("Running");
    expect(host.querySelector(".agent-spinner")).not.toBeNull();
  });

  it("shows the reasoning while it is written, and folds it once done", () => {
    show({ kind: "thinking", id: "k", text: "Primero miro el test que falla.", live: true });
    expect(host.querySelector(".agent-thinking-head")?.textContent).toContain("Thinking…");
    expect(host.querySelector(".agent-thinking-text")?.textContent).toBe("Primero miro el test que falla.");

    show({ kind: "thinking", id: "k", text: "Primero miro el test que falla.", live: false });
    expect(host.querySelector(".agent-thinking-head")?.textContent).toContain("Thought");
    expect(host.querySelector(".agent-thinking-text")).toBeNull();
  });

  it("shows the plan with what is done", () => {
    show(
      tool({
        name: "TodoWrite",
        category: "todo",
        todos: [
          { text: "Leer el código", status: "completed" },
          { text: "Arreglar el bug", status: "in_progress" },
        ],
      }),
    );
    const todos = [...host.querySelectorAll(".agent-todo")].map((t) => t.getAttribute("data-status"));
    expect(todos).toEqual(["completed", "in_progress"]);
    expect(host.querySelector(".agent-tool-meta")?.textContent).toContain("1/2");
  });
});
