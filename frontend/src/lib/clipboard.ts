import { invoke } from "@tauri-apps/api/core";

/**
 * System clipboard text. WebKitGTK rejects navigator.clipboard even inside a
 * key press, so Tauri's clipboard-manager plugin is tried first; when this
 * build does not register it, the web API is the fallback.
 */
export async function readClipboardText(): Promise<string> {
  try {
    return await invoke<string>("plugin:clipboard-manager|read_text");
  } catch {
    return navigator.clipboard.readText();
  }
}

export async function writeClipboardText(text: string): Promise<void> {
  try {
    await invoke("plugin:clipboard-manager|write_text", { text });
  } catch {
    await navigator.clipboard.writeText(text);
  }
}
