import { describe, expect, it, vi } from "vitest";
import { type CloseSteps, closeWindow } from "./closeGuard";

function steps(running: number, confirmed: boolean): CloseSteps & { order: string[] } {
  const order: string[] = [];
  return {
    order,
    runningPrograms: vi.fn(async () => running),
    confirm: vi.fn(async () => {
      order.push("confirm");
      return confirmed;
    }),
    prepare: vi.fn(async () => order.push("prepare")),
    destroy: vi.fn(async () => order.push("destroy")),
  };
}

describe("closeWindow", () => {
  it("closes at once when nothing runs, saving the terminals first", async () => {
    const s = steps(0, false);
    expect(await closeWindow(s)).toBe(true);
    expect(s.order).toEqual(["prepare", "destroy"]);
  });

  it("asks when programs run, and stays open if the person keeps them", async () => {
    const s = steps(2, false);
    expect(await closeWindow(s)).toBe(false);
    expect(s.confirm).toHaveBeenCalledWith(expect.objectContaining({ title: "Close Godesk?" }));
    expect(vi.mocked(s.confirm).mock.calls[0]![0].message).toMatch(/^2 terminals still run programs/);
    expect(s.order).toEqual(["confirm"]);
  });

  it("closes after the person confirms, and even if saving fails", async () => {
    const s = steps(1, true);
    s.prepare = vi.fn(async () => {
      throw new Error("disk full");
    });
    expect(await closeWindow(s)).toBe(true);
    expect(s.destroy).toHaveBeenCalled();
  });
});
