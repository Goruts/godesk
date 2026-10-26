import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import type { FormEvent, KeyboardEvent } from "react";
import { AGENTS, AgentStream, agentId, formatDuration, formatTokens, shortPath } from "../agent/protocol";
import type { AgentId, AgentState } from "../agent/protocol";
import { api, errorMessage, events } from "../lib/api";
import { encodeBase64 } from "../lib/base64";
import { PLATFORM } from "../lib/platform";
import { type Recorder, listMicrophones, startRecording } from "../lib/recorder";
import { useAppStore } from "../store/appStore";
import { AgentEntry } from "./AgentEntry";
import { agentInputs, takePendingFocus } from "./TerminalView";

type Props = { terminalId: string; agent: string | null | undefined };

/** How close to the end the log must be to follow what comes next. */
const FOLLOW_SLACK = 64;

/** What a message the user writes asks of Godesk's view instead of the agent. */
export type CommandKind = "model" | "shell" | "classic" | "restart" | "stop" | "usage" | "status" | "help";
export type ViewCommand = { kind: "model"; model: string | null } | { kind: Exclude<CommandKind, "model"> } | null;

/**
 * Every command Godesk's agent view understands, as a CLI would. These are the
 * same for every provider (Claude Code, Codex, OpenCode, MiMo), because Godesk
 * acts on them itself; anything else typed is sent to the agent as a message.
 * The first name is the one shown; the rest are accepted aliases.
 */
export const COMMANDS: ReadonlyArray<{ names: string[]; kind: CommandKind; arg: string; desc: string }> = [
  { names: ["/model"], kind: "model", arg: "<name>", desc: "Switch the model · empty resets to the default" },
  { names: ["/usage", "/tokens"], kind: "usage", arg: "", desc: "Tokens and cost of this session" },
  { names: ["/status"], kind: "status", arg: "", desc: "Model, folder and session" },
  { names: ["/stop"], kind: "stop", arg: "", desc: "Stop the turn being worked on" },
  { names: ["/restart"], kind: "restart", arg: "", desc: "Restart the agent on the same conversation" },
  { names: ["/classic"], kind: "classic", arg: "", desc: "Open the agent on its own screen" },
  { names: ["/exit", "/quit", "/shell"], kind: "shell", arg: "", desc: "Close the agent, back to the shell" },
  { names: ["/help"], kind: "help", arg: "", desc: "List the commands" },
];

/** What the message asks of the view, or null when it is a message for the agent. */
export function viewCommand(text: string): ViewCommand {
  const [word, ...rest] = text.trim().split(/\s+/);
  const spec = COMMANDS.find((c) => c.names.includes((word ?? "").toLowerCase()));
  if (!spec) return null;
  if (spec.kind === "model") return rest.length <= 1 ? { kind: "model", model: rest[0] ?? null } : null;
  return rest.length === 0 ? { kind: spec.kind } : null;
}

/** The language to dictate in, from the browser's locale ("es-ES" → "es"). */
const VOICE_LANG = typeof navigator !== "undefined" ? navigator.language || "auto" : "auto";

/** The chosen microphone, remembered across terminals and restarts. */
const MIC_KEY = "godesk.mic";
function savedMic(): string {
  try {
    return localStorage.getItem(MIC_KEY) ?? "";
  } catch {
    return "";
  }
}
function rememberMic(id: string): void {
  try {
    localStorage.setItem(MIC_KEY, id);
  } catch {
    // Private mode: the choice just will not persist.
  }
}

/** One row of the autocomplete menu: a Godesk view command, or one the agent
 * itself reports (passed through to it when run). */
type MenuCommand = {
  cmd: string;
  /** Right-hand hint: an argument placeholder, or which agent runs it. */
  hint: string;
  desc: string;
  /** Selecting it fills `cmd ` and waits (an argument may follow) rather than
   * running at once. */
  fill: boolean;
  /** It must carry an argument before it runs (only `/model`). */
  requireArg: boolean;
  /** It is the agent's own command, so it is sent to the agent, not handled here. */
  agent: boolean;
};

/** Things to ask an agent that has not been asked anything yet. */
const SUGGESTIONS = [
  "Explain how this project is organized",
  "Find bugs in the latest changes",
  "Write tests for the code without them",
];

function escapeRegExp(text: string): string {
  return text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

/** A name matches the typed word forwards (`/mo` → `/model`) or just past it
 * (`/models` → `/model`), so a near miss still points at the real command. */
function nameMatches(name: string, word: string): boolean {
  return name.startsWith(word) || word.startsWith(name);
}

/**
 * The commands offered for `draft`: Godesk's own view commands first, then the
 * agent's own commands (those it reported and Godesk does not already handle),
 * so the menu covers every command the provider has, not just a fixed few.
 */
function matchingCommands(draft: string, agentCommands: string[], agentLabel: string): MenuCommand[] {
  if (!/^\/\S*$/.test(draft)) return [];
  const word = draft.toLowerCase();
  const own = COMMANDS.filter((c) => c.names.some((n) => nameMatches(n, word))).map((c) => ({
    cmd: c.names[0] as string,
    hint: c.arg,
    desc: c.desc,
    fill: c.kind === "model",
    requireArg: c.kind === "model",
    agent: false,
  }));
  const ownNames = new Set(COMMANDS.flatMap((c) => c.names));
  const fromAgent = agentCommands
    .filter((n) => !ownNames.has(n) && nameMatches(n, word))
    .map((n) => ({ cmd: n, hint: `${agentLabel} command`, desc: "", fill: true, requireArg: false, agent: true }));
  return [...own, ...fromAgent];
}

/** Whether `draft` already holds `cmd` as a whole word (so an argument may follow). */
function commandTyped(draft: string, cmd: string): boolean {
  return new RegExp(`^${escapeRegExp(cmd)}(?=\\s|$)`, "i").test(draft);
}

/** The agent's own mark, drawn in the text color. */
export function AgentMark({ agent, size = 16 }: { agent: AgentId; size?: number }) {
  return (
    <svg className="agent-mark" data-agent={agent} viewBox="0 0 24 24" width={size} height={size} aria-hidden="true">
      {agent === "claude" ? (
        <path
          fill="currentColor"
          d="M12 2.5c.55 0 .9.4.95.95l.55 5.4 3.6-4.05c.37-.42.9-.45 1.28-.08.38.38.35.92-.07 1.29l-4.05 3.6 5.4.55c.55.05.94.4.94.94s-.4.9-.94.95l-5.4.55 4.05 3.6c.42.37.45.9.07 1.28-.38.38-.91.35-1.28-.07l-3.6-4.05-.55 5.4c-.05.55-.4.94-.95.94s-.9-.4-.95-.94l-.55-5.4-3.6 4.05c-.37.42-.9.45-1.28.07-.38-.38-.35-.91.07-1.28l4.05-3.6-5.4-.55C2.9 12.9 2.5 12.55 2.5 12s.4-.89.95-.94l5.4-.55L4.8 6.9c-.42-.37-.45-.9-.07-1.28.38-.38.91-.35 1.28.07l3.6 4.05.55-5.4c.05-.55.4-.95.95-.95z"
        />
      ) : agent === "codex" ? (
        <g fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round">
          <rect x="2.75" y="3.75" width="18.5" height="16.5" rx="4.5" />
          <path d="m7.5 9.5 3 2.5-3 2.5M12.5 15h4" />
        </g>
      ) : (
        <g fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinejoin="round">
          <rect x="4" y="3" width="16" height="18" rx="2" />
          <rect x="8" y="7" width="8" height="10" rx="1" fill="currentColor" fillOpacity="0.35" />
        </g>
      )}
    </svg>
  );
}

/** A microphone, for the dictation button. */
function MicIcon() {
  return (
    <svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <rect x="9" y="3" width="6" height="11" rx="3" />
      <path d="M5 11a7 7 0 0 0 14 0M12 18v3" />
    </svg>
  );
}

/** The time now, ticking each second while `active`. */
function useNow(active: boolean): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!active) return;
    setNow(Date.now());
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [active]);
  return now;
}

/**
 * A terminal that shows a coding agent: the agent its shell started runs in
 * its machine-readable mode and Godesk draws its work — what it thinks, says,
 * runs and changes — instead of the agent's own screen. Claude Code, Codex,
 * OpenCode and MiMo Code look the same. The shell stays under it; `/exit`
 * goes back to it.
 */
export function AgentTerminal({ terminalId, agent: agentName }: Props) {
  const agent = agentId(agentName);
  const meta = AGENTS[agent];
  const [state, setState] = useState<AgentState>(() => new AgentStream(agent).current);
  const [draft, setDraft] = useState("");
  const [error, setError] = useState<string | null>(null);
  /** What Godesk itself says about a command of the view (`/model`). */
  const [info, setInfo] = useState<string | null>(null);
  const [following, setFollowing] = useState(true);
  /** The command-autocomplete menu: which row is highlighted, and whether the
   * user dismissed it for the current draft. */
  const [cmdSel, setCmdSel] = useState(0);
  const [cmdDismissed, setCmdDismissed] = useState(false);
  /** Voice input: idle, recording the mic, or busy (downloading/transcribing). */
  const [voice, setVoice] = useState<"idle" | "recording" | "busy">("idle");
  const [voiceNote, setVoiceNote] = useState<string | null>(null);
  const [micMenu, setMicMenu] = useState(false);
  const [mics, setMics] = useState<{ id: string; label: string }[]>([]);
  const [micId, setMicId] = useState(savedMic);
  const recorderRef = useRef<Recorder | null>(null);
  /** What the draft held before recording; transcribed text is added after it. */
  const voiceBase = useRef("");
  /** The live re-transcription timer, and a guard against overlapping requests. */
  const streamTimer = useRef<number | undefined>(undefined);
  const transcribing = useRef(false);
  /** LocalAgreement state: words two passes have agreed on (so they are shown
   * and never revised), and the previous pass's full hypothesis to compare. */
  const committedWords = useRef<string[]>([]);
  const prevHypothesis = useRef<string[]>([]);
  const streamRef = useRef(new AgentStream(agent));
  const sentRef = useRef<string[]>([]);
  const logRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const followRef = useRef(true);
  const notify = useAppStore((s) => s.notify);
  const detachAgent = useAppStore((s) => s.detachAgent);
  const agentToClassic = useAppStore((s) => s.agentToClassic);

  const working = state.status === "working";
  const stopped = state.status === "stopped";
  const now = useNow(working);

  // The agent's stream: what it already said, then what it says next. Lines
  // that arrive while the history is read are held, and those the history
  // already has are skipped.
  useEffect(() => {
    const stream = new AgentStream(agent);
    streamRef.current = stream;
    let disposed = false;
    let seen: number | null = null;
    const held: Array<[string, number]> = [];
    const cleanups: Array<() => void> = [];

    void (async () => {
      const unlistenLine = await events.onAgentLine(terminalId, (line, seq) => {
        if (disposed) return;
        if (seen === null) held.push([line, seq]);
        else if (seq > seen) setState(stream.pushLine(line));
      });
      const unlistenExit = await events.onAgentExit(terminalId, () => {
        if (!disposed) setState(stream.stop());
      });
      if (disposed) {
        unlistenLine();
        unlistenExit();
        return;
      }
      cleanups.push(unlistenLine, unlistenExit);
      try {
        const history = await api.agentOutput(terminalId);
        if (disposed) return;
        for (const line of history.text.split("\n")) if (line.trim()) stream.pushLine(line);
        seen = history.lines;
        for (const [line, seq] of held) if (seq > history.lines) stream.pushLine(line);
        held.length = 0;
        sentRef.current = stream.current.items.flatMap((i) => (i.kind === "prompt" ? [i.text] : []));
        setState(stream.current);
        const folder = await api.agentFolder(terminalId).catch(() => null);
        if (!disposed && folder) setState((s) => (s.cwd ? s : { ...s, cwd: folder }));
        // A one-turn agent has no process until asked: it is ready now.
        if (!disposed && agent !== "claude" && stream.current.status === "starting") {
          setState((s) => ({ ...s, status: "idle" }));
        }
      } catch (e) {
        if (!disposed) setError(errorMessage(e));
      }
    })();

    return () => {
      disposed = true;
      for (const cleanup of cleanups.reverse()) cleanup();
    };
  }, [terminalId, agent]);

  // Take the keyboard in the composer when this view opens — a new terminal,
  // a takeover of the shell, or a click on the window — so you can type at
  // once. It does not steal focus from another terminal mid-use (e.g. a bulk
  // restore on start): it only focuses when nothing else is focused, the focus
  // is already inside this window, or focus was explicitly requested.
  useEffect(() => {
    const focus = () => inputRef.current?.focus();
    agentInputs.set(terminalId, focus);
    const active = document.activeElement;
    const here = inputRef.current?.closest(".term-window")?.contains(active) ?? false;
    if (takePendingFocus(terminalId) || !active || active === document.body || here) focus();
    return () => {
      if (agentInputs.get(terminalId) === focus) agentInputs.delete(terminalId);
    };
  }, [terminalId]);

  // Follow the newest line, unless the user scrolled up to read.
  useLayoutEffect(() => {
    const log = logRef.current;
    if (log && followRef.current) log.scrollTop = log.scrollHeight;
  }, [state, error]);

  const onScroll = () => {
    const log = logRef.current;
    if (!log) return;
    const follow = log.scrollHeight - log.scrollTop - log.clientHeight < FOLLOW_SLACK;
    followRef.current = follow;
    if (follow !== following) setFollowing(follow);
  };

  const jumpToEnd = () => {
    const log = logRef.current;
    if (!log) return;
    followRef.current = true;
    setFollowing(true);
    log.scrollTo({ top: log.scrollHeight, behavior: "smooth" });
  };

  /** The field grows with what is written in it, up to its own height. */
  const grow = (field: HTMLTextAreaElement | null) => {
    if (!field) return;
    field.style.height = "auto";
    field.style.height = `${Math.min(field.scrollHeight, 180)}px`;
  };

  const setDraftText = (text: string) => {
    setDraft(text);
    requestAnimationFrame(() => {
      grow(inputRef.current);
      inputRef.current?.focus();
    });
  };

  const showTranscript = (text: string) => {
    const base = voiceBase.current;
    const spoken = text.trim();
    setDraftText(base && spoken ? `${base} ${spoken}` : spoken || base);
  };

  // A live pass over the audio so far, while still recording. To avoid the
  // "types it then rewrites it" flicker, only words that two consecutive passes
  // agree on are shown (LocalAgreement): the text grows and is never revised,
  // lagging the spoken word by about one tick. Overlapping requests are skipped.
  const streamInterim = async () => {
    const recorder = recorderRef.current;
    if (!recorder || transcribing.current) return;
    transcribing.current = true;
    try {
      const text = await api.transcribe(encodeBase64(recorder.snapshot()), VOICE_LANG);
      if (!recorderRef.current) return;
      const hyp = text.split(/\s+/).filter(Boolean);
      let agreed = 0;
      while (agreed < hyp.length && agreed < prevHypothesis.current.length && hyp[agreed] === prevHypothesis.current[agreed]) {
        agreed++;
      }
      // Commit the agreed prefix, never shrinking it, so shown text is stable.
      if (agreed > committedWords.current.length) committedWords.current = hyp.slice(0, agreed);
      prevHypothesis.current = hyp;
      showTranscript(committedWords.current.join(" "));
    } catch {
      // Interim failures are ignored; the final pass reports any real error.
    } finally {
      transcribing.current = false;
    }
  };

  const stopStream = () => {
    if (streamTimer.current !== undefined) window.clearInterval(streamTimer.current);
    streamTimer.current = undefined;
  };

  const toggleVoice = async () => {
    if (voice === "busy") return;
    if (voice === "recording") {
      stopStream();
      const recorder = recorderRef.current;
      recorderRef.current = null;
      if (!recorder) {
        setVoice("idle");
        return;
      }
      const wav = await recorder.stop();
      // Final pass, downloading the model the first time (then it is kept).
      setVoice("busy");
      try {
        if (!(await api.whisperReady())) {
          setVoiceNote("Preparing offline voice (one-time download)…");
          await api.whisperInstall();
        }
        setVoiceNote("Transcribing…");
        showTranscript(await api.transcribe(encodeBase64(wav), VOICE_LANG));
      } catch (e) {
        setError(errorMessage(e));
      } finally {
        setVoice("idle");
        setVoiceNote(null);
      }
      return;
    }
    setError(null);
    setMicMenu(false);
    voiceBase.current = draft.replace(/\s+$/, "");
    committedWords.current = [];
    prevHypothesis.current = [];
    try {
      recorderRef.current = await startRecording(micId || undefined);
      setVoice("recording");
    } catch {
      setError("Microphone access was blocked, or no microphone is available.");
      return;
    }
    // Live updates only when the server is ready (first use downloads on stop).
    // A failure here must never cancel the recording already under way.
    try {
      if (await api.whisperCanStream()) {
        streamTimer.current = window.setInterval(() => void streamInterim(), 1000);
      }
    } catch {
      // No streaming capability (older backend): the final pass still runs.
    }
  };

  const toggleMicMenu = async () => {
    if (micMenu) {
      setMicMenu(false);
      return;
    }
    setMics(await listMicrophones());
    setMicMenu(true);
  };
  const chooseMic = (id: string) => {
    setMicId(id);
    rememberMic(id);
    setMicMenu(false);
  };

  // Show the one-time download's progress while it runs.
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    void events
      .onWhisperProgress((p) => {
        if (p.phase === "done") return;
        const pct = p.total ? Math.round(((p.received ?? 0) / p.total) * 100) : 0;
        setVoiceNote(`Downloading voice ${p.phase === "model" ? "model" : "engine"}… ${pct}%`);
      })
      .then((u) => (unlisten = u));
    return () => unlisten?.();
  }, []);

  // Release the microphone and stop the timer if the view goes away.
  useEffect(() => {
    return () => {
      if (streamTimer.current !== undefined) window.clearInterval(streamTimer.current);
      void recorderRef.current?.stop();
    };
  }, []);

  // Claude Code takes a message while it works (it waits its turn); a
  // one-turn agent has to finish or be stopped first. Going back to the
  // shell is always possible.
  // A command of the view always goes through; a message waits its turn.
  const canSend = (!stopped && (!working || agent === "claude")) || viewCommand(draft) !== null;

  const cmdMatches = matchingCommands(draft, state.commands, meta.label);
  const cmdOpen = cmdMatches.length > 0 && !cmdDismissed;
  const cmdSelected = Math.min(cmdSel, Math.max(0, cmdMatches.length - 1));

  /** Picks a command from the menu: ones that may take an argument are filled
   * in and wait for it; the rest run straight away. */
  const pickCommand = (choice: MenuCommand) => {
    setCmdDismissed(true);
    if (choice.fill) setDraftText(`${choice.cmd} `);
    else void send(choice.cmd);
  };

  const usageText = () => {
    if (!state.input && !state.output && !state.turns) return "No tokens used yet.";
    const parts = [formatTokens(state.input, state.output)];
    if (state.cost) parts.push(`$${state.cost.toFixed(state.cost < 0.01 ? 3 : 2)}`);
    if (state.turns) parts.push(`${state.turns} turn${state.turns === 1 ? "" : "s"}`);
    return parts.join(" · ");
  };

  const statusText = () => {
    const parts = [meta.label];
    if (state.model) parts.push(state.model);
    if (state.cwd) parts.push(shortPath(state.cwd));
    parts.push(statusLabel(state.status));
    if (state.sessionId) parts.push(`session ${state.sessionId.slice(0, 8)}`);
    return parts.join(" · ");
  };

  const helpText = () => COMMANDS.map((c) => `${c.names.join(", ")}${c.arg ? ` ${c.arg}` : ""} — ${c.desc}`).join("\n");

  const send = async (text: string) => {
    const message = text.trim();
    if (!message) return;
    setDraftText("");
    setError(null);
    setInfo(null);
    const command = viewCommand(message);
    if (command) {
      try {
        switch (command.kind) {
          case "shell":
            await detachAgent(terminalId);
            break;
          case "classic":
            await agentToClassic(terminalId);
            break;
          case "restart":
            setInfo(`Restarting ${meta.label}…`);
            await api.restartAgent(terminalId);
            break;
          case "stop":
            await api.interruptAgent(terminalId);
            break;
          case "model":
            await api.setAgentModel(terminalId, command.model);
            setInfo(
              command.model
                ? `${meta.label} works with ${command.model} from the next message.`
                : `${meta.label} works with its default model from the next message.`,
            );
            break;
          case "usage":
            setInfo(usageText());
            break;
          case "status":
            setInfo(statusText());
            break;
          case "help":
            setInfo(helpText());
            break;
        }
      } catch (e) {
        setError(errorMessage(e));
      }
      return;
    }
    sentRef.current = [...sentRef.current, message];
    followRef.current = true;
    setFollowing(true);
    try {
      await api.sendAgentMessage(terminalId, message);
    } catch (e) {
      setError(errorMessage(e));
      notify(errorMessage(e));
    }
  };

  const interrupt = useCallback(async () => {
    try {
      await api.interruptAgent(terminalId);
    } catch (e) {
      notify(errorMessage(e));
    }
  }, [terminalId, notify]);

  const answerPermission = useCallback(
    (requestId: string, decision: "allow" | "always" | "deny") => {
      api.answerAgentPermission(terminalId, requestId, decision).catch((e: unknown) => notify(errorMessage(e)));
    },
    [terminalId, notify],
  );

  const restart = async () => {
    setError(null);
    try {
      await api.restartAgent(terminalId);
    } catch (e) {
      setError(errorMessage(e));
    }
  };

  const onSubmit = (e: FormEvent) => {
    e.preventDefault();
    if (canSend) void send(draft);
  };

  const onKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    // The command-autocomplete menu takes the navigation keys first.
    if (cmdOpen) {
      const choice = cmdMatches[cmdSelected];
      if (e.key === "ArrowDown") {
        e.preventDefault();
        setCmdSel((i) => (Math.min(i, cmdMatches.length - 1) + 1) % cmdMatches.length);
        return;
      }
      if (e.key === "ArrowUp") {
        e.preventDefault();
        setCmdSel((i) => (Math.min(i, cmdMatches.length - 1) + cmdMatches.length - 1) % cmdMatches.length);
        return;
      }
      if (e.key === "Tab" && choice) {
        e.preventDefault();
        setCmdDismissed(!choice.fill);
        setDraftText(choice.fill ? `${choice.cmd} ` : choice.cmd);
        return;
      }
      if (e.key === "Escape") {
        e.preventDefault();
        setCmdDismissed(true);
        return;
      }
      if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing && choice) {
        e.preventDefault();
        const full = commandTyped(draft, choice.cmd);
        const hasArg = full && draft.slice(choice.cmd.length).trim().length > 0;
        if (!full) {
          // Finish typing it; a command that may take an argument keeps a space.
          if (choice.fill) setDraftText(`${choice.cmd} `);
          else void send(choice.cmd);
        } else if (choice.requireArg && !hasArg) {
          setDraftText(`${choice.cmd} `);
        } else {
          // Agent commands and ones with an argument send what was typed; a
          // plain view command sends its canonical name.
          void send(choice.agent || choice.requireArg ? draft : choice.cmd);
        }
        return;
      }
    }
    if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
      e.preventDefault();
      if (canSend) void send(draft);
      return;
    }
    if (e.key === "Escape" && working) {
      e.preventDefault();
      e.stopPropagation();
      void interrupt();
      return;
    }
    // The last prompt again, as in a shell.
    if (e.key === "ArrowUp" && !draft) {
      const last = sentRef.current[sentRef.current.length - 1];
      if (last) {
        e.preventDefault();
        setDraftText(last);
      }
    }
  };

  const elapsed = working && state.turnStartedAt ? formatDuration(Math.max(0, now - state.turnStartedAt)) : null;
  // Thinking or writing shows itself; between those, say the agent is busy.
  const last = state.items[state.items.length - 1];
  const showsItself =
    ((last?.kind === "text" || last?.kind === "thinking") && last.live) ||
    (last?.kind === "tool" && last.status === "running") ||
    state.items.some((i) => i.kind === "permission" && i.status === "pending");

  return (
    <div className="agent-view" data-agent={agent} data-status={state.status}>
      <div className="agent-head">
        <span className="agent-badge">
          <AgentMark agent={agent} />
        </span>
        <div className="agent-ident">
          <span className="agent-name">{meta.label}</span>
          {state.model ? <span className="agent-model">{state.model}</span> : null}
        </div>
        {state.cwd ? (
          <span className="agent-cwd" title={state.cwd}>
            {shortPath(state.cwd)}
          </span>
        ) : null}
        <span className="agent-status" data-status={state.status}>
          <span className="agent-status-dot" aria-hidden="true" />
          {statusLabel(state.status)}
          {elapsed ? <span className="agent-status-time">{elapsed}</span> : null}
        </span>
        <button
          type="button"
          className="agent-detach"
          title={`Open ${meta.label} on its own screen in this terminal, in the same conversation`}
          onClick={() => void agentToClassic(terminalId)}
        >
          Classic
        </button>
        <button
          type="button"
          className="agent-detach"
          title="Close the agent and go back to the shell (/exit)"
          onClick={() => void detachAgent(terminalId)}
        >
          <span aria-hidden="true">←</span> Shell
        </button>
      </div>

      <div className="agent-log" ref={logRef} onScroll={onScroll}>
        <div className="agent-thread">
          {state.items.length === 0 ? (
            <div className="agent-welcome">
              <span className="agent-welcome-mark">
                <AgentMark agent={agent} size={30} />
              </span>
              <p className="agent-welcome-title">
                {state.status === "starting" ? `Starting ${meta.label}…` : `${meta.label} is ready`}
              </p>
              <p className="agent-welcome-text">
                It works in <code>{shortPath(state.cwd) || "this folder"}</code>. You will see what it thinks, the
                commands it runs and the code it changes.
              </p>
              <div className="agent-suggestions">
                {SUGGESTIONS.map((s) => (
                  <button key={s} type="button" className="agent-suggestion" onClick={() => setDraftText(s)}>
                    {s}
                  </button>
                ))}
              </div>
            </div>
          ) : (
            state.items.map((item, i) => (
              <AgentEntry key={i} item={item} agent={agent} cwd={state.cwd} onPermission={answerPermission} />
            ))
          )}
          {working && !showsItself ? (
            <div className="agent-working" aria-live="polite">
              <span className="agent-working-orbit" aria-hidden="true" />
              <span className="agent-shimmer">{state.items.some((i) => i.kind === "tool") ? "Working…" : "Thinking…"}</span>
              {elapsed ? <span className="agent-working-time">{elapsed}</span> : null}
              <span className="agent-working-hint">Esc to stop</span>
            </div>
          ) : null}
          {error ? (
            <div className="agent-notice" data-tone="error" role="alert">
              <span>{error}</span>
            </div>
          ) : null}
          {info ? (
            <div className="agent-notice" data-tone="info" role="status">
              <span>{info}</span>
            </div>
          ) : null}
        </div>
      </div>

      {!following ? (
        <button type="button" className="agent-jump" onClick={jumpToEnd}>
          ↓ Latest
        </button>
      ) : null}

      <form className="agent-composer" onSubmit={onSubmit}>
        {stopped ? (
          <div className="agent-stopped">
            <span>{meta.label} stopped.</span>
            <button type="button" className="agent-restart" onClick={() => void restart()}>
              Restart
            </button>
          </div>
        ) : null}
        {cmdOpen ? (
          <ul className="agent-cmd-menu" role="listbox" aria-label="Commands">
            {cmdMatches.map((c, i) => (
              <li key={c.cmd} role="presentation">
                <button
                  type="button"
                  role="option"
                  aria-selected={i === cmdSelected}
                  className={`agent-cmd-item${i === cmdSelected ? " is-selected" : ""}`}
                  onMouseEnter={() => setCmdSel(i)}
                  onMouseDown={(e) => {
                    // Keep the focus in the textarea, then act on the choice.
                    e.preventDefault();
                    pickCommand(c);
                  }}
                >
                  <span className="agent-cmd-name">
                    {c.cmd}
                    {!c.agent && c.hint ? <span className="agent-cmd-arg"> {c.hint}</span> : null}
                  </span>
                  <span className="agent-cmd-desc">{c.agent ? c.hint : c.desc}</span>
                </button>
              </li>
            ))}
          </ul>
        ) : null}
        {micMenu ? (
          <ul className="agent-cmd-menu" role="listbox" aria-label="Microphones">
            {[{ id: "", label: "Default microphone" }, ...mics].map((m) => (
              <li key={m.id || "default"}>
                <button
                  type="button"
                  role="option"
                  aria-selected={m.id === micId}
                  className={`agent-cmd-item${m.id === micId ? " is-selected" : ""}`}
                  onMouseDown={(e) => {
                    e.preventDefault();
                    chooseMic(m.id);
                  }}
                >
                  <span className="agent-cmd-name">{m.id === micId ? "● " : "○ "}</span>
                  <span className="agent-cmd-desc">{m.label}</span>
                </button>
              </li>
            ))}
          </ul>
        ) : null}
        {voice !== "idle" ? (
          <div className="agent-voice" data-state={voice} aria-live="polite">
            <span className="agent-voice-dot" aria-hidden="true" />
            {voice === "recording" ? "Recording… click the mic to stop" : (voiceNote ?? "Working…")}
          </div>
        ) : null}
        <div className="agent-input">
          <span className="agent-input-caret" aria-hidden="true">
            ›
          </span>
          <textarea
            ref={inputRef}
            className="agent-textarea"
            value={draft}
            onChange={(e) => {
              setDraft(e.target.value);
              setCmdSel(0);
              setCmdDismissed(false);
              grow(e.target);
            }}
            onKeyDown={onKeyDown}
            placeholder={stopped ? `${meta.label} is not running · /exit for the shell` : meta.placeholder}
            aria-label={`Message to ${meta.label}`}
            rows={1}
            spellCheck={false}
          />
          {PLATFORM === "windows" ? (
            <span className="agent-mic-group">
              <button
                type="button"
                className={`agent-mic${voice === "recording" ? " is-listening" : ""}`}
                onClick={() => void toggleVoice()}
                disabled={voice === "busy"}
                title={
                  voice === "recording"
                    ? "Stop and transcribe"
                    : voice === "busy"
                      ? (voiceNote ?? "Working…")
                      : "Dictate — record and transcribe offline"
                }
                aria-label="Dictate"
                aria-pressed={voice === "recording"}
              >
                <MicIcon />
              </button>
              {voice === "idle" ? (
                <button
                  type="button"
                  className="agent-mic-pick"
                  onClick={() => void toggleMicMenu()}
                  title="Choose microphone"
                  aria-label="Choose microphone"
                  aria-expanded={micMenu}
                >
                  ▾
                </button>
              ) : null}
            </span>
          ) : null}
          {working ? (
            <button type="button" className="agent-stop" onClick={() => void interrupt()} title="Stop (Esc)">
              <span className="agent-stop-square" aria-hidden="true" />
              Stop
            </button>
          ) : null}
          {!working || agent === "claude" ? (
            <button type="submit" className="agent-send" disabled={!canSend || !draft.trim()} title="Send (Enter)">
              Send
            </button>
          ) : null}
        </div>
        <div className="agent-foot">
          <span className="agent-keys">
            <kbd>Enter</kbd> send · <kbd>Shift</kbd>+<kbd>Enter</kbd> new line · <kbd>↑</kbd> last prompt ·{" "}
            <kbd>/</kbd> for commands
          </span>
          <span className="agent-stats">
            {state.input || state.output ? <span>{formatTokens(state.input, state.output)}</span> : null}
            {state.cost ? <span>${state.cost.toFixed(state.cost < 0.01 ? 3 : 2)}</span> : null}
            {state.turns ? (
              <span>
                {state.turns} turn{state.turns === 1 ? "" : "s"}
              </span>
            ) : null}
          </span>
        </div>
      </form>
    </div>
  );
}

function statusLabel(status: AgentState["status"]): string {
  switch (status) {
    case "working":
      return "Working";
    case "idle":
      return "Ready";
    case "stopped":
      return "Stopped";
    default:
      return "Starting";
  }
}
