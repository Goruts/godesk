//! The Godesk domain model. This is the single definition of every type that
//! crosses the IPC boundary; the TypeScript types in `frontend/src/bindings`
//! are generated from it by `ts-rs` (`cargo test export_bindings`).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Current `state.json` schema version.
pub const STATE_VERSION: u32 = 1;

pub const MIN_ZOOM: f64 = 0.1;
pub const MAX_ZOOM: f64 = 4.0;
pub const MIN_TERMINAL_WIDTH: f64 = 320.0;
pub const MIN_TERMINAL_HEIGHT: f64 = 200.0;

/// Viewport of a workspace. A canvas point `p` is drawn on screen at
/// `p * zoom + (x, y)`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Camera {
    pub x: f64,
    pub y: f64,
    pub zoom: f64,
}

impl Default for Camera {
    fn default() -> Self {
        Self { x: 0.0, y: 0.0, zoom: 1.0 }
    }
}

impl Camera {
    /// Returns a copy with a finite position and the zoom clamped to the
    /// supported range.
    pub fn sanitized(self) -> Self {
        let finite = |v: f64| if v.is_finite() { v } else { 0.0 };
        let zoom = if self.zoom.is_finite() { self.zoom.clamp(MIN_ZOOM, MAX_ZOOM) } else { 1.0 };
        Self { x: finite(self.x), y: finite(self.y), zoom }
    }
}

/// A terminal window placed on the canvas. Position and size are in canvas
/// coordinates, never screen coordinates.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TerminalWindow {
    pub id: String,
    pub name: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    /// Stacking order inside the workspace; higher is in front.
    pub z: u32,
}

/// Canvas rectangle used when moving or resizing a terminal.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    /// Returns a copy with finite values and at least the minimum terminal size.
    pub fn sanitized(self) -> Self {
        let finite = |v: f64, fallback: f64| if v.is_finite() { v } else { fallback };
        Self {
            x: finite(self.x, 0.0),
            y: finite(self.y, 0.0),
            width: finite(self.width, MIN_TERMINAL_WIDTH).max(MIN_TERMINAL_WIDTH),
            height: finite(self.height, MIN_TERMINAL_HEIGHT).max(MIN_TERMINAL_HEIGHT),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum LinkKind {
    /// Drawn by the user; persisted in `state.json`.
    Manual,
    /// Inferred from terminal traffic; kept in memory only and expires.
    Detected,
}

/// A directed, purely visual relation between two terminals of the same
/// workspace.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Link {
    pub id: String,
    pub source_id: String,
    pub target_id: String,
    pub kind: LinkKind,
    /// Unix epoch milliseconds of the last observed interaction.
    #[ts(type = "number")]
    pub last_activity_at: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Workspace {
    pub id: String,
    pub name: String,
    pub camera: Camera,
    pub terminals: Vec<TerminalWindow>,
    pub links: Vec<Link>,
}

/// Whole application model. It is both what `get_state` returns and the
/// shape of `state.json` (where only `manual` links are written).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AppSnapshot {
    pub version: u32,
    pub active_workspace_id: String,
    pub workspaces: Vec<Workspace>,
}

/// Result of attaching a view to a terminal session: the retained output to
/// replay and the byte offset it ends at.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AttachResult {
    /// Base64 of the retained output (up to 5000 lines / 1 MB).
    pub data: String,
    /// Total bytes the session has produced up to the end of `data`.
    #[ts(type = "number")]
    pub seq: u64,
    pub alive: bool,
}

/// Payload of the `pty-output-<terminalId>` event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PtyOutputEvent {
    /// Base64 of the raw bytes read from the PTY.
    pub data: String,
    /// Total bytes the session has produced up to the end of this chunk.
    #[ts(type = "number")]
    pub seq: u64,
}

/// Payload of the `pty-exit-<terminalId>` event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PtyExitEvent {
    pub exit_code: Option<u32>,
}

/// Payload of the `links-changed` event: the full link list of a workspace.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LinksChangedEvent {
    pub workspace_id: String,
    pub links: Vec<Link>,
}
