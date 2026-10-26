import type { Link } from "../bindings/Link";
import type { Rect } from "../bindings/Rect";
import type { Subagent } from "../bindings/Subagent";
import type { TerminalAgent } from "../bindings/TerminalAgent";
import type { TerminalWindow } from "../bindings/TerminalWindow";

/** A finished subagent stays on the canvas this long, then goes. */
export const FINISHED_SUBAGENT_MS = 10 * 60_000;
/** At most this many subagents of one agent are shown, the latest. */
export const MAX_SUBAGENTS_SHOWN = 6;
export const SUBAGENT_WIDTH = 440;
export const SUBAGENT_HEIGHT = 280;
const SUBAGENT_GAP = 16;
const SUBAGENT_OFFSET = 48;

/** Canvas id of a subagent's card (its layout and link endpoint). */
export const subagentCardId = (subagentId: string) => `sub:${subagentId}`;
const SUBAGENT_LINK_PREFIX = "sub-link:";
export const isSubagentLink = (linkId: string) => linkId.startsWith(SUBAGENT_LINK_PREFIX);

/** A subagent shown on the canvas, beside the terminal of the agent that started it. */
export type SubagentCard = Subagent & { terminalId: string; rect: Rect };

/**
 * The subagents to show for one workspace: running ones, and finished ones
 * for a while, not dismissed, stacked to the right of their parent terminal
 * unless moved.
 */
export function subagentCards(
  agents: TerminalAgent[],
  terminals: TerminalWindow[],
  dismissed: string[],
  moved: Record<string, Rect>,
  now: number,
): SubagentCard[] {
  const cards: SubagentCard[] = [];
  for (const agent of agents) {
    const parent = terminals.find((t) => t.id === agent.terminalId);
    if (!parent) continue;
    const shown = agent.subagents
      .filter((s) => !dismissed.includes(s.id) && (!s.finished || now - s.updatedAt < FINISHED_SUBAGENT_MS))
      .slice(-MAX_SUBAGENTS_SHOWN);
    shown.forEach((sub, index) => {
      const stacked = {
        x: parent.x + parent.width + SUBAGENT_OFFSET,
        y: parent.y + index * (SUBAGENT_HEIGHT + SUBAGENT_GAP),
        width: SUBAGENT_WIDTH,
        height: SUBAGENT_HEIGHT,
      };
      cards.push({ ...sub, terminalId: agent.terminalId, rect: moved[sub.id] ?? stacked });
    });
  }
  return cards;
}

/** Links from each agent to its subagents, lit while a subagent works. */
export function subagentLinks(cards: SubagentCard[]): Link[] {
  return cards.map((card) => ({
    id: `${SUBAGENT_LINK_PREFIX}${card.id}`,
    sourceId: card.terminalId,
    targetId: subagentCardId(card.id),
    kind: "detected",
    lastActivityAt: card.finished ? 0 : card.updatedAt,
  }));
}

/** 950 → "950", 12_300 → "12.3k", 4_560_000 → "4.56M". */
export function formatTokens(count: number): string {
  if (count < 1000) return String(count);
  if (count < 1_000_000) return `${Number((count / 1000).toFixed(count < 10_000 ? 2 : 1))}k`;
  return `${Number((count / 1_000_000).toFixed(count < 10_000_000 ? 2 : 1))}M`;
}
