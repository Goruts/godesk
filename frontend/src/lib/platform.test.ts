import { describe, expect, it } from "vitest";
import { detectPlatform, modifierLabel } from "./platform";

const WEBVIEW2 =
  "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36 Edg/131.0.0.0";
const WKWEBVIEW = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko)";
const WEBKITGTK = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Safari/605.1.15";

describe("platform", () => {
  it("tells the three webviews apart by their user agent", () => {
    expect(detectPlatform(WEBVIEW2)).toBe("windows");
    expect(detectPlatform(WKWEBVIEW)).toBe("macos");
    expect(detectPlatform(WEBKITGTK)).toBe("linux");
  });

  it("names the shortcut modifier the way each OS prints it", () => {
    expect(modifierLabel("windows")).toBe("Ctrl");
    expect(modifierLabel("linux")).toBe("Ctrl");
    expect(modifierLabel("macos")).toBe("⌘");
  });
});
