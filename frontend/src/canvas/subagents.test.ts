import { describe, expect, it } from "vitest";
import type { Subagent } from "../bindings/Subagent";
import type { TerminalWindow } from "../bindings/TerminalWindow";
import { FINISHED_SUBAGENT_MS, formatTokens, subagentCards, subagentLinks } from "./subagents";

const NOW = 10_000_000;

function sub(id: string, finished = false, updatedAt = NOW): Subagent {
  return { id, kind: "Explore", description: id, finished, tokens: { input: 0, output: 0 }, lines: [], updatedAt };
}

const parent: TerminalWindow = { id: "t1", name: "planner", x: 100, y: 50, width: 600, height: 400, z: 1 };

describe("subagent cards", () => {
  it("stacks running and recently finished subagents beside their agent", () => {
    const agents = [
      {
        terminalId: "t1",
        agent: "claude",
        tokens: { input: 0, output: 0 },
        subagents: [sub("old", true, NOW - FINISHED_SUBAGENT_MS - 1), sub("a"), sub("b", true), sub("gone")],
      },
    ];
    const cards = subagentCards(agents, [parent], ["gone"], { b: { x: 0, y: 0, width: 300, height: 200 } }, NOW);
    expect(cards.map((c) => c.id)).toEqual(["a", "b"]);
    expect(cards[0]!.rect).toEqual({ x: 748, y: 50, width: 440, height: 280 });
    expect(cards[1]!.rect).toEqual({ x: 0, y: 0, width: 300, height: 200 });

    const links = subagentLinks(cards);
    expect(links[0]).toMatchObject({ sourceId: "t1", targetId: "sub:a", lastActivityAt: NOW });
    expect(links[1]!.lastActivityAt).toBe(0);
  });

  it("shows nothing for an agent whose terminal is in another workspace", () => {
    const agents = [{ terminalId: "elsewhere", agent: "claude", tokens: { input: 0, output: 0 }, subagents: [sub("a")] }];
    expect(subagentCards(agents, [parent], [], {}, NOW)).toEqual([]);
  });
});

describe("formatTokens", () => {
  it("abbreviates", () => {
    expect([950, 1234, 12_345, 4_560_000, 45_600_000].map(formatTokens)).toEqual(["950", "1.23k", "12.3k", "4.56M", "45.6M"]);
  });
});
