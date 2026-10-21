import { describe, expect, it } from "vitest";
import { viewport } from "./viewport";

describe("viewport.visibleRect", () => {
  it("is the canvas area on screen, in canvas units", () => {
    viewport.setBounds({ left: 0, top: 0, width: 1000, height: 800 });
    viewport.load("w1", { x: -200, y: 100, zoom: 2 });

    expect(viewport.visibleRect()).toEqual({ x: 100, y: -50, width: 500, height: 400 });
  });
});
