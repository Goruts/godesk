import { describe, expect, it } from "vitest";
import { viewCommand } from "./AgentTerminal";

describe("viewCommand", () => {
  it("goes back to the shell with /exit, /quit or /shell", () => {
    for (const text of ["/exit", " /quit ", "/SHELL"]) expect(viewCommand(text)).toEqual({ kind: "shell" });
  });

  it("changes the model with /model, or goes back to the default", () => {
    expect(viewCommand("/model gpt-6-luna")).toEqual({ kind: "model", model: "gpt-6-luna" });
    expect(viewCommand("/model")).toEqual({ kind: "model", model: null });
  });

  it("recognizes the no-argument view commands and their aliases", () => {
    expect(viewCommand("/usage")).toEqual({ kind: "usage" });
    expect(viewCommand("/tokens")).toEqual({ kind: "usage" });
    expect(viewCommand("/status")).toEqual({ kind: "status" });
    expect(viewCommand("/stop")).toEqual({ kind: "stop" });
    expect(viewCommand("/restart")).toEqual({ kind: "restart" });
    expect(viewCommand("/classic")).toEqual({ kind: "classic" });
    expect(viewCommand(" /HELP ")).toEqual({ kind: "help" });
  });

  it("leaves everything else to the agent", () => {
    // Unknown commands, extra arguments, and a real agent command all pass through.
    for (const text of ["exit", "/exit now", "/model a b", "/usage now", "/review", "/clear", "explain /exit"])
      expect(viewCommand(text)).toBeNull();
  });
});
