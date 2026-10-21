export type Platform = "windows" | "macos" | "linux";

/** The OS the webview runs on: WebView2 reports "Windows", WKWebView "Mac". */
export function detectPlatform(userAgent: string): Platform {
  if (/Windows/i.test(userAgent)) return "windows";
  if (/Macintosh|Mac OS X/i.test(userAgent)) return "macos";
  return "linux";
}

export const PLATFORM = detectPlatform(typeof navigator === "undefined" ? "" : navigator.userAgent);

/** Label of the primary shortcut modifier: ⌘ on macOS, Ctrl elsewhere. */
export function modifierLabel(platform: Platform): string {
  return platform === "macos" ? "⌘" : "Ctrl";
}

export const MOD_KEY = modifierLabel(PLATFORM);
