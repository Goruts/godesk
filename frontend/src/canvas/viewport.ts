import type { Camera } from "../bindings/Camera";
import type { Rect } from "../bindings/Rect";
import { type Point, clampZoom, screenToWorld } from "./camera";

export type CameraChange = { workspaceId: string; camera: Camera; fromUser: boolean };
type Listener = (change: CameraChange) => void;

/** Where the canvas element sits in the window, in client pixels. */
export type CanvasBounds = { left: number; top: number; width: number; height: number };

/**
 * Live camera of the visible workspace. It lives outside React: panning and
 * zooming write the transform straight to the DOM through subscribers, and
 * the store only receives the (debounced) result for persistence.
 */
class Viewport {
  private camera: Camera = { x: 0, y: 0, zoom: 1 };
  private workspaceId = "";
  private bounds: CanvasBounds = { left: 0, top: 0, width: 0, height: 0 };
  private readonly listeners = new Set<Listener>();

  get(): Camera {
    return this.camera;
  }

  get zoom(): number {
    return this.camera.zoom;
  }

  getBounds(): CanvasBounds {
    return this.bounds;
  }

  setBounds(bounds: CanvasBounds): void {
    this.bounds = bounds;
  }

  /** Shows another workspace's saved camera (not a user change). */
  load(workspaceId: string, camera: Camera): void {
    this.workspaceId = workspaceId;
    this.apply({ ...camera, zoom: clampZoom(camera.zoom) }, false);
  }

  /** A pan/zoom made by the user. */
  set(camera: Camera): void {
    this.apply({ ...camera, zoom: clampZoom(camera.zoom) }, true);
  }

  /** Converts client (window) coordinates to canvas-element coordinates. */
  clientToScreen(clientX: number, clientY: number): Point {
    return { x: clientX - this.bounds.left, y: clientY - this.bounds.top };
  }

  clientToWorld(clientX: number, clientY: number): Point {
    return screenToWorld(this.camera, this.clientToScreen(clientX, clientY));
  }

  /** Canvas point at the center of the visible area. */
  center(): Point {
    return screenToWorld(this.camera, { x: this.bounds.width / 2, y: this.bounds.height / 2 });
  }

  /** The canvas area on screen, in canvas units. */
  visibleRect(): Rect {
    const topLeft = screenToWorld(this.camera, { x: 0, y: 0 });
    const bottomRight = screenToWorld(this.camera, { x: this.bounds.width, y: this.bounds.height });
    return { x: topLeft.x, y: topLeft.y, width: bottomRight.x - topLeft.x, height: bottomRight.y - topLeft.y };
  }

  subscribe(listener: Listener): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  private apply(camera: Camera, fromUser: boolean): void {
    this.camera = camera;
    const change = { workspaceId: this.workspaceId, camera, fromUser };
    for (const listener of this.listeners) listener(change);
  }
}

export const viewport = new Viewport();
