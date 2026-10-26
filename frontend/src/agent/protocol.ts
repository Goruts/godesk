/**
 * What an agent terminal shows, read from the agent's own stream: one JSON
 * event per line, in the agent's protocol (Claude Code, Codex or OpenCode),
 * plus the few `godesk.*` events the backend adds (the user's prompt, an
 * interruption, the end of a one-turn agent's turn). Every agent becomes the
 * same conversation, so one view draws them all. This module knows no React.
 */

export type AgentId = "claude" | "codex" | "opencode" | "mimo";

/** What kind of work a tool does, to draw it. */
export type ToolCategory = "shell" | "read" | "edit" | "search" | "web" | "agent" | "todo" | "other";
export type ToolStatus = "running" | "done" | "error";
export type TodoStatus = "pending" | "in_progress" | "completed";
export type Todo = { text: string; status: TodoStatus };
export type DiffLine = { kind: "add" | "del"; text: string };
export type FileChange = { path: string; change: "add" | "update" | "delete" };

export type ToolItem = {
  kind: "tool";
  id: string;
  /** The tool's own name ("Bash", "read", "command_execution"…). */
  name: string;
  category: ToolCategory;
  /** The command it runs, the file it touches or what it looks for. */
  summary: string;
  status: ToolStatus;
  output: string;
  exitCode: number | null;
  diff: DiffLine[] | null;
  files: FileChange[] | null;
  todos: Todo[] | null;
};

/** How the user answered a permission: once, from now on, or not; expired when the agent stopped waiting. */
export type PermissionStatus = "pending" | "allow" | "always" | "deny" | "expired";

/** A tool the agent wants to use and waits for the user to allow. */
export type PermissionItem = {
  kind: "permission";
  /** The agent's request id, to answer it. */
  id: string;
  tool: string;
  category: ToolCategory;
  /** The command it would run, the file it would touch… */
  summary: string;
  /** Why it asks ("This command requires approval"). */
  reason: string;
  /** The agent offered a rule to allow it from now on. */
  canAlways: boolean;
  status: PermissionStatus;
};

/** What the agent said or did, in the order it happened. */
export type AgentItem =
  | PermissionItem
  | { kind: "prompt"; text: string }
  | { kind: "text"; id: string; text: string; live: boolean }
  | { kind: "thinking"; id: string; text: string; live: boolean }
  | ToolItem
  | { kind: "notice"; text: string; tone: "info" | "warning" | "error" }
  | { kind: "turn"; durationMs: number | null; input: number; output: number; cost: number | null };

export type AgentStatus = "starting" | "working" | "idle" | "stopped";

export type AgentState = {
  items: AgentItem[];
  model: string | null;
  cwd: string | null;
  sessionId: string | null;
  status: AgentStatus;
  /** When the turn being worked on began (ms since the epoch). */
  turnStartedAt: number | null;
  /** Tokens of the whole session: what went in (with the cache) and out. */
  input: number;
  output: number;
  /** What the session has cost, when the agent says. */
  cost: number | null;
  turns: number;
  /** The agent's own slash commands, as it reports them (`/compact`, custom
   * project commands…), so the view can offer them for autocompletion. */
  commands: string[];
};

export function emptyState(): AgentState {
  return {
    items: [],
    model: null,
    cwd: null,
    sessionId: null,
    status: "starting",
    turnStartedAt: null,
    input: 0,
    output: 0,
    cost: null,
    turns: 0,
    commands: [],
  };
}

/** A command name as `/name`, however the agent spells it. */
function slashName(value: unknown): string | null {
  const text = typeof value === "string" ? value.trim() : "";
  if (!text) return null;
  const name = text.startsWith("/") ? text : `/${text}`;
  return /^\/[\w:-]+$/.test(name) ? name.toLowerCase() : null;
}

/** How each agent is shown. */
export const AGENTS: Record<AgentId, { label: string; placeholder: string }> = {
  claude: { label: "Claude Code", placeholder: "Ask Claude Code to do something…" },
  codex: { label: "Codex", placeholder: "Ask Codex to do something…" },
  opencode: { label: "OpenCode", placeholder: "Ask OpenCode to do something…" },
  mimo: { label: "MiMo Code", placeholder: "Ask MiMo Code to do something…" },
};

export function agentId(name: string | null | undefined): AgentId {
  return name === "codex" || name === "opencode" || name === "mimo" ? name : "claude";
}

type Json = Record<string, unknown>;

function isObject(value: unknown): value is Json {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function str(value: unknown): string {
  return typeof value === "string" ? value : "";
}

function num(value: unknown): number {
  return typeof value === "number" && Number.isFinite(value) ? value : 0;
}

/** Colors and cursor moves a program wrote, which mean nothing here. */
const ANSI = /\x1b\[[0-?]*[ -/]*[@-~]|\x1b\][^\x07]*(\x07|\x1b\\)/g;

export function stripAnsi(text: string): string {
  return text.replace(ANSI, "");
}

/** The text of a tool's result, which can be a string or a list of blocks. */
function resultText(content: unknown): string {
  if (typeof content === "string") return content;
  if (Array.isArray(content)) {
    return content
      .map((block) => (typeof block === "string" ? block : isObject(block) ? str(block["text"]) : ""))
      .filter(Boolean)
      .join("\n");
  }
  return isObject(content) ? str(content["text"]) : "";
}

/** The kind of work a tool does, from its name in any agent. */
export function toolCategory(name: string): ToolCategory {
  const n = name.toLowerCase();
  if (["bash", "powershell", "shell", "command_execution", "bashoutput", "monitor"].includes(n)) return "shell";
  if (["read", "view", "notebookread"].includes(n)) return "read";
  if (["edit", "write", "multiedit", "notebookedit", "patch", "file_change", "apply_patch"].includes(n)) return "edit";
  if (["grep", "glob", "list", "ls", "find", "lsp"].includes(n)) return "search";
  if (["webfetch", "websearch", "web_search", "fetch"].includes(n)) return "web";
  if (["task", "agent"].includes(n)) return "agent";
  if (["todowrite", "todoread", "todo_list"].includes(n)) return "todo";
  return "other";
}

/**
 * One line to describe a tool call: the command it runs, the file it touches
 * or the pattern it looks for. The first useful string of the input wins.
 */
export function toolSummary(name: string, input: unknown): string {
  if (!isObject(input)) return "";
  // A search says best what it looks for, wherever it looks.
  if (toolCategory(name) === "search" && str(input["pattern"])) return str(input["pattern"]);
  for (const key of [
    "command",
    "file_path",
    "filePath",
    "notebook_path",
    "path",
    "pattern",
    "url",
    "query",
    "description",
    "skill",
    "prompt",
  ]) {
    const value = str(input[key]);
    if (value) return value;
  }
  return "";
}

/**
 * The command Codex ran, without the shell it wrapped it in:
 * `"powershell.exe" -Command "Get-Content a.txt"` → `Get-Content a.txt`.
 */
export function unwrapCommand(command: string): string {
  const wrapped = command.match(/^"?[^"]*?(?:powershell|pwsh|bash|cmd)(?:\.exe)?"?\s+(?:-Command|-c|-lc|\/c)\s+([\s\S]+)$/i);
  if (!wrapped) return command;
  const inner = (wrapped[1] ?? "").trim();
  return inner.startsWith('"') && inner.endsWith('"') ? inner.slice(1, -1).replace(/\\"/g, '"') : inner;
}

/**
 * The lines an edit takes out and puts in. The lines both sides share at the
 * start and the end are left out: they did not change.
 */
export function lineDiff(before: string, after: string): DiffLine[] {
  const a = before ? before.split("\n") : [];
  const b = after ? after.split("\n") : [];
  let start = 0;
  while (start < a.length && start < b.length && a[start] === b[start]) start++;
  let endA = a.length;
  let endB = b.length;
  while (endA > start && endB > start && a[endA - 1] === b[endB - 1]) {
    endA--;
    endB--;
  }
  return [
    ...a.slice(start, endA).map((text) => ({ kind: "del" as const, text })),
    ...b.slice(start, endB).map((text) => ({ kind: "add" as const, text })),
  ];
}

/** The change an editing tool makes, from its input, in any agent's words. */
function editDiff(input: unknown): DiffLine[] | null {
  if (!isObject(input)) return null;
  const before = str(input["old_string"] ?? input["oldString"]);
  const after = str(input["new_string"] ?? input["newString"]);
  if (before || after) return lineDiff(before, after);
  if (Array.isArray(input["edits"])) {
    return input["edits"].flatMap((edit) =>
      isObject(edit) ? lineDiff(str(edit["old_string"] ?? edit["oldString"]), str(edit["new_string"] ?? edit["newString"])) : [],
    );
  }
  const content = str(input["content"]);
  return content ? lineDiff("", content) : null;
}

function todoStatus(value: unknown): TodoStatus {
  return value === "completed" || value === "in_progress" ? value : "pending";
}

/** A plan, from Claude's or OpenCode's TodoWrite input. */
function todosOf(input: unknown): Todo[] | null {
  if (!isObject(input) || !Array.isArray(input["todos"])) return null;
  return input["todos"].filter(isObject).map((t) => ({ text: str(t["content"]), status: todoStatus(t["status"]) }));
}

/** An agent's error, with what to do when it is the model that is refused. */
function errorText(raw: unknown): string {
  const text = agentError(raw);
  const modelRefused = /model/i.test(text) && /not supported|not found|does not exist|unknown|not available/i.test(text);
  return modelRefused ? `${text} Write /model <name> to use another model.` : text;
}

/** An agent's error, often a JSON document inside a string. */
function agentError(raw: unknown): string {
  const text = isObject(raw) ? str(raw["message"]) || str(isObject(raw["data"]) ? raw["data"]["message"] : "") : str(raw);
  try {
    const inner: unknown = JSON.parse(text);
    if (isObject(inner)) {
      const nested = isObject(inner["error"]) ? str(inner["error"]["message"]) : str(inner["message"]);
      if (nested) return nested;
    }
  } catch {
    // Already plain text.
  }
  return text || "The agent reported an error.";
}

function tool(id: string, name: string, input: unknown, category = toolCategory(name)): ToolItem {
  return {
    kind: "tool",
    id,
    name,
    category,
    summary: toolSummary(name, input),
    status: "running",
    output: "",
    exitCode: null,
    diff: category === "edit" ? editDiff(input) : null,
    files: null,
    todos: category === "todo" ? todosOf(input) : null,
  };
}

/**
 * Reads an agent's stream, a line at a time, into what the terminal shows.
 * Items are replaced, never changed in place, so React sees what changed.
 */
export class AgentStream {
  private state: AgentState = emptyState();
  /** Where each item that can still change is, by its id. */
  private index = new Map<string, number>();
  /** The tokens and cost of the turn in progress (one-turn agents). */
  private turn = { input: 0, output: 0, cost: 0, hasCost: false };
  private interrupted = false;
  private rateLimit = "";

  constructor(readonly agent: AgentId) {}

  /** The state as it is now. */
  get current(): AgentState {
    return this.state;
  }

  /**
   * Feeds one line of the agent's output. A line that is not JSON is what
   * the agent wrote on its error stream (`! …`): it says why it stopped.
   */
  pushLine(line: string): AgentState {
    const text = line.replace(/\r$/, "");
    if (text.startsWith("! ")) {
      this.notice(stripAnsi(text.slice(2)), "warning");
      return this.state;
    }
    const start = text.indexOf("{");
    if (start < 0) return this.state;
    let event: unknown;
    try {
      event = JSON.parse(text.slice(start));
    } catch {
      return this.state;
    }
    if (!isObject(event)) return this.state;
    const type = str(event["type"]);
    if (type.startsWith("godesk.")) this.godesk(type, event);
    else if (this.agent === "codex") this.codex(type, event);
    // MiMo Code is OpenCode under another name: the same events.
    else if (this.agent === "opencode" || this.agent === "mimo") this.opencode(type, event);
    else this.claude(type, event);
    return this.state;
  }

  /** Marks the agent as gone. */
  stop(): AgentState {
    this.finishLive();
    this.set({ status: "stopped", turnStartedAt: null });
    return this.state;
  }

  // ---------- Godesk's own events ----------

  private godesk(type: string, event: Json): void {
    const at = num(event["at"]) || null;
    switch (type) {
      case "godesk.prompt":
        this.add({ kind: "prompt", text: str(event["text"]) });
        this.turn = { input: 0, output: 0, cost: 0, hasCost: false };
        this.interrupted = false;
        this.set({ status: "working", turnStartedAt: at ?? Date.now() });
        break;
      case "godesk.interrupt":
        this.interrupted = true;
        this.expirePermissions();
        break;
      case "godesk.permission": {
        const decision = str(event["decision"]);
        if (decision === "allow" || decision === "always" || decision === "deny") {
          this.updatePermission(str(event["request_id"]), decision);
        }
        break;
      }
      case "godesk.turn_end": {
        this.finishLive();
        this.endRunningTools(this.interrupted ? "error" : "done");
        const code = event["code"];
        if (this.interrupted) this.notice("Stopped. The conversation goes on from here.", "info");
        else if (code !== 0) this.notice(`${AGENTS[this.agent].label} stopped with an error (exit code ${String(code ?? "unknown")}).`, "error");
        const started = this.state.turnStartedAt;
        this.add({
          kind: "turn",
          durationMs: started && at ? at - started : null,
          input: this.turn.input,
          output: this.turn.output,
          cost: this.turn.hasCost ? this.turn.cost : null,
        });
        this.set({ status: "idle", turnStartedAt: null, turns: this.state.turns + 1 });
        break;
      }
      case "godesk.restart":
        // A turn the agent was on when it stopped is over.
        this.finishLive();
        this.endRunningTools("error");
        this.expirePermissions();
        this.notice(
          event["reopened"] === true
            ? "Godesk started again: the conversation goes on from here."
            : `${AGENTS[this.agent].label} started again.`,
          "info",
        );
        this.set({ status: this.agent === "claude" ? "starting" : "idle", turnStartedAt: null });
        break;
      default:
        break;
    }
  }

  // ---------- Claude Code: --output-format stream-json ----------

  private claude(type: string, event: Json): void {
    // A subagent's own work: its card on the canvas shows it.
    if (event["parent_tool_use_id"]) return;
    switch (type) {
      case "system":
        if (event["subtype"] === "init") {
          const commands = Array.isArray(event["slash_commands"])
            ? [...new Set(event["slash_commands"].map(slashName).filter((n): n is string => n !== null))]
            : this.state.commands;
          this.set({
            model: str(event["model"]) || this.state.model,
            cwd: str(event["cwd"]) || this.state.cwd,
            sessionId: str(event["session_id"]) || this.state.sessionId,
            status: this.state.status === "starting" ? "idle" : this.state.status,
            commands,
          });
        } else if (event["subtype"] === "permission_denied") {
          const name = str(event["tool_name"]) || "A tool";
          this.notice(`${name} needs a permission that was not granted.`, "warning");
        }
        break;
      case "stream_event":
        this.claudeDelta(isObject(event["event"]) ? event["event"] : {});
        break;
      case "assistant":
        this.claudeMessage(event);
        break;
      case "user":
        this.claudeResults(event);
        break;
      case "result":
        this.expirePermissions();
        this.claudeResult(event);
        break;
      case "control_request": {
        const request = isObject(event["request"]) ? event["request"] : {};
        if (request["subtype"] !== "can_use_tool") break;
        const name = str(request["display_name"]) || str(request["tool_name"]) || "A tool";
        this.finishLive();
        this.add({
          kind: "permission",
          id: str(event["request_id"]),
          tool: name,
          category: toolCategory(str(request["tool_name"])),
          summary: toolSummary(str(request["tool_name"]), request["input"]),
          reason: str(request["decision_reason"]),
          canAlways: Array.isArray(request["permission_suggestions"]) && request["permission_suggestions"].length > 0,
          status: "pending",
        });
        break;
      }
      case "control_cancel_request":
        this.updatePermission(str(event["request_id"]), "expired");
        break;
      case "rate_limit_event":
        this.claudeRateLimit(event);
        break;
      default:
        break;
    }
  }

  /** Text and thinking as they are written. */
  private claudeDelta(event: Json): void {
    const index = num(event["index"]);
    if (event["type"] === "message_start") {
      const message = isObject(event["message"]) ? event["message"] : {};
      if (str(message["model"])) this.set({ model: str(message["model"]) });
      this.set({ status: "working" });
      return;
    }
    if (event["type"] !== "content_block_delta" || !isObject(event["delta"])) return;
    const delta = event["delta"];
    if (delta["type"] === "text_delta") this.appendLive("text", `live-text-${index}`, str(delta["text"]));
    else if (delta["type"] === "thinking_delta") this.appendLive("thinking", `live-thinking-${index}`, str(delta["thinking"]));
  }

  private claudeMessage(event: Json): void {
    const message = isObject(event["message"]) ? event["message"] : {};
    const content = Array.isArray(message["content"]) ? message["content"] : [];
    if (this.state.status !== "working") this.set({ status: "working" });
    for (const block of content) {
      if (!isObject(block)) continue;
      if (block["type"] === "text") this.finishLiveAs("text", str(block["text"]));
      else if (block["type"] === "thinking") this.finishLiveAs("thinking", str(block["thinking"]));
      else if (block["type"] === "tool_use") {
        const name = str(block["name"]) || "tool";
        this.upsert(tool(str(block["id"]) || name, name, block["input"]));
      }
    }
  }

  private claudeResults(event: Json): void {
    const message = isObject(event["message"]) ? event["message"] : {};
    const content = Array.isArray(message["content"]) ? message["content"] : [];
    for (const block of content) {
      if (!isObject(block)) continue;
      if (block["type"] === "tool_result") {
        const id = str(block["tool_use_id"]);
        this.updateTool(id, {
          status: block["is_error"] === true ? "error" : "done",
          output: stripAnsi(resultText(block["content"])),
        });
      } else if (block["type"] === "text" && str(block["text"]).startsWith("[Request interrupted")) {
        this.endRunningTools("error");
        this.notice("Stopped. The conversation goes on from here.", "info");
      }
    }
  }

  private claudeResult(event: Json): void {
    this.finishLive();
    this.endRunningTools("done");
    const usage = isObject(event["usage"]) ? event["usage"] : {};
    const input =
      num(usage["input_tokens"]) + num(usage["cache_read_input_tokens"]) + num(usage["cache_creation_input_tokens"]);
    const output = num(usage["output_tokens"]);
    // Claude Code reports what the whole session has cost so far.
    const total = typeof event["total_cost_usd"] === "number" ? event["total_cost_usd"] : null;
    const turnCost = total !== null ? Math.max(0, total - (this.state.cost ?? 0)) : null;
    if (event["is_error"] === true && str(event["result"])) this.notice(str(event["result"]), "error");
    if (input || output) {
      this.add({
        kind: "turn",
        durationMs: typeof event["duration_ms"] === "number" ? event["duration_ms"] : null,
        input,
        output,
        cost: turnCost,
      });
    }
    this.set({
      status: "idle",
      turnStartedAt: null,
      input: this.state.input + input,
      output: this.state.output + output,
      cost: total ?? this.state.cost,
      turns: this.state.turns + 1,
    });
  }

  private claudeRateLimit(event: Json): void {
    const info = isObject(event["rate_limit_info"]) ? event["rate_limit_info"] : {};
    const status = str(info["status"]);
    if (!status || status === "allowed") return;
    const key = `${status}:${str(info["rateLimitType"])}`;
    if (key === this.rateLimit) return;
    this.rateLimit = key;
    const used = Math.round(num(info["utilization"]) * 100);
    const window = str(info["rateLimitType"]).replace(/_/g, "-") || "usage";
    this.notice(
      status === "rejected"
        ? `The ${window} usage limit is reached.`
        : `${used ? `${used} % of the` : "Close to the"} ${window} usage limit${used ? " is used" : ""}.`,
      status === "rejected" ? "error" : "warning",
    );
  }

  // ---------- Codex: codex exec --json ----------

  private codex(type: string, event: Json): void {
    switch (type) {
      case "thread.started":
        this.set({ sessionId: str(event["thread_id"]) || this.state.sessionId });
        break;
      case "turn.started":
        this.set({ status: "working" });
        break;
      case "item.started":
      case "item.updated":
      case "item.completed":
        if (isObject(event["item"])) this.codexItem(event["item"], type === "item.completed");
        break;
      case "turn.completed": {
        const usage = isObject(event["usage"]) ? event["usage"] : {};
        this.turn.input += num(usage["input_tokens"]);
        this.turn.output += num(usage["output_tokens"]);
        this.set({ input: this.state.input + num(usage["input_tokens"]), output: this.state.output + num(usage["output_tokens"]) });
        break;
      }
      case "turn.failed":
        this.notice(errorText(event["error"]), "error");
        break;
      case "error":
        this.notice(errorText(event["message"]), "error");
        break;
      default:
        break;
    }
  }

  private codexItem(item: Json, done: boolean): void {
    // Item ids start again each turn: the turn makes them unique.
    const id = `${this.state.turns}:${str(item["id"])}`;
    const status: ToolStatus =
      item["status"] === "failed" || (typeof item["exit_code"] === "number" && item["exit_code"] !== 0)
        ? "error"
        : done || item["status"] === "completed"
          ? "done"
          : "running";
    switch (item["type"]) {
      case "agent_message":
        this.upsertText("text", id, str(item["text"]));
        break;
      case "reasoning":
        this.upsertText("thinking", id, str(item["text"]));
        break;
      case "command_execution":
        this.upsert({
          ...tool(id, "Shell", null, "shell"),
          summary: unwrapCommand(str(item["command"])),
          status,
          output: stripAnsi(str(item["aggregated_output"])),
          exitCode: typeof item["exit_code"] === "number" ? item["exit_code"] : null,
        });
        break;
      case "file_change": {
        const changes = Array.isArray(item["changes"]) ? item["changes"].filter(isObject) : [];
        const files: FileChange[] = changes.map((c) => ({
          path: str(c["path"]),
          change: c["kind"] === "add" ? "add" : c["kind"] === "delete" ? "delete" : "update",
        }));
        this.upsert({ ...tool(id, "Edit", null, "edit"), summary: files.map((f) => f.path).join(", "), status, files });
        break;
      }
      case "mcp_tool_call":
        this.upsert({
          ...tool(id, `${str(item["server"])}.${str(item["tool"])}`, item["arguments"], "other"),
          status,
          output: resultText(isObject(item["result"]) ? item["result"]["content"] : "") || errorText(item["error"] ?? ""),
        });
        break;
      case "web_search":
        this.upsert({ ...tool(id, "Web search", null, "web"), summary: str(item["query"]), status });
        break;
      case "todo_list": {
        const todos = (Array.isArray(item["items"]) ? item["items"].filter(isObject) : []).map((t) => ({
          text: str(t["text"]),
          status: t["completed"] === true ? ("completed" as const) : ("pending" as const),
        }));
        this.upsert({ ...tool(id, "Plan", null, "todo"), status: "done", todos });
        break;
      }
      case "error":
        this.notice(errorText(item["message"]), item["message"] && str(item["message"]).startsWith("Model metadata") ? "info" : "error");
        break;
      default:
        break;
    }
  }

  // ---------- OpenCode: opencode run --format json ----------

  private opencode(type: string, event: Json): void {
    if (str(event["sessionID"]) && !this.state.sessionId) this.set({ sessionId: str(event["sessionID"]) });
    const part = isObject(event["part"]) ? event["part"] : {};
    switch (type) {
      case "step_start":
        this.set({ status: "working" });
        break;
      case "text":
        this.upsertText("text", str(part["id"]), str(part["text"]));
        break;
      case "reasoning":
        this.upsertText("thinking", str(part["id"]), str(part["text"]));
        break;
      case "tool_use": {
        const state = isObject(part["state"]) ? part["state"] : {};
        const name = str(part["tool"]) || "tool";
        const status: ToolStatus =
          state["status"] === "error" ? "error" : state["status"] === "completed" ? "done" : "running";
        const output = state["status"] === "error" ? errorText(state["error"]) : str(state["output"]);
        this.upsert({ ...tool(str(part["callID"]) || str(part["id"]), name, state["input"]), status, output: stripAnsi(output) });
        break;
      }
      case "step_finish": {
        const tokens = isObject(part["tokens"]) ? part["tokens"] : {};
        const cache = isObject(tokens["cache"]) ? tokens["cache"] : {};
        const input = num(tokens["input"]) + num(cache["read"]) + num(cache["write"]);
        const output = num(tokens["output"]) + num(tokens["reasoning"]);
        this.turn.input += input;
        this.turn.output += output;
        if (typeof part["cost"] === "number") {
          this.turn.cost += part["cost"];
          this.turn.hasCost = true;
        }
        this.set({
          input: this.state.input + input,
          output: this.state.output + output,
          cost: typeof part["cost"] === "number" ? (this.state.cost ?? 0) + part["cost"] : this.state.cost,
        });
        break;
      }
      case "error":
        this.notice(errorText(event["error"]), "error");
        break;
      default:
        break;
    }
  }

  // ---------- Items ----------

  private set(patch: Partial<AgentState>): void {
    this.state = { ...this.state, ...patch };
  }

  private add(item: AgentItem): void {
    const items = [...this.state.items, item];
    if ("id" in item) this.index.set(`${item.kind}:${item.id}`, items.length - 1);
    this.set({ items });
  }

  private replace(at: number, item: AgentItem): void {
    const items = this.state.items.slice();
    items[at] = item;
    this.set({ items });
  }

  private upsert(item: ToolItem): void {
    const at = this.index.get(`tool:${item.id}`);
    if (at === undefined) {
      this.add(item);
      return;
    }
    const old = this.state.items[at] as ToolItem;
    // A tool's input is only complete once: keep what was known.
    this.replace(at, {
      ...item,
      diff: item.diff ?? old.diff,
      files: item.files ?? old.files,
      todos: item.todos ?? old.todos,
      output: item.output || old.output,
    });
  }

  private updatePermission(id: string, status: PermissionStatus): void {
    const at = this.index.get(`permission:${id}`);
    const item = at === undefined ? undefined : this.state.items[at];
    if (at === undefined || item?.kind !== "permission" || item.status !== "pending") return;
    this.replace(at, { ...item, status });
  }

  /** The agent no longer waits for what it asked. */
  private expirePermissions(): void {
    if (!this.state.items.some((i) => i.kind === "permission" && i.status === "pending")) return;
    this.set({
      items: this.state.items.map((i) => (i.kind === "permission" && i.status === "pending" ? { ...i, status: "expired" } : i)),
    });
  }

  private updateTool(id: string, patch: Partial<ToolItem>): void {
    const at = this.index.get(`tool:${id}`);
    if (at === undefined) return;
    this.replace(at, { ...(this.state.items[at] as ToolItem), ...patch });
  }

  private upsertText(kind: "text" | "thinking", id: string, text: string): void {
    if (!text.trim()) return;
    const at = this.index.get(`${kind}:${id}`);
    if (at === undefined) this.add({ kind, id, text, live: false });
    else this.replace(at, { kind, id, text, live: false });
  }

  /** Text that is still being written. */
  private appendLive(kind: "text" | "thinking", id: string, piece: string): void {
    if (!piece) return;
    const at = this.index.get(`${kind}:${id}`);
    const current = at === undefined ? undefined : this.state.items[at];
    if (current && (current.kind === "text" || current.kind === "thinking") && current.live) {
      this.replace(at as number, { ...current, text: current.text + piece });
      return;
    }
    this.add({ kind, id, text: piece, live: true });
  }

  /** The finished text replaces what was written of it so far. */
  private finishLiveAs(kind: "text" | "thinking", text: string): void {
    for (let at = this.state.items.length - 1; at >= 0; at--) {
      const item = this.state.items[at];
      if (item?.kind === kind && item.live) {
        this.index.delete(`${kind}:${item.id}`);
        if (text.trim()) this.replace(at, { ...item, text, live: false });
        else this.set({ items: this.state.items.filter((_, i) => i !== at) });
        return;
      }
    }
    if (text.trim()) this.add({ kind, id: `${kind}-${this.state.items.length}`, text, live: false });
  }

  private finishLive(): void {
    if (!this.state.items.some((i) => (i.kind === "text" || i.kind === "thinking") && i.live)) return;
    this.set({
      items: this.state.items.map((i) => ((i.kind === "text" || i.kind === "thinking") && i.live ? { ...i, live: false } : i)),
    });
    for (const key of [...this.index.keys()]) if (key.includes(":live-")) this.index.delete(key);
  }

  private endRunningTools(status: ToolStatus): void {
    if (!this.state.items.some((i) => i.kind === "tool" && i.status === "running")) return;
    this.set({ items: this.state.items.map((i) => (i.kind === "tool" && i.status === "running" ? { ...i, status } : i)) });
  }

  /** A notice, unless the same one was just said (Codex reports a failed turn twice). */
  private notice(text: string, tone: "info" | "warning" | "error"): void {
    const said = text.trim();
    if (!said) return;
    const recent = this.state.items.slice(-3);
    if (recent.some((i) => i.kind === "notice" && i.text === said)) return;
    this.add({ kind: "notice", text: said, tone });
  }
}

/** A short folder path to show in the header. */
export function shortPath(path: string | null): string {
  if (!path) return "";
  const parts = path.split(/[\\/]/).filter(Boolean);
  return parts.length > 2 ? `…/${parts.slice(-2).join("/")}` : path;
}

/** A path inside the agent's folder, written from there. */
export function relativePath(path: string, cwd: string | null): string {
  if (!cwd) return path;
  const norm = (p: string) => p.replace(/\\/g, "/").replace(/\/+$/, "");
  const full = norm(path);
  const base = norm(cwd);
  return full.toLowerCase().startsWith(`${base.toLowerCase()}/`) ? full.slice(base.length + 1) : path;
}

/** A token count, short: 980, 12.3k, 1.2M. */
export function shortCount(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1000) return `${(n / 1000).toFixed(1)}k`;
  return String(n);
}

/** Tokens as a short string: "12.3k in · 2.1k out". */
export function formatTokens(input: number, output: number): string {
  return `${shortCount(input)} in · ${shortCount(output)} out`;
}

/** A length of time, short: 850ms, 12s, 3m 05s. */
export function formatDuration(ms: number): string {
  if (ms < 1000) return `${Math.max(0, Math.round(ms))}ms`;
  const seconds = Math.round(ms / 1000);
  if (seconds < 60) return `${seconds}s`;
  return `${Math.floor(seconds / 60)}m ${String(seconds % 60).padStart(2, "0")}s`;
}
