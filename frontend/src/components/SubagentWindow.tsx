import { memo, useLayoutEffect, useRef } from "react";
import type { PointerEvent as ReactPointerEvent } from "react";
import type { Rect } from "../bindings/Rect";
import { dragOffset } from "../canvas/camera";
import { trackDrag } from "../canvas/interaction";
import { layout } from "../canvas/layout";
import { type SubagentCard, formatTokens, subagentCardId } from "../canvas/subagents";
import { viewport } from "../canvas/viewport";
import { useAppStore } from "../store/appStore";

/** Lines this close to the bottom count as following the end. */
const FOLLOW_SLACK_PX = 24;

type Props = { card: SubagentCard };

/**
 * A subagent's transcript, read-only, beside the agent that started it.
 * Subagents run inside their agent's process and have no terminal: this is
 * what they say and the tools they call, as their agent records it.
 */
export const SubagentWindow = memo(function SubagentWindow({ card }: Props) {
  const ref = useRef<HTMLDivElement>(null);
  const bodyRef = useRef<HTMLDivElement>(null);
  const following = useRef(true);
  const moveSubagent = useAppStore((s) => s.moveSubagent);
  const dismissSubagent = useAppStore((s) => s.dismissSubagent);
  const layoutId = subagentCardId(card.id);
  const { rect } = card;

  useLayoutEffect(() => {
    layout.set(layoutId, rect);
  }, [layoutId, rect.x, rect.y, rect.width, rect.height]);
  useLayoutEffect(() => () => layout.delete(layoutId), [layoutId]);

  // Keep the newest line in view, unless the reader scrolled up.
  useLayoutEffect(() => {
    const body = bodyRef.current;
    if (body && following.current) body.scrollTop = body.scrollHeight;
  }, [card.lines]);

  const onScroll = () => {
    const body = bodyRef.current;
    if (body) following.current = body.scrollHeight - body.scrollTop - body.clientHeight < FOLLOW_SLACK_PX;
  };

  const startMove = (e: ReactPointerEvent) => {
    const el = ref.current;
    if (!el || e.button !== 0 || (e.target as HTMLElement).closest("button")) return;
    e.preventDefault();
    e.stopPropagation();
    const start: Rect = rect;
    let current = start;
    const origin = viewport.clientToWorld(e.clientX, e.clientY);
    trackDrag(
      e,
      "grabbing",
      (_dx, _dy, ev) => {
        const d = dragOffset(viewport.get(), origin, viewport.clientToScreen(ev.clientX, ev.clientY));
        current = { ...start, x: start.x + d.x, y: start.y + d.y };
        el.style.left = `${current.x}px`;
        el.style.top = `${current.y}px`;
        layout.set(layoutId, current);
      },
      () => {
        if (current !== start) moveSubagent(card.id, current);
      },
    );
  };

  const total = card.tokens.input + card.tokens.output;
  return (
    <div
      ref={ref}
      className="term-window sub-window"
      role="group"
      aria-label={`Subagent ${card.kind}: ${card.description}`}
      data-subagent-id={card.id}
      data-finished={card.finished || undefined}
      style={{ left: rect.x, top: rect.y, width: rect.width, height: rect.height }}
    >
      <div className="term-titlebar" onPointerDown={startMove}>
        <span className="app-program">{card.kind}</span>
        <span className="term-title" title={card.description}>
          {card.description || "Subagent"}
        </span>
        <span className="sub-status">{card.finished ? "done" : "working"}</span>
        <span className="sub-tokens" title={`${card.tokens.input} read · ${card.tokens.output} written`}>
          {formatTokens(total)}
        </span>
        <button
          type="button"
          className="term-close"
          aria-label={`Hide subagent ${card.description}`}
          title="Hide (the subagent keeps working)"
          onClick={() => dismissSubagent(card.id)}
        >
          ×
        </button>
      </div>
      <div ref={bodyRef} className="term-body sub-body" onScroll={onScroll}>
        {card.lines.length === 0 ? (
          <p className="sub-line sub-empty">Starting…</p>
        ) : (
          card.lines.map((line, i) => (
            <p key={i} className={line.startsWith("▸") ? "sub-line sub-tool" : "sub-line"}>
              {line}
            </p>
          ))
        )}
      </div>
    </div>
  );
});
