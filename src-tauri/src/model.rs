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
    /// The coding agent this terminal belongs to ("claude"), if any. An agent
    /// terminal runs the agent itself, not a shell, and the frontend draws
    /// what the agent says instead of a terminal screen.
    #[serde(default)]
    #[ts(optional, type = "string | null")]
    pub agent: Option<String>,
}

/// A knowledge base placed on the canvas: a folder of documents the user can
/// connect to a terminal with an arrow, so the agent there can read it on
/// demand. Only its identity and placement live in the model; the folder is
/// indexed and its text cached outside `state.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct KnowledgeBase {
    pub id: String,
    pub name: String,
    /// The folder on disk this node represents. The backend never gives this
    /// path to an agent: the agent reaches the contents only through the
    /// arrow-gated `godesk kb` commands.
    pub path: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    /// Stacking order inside the workspace; higher is in front. Shared with
    /// terminals, so a knowledge base and a terminal never collide in z.
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
    /// Knowledge-base nodes placed on the canvas. Defaulted so a `state.json`
    /// written before this feature still loads.
    #[serde(default)]
    pub knowledge_bases: Vec<KnowledgeBase>,
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

/// Payload of the `knowledge-bases-changed` event: a workspace's full list of
/// knowledge-base nodes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct KnowledgeBasesChangedEvent {
    pub workspace_id: String,
    pub knowledge_bases: Vec<KnowledgeBase>,
}

/// What to open on the canvas as a native window (Windows only).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum AppLaunch {
    /// A new VS Code window, on `folder` if given.
    VsCode { folder: Option<String> },
    /// A File Explorer window on `folder`, or on home.
    Explorer { folder: Option<String> },
    /// A file (or folder) with the program Windows opens it with.
    Open { path: String },
    /// A window that is already open, from [`NativeWindow::handle`].
    Window { handle: String },
}

/// A native window of another program shown on the canvas. Only its identity
/// lives in the backend; where it sits on the canvas is the frontend's.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct EmbeddedApp {
    pub id: String,
    /// The window's own title.
    pub title: String,
    /// The program's file name, such as `Code.exe`.
    pub program: String,
}

/// An open window that can be brought onto the canvas.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct NativeWindow {
    /// Opaque handle to pass back in [`AppLaunch::Window`].
    pub handle: String,
    pub title: String,
    pub program: String,
}

/// Where an embedded app's content goes, in the webview's client (CSS)
/// pixels, and the areas above it that it must leave visible.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AppPlacement {
    pub id: String,
    pub rect: Rect,
    pub visible: bool,
    /// Parts of the page drawn above the app (the dock, menus, windows in
    /// front of it), cut out of it.
    pub cutouts: Vec<Rect>,
}

/// Payload of the `app-changed` event: an embedded window was retitled or
/// closed by its program.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AppChangedEvent {
    pub id: String,
    pub title: String,
    pub closed: bool,
}

/// Payload of the `app-captured` event: a program started in a terminal
/// showed a window, which is now on the canvas.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AppCapturedEvent {
    pub app: EmbeddedApp,
    /// The terminal whose shell started it; the card goes beside it.
    pub terminal_id: String,
}

/// Payload of the `app-moved` event: an embedded window was moved or
/// resized by its own frame; its card should follow.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AppMovedEvent {
    pub id: String,
    /// Where the window now is, in the webview's client (CSS) pixels.
    pub rect: Rect,
}

/// Tokens a model read (prompt, context and cache) and wrote (answers,
/// reasoning, tool calls).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TokenUsage {
    #[ts(type = "number")]
    pub input: u64,
    #[ts(type = "number")]
    pub output: u64,
}

impl std::ops::AddAssign for TokenUsage {
    fn add_assign(&mut self, other: Self) {
        self.input += other.input;
        self.output += other.output;
    }
}

/// A subagent an agent started (Claude Code's Task/Agent tool). It has no
/// terminal of its own; its transcript is shown read-only beside its parent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Subagent {
    pub id: String,
    /// The kind of subagent, such as `Explore`.
    pub kind: String,
    /// What it was asked to do, in a few words.
    pub description: String,
    pub finished: bool,
    pub tokens: TokenUsage,
    /// The end of its transcript as text: what it says and the tools it calls.
    pub lines: Vec<String>,
    /// Unix epoch milliseconds of its last change.
    #[ts(type = "number")]
    pub updated_at: u64,
}

/// The coding agent a terminal runs, read from the agent's session files.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TerminalAgent {
    pub terminal_id: String,
    /// `claude` or `codex`.
    pub agent: String,
    /// The session's tokens, its subagents' included.
    pub tokens: TokenUsage,
    pub subagents: Vec<Subagent>,
}

/// Payload of the `agents-changed` event.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AgentsChangedEvent {
    pub terminals: Vec<TerminalAgent>,
    /// Every agent's tokens together.
    pub total: TokenUsage,
}

/// Payload of the `app-preview` event: what an embedded window showed when it
/// was hidden (zoomed out), for its card to show instead.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AppPreviewEvent {
    pub id: String,
    /// A PNG data URL.
    pub image: String,
}
