//! Heuristic detection of agents interacting with each other.
//!
//! Two signals create or refresh a `detected` link from terminal A to B:
//! 1. **Mention** — A's output contains B's name or id (as a whole word,
//!    case-insensitive, after removing escape sequences).
//! 2. **Relay** — text sent to B as input was produced by A in the last
//!    [`INPUT_CORRELATION_WINDOW_MS`] (for example, the user copies an
//!    answer from one agent and pastes it into another).
//!
//! The detector only reports pairs; link bookkeeping (creation, refresh and
//! expiry after [`DETECTED_LINK_TTL_MS`]) lives in [`crate::state`].

use std::collections::{HashMap, VecDeque};

use crate::text::{AnsiStripper, Utf8Stream, contains_word, normalize, tail};

/// Time without activity after which a `detected` link disappears.
pub const DETECTED_LINK_TTL_MS: u64 = 30_000;
/// How long output stays eligible for the relay signal.
pub const INPUT_CORRELATION_WINDOW_MS: u64 = 4_000;
/// While an existing link is only being refreshed, `links-changed` is sent
/// at most once per period of this length.
pub const LINK_ACTIVITY_THROTTLE_MS: u64 = 1_000;
/// Names shorter than this are ignored by the mention signal.
pub const MIN_NAME_CHARS: usize = 3;
/// Input shorter than this (after normalization) never triggers a relay.
pub const MIN_RELAY_CHARS: usize = 8;
/// Only the last characters of an input line are compared with recent output.
const RELAY_PROBE_CHARS: usize = 64;
/// Output carried between chunks so names split across reads still match.
const OUTPUT_TAIL_BYTES: usize = 256;
/// Upper bound of recent output kept per terminal for the relay signal.
const RECENT_OUTPUT_BYTES: usize = 16 * 1024;
/// Upper bound of an input line being typed.
const INPUT_LINE_BYTES: usize = 4 * 1024;

/// A terminal that may be the other end of a link.
#[derive(Debug, Clone, Copy)]
pub struct Peer<'a> {
    pub id: &'a str,
    pub name: &'a str,
}

#[derive(Debug, Default)]
struct OutputTracker {
    utf8: Utf8Stream,
    ansi: AnsiStripper,
    /// Normalized end of the previous output, used to match across chunks.
    tail: String,
    /// Normalized output of the last few seconds, oldest first.
    recent: String,
    /// `(timestamp, byte length)` of each segment of `recent`.
    segments: VecDeque<(u64, usize)>,
}

impl OutputTracker {
    fn prune(&mut self, now: u64) {
        while let Some(&(at, len)) = self.segments.front() {
            let expired = now.saturating_sub(at) > INPUT_CORRELATION_WINDOW_MS;
            if !expired && self.recent.len() <= RECENT_OUTPUT_BYTES {
                break;
            }
            self.recent.drain(..len);
            self.segments.pop_front();
        }
    }
}

#[derive(Debug, Default)]
struct InputTracker {
    utf8: Utf8Stream,
    ansi: AnsiStripper,
    line: String,
}

#[derive(Debug, Default)]
pub struct LinkDetector {
    outputs: HashMap<String, OutputTracker>,
    inputs: HashMap<String, InputTracker>,
}

/// Normalizes a chunk while keeping one space where it started or ended with
/// whitespace, so consecutive chunks join like the original text.
fn normalize_chunk(text: &str) -> String {
    let core = normalize(text);
    if core.is_empty() {
        return if text.is_empty() { String::new() } else { " ".to_owned() };
    }
    let lead = if text.starts_with(char::is_whitespace) { " " } else { "" };
    let trail = if text.ends_with(char::is_whitespace) { " " } else { "" };
    format!("{lead}{core}{trail}")
}

/// Appends `chunk` to `base` without doubling the space at the seam.
fn join(base: &str, chunk: &str) -> String {
    if base.ends_with(' ') && chunk.starts_with(' ') {
        format!("{base}{}", &chunk[1..])
    } else {
        format!("{base}{chunk}")
    }
}

impl LinkDetector {
    /// Feeds output produced by `source` and returns the ids of the peers it
    /// mentions. `peers` are the terminals of the same workspace.
    pub fn observe_output(&mut self, source: &str, bytes: &[u8], peers: &[Peer], now: u64) -> Vec<String> {
        let tracker = self.outputs.entry(source.to_owned()).or_default();
        let decoded = tracker.utf8.decode(bytes);
        let chunk = normalize_chunk(&tracker.ansi.strip(&decoded));
        if chunk.is_empty() {
            return Vec::new();
        }

        let haystack = join(&tracker.tail, &chunk);
        // Only matches that end in the new chunk count; the tail was already scanned.
        let min_end = tracker.tail.len();
        let mentioned = peers
            .iter()
            .filter(|peer| peer.id != source)
            .filter(|peer| {
                let name = normalize(peer.name);
                let by_name = name.chars().count() >= MIN_NAME_CHARS && contains_word(&haystack, &name, min_end);
                by_name || contains_word(&haystack, &peer.id.to_lowercase(), min_end)
            })
            .map(|peer| peer.id.to_owned())
            .collect();
        tracker.tail = tail(&haystack, OUTPUT_TAIL_BYTES).to_owned();

        let before = tracker.recent.len();
        tracker.recent = join(&tracker.recent, &chunk);
        tracker.segments.push_back((now, tracker.recent.len() - before));
        tracker.prune(now);
        mentioned
    }

    /// Feeds input sent to `target` and returns the ids of the peers that
    /// produced that text recently.
    pub fn observe_input(&mut self, target: &str, bytes: &[u8], peers: &[Peer], now: u64) -> Vec<String> {
        let input = self.inputs.entry(target.to_owned()).or_default();
        let decoded = input.utf8.decode(bytes);
        let stripped = input.ansi.strip(&decoded);

        let mut probes = Vec::new();
        let mut parts = stripped.split('\n').peekable();
        while let Some(part) = parts.next() {
            input.line.push_str(part);
            let probe = normalize(&input.line);
            if probe.chars().count() >= MIN_RELAY_CHARS {
                let skip = probe.chars().count().saturating_sub(RELAY_PROBE_CHARS);
                probes.push(probe.chars().skip(skip).collect::<String>().trim().to_owned());
            }
            if parts.peek().is_some() {
                input.line.clear();
            }
        }
        input.line = tail(&input.line, INPUT_LINE_BYTES).to_owned();

        if probes.is_empty() {
            return Vec::new();
        }
        let mut sources = Vec::new();
        for peer in peers.iter().filter(|peer| peer.id != target) {
            let Some(output) = self.outputs.get_mut(peer.id) else { continue };
            output.prune(now);
            if probes.iter().any(|probe| output.recent.contains(probe.as_str())) {
                sources.push(peer.id.to_owned());
            }
        }
        sources
    }

    /// Drops all state of a terminal that no longer exists.
    pub fn forget(&mut self, terminal_id: &str) {
        self.outputs.remove(terminal_id);
        self.inputs.remove(terminal_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "aaaaaaaa-0000-0000-0000-000000000001";
    const B: &str = "bbbbbbbb-0000-0000-0000-000000000002";
    const C: &str = "cccccccc-0000-0000-0000-000000000003";

    fn peers() -> Vec<Peer<'static>> {
        vec![Peer { id: A, name: "Architect" }, Peer { id: B, name: "Backend" }, Peer { id: C, name: "QA" }]
    }

    #[test]
    fn mention_by_name_is_case_insensitive_and_strips_ansi() {
        let mut d = LinkDetector::default();
        let out = d.observe_output(A, b"Delegating to \x1b[1mBACKEND\x1b[0m now\r\n", &peers(), 0);
        assert_eq!(out, vec![B.to_owned()]);
    }

    #[test]
    fn mention_by_id() {
        let mut d = LinkDetector::default();
        let out = d.observe_output(A, format!("send to {C}").as_bytes(), &peers(), 0);
        assert_eq!(out, vec![C.to_owned()], "QA is too short to match by name, but the id matches");
    }

    #[test]
    fn mention_split_across_chunks_is_reported_once() {
        let mut d = LinkDetector::default();
        assert!(d.observe_output(A, b"ask Back", &peers(), 0).is_empty());
        assert_eq!(d.observe_output(A, b"end please", &peers(), 1), vec![B.to_owned()]);
        assert!(d.observe_output(A, b" ok", &peers(), 2).is_empty(), "the old mention is not re-reported");
    }

    #[test]
    fn partial_words_and_self_mentions_are_ignored() {
        let mut d = LinkDetector::default();
        assert!(d.observe_output(A, b"Architect here, subbackends ready", &peers(), 0).is_empty());
    }

    #[test]
    fn relay_links_the_terminal_that_produced_the_pasted_text() {
        let mut d = LinkDetector::default();
        d.observe_output(A, b"Plan:\r\n  implement the   login endpoint\r\n", &peers(), 1_000);
        let sources = d.observe_input(B, b"implement the login endpoint", &peers(), 2_000);
        assert_eq!(sources, vec![A.to_owned()]);
    }

    #[test]
    fn relay_ignores_short_or_stale_text() {
        let mut d = LinkDetector::default();
        d.observe_output(A, b"run the migration script", &peers(), 0);
        assert!(d.observe_input(B, b"ls", &peers(), 100).is_empty(), "too short");
        let late = INPUT_CORRELATION_WINDOW_MS + 1;
        assert!(d.observe_input(C, b"run the migration script", &peers(), late).is_empty(), "outside the window");
    }

    #[test]
    fn relay_accumulates_typed_characters_per_line() {
        let mut d = LinkDetector::default();
        d.observe_output(A, b"deploy staging now", &peers(), 0);
        let mut found = Vec::new();
        for ch in "deploy staging".bytes() {
            found.extend(d.observe_input(B, &[ch], &peers(), 10));
        }
        assert!(found.contains(&A.to_owned()));
        d.observe_input(B, b"\r", &peers(), 20);
        assert!(d.observe_input(B, b"x", &peers(), 30).is_empty(), "a new line starts empty");
    }

    #[test]
    fn forget_drops_recent_output() {
        let mut d = LinkDetector::default();
        d.observe_output(A, b"some long recent output", &peers(), 0);
        d.forget(A);
        assert!(d.observe_input(B, b"some long recent output", &peers(), 1).is_empty());
    }

    #[test]
    fn recent_output_is_bounded() {
        let mut d = LinkDetector::default();
        let line = "x".repeat(1000) + "\n";
        for _ in 0..100 {
            d.observe_output(A, line.as_bytes(), &peers(), 0);
        }
        assert!(d.outputs[A].recent.len() <= RECENT_OUTPUT_BYTES);
        let total: usize = d.outputs[A].segments.iter().map(|s| s.1).sum();
        assert_eq!(total, d.outputs[A].recent.len());
    }
}
