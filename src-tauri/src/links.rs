//! Links are the arrows between terminals. The user can draw one by hand, and
//! Godesk draws one itself the first time a terminal hands work to another by
//! name with `godesk send` (or reads it with `godesk read`), so the canvas
//! shows who instructs whom (see [`crate::bridge`]). Either way the arrow is
//! saved with the workspace and the user can delete it. There is no heuristic
//! detection from the output; only an explicit `godesk` call draws one.

/// While an existing arrow is only being refreshed by live traffic,
/// `links-changed` is sent at most once per period of this length, so its
/// pulse stays lively without flooding the frontend.
pub const LINK_ACTIVITY_THROTTLE_MS: u64 = 1_000;
