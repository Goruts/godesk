import { useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import type { Link } from "../bindings/Link";
import type { TerminalWindow } from "../bindings/TerminalWindow";
import { layout } from "../canvas/layout";
import { linkDraft } from "../canvas/linkDraft";
import { isLinkActive, planLinks } from "../canvas/linkLayout";

const ACTIVITY_REFRESH_MS = 500;
/** Distance (px) of the hover label from the pointer and the window edges. */
const TOOLTIP_GAP = 12;

type Elements = { group: SVGGElement; glow: SVGPathElement; line: SVGPathElement; hit: SVGPathElement };

type Props = {
  links: Link[];
  terminals: TerminalWindow[];
  visible: boolean;
  selectedId: string | null;
  onSelect: (id: string) => void;
};

/**
 * SVG layer inside the transformed world, below every window. React renders
 * one group per link; path data, the SVG box and activity state are written
 * directly to the DOM, so dragging, resizing, panning and zooming never
 * re-render it. Activity lives in `data-activity`, an attribute React does
 * not own, so re-renders (selection, hover) cannot reset it.
 */
export function LinkLayer({ links, terminals, visible, selectedId, onSelect }: Props) {
  const svgRef = useRef<SVGSVGElement>(null);
  const draftRef = useRef<SVGPathElement>(null);
  const elements = useRef(new Map<string, Elements>());
  const linksRef = useRef(links);
  linksRef.current = links;
  const [hoverId, setHoverId] = useState<string | null>(null);
  const tooltipRef = useRef<HTMLDivElement>(null);
  const pointerRef = useRef({ x: 0, y: 0 });

  /** Follows the pointer, flipping to its other side near the window edges. */
  const placeTooltip = () => {
    const tooltip = tooltipRef.current;
    if (!tooltip) return;
    const { x, y } = pointerRef.current;
    const width = tooltip.offsetWidth;
    const height = tooltip.offsetHeight;
    const left = x + TOOLTIP_GAP + width > window.innerWidth - TOOLTIP_GAP ? x - TOOLTIP_GAP - width : x + TOOLTIP_GAP;
    const top = y + TOOLTIP_GAP + height > window.innerHeight - TOOLTIP_GAP ? y - TOOLTIP_GAP - height : y + TOOLTIP_GAP;
    tooltip.style.left = `${Math.max(0, left)}px`;
    tooltip.style.top = `${Math.max(0, top)}px`;
  };
  useLayoutEffect(placeTooltip, [hoverId]);

  // Geometry → DOM on layout or draft changes, at most once per frame.
  useLayoutEffect(() => {
    let frame = 0;
    const redraw = () => {
      frame = 0;
      const plan = planLinks(linksRef.current, (id) => layout.get(id), linkDraft.get());
      for (const [id, geometry] of plan.links) {
        const els = elements.current.get(id);
        if (!els) continue;
        els.group.style.display = geometry ? "" : "none";
        if (!geometry) continue;
        for (const path of [els.glow, els.line, els.hit]) path.setAttribute("d", geometry.d);
      }
      const draftPath = draftRef.current;
      if (draftPath) {
        draftPath.style.display = plan.draft ? "" : "none";
        if (plan.draft) draftPath.setAttribute("d", plan.draft.d);
      }
      // Fit the SVG box to what it draws: engines differ on hit-testing
      // shapes that overflow it.
      const svg = svgRef.current;
      if (svg && plan.bounds) {
        const { x, y, width, height } = plan.bounds;
        svg.setAttribute("viewBox", `${x} ${y} ${width} ${height}`);
        svg.style.left = `${x}px`;
        svg.style.top = `${y}px`;
        svg.style.width = `${width}px`;
        svg.style.height = `${height}px`;
      }
    };
    const schedule = () => {
      if (!frame) frame = requestAnimationFrame(redraw);
    };
    redraw();
    const unsubscribeLayout = layout.subscribe(schedule);
    const unsubscribeDraft = linkDraft.subscribe(schedule);
    return () => {
      unsubscribeLayout();
      unsubscribeDraft();
      cancelAnimationFrame(frame);
    };
  }, [links]);

  // Recent activity animates and highlights a link; it dims once idle.
  useLayoutEffect(() => {
    const refresh = () => {
      const now = Date.now();
      for (const link of linksRef.current) {
        const group = elements.current.get(link.id)?.group;
        const activity = isLinkActive(link, now) ? "active" : "idle";
        if (group && group.dataset["activity"] !== activity) group.dataset["activity"] = activity;
      }
    };
    refresh();
    const timer = window.setInterval(refresh, ACTIVITY_REFRESH_MS);
    return () => window.clearInterval(timer);
  }, [links]);

  const names = new Map(terminals.map((t) => [t.id, t.name]));
  const hovered = hoverId === null ? undefined : links.find((l) => l.id === hoverId);

  return (
    <>
      <svg ref={svgRef} className={`link-layer${visible ? "" : " links-hidden"}`} aria-hidden="true">
        <defs>
          {(["manual", "detected", "selected"] as const).map((kind) => (
            <marker
              key={kind}
              id={`link-arrow-${kind}`}
              className={`link-arrow link-arrow-${kind}`}
              viewBox="0 0 10 10"
              refX="10"
              refY="5"
              markerWidth="5"
              markerHeight="5"
              markerUnits="strokeWidth"
              orient="auto"
            >
              <path d="M 0 0 L 10 5 L 0 10 z" />
            </marker>
          ))}
        </defs>
        {links.map((link) => {
          const selected = link.id === selectedId;
          return (
            <g
              key={link.id}
              className={`link link-${link.kind}${selected ? " is-selected" : ""}`}
              ref={(group) => {
                if (!group) {
                  elements.current.delete(link.id);
                  return;
                }
                const path = (part: string) => group.querySelector<SVGPathElement>(`.link-${part}`)!;
                elements.current.set(link.id, { group, glow: path("glow"), line: path("line"), hit: path("hit") });
              }}
            >
              <path className="link-glow" />
              <path className="link-line" markerEnd={`url(#link-arrow-${selected ? "selected" : link.kind})`} />
              <path
                className="link-hit"
                onPointerDown={(e) => {
                  e.stopPropagation();
                  // Leave the terminal so Delete reaches the link, not the shell.
                  if (e.button === 0 && document.activeElement instanceof HTMLElement) document.activeElement.blur();
                }}
                onClick={(e) => {
                  e.stopPropagation();
                  onSelect(link.id);
                }}
                onPointerMove={(e) => {
                  pointerRef.current = { x: e.clientX, y: e.clientY };
                  if (hoverId === link.id) placeTooltip();
                  else setHoverId(link.id);
                }}
                onPointerLeave={() => setHoverId(null)}
              />
            </g>
          );
        })}
        <path ref={draftRef} className="link-draft" markerEnd="url(#link-arrow-manual)" />
      </svg>
      {visible &&
        hovered &&
        createPortal(
          <div ref={tooltipRef} className="link-tooltip" role="tooltip">
            <span>{names.get(hovered.sourceId) ?? "?"}</span>
            <span className="link-tooltip-arrow">→</span>
            <span>{names.get(hovered.targetId) ?? "?"}</span>
            <span className="link-tooltip-kind">{hovered.kind}</span>
            <span className="link-tooltip-hint">
              {hovered.id === selectedId ? (
                <>
                  <kbd>Delete</kbd> to remove
                </>
              ) : (
                "Click to select"
              )}
            </span>
          </div>,
          document.body,
        )}
    </>
  );
}
