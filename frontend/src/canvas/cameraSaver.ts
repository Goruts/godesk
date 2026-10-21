import type { Camera } from "../bindings/Camera";
import type { CameraChange } from "./viewport";

export type CameraSaver = {
  /** Feed every viewport change here. */
  onChange: (change: CameraChange) => void;
  /** Writes the pending camera now, if any. */
  flush: () => void;
};

/**
 * Debounces saving the cameras the user moves. A pending save belongs to the
 * workspace it was made in, so it is written at once when another camera is
 * loaded (a workspace switch) or another workspace's camera moves: a quick
 * switch neither drops it nor shows a stale camera when coming back.
 */
export function createCameraSaver(save: (workspaceId: string, camera: Camera) => void, delayMs: number): CameraSaver {
  let pending: { workspaceId: string; camera: Camera } | null = null;
  let timer: ReturnType<typeof setTimeout> | undefined;

  const flush = () => {
    clearTimeout(timer);
    if (!pending) return;
    const { workspaceId, camera } = pending;
    pending = null;
    save(workspaceId, camera);
  };

  const onChange = ({ workspaceId, camera, fromUser }: CameraChange) => {
    if (!fromUser || pending?.workspaceId !== workspaceId) flush();
    if (!fromUser || !workspaceId) return;
    pending = { workspaceId, camera };
    clearTimeout(timer);
    timer = setTimeout(flush, delayMs);
  };

  return { onChange, flush };
}
