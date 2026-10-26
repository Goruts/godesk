import { describe, expect, it } from "vitest";
import {
  AgentStream,
  formatDuration,
  formatTokens,
  lineDiff,
  relativePath,
  shortPath,
  toolSummary,
  unwrapCommand,
} from "./protocol";
import type { AgentItem, ToolItem } from "./protocol";

/** Feeds events, one JSON line each, as the backend emits them. */
function feed(stream: AgentStream, ...events: unknown[]) {
  for (const event of events) stream.pushLine(typeof event === "string" ? event : JSON.stringify(event));
  return stream.current;
}

function tools(items: AgentItem[]): ToolItem[] {
  return items.filter((i): i is ToolItem => i.kind === "tool");
}

describe("Claude Code's stream", () => {
  it("reads what the agent said, did and answered", () => {
    const state = feed(
      new AgentStream("claude"),
      { type: "system", subtype: "init", cwd: "C:\\work\\godesk", session_id: "abc", model: "claude-opus-5-5" },
      { type: "godesk.prompt", text: "mira main.rs", at: 1000 },
      {
        type: "assistant",
        message: {
          content: [
            { type: "text", text: "Voy a mirar el archivo." },
            { type: "tool_use", id: "t1", name: "Read", input: { file_path: "C:\\work\\godesk\\main.rs" } },
          ],
        },
      },
      { type: "user", message: { content: [{ type: "tool_result", tool_use_id: "t1", content: "fn main() {}" }] } },
      {
        type: "result",
        usage: { input_tokens: 10, cache_read_input_tokens: 90, output_tokens: 5 },
        total_cost_usd: 0.02,
        duration_ms: 1200,
      },
    );
    expect(state.model).toBe("claude-opus-5-5");
    expect(state.status).toBe("idle");
    expect(state.input).toBe(100);
    expect(state.output).toBe(5);
    expect(state.cost).toBe(0.02);
    expect(state.items.map((i) => i.kind)).toEqual(["prompt", "text", "tool", "turn"]);
    const [read] = tools(state.items);
    expect(read).toMatchObject({ category: "read", status: "done", output: "fn main() {}", summary: "C:\\work\\godesk\\main.rs" });
  });

  it("shows text as it is written, then the finished text in its place", () => {
    const stream = new AgentStream("claude");
    const delta = (text: string) => ({
      type: "stream_event",
      event: { type: "content_block_delta", index: 0, delta: { type: "text_delta", text } },
    });
    feed(stream, delta("Ho"), delta("la"));
    expect(stream.current.items).toEqual([{ kind: "text", id: "live-text-0", text: "Hola", live: true }]);
    feed(stream, { type: "assistant", message: { content: [{ type: "text", text: "Hola." }] } });
    expect(stream.current.items).toHaveLength(1);
    expect(stream.current.items[0]).toMatchObject({ text: "Hola.", live: false });
    // The next message writes a text of its own.
    feed(stream, delta("Otra"));
    expect(stream.current.items).toHaveLength(2);
  });

  it("counts the session's cost once, though Claude Code reports it as a running total", () => {
    const stream = new AgentStream("claude");
    const result = (total: number) => ({ type: "result", usage: { input_tokens: 1, output_tokens: 1 }, total_cost_usd: total });
    feed(stream, result(0.05), result(0.12));
    expect(stream.current.cost).toBe(0.12);
    const turns = stream.current.items.filter((i) => i.kind === "turn");
    expect(turns.map((t) => t.kind === "turn" && t.cost?.toFixed(2))).toEqual(["0.05", "0.07"]);
  });

  it("shows an edit as the lines it takes out and puts in", () => {
    const state = feed(new AgentStream("claude"), {
      type: "assistant",
      message: {
        content: [
          { type: "tool_use", id: "e1", name: "Edit", input: { file_path: "a.rs", old_string: "a\nb\nc", new_string: "a\nB\nc" } },
        ],
      },
    });
    expect(tools(state.items)[0]?.diff).toEqual([
      { kind: "del", text: "b" },
      { kind: "add", text: "B" },
    ]);
  });

  it("leaves a subagent's work to its own card", () => {
    const state = feed(new AgentStream("claude"), {
      type: "assistant",
      parent_tool_use_id: "task1",
      message: { content: [{ type: "text", text: "subagent" }] },
    });
    expect(state.items).toHaveLength(0);
  });

  it("warns about a usage limit once, and not while all is well", () => {
    const limit = (status: string) => ({
      type: "rate_limit_event",
      rate_limit_info: { status, rateLimitType: "seven_day", utilization: 0.93 },
    });
    const state = feed(new AgentStream("claude"), limit("allowed"), limit("allowed_warning"), limit("allowed_warning"));
    expect(state.items).toEqual([{ kind: "notice", text: "93 % of the seven-day usage limit is used.", tone: "warning" }]);
  });

  it("says when a tool was not allowed, and what the agent wrote on its error stream", () => {
    const state = feed(
      new AgentStream("claude"),
      { type: "system", subtype: "permission_denied", tool_name: "Bash" },
      "! \x1b[31mError: not logged in\x1b[0m",
    );
    expect(state.items.map((i) => i.kind === "notice" && i.text)).toEqual([
      "Bash needs a permission that was not granted.",
      "Error: not logged in",
    ]);
  });
});

describe("Codex's stream", () => {
  // Lines as `codex exec --json` wrote them.
  const turn = [
    { type: "godesk.prompt", text: "lee a.txt", at: 1000 },
    { type: "thread.started", thread_id: "01a16183-c118" },
    { type: "turn.started" },
    {
      type: "item.started",
      item: {
        id: "item_0",
        type: "command_execution",
        command: '"C:\\\\Windows\\\\System32\\\\WindowsPowerShell\\\\v1.0\\\\powershell.exe" -Command "Get-Content -LiteralPath \'a.txt\'"',
        aggregated_output: "",
        exit_code: null,
        status: "in_progress",
      },
    },
    {
      type: "item.completed",
      item: {
        id: "item_0",
        type: "command_execution",
        command: '"C:\\\\Windows\\\\System32\\\\WindowsPowerShell\\\\v1.0\\\\powershell.exe" -Command "Get-Content -LiteralPath \'a.txt\'"',
        aggregated_output: "hola\r\n",
        exit_code: 0,
        status: "completed",
      },
    },
    { type: "item.completed", item: { id: "item_1", type: "agent_message", text: "`a.txt` pone «hola»." } },
    { type: "turn.completed", usage: { input_tokens: 27018, cached_input_tokens: 20864, output_tokens: 158 } },
    { type: "godesk.turn_end", code: 0, at: 4500 },
  ];

  it("reads a turn: the command it ran, its answer and what it cost", () => {
    const state = feed(new AgentStream("codex"), ...turn);
    expect(state.sessionId).toBe("01a16183-c118");
    expect(state.status).toBe("idle");
    expect(state.items.map((i) => i.kind)).toEqual(["prompt", "tool", "text", "turn"]);
    expect(tools(state.items)[0]).toMatchObject({
      category: "shell",
      summary: "Get-Content -LiteralPath 'a.txt'",
      status: "done",
      exitCode: 0,
      output: "hola\r\n",
    });
    expect(state.items[3]).toEqual({ kind: "turn", durationMs: 3500, input: 27018, output: 158, cost: null });
  });

  it("keeps the items of each turn apart, though their ids start again", () => {
    const state = feed(new AgentStream("codex"), ...turn, ...turn);
    expect(tools(state.items)).toHaveLength(2);
  });

  it("says why a turn failed, in plain words", () => {
    const message =
      '{"type":"error","status":400,"error":{"type":"invalid_request_error","message":"The \'gpt-x\' model is not supported."}}';
    const state = feed(
      new AgentStream("codex"),
      { type: "godesk.prompt", text: "hola", at: 1 },
      { type: "error", message },
      { type: "turn.failed", error: { message } },
      { type: "godesk.turn_end", code: 1, at: 2 },
    );
    const notices = state.items.filter((i) => i.kind === "notice").map((i) => i.kind === "notice" && i.text);
    expect(notices).toEqual(["The 'gpt-x' model is not supported. Write /model <name> to use another model.", "Codex stopped with an error (exit code 1)."]);
    expect(state.status).toBe("idle");
  });

  it("lists the files a change touched", () => {
    const state = feed(new AgentStream("codex"), {
      type: "item.completed",
      item: { id: "item_2", type: "file_change", status: "completed", changes: [{ path: "src/a.rs", kind: "update" }] },
    });
    expect(tools(state.items)[0]?.files).toEqual([{ path: "src/a.rs", change: "update" }]);
  });
});

describe("OpenCode's stream", () => {
  it("reads a turn: the tool, the answer and the tokens of each step", () => {
    const session = "ses_e9e7caffbffe";
    const state = feed(
      new AgentStream("opencode"),
      { type: "godesk.prompt", text: "lee a.txt", at: 1000 },
      { type: "step_start", sessionID: session, part: { type: "step-start" } },
      {
        type: "tool_use",
        sessionID: session,
        part: {
          type: "tool",
          tool: "read",
          callID: "call_1",
          state: { status: "completed", input: { filePath: "C:\\p\\a.txt" }, output: "1: hola" },
        },
      },
      { type: "text", sessionID: session, part: { id: "prt_1", type: "text", text: "I'll read the file." } },
      {
        type: "step_finish",
        sessionID: session,
        part: { reason: "tool-calls", tokens: { input: 8753, output: 83, reasoning: 0, cache: { write: 0, read: 1950 } }, cost: 0 },
      },
      { type: "text", sessionID: session, part: { id: "prt_2", type: "text", text: "Pone «hola»." } },
      {
        type: "step_finish",
        sessionID: session,
        part: { reason: "stop", tokens: { input: 180, output: 14, reasoning: 0, cache: { write: 0, read: 10703 } }, cost: 0 },
      },
      { type: "godesk.turn_end", code: 0, at: 3000 },
    );
    expect(state.sessionId).toBe(session);
    expect(state.items.map((i) => i.kind)).toEqual(["prompt", "tool", "text", "text", "turn"]);
    expect(tools(state.items)[0]).toMatchObject({ category: "read", summary: "C:\\p\\a.txt", status: "done" });
    expect(state.items[4]).toEqual({ kind: "turn", durationMs: 2000, input: 21586, output: 97, cost: 0 });
  });

  it("shows a stopped turn as stopped, not as an error", () => {
    const state = feed(
      new AgentStream("opencode"),
      { type: "godesk.prompt", text: "cuenta", at: 1 },
      { type: "tool_use", part: { tool: "bash", callID: "c", state: { status: "running", input: { command: "sleep 9" } } } },
      { type: "godesk.interrupt", at: 2 },
      { type: "godesk.turn_end", code: 1, at: 3 },
    );
    expect(tools(state.items)[0]?.status).toBe("error");
    const notices = state.items.filter((i) => i.kind === "notice");
    expect(notices).toEqual([{ kind: "notice", text: "Stopped. The conversation goes on from here.", tone: "info" }]);
  });
});

describe("a tool's summary", () => {
  it("is the command, the file or the pattern", () => {
    expect(toolSummary("Bash", { command: "cargo test" })).toBe("cargo test");
    expect(toolSummary("Edit", { file_path: "a.rs" })).toBe("a.rs");
    expect(toolSummary("Grep", { pattern: "fn main", path: "src" })).toBe("fn main");
    expect(toolSummary("edit", { filePath: "b.rs" })).toBe("b.rs");
    expect(toolSummary("Whatever", {})).toBe("");
  });

  it("is the command without the shell Codex wrapped it in", () => {
    expect(unwrapCommand('"C:\\\\pwsh\\\\powershell.exe" -Command "ls -Force"')).toBe("ls -Force");
    expect(unwrapCommand("bash -lc 'npm test'")).toBe("'npm test'");
    expect(unwrapCommand("npm test")).toBe("npm test");
  });
});

describe("the small things the view shows", () => {
  it("diffs only the lines that changed", () => {
    expect(lineDiff("", "a\nb")).toEqual([
      { kind: "add", text: "a" },
      { kind: "add", text: "b" },
    ]);
    expect(lineDiff("x\ny", "x\ny")).toEqual([]);
  });

  it("shortens folders, paths, tokens and times", () => {
    expect(shortPath("C:\\Users\\Jesus\\Documents\\godesk")).toBe("…/Documents/godesk");
    expect(relativePath("C:\\work\\godesk\\src\\a.rs", "C:\\work\\godesk")).toBe("src/a.rs");
    expect(relativePath("D:\\other\\a.rs", "C:\\work")).toBe("D:\\other\\a.rs");
    expect(formatTokens(12300, 2100)).toBe("12.3k in · 2.1k out");
    expect(formatDuration(850)).toBe("850ms");
    expect(formatDuration(185_000)).toBe("3m 05s");
  });
});

describe("MiMo Code's stream", () => {
  it("is OpenCode's: the same events, the same conversation", () => {
    // What `mimo run --format json` wrote on this machine.
    const state = feed(
      new AgentStream("mimo"),
      { type: "godesk.prompt", text: "di solo: hola", at: 1000 },
      { type: "step_start", sessionID: "ses_ffe5", part: { type: "step-start" } },
      { type: "text", sessionID: "ses_ffe5", part: { id: "prt_1", type: "text", text: "¡Hola!" } },
      {
        type: "step_finish",
        sessionID: "ses_ffe5",
        part: { reason: "stop", tokens: { input: 15007, output: 5, reasoning: 65, cache: { write: 0, read: 10240 } }, cost: 0.0066 },
      },
      { type: "godesk.turn_end", code: 0, at: 2500 },
    );
    expect(state.sessionId).toBe("ses_ffe5");
    expect(state.items.map((i) => i.kind)).toEqual(["prompt", "text", "turn"]);
    expect(state.status).toBe("idle");
  });
});

describe("Godesk starting again", () => {
  it("ends the turn that was going on and says the conversation goes on", () => {
    const state = feed(
      new AgentStream("codex"),
      { type: "godesk.prompt", text: "arregla el build", at: 1 },
      { type: "item.started", item: { id: "i1", type: "command_execution", command: "cargo build", status: "in_progress" } },
      { type: "godesk.restart", reopened: true, at: 9 },
    );
    expect(state.status).toBe("idle");
    expect(tools(state.items)[0]?.status).toBe("error");
    expect(state.items.at(-1)).toMatchObject({ kind: "notice", text: expect.stringContaining("Godesk started again") });
  });
});

describe("Claude Code asking for a permission", () => {
  const asked = {
    type: "control_request",
    request_id: "d217",
    request: {
      subtype: "can_use_tool",
      tool_name: "Bash",
      display_name: "Bash",
      input: { command: 'godesk send "Mimo" "hola"' },
      permission_suggestions: [{ type: "addRules", behavior: "allow" }],
      decision_reason: "This command requires approval",
    },
  };

  it("shows what the agent waits for, and how the user answered", () => {
    const stream = new AgentStream("claude");
    let state = feed(stream, { type: "godesk.prompt", text: "manda hola a Mimo", at: 1 }, asked);
    expect(state.items[1]).toMatchObject({
      kind: "permission",
      id: "d217",
      summary: 'godesk send "Mimo" "hola"',
      canAlways: true,
      status: "pending",
    });
    state = feed(stream, { type: "godesk.permission", request_id: "d217", decision: "always", at: 2 });
    expect(state.items[1]).toMatchObject({ status: "always" });
  });

  it("stops asking when the turn ends or is stopped", () => {
    const ended = feed(new AgentStream("claude"), asked, { type: "result", usage: {} });
    expect(ended.items[0]).toMatchObject({ status: "expired" });
    const stopped = feed(new AgentStream("claude"), asked, { type: "godesk.interrupt", at: 3 });
    expect(stopped.items[0]).toMatchObject({ status: "expired" });
    const cancelled = feed(new AgentStream("claude"), asked, { type: "control_cancel_request", request_id: "d217" });
    expect(cancelled.items[0]).toMatchObject({ status: "expired" });
  });
});
