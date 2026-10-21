// @vitest-environment jsdom
import { act } from "react";
import { type Root, createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AttachResult } from "../bindings/AttachResult";
import type { PtyExitEvent } from "../bindings/PtyExitEvent";
import type { PtyOutputEvent } from "../bindings/PtyOutputEvent";
import { api } from "../lib/api";
import { encodeText } from "../lib/base64";
import { readClipboardText, writeClipboardText } from "../lib/clipboard";
import { useAppStore } from "../store/appStore";
import { TerminalView, focusTerminalView, terminalOptions, terminals } from "./TerminalView";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

// Stand-ins for xterm and the Tauri bridge: just enough surface to watch
// what the view sends and writes.
const fake = vi.hoisted(() => {
  class FakeTerminal {
    static instances: FakeTerminal[] = [];
    cols = 80;
    rows = 24;
    written: string[] = [];
    focused = 0;
    disposed = false;
    keyHandler: ((e: KeyboardEvent) => boolean) | null = null;
    onDataListener: ((data: string) => void) | null = null;
    onBinaryListener: ((data: string) => void) | null = null;
    onResizeListener: ((size: { cols: number; rows: number }) => void) | null = null;

    constructor() {
      FakeTerminal.instances.push(this);
    }
    open(parent: HTMLElement) {
      const screen = document.createElement("div");
      screen.className = "xterm-screen";
      parent.append(screen);
    }
    loadAddon() {}
    attachCustomKeyEventHandler(handler: (e: KeyboardEvent) => boolean) {
      this.keyHandler = handler;
    }
    write(data: Uint8Array) {
      this.written.push(new TextDecoder().decode(data));
    }
    focus() {
      this.focused++;
    }
    selection = "";
    pasted: string[] = [];
    hasSelection() {
      return this.selection !== "";
    }
    getSelection() {
      return this.selection;
    }
    paste(text: string) {
      this.pasted.push(text);
    }
    dispose() {
      this.disposed = true;
    }
    onData(listener: (data: string) => void) {
      this.onDataListener = listener;
      return { dispose: () => (this.onDataListener = null) };
    }
    onBinary(listener: (data: string) => void) {
      this.onBinaryListener = listener;
      return { dispose: () => (this.onBinaryListener = null) };
    }
    onResize(listener: (size: { cols: number; rows: number }) => void) {
      this.onResizeListener = listener;
      return { dispose: () => (this.onResizeListener = null) };
    }
  }

  return {
    FakeTerminal,
    webglThrows: false,
    calls: [] as string[],
    output: new Map<string, (e: PtyOutputEvent) => void>(),
    exit: new Map<string, (e: PtyExitEvent) => void>(),
  };
});

vi.mock("@xterm/xterm", () => ({ Terminal: fake.FakeTerminal }));
vi.mock("../lib/clipboard", () => ({
  readClipboardText: vi.fn(async () => "echo pasted"),
  writeClipboardText: vi.fn(async () => undefined),
}));
vi.mock("@xterm/xterm/css/xterm.css", () => ({}));
vi.mock("@xterm/addon-fit", () => ({ FitAddon: class { fit() {} } }));
vi.mock("@xterm/addon-webgl", () => ({
  WebglAddon: class {
    constructor() {
      if (fake.webglThrows) throw new Error("WebGL2 is not available");
    }
    onContextLoss() {}
    dispose() {}
  },
}));
vi.mock("../lib/api", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../lib/api")>()),
  api: {
    attachTerminal: vi.fn(),
    writeTerminal: vi.fn(async () => null),
    resizeTerminal: vi.fn(async () => null),
    restartTerminal: vi.fn(async () => null),
  },
  events: {
    onPtyOutput: vi.fn(async (id: string, handler: (e: PtyOutputEvent) => void) => {
      fake.calls.push("listen:output");
      fake.output.set(id, handler);
      return () => fake.output.delete(id);
    }),
    onPtyExit: vi.fn(async (id: string, handler: (e: PtyExitEvent) => void) => {
      fake.calls.push("listen:exit");
      fake.exit.set(id, handler);
      return () => fake.exit.delete(id);
    }),
  },
}));

const RESIZE_SETTLE_MS = 100;

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((r) => (resolve = r));
  return { promise, resolve };
}

const term = () => fake.FakeTerminal.instances.at(-1)!;
const emitOutput = (text: string, seq: number) => act(() => fake.output.get("t1")!({ data: encodeText(text), seq }));
const attachReply = (text: string, seq: number, alive = true): AttachResult => ({ data: encodeText(text), seq, alive });

let container: HTMLDivElement;
let root: Root;

async function mount(terminalId = "t1") {
  await act(async () => root.render(<TerminalView terminalId={terminalId} />));
}

beforeEach(() => {
  vi.stubGlobal(
    "ResizeObserver",
    class {
      observe() {}
      disconnect() {}
    },
  );
  vi.stubGlobal("requestAnimationFrame", () => 0);
  vi.stubGlobal("cancelAnimationFrame", () => undefined);
  vi.mocked(api.attachTerminal).mockImplementation(async () => {
    fake.calls.push("attach");
    return attachReply("", 0);
  });
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
  fake.FakeTerminal.instances = [];
  fake.webglThrows = false;
  fake.calls = [];
  vi.clearAllMocks();
  vi.unstubAllGlobals();
});

describe("TerminalView", () => {
  it("listens before attaching, replays the buffer once and drops output it already contains", async () => {
    const attach = deferred<AttachResult>();
    vi.mocked(api.attachTerminal).mockImplementation(() => {
      fake.calls.push("attach");
      return attach.promise;
    });
    await mount();
    expect(fake.calls).toEqual(["listen:output", "listen:exit", "attach"]);

    // Output produced while the attach is in flight: the first chunk is in
    // the snapshot (it ends at seq 5), the second is not.
    emitOutput("hello", 5);
    emitOutput(" world", 11);
    await act(async () => attach.resolve(attachReply("hello", 5)));
    emitOutput("!", 12);

    expect(term().written).toEqual(["hello", " world", "!"]);
  });

  it("attaches at the fitted size and reports a resize burst once", async () => {
    await mount();
    expect(api.attachTerminal).toHaveBeenCalledWith("t1", 80, 24);

    act(() => {
      term().onResizeListener!({ cols: 90, rows: 30 });
      term().onResizeListener!({ cols: 100, rows: 40 });
    });
    await new Promise((resolve) => setTimeout(resolve, RESIZE_SETTLE_MS));

    expect(api.resizeTerminal).toHaveBeenCalledTimes(1);
    expect(api.resizeTerminal).toHaveBeenCalledWith("t1", 100, 40);
  });

  it("sends typed text as UTF-8 and binary input byte for byte, both base64", async () => {
    await mount();
    term().onDataListener!("é");
    term().onBinaryListener!("\x80");

    expect(api.writeTerminal).toHaveBeenNthCalledWith(1, "t1", "w6k=");
    expect(api.writeTerminal).toHaveBeenNthCalledWith(2, "t1", "gA==");
  });

  it("keeps the exit notice when the exit event beats the attach reply", async () => {
    const attach = deferred<AttachResult>();
    vi.mocked(api.attachTerminal).mockReturnValue(attach.promise);
    await mount();

    act(() => fake.exit.get("t1")!({ exitCode: 3 }));
    await act(async () => attach.resolve(attachReply("bye", 3, true)));

    expect(container.querySelector(".terminal-overlay")?.textContent).toContain("Process exited with code 3.");
  });

  it("shows the restart button for a terminal whose shell already ended", async () => {
    vi.mocked(api.attachTerminal).mockResolvedValueOnce(attachReply("done", 4, false));
    await mount();

    const restart = container.querySelector<HTMLButtonElement>(".terminal-overlay button")!;
    expect(restart.textContent).toBe("Restart shell");
    await act(async () => restart.click());

    expect(api.restartTerminal).toHaveBeenCalledWith("t1", 80, 24);
    expect(container.querySelector(".terminal-overlay")).toBeNull();
    expect(fake.FakeTerminal.instances).toHaveLength(2);
    expect(fake.FakeTerminal.instances[0]!.disposed).toBe(true);
  });

  it("still opens a working terminal when WebGL is unavailable", async () => {
    fake.webglThrows = true;
    await mount();

    expect(terminals.get("t1")).toBe(term());
    expect(api.attachTerminal).toHaveBeenCalledWith("t1", 80, 24);
  });

  it("takes a focus requested before it mounted", async () => {
    focusTerminalView("t1");
    await mount();
    expect(term().focused).toBe(1);

    focusTerminalView("t1");
    expect(term().focused).toBe(2);
  });

  it("keeps Ctrl+Shift+V from also triggering the webview's own paste", async () => {
    await mount();
    const paste = new KeyboardEvent("keydown", { key: "V", ctrlKey: true, shiftKey: true, cancelable: true });

    expect(term().keyHandler!(paste)).toBe(false);
    expect(paste.defaultPrevented).toBe(true);
  });

  it("Ctrl+Shift+V pastes the clipboard and Ctrl+Shift+C copies the selection", async () => {
    await mount();
    await act(async () => void term().keyHandler!(new KeyboardEvent("keydown", { key: "V", ctrlKey: true, shiftKey: true })));
    term().selection = "ls -la";
    term().keyHandler!(new KeyboardEvent("keydown", { key: "C", ctrlKey: true, shiftKey: true }));

    expect(term().pasted).toEqual(["echo pasted"]);
    expect(writeClipboardText).toHaveBeenCalledWith("ls -la");
  });

  it("says so when the clipboard cannot be read, instead of doing nothing", async () => {
    useAppStore.setState({ toast: null });
    vi.mocked(readClipboardText).mockRejectedValueOnce(new Error("NotAllowedError"));
    await mount();
    await act(async () => void term().keyHandler!(new KeyboardEvent("keydown", { key: "V", ctrlKey: true, shiftKey: true })));

    expect(term().pasted).toEqual([]);
    expect(useAppStore.getState().toast).toBe("Paste failed: NotAllowedError");
  });

  it("stops listening and disposes xterm on unmount", async () => {
    await mount();
    const instance = term();
    act(() => root.render(<></>));

    expect(instance.disposed).toBe(true);
    expect(terminals.has("t1")).toBe(false);
    expect(fake.output.has("t1")).toBe(false);
    expect(fake.exit.has("t1")).toBe(false);
  });
});

describe("terminalOptions", () => {
  it("tells xterm about ConPTY on Windows only", () => {
    expect(terminalOptions("windows").windowsPty).toEqual({ backend: "conpty" });
    expect(terminalOptions("linux").windowsPty).toBeUndefined();
    expect(terminalOptions("macos").windowsPty).toBeUndefined();
  });
});
