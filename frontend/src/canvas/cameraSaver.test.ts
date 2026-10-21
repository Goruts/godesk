import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Camera } from "../bindings/Camera";
import { createCameraSaver } from "./cameraSaver";

const DELAY = 300;
const cam = (x: number): Camera => ({ x, y: 0, zoom: 1 });

let save: ReturnType<typeof vi.fn<(workspaceId: string, camera: Camera) => void>>;

beforeEach(() => {
  vi.useFakeTimers();
  save = vi.fn();
});

afterEach(() => {
  vi.useRealTimers();
});

describe("createCameraSaver", () => {
  it("saves only the last user change once the delay passes", () => {
    const saver = createCameraSaver(save, DELAY);
    saver.onChange({ workspaceId: "a", camera: cam(1), fromUser: true });
    saver.onChange({ workspaceId: "a", camera: cam(2), fromUser: true });
    vi.advanceTimersByTime(DELAY - 1);
    expect(save).not.toHaveBeenCalled();

    vi.advanceTimersByTime(1);
    expect(save).toHaveBeenCalledTimes(1);
    expect(save).toHaveBeenCalledWith("a", cam(2));
  });

  it("never saves a camera that was only loaded", () => {
    const saver = createCameraSaver(save, DELAY);
    saver.onChange({ workspaceId: "a", camera: cam(1), fromUser: false });
    vi.runAllTimers();
    saver.flush();

    expect(save).not.toHaveBeenCalled();
  });

  it("writes the pending camera at once when another workspace is loaded", () => {
    const saver = createCameraSaver(save, DELAY);
    saver.onChange({ workspaceId: "a", camera: cam(1), fromUser: true });
    saver.onChange({ workspaceId: "b", camera: cam(9), fromUser: false });

    expect(save).toHaveBeenCalledWith("a", cam(1));
    vi.runAllTimers();
    expect(save).toHaveBeenCalledTimes(1);
  });

  it("does not let a change in one workspace cancel the pending save of another", () => {
    const saver = createCameraSaver(save, DELAY);
    saver.onChange({ workspaceId: "a", camera: cam(1), fromUser: true });
    saver.onChange({ workspaceId: "b", camera: cam(2), fromUser: true });
    vi.runAllTimers();

    expect(save.mock.calls).toEqual([
      ["a", cam(1)],
      ["b", cam(2)],
    ]);
  });

  it("flush writes what is pending and nothing twice", () => {
    const saver = createCameraSaver(save, DELAY);
    saver.onChange({ workspaceId: "a", camera: cam(1), fromUser: true });
    saver.flush();
    saver.flush();
    vi.runAllTimers();

    expect(save).toHaveBeenCalledTimes(1);
  });
});
