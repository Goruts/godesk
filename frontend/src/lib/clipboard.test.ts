import { invoke } from "@tauri-apps/api/core";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { readClipboardText, writeClipboardText } from "./clipboard";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const web = { readText: vi.fn(async () => "from web"), writeText: vi.fn(async () => undefined) };

beforeEach(() => {
  vi.stubGlobal("navigator", { clipboard: web });
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.clearAllMocks();
});

describe("clipboard", () => {
  it("goes through the Tauri plugin when the build registers it", async () => {
    vi.mocked(invoke).mockResolvedValueOnce("from plugin").mockResolvedValueOnce(null);

    await expect(readClipboardText()).resolves.toBe("from plugin");
    await writeClipboardText("copied");

    expect(invoke).toHaveBeenCalledWith("plugin:clipboard-manager|read_text");
    expect(invoke).toHaveBeenCalledWith("plugin:clipboard-manager|write_text", { text: "copied" });
    expect(web.readText).not.toHaveBeenCalled();
    expect(web.writeText).not.toHaveBeenCalled();
  });

  it("falls back to the web clipboard without the plugin", async () => {
    vi.mocked(invoke).mockRejectedValue("plugin clipboard-manager not found");

    await expect(readClipboardText()).resolves.toBe("from web");
    await writeClipboardText("copied");

    expect(web.writeText).toHaveBeenCalledWith("copied");
  });

  it("rejects when neither can reach the clipboard", async () => {
    vi.mocked(invoke).mockRejectedValue("plugin clipboard-manager not found");
    web.readText.mockRejectedValueOnce(new Error("NotAllowedError"));

    await expect(readClipboardText()).rejects.toThrow("NotAllowedError");
  });
});
