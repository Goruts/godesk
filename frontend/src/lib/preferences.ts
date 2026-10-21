/**
 * Per-device view preferences kept in the webview's localStorage. Storage can
 * be missing or throw (private mode, cleared data), so every access falls
 * back to the default instead of failing.
 */
export function readFlag(key: string, fallback: boolean): boolean {
  try {
    const value = window.localStorage.getItem(key);
    return value === null ? fallback : value === "true";
  } catch {
    return fallback;
  }
}

export function writeFlag(key: string, value: boolean): void {
  try {
    window.localStorage.setItem(key, String(value));
  } catch {
    // Not persisted: the choice still applies until the app closes.
  }
}
