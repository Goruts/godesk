//! Links are the arrows the user draws between terminals. An arrow is an
//! explicit, two-way permission to talk: `godesk send`/`read` only reach a
//! terminal joined to the caller by one (see [`crate::bridge`]), and deleting
//! the arrow cuts the channel. There is no longer any heuristic detection of
//! who talks to whom; the model keeps only what the user drew (see
//! [`crate::state`]).

/// While an existing arrow is only being refreshed by live traffic,
/// `links-changed` is sent at most once per period of this length, so its
/// pulse stays lively without flooding the frontend.
pub const LINK_ACTIVITY_THROTTLE_MS: u64 = 1_000;
