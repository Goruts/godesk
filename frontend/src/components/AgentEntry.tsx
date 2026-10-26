import { memo, useState } from "react";
import type { ReactNode } from "react";
import { CodeBlock, Markdown } from "../agent/markdown";
import { formatDuration, relativePath, shortCount } from "../agent/protocol";
import type { AgentId, AgentItem, PermissionItem, ToolCategory, ToolItem } from "../agent/protocol";

/** How the user answers a permission the agent asked for. */
export type PermissionAnswer = (requestId: string, decision: "allow" | "always" | "deny") => void;

/** Lines of output or diff shown before "show all". */
const PREVIEW_LINES = 14;

/** 16×16 line icons for the agent view, drawn with the current text color. */
function Glyph({ children, size = 14 }: { children: ReactNode; size?: number }) {
  return (
    <svg
      viewBox="0 0 16 16"
      width={size}
      height={size}
      aria-hidden="true"
      focusable="false"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.4"
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      {children}
    </svg>
  );
}

const CATEGORY_ICON: Record<ToolCategory, ReactNode> = {
  shell: <path d="M2.5 4.5 6 8l-3.5 3.5M8 11.5h5.5" />,
  read: (
    <>
      <path d="M4 1.75h5.25L12.5 5v9.25H4z" />
      <path d="M9 1.75V5.25h3.5M6.25 8.5h4M6.25 11h4" />
    </>
  ),
  edit: <path d="M10.5 2.5 13.5 5.5 6 13H3v-3zM9 4l3 3" />,
  search: (
    <>
      <circle cx="7" cy="7" r="4.25" />
      <path d="m10.25 10.25 3.25 3.25" />
    </>
  ),
  web: (
    <>
      <circle cx="8" cy="8" r="5.75" />
      <path d="M2.25 8h11.5M8 2.25c1.75 1.6 2.6 3.5 2.6 5.75S9.75 12.15 8 13.75C6.25 12.15 5.4 10.25 5.4 8S6.25 3.85 8 2.25" />
    </>
  ),
  agent: <path d="M8 2v2.5M8 11.5V14M2 8h2.5M11.5 8H14M3.75 3.75l1.75 1.75M10.5 10.5l1.75 1.75M3.75 12.25 5.5 10.5M10.5 5.5l1.75-1.75" />,
  todo: <path d="M2.5 4.25 3.75 5.5 6 3.25M2.5 10.25l1.25 1.25L6 9.25M8.5 4.5h5M8.5 10.5h5" />,
  other: <path d="M9.75 2.5a3 3 0 0 0-3.4 4.1L2.5 10.45 5.55 13.5l3.85-3.85a3 3 0 0 0 4.1-3.4l-1.9 1.9-2.15-.55-.55-2.15z" />,
};

export function ChevronGlyph() {
  return (
    <Glyph size={12}>
      <path d="m6 4 4 4-4 4" />
    </Glyph>
  );
}

function StatusGlyph({ status }: { status: ToolItem["status"] }) {
  if (status === "running") return <span className="agent-spinner" aria-label="Running" />;
  return (
    <span className="agent-tool-state" data-status={status} aria-label={status === "done" ? "Done" : "Failed"}>
      <Glyph size={12}>{status === "done" ? <path d="m3.5 8.5 3 3 6-7" /> : <path d="m4.5 4.5 7 7m0-7-7 7" />}</Glyph>
    </span>
  );
}

/** The verb a tool card reads with, by what the tool does and whether it is done. */
function verb(item: ToolItem): string {
  const running = item.status === "running";
  const name = item.name.toLowerCase();
  switch (item.category) {
    case "shell":
      return running ? "Running" : "Ran";
    case "read":
      return running ? "Reading" : "Read";
    case "edit":
      if (name === "write" || (item.diff && item.diff.every((l) => l.kind === "add") && !item.files)) {
        return running ? "Writing" : "Wrote";
      }
      return running ? "Editing" : "Edited";
    case "search":
      return running ? "Searching" : "Searched";
    case "web":
      return running ? "Browsing" : name.includes("search") ? "Searched the web" : "Fetched";
    case "agent":
      return running ? "Delegating" : "Delegated";
    case "todo":
      return "Plan";
    default:
      return item.name;
  }
}

/** The file a write puts its content in, to name the language of the code. */
function languageOf(path: string): string {
  return path.match(/\.([a-z0-9]{1,6})$/i)?.[1]?.toLowerCase() ?? "";
}

function Lines({ text, className }: { text: string; className: string }) {
  const [all, setAll] = useState(false);
  const lines = text.replace(/\s+$/, "").split("\n");
  const shown = all ? lines : lines.slice(0, PREVIEW_LINES);
  return (
    <>
      <pre className={className}>{shown.join("\n")}</pre>
      {lines.length > PREVIEW_LINES ? (
        <button type="button" className="agent-more" onClick={() => setAll(!all)}>
          {all ? "Show less" : `Show all ${lines.length} lines`}
        </button>
      ) : null}
    </>
  );
}

function Diff({ item }: { item: ToolItem }) {
  const [all, setAll] = useState(false);
  const diff = item.diff ?? [];
  const shown = all ? diff : diff.slice(0, PREVIEW_LINES * 2);
  return (
    <>
      <div className="agent-diff" role="table" aria-label="Changes">
        {shown.map((line, i) => (
          <div key={i} className="agent-diff-line" data-kind={line.kind} role="row">
            <span className="agent-diff-sign" aria-hidden="true">
              {line.kind === "add" ? "+" : "−"}
            </span>
            <span className="agent-diff-text">{line.text || " "}</span>
          </div>
        ))}
      </div>
      {diff.length > shown.length || all ? (
        <button type="button" className="agent-more" onClick={() => setAll(!all)}>
          {all ? "Show less" : `Show all ${diff.length} lines`}
        </button>
      ) : null}
    </>
  );
}

function ToolBody({ item, cwd }: { item: ToolItem; cwd: string | null }) {
  if (item.todos) {
    return (
      <ul className="agent-todos">
        {item.todos.map((todo, i) => (
          <li key={i} className="agent-todo" data-status={todo.status}>
            <span className="agent-todo-box" aria-hidden="true">
              {todo.status === "completed" ? (
                <Glyph size={10}>
                  <path d="m3.5 8.5 3 3 6-7" />
                </Glyph>
              ) : null}
            </span>
            <span>{todo.text}</span>
          </li>
        ))}
      </ul>
    );
  }
  if (item.category === "edit" && item.diff && item.diff.length) {
    // A new file reads best as code; a change as what it takes out and puts in.
    const wrote = item.diff.every((l) => l.kind === "add") && item.name.toLowerCase() === "write";
    return wrote ? (
      <CodeBlock code={item.diff.map((l) => l.text).join("\n")} language={languageOf(item.summary)} />
    ) : (
      <Diff item={item} />
    );
  }
  if (item.files && item.files.length) {
    return (
      <ul className="agent-files">
        {item.files.map((file, i) => (
          <li key={i} data-change={file.change}>
            <span className="agent-file-change">{file.change === "add" ? "A" : file.change === "delete" ? "D" : "M"}</span>
            <span className="agent-file-path">{relativePath(file.path, cwd)}</span>
          </li>
        ))}
      </ul>
    );
  }
  if (item.category === "shell") {
    return (
      <div className="agent-console">
        <div className="agent-console-command">
          <span className="agent-console-prompt" aria-hidden="true">
            $
          </span>
          <span>{item.summary}</span>
        </div>
        {item.output.trim() ? <Lines text={item.output} className="agent-console-output" /> : null}
        {item.exitCode !== null && item.exitCode !== 0 ? (
          <div className="agent-console-exit">exit code {item.exitCode}</div>
        ) : null}
      </div>
    );
  }
  if (!item.output.trim()) return null;
  return <Lines text={item.output} className="agent-tool-output" />;
}

/** What a tool did, as a card that opens to show its code or its output. */
function ToolCard({ item, cwd }: { item: ToolItem; cwd: string | null }) {
  const changes = item.diff?.length ?? 0;
  // A plan, a change and a failure are worth seeing at once; the rest on demand.
  const [open, setOpen] = useState<boolean | null>(null);
  const isOpen =
    open ?? (item.category === "todo" || (item.category === "edit" && changes > 0 && changes <= 40) || item.status === "error");
  const added = item.diff?.filter((l) => l.kind === "add").length ?? 0;
  const removed = changes - added;
  const hasBody =
    Boolean(item.todos?.length) || changes > 0 || Boolean(item.files?.length) || item.category === "shell" || Boolean(item.output.trim());
  const summary = item.category === "edit" || item.category === "read" ? relativePath(item.summary, cwd) : item.summary;

  return (
    <div className="agent-tool" data-category={item.category} data-status={item.status} data-open={isOpen || undefined}>
      <button
        type="button"
        className="agent-tool-head"
        onClick={() => hasBody && setOpen(!isOpen)}
        aria-expanded={hasBody ? isOpen : undefined}
        disabled={!hasBody}
      >
        <span className="agent-tool-icon">
          <Glyph>{CATEGORY_ICON[item.category]}</Glyph>
        </span>
        <span className="agent-tool-verb">{verb(item)}</span>
        {summary ? (
          <code className="agent-tool-summary" title={item.summary}>
            {summary.split("\n")[0]}
          </code>
        ) : null}
        <span className="agent-tool-meta">
          {changes > 0 && item.category === "edit" ? (
            <span className="agent-tool-delta">
              {added ? <span data-kind="add">+{added}</span> : null}
              {removed ? <span data-kind="del">−{removed}</span> : null}
            </span>
          ) : null}
          {item.todos ? (
            <span>
              {item.todos.filter((t) => t.status === "completed").length}/{item.todos.length}
            </span>
          ) : null}
          <StatusGlyph status={item.status} />
          {hasBody ? (
            <span className="agent-chevron">
              <ChevronGlyph />
            </span>
          ) : null}
        </span>
      </button>
      {isOpen && hasBody ? (
        <div className="agent-tool-body">
          <ToolBody item={item} cwd={cwd} />
        </div>
      ) : null}
    </div>
  );
}

/** The agent's reasoning: open while it is written, folded once done. */
function Thinking({ text, live }: { text: string; live: boolean }) {
  const [open, setOpen] = useState<boolean | null>(null);
  const isOpen = open ?? live;
  const words = text.trim().split(/\s+/).length;
  return (
    <div className="agent-thinking" data-live={live || undefined} data-open={isOpen || undefined}>
      <button type="button" className="agent-thinking-head" onClick={() => setOpen(!isOpen)} aria-expanded={isOpen}>
        <span className="agent-thinking-icon" aria-hidden="true">
          <Glyph>
            <path d="M5.5 12.5h5M6.25 14.25h3.5M8 1.75a4.25 4.25 0 0 0-2.5 7.7c.5.4.75.9.75 1.55v.25h3.5V11c0-.65.25-1.15.75-1.55A4.25 4.25 0 0 0 8 1.75z" />
          </Glyph>
        </span>
        <span className={live ? "agent-shimmer" : undefined}>{live ? "Thinking…" : "Thought"}</span>
        <span className="agent-thinking-count">{words} words</span>
        <span className="agent-chevron">
          <ChevronGlyph />
        </span>
      </button>
      {isOpen ? <div className="agent-thinking-text">{text}</div> : null}
    </div>
  );
}

const PERMISSION_OUTCOME: Record<Exclude<PermissionItem["status"], "pending">, string> = {
  allow: "Allowed once",
  always: "Always allowed",
  deny: "Denied",
  expired: "No longer asked",
};

/** A tool the agent waits to be allowed to use, with the user's answer. */
function Permission({ item, onAnswer }: { item: PermissionItem; onAnswer: PermissionAnswer | undefined }) {
  const pending = item.status === "pending";
  return (
    <div className="agent-permission" data-status={item.status} role={pending ? "alert" : undefined}>
      <div className="agent-permission-head">
        <span className="agent-tool-icon" aria-hidden="true">
          <Glyph>{CATEGORY_ICON[item.category]}</Glyph>
        </span>
        <span className="agent-permission-title">
          {pending ? `${item.tool} wants to run` : item.tool}
        </span>
        {item.status !== "pending" ? <span className="agent-permission-outcome">{PERMISSION_OUTCOME[item.status]}</span> : null}
      </div>
      {item.summary ? <code className="agent-permission-summary">{item.summary}</code> : null}
      {pending ? (
        <div className="agent-permission-actions">
          <button type="button" className="agent-permission-allow" onClick={() => onAnswer?.(item.id, "allow")}>
            Allow
          </button>
          {item.canAlways ? (
            <button
              type="button"
              className="agent-permission-always"
              title="Allow it now and from now on in this folder (.claude/settings.local.json)"
              onClick={() => onAnswer?.(item.id, "always")}
            >
              Always allow
            </button>
          ) : null}
          <button type="button" className="agent-permission-deny" onClick={() => onAnswer?.(item.id, "deny")}>
            Deny
          </button>
          {item.reason ? <span className="agent-permission-reason">{item.reason}</span> : null}
        </div>
      ) : null}
    </div>
  );
}

/** One thing the agent said or did. */
export const AgentEntry = memo(function AgentEntry({
  item,
  agent,
  cwd,
  onPermission,
}: {
  item: AgentItem;
  agent: AgentId;
  cwd: string | null;
  onPermission?: PermissionAnswer;
}) {
  switch (item.kind) {
    case "permission":
      return <Permission item={item} onAnswer={onPermission} />;
    case "prompt":
      return (
        <div className="agent-entry agent-entry-prompt">
          <div className="agent-prompt">{item.text}</div>
        </div>
      );
    case "text":
      return (
        <div className="agent-entry agent-entry-text" data-live={item.live || undefined}>
          <span className="agent-entry-mark" data-agent={agent} aria-hidden="true" />
          <div className="agent-entry-body">
            <Markdown text={item.text} />
          </div>
        </div>
      );
    case "thinking":
      return <Thinking text={item.text} live={item.live} />;
    case "tool":
      return <ToolCard item={item} cwd={cwd} />;
    case "notice":
      return (
        <div className="agent-notice" data-tone={item.tone} role={item.tone === "error" ? "alert" : undefined}>
          <Glyph size={13}>
            {item.tone === "info" ? (
              <>
                <circle cx="8" cy="8" r="5.75" />
                <path d="M8 7.25v3.5M8 5.25v.01" />
              </>
            ) : (
              <path d="M8 2.25 14 13H2zM8 6.5v3M8 11.25v.01" />
            )}
          </Glyph>
          <span>{item.text}</span>
        </div>
      );
    case "turn": {
      const parts = [
        item.durationMs !== null ? formatDuration(item.durationMs) : null,
        item.input || item.output ? `${shortCount(item.input)} in · ${shortCount(item.output)} out` : null,
        item.cost ? `$${item.cost.toFixed(item.cost < 0.01 ? 3 : 2)}` : null,
      ].filter(Boolean);
      return (
        <div className="agent-turn" role="separator">
          <span>{parts.length ? `Done · ${parts.join(" · ")}` : "Done"}</span>
        </div>
      );
    }
    default:
      return null;
  }
});
