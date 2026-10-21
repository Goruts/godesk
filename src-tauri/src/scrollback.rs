//! Bounded buffer of recent PTY output, replayed when a view re-attaches to a
//! terminal (for example after switching workspaces).

use std::collections::VecDeque;

pub const MAX_SCROLLBACK_LINES: usize = 5000;
pub const MAX_SCROLLBACK_BYTES: usize = 1024 * 1024;

#[derive(Debug)]
pub struct Scrollback {
    buf: VecDeque<u8>,
    newlines: usize,
    /// Bytes ever appended; the end offset of the buffer.
    total: u64,
    max_lines: usize,
    max_bytes: usize,
}

impl Default for Scrollback {
    fn default() -> Self {
        Self::with_limits(MAX_SCROLLBACK_LINES, MAX_SCROLLBACK_BYTES)
    }
}

impl Scrollback {
    pub fn with_limits(max_lines: usize, max_bytes: usize) -> Self {
        Self { buf: VecDeque::new(), newlines: 0, total: 0, max_lines, max_bytes }
    }

    /// Appends a chunk and returns the new end offset.
    pub fn push(&mut self, chunk: &[u8]) -> u64 {
        self.buf.extend(chunk);
        self.newlines += chunk.iter().filter(|&&b| b == b'\n').count();
        self.total += chunk.len() as u64;
        self.trim();
        self.total
    }

    /// Drops whole lines from the front until both limits hold. Cutting on a
    /// newline keeps escape sequences and UTF-8 characters of the remaining
    /// lines intact.
    fn trim(&mut self) {
        while self.newlines > self.max_lines || self.buf.len() > self.max_bytes {
            match self.buf.iter().position(|&b| b == b'\n') {
                Some(pos) => {
                    self.buf.drain(..=pos);
                    self.newlines -= 1;
                }
                None => {
                    // A single line longer than the byte limit: keep its tail.
                    let excess = self.buf.len().saturating_sub(self.max_bytes);
                    self.buf.drain(..excess);
                    break;
                }
            }
        }
    }

    pub fn snapshot(&self) -> (Vec<u8>, u64) {
        (self.buf.iter().copied().collect(), self.total)
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    pub fn lines(&self) -> usize {
        self.newlines
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_everything_under_the_limits() {
        let mut sb = Scrollback::default();
        assert_eq!(sb.push(b"one\ntwo\n"), 8);
        assert_eq!(sb.snapshot(), (b"one\ntwo\n".to_vec(), 8));
    }

    #[test]
    fn drops_oldest_lines_over_line_limit() {
        let mut sb = Scrollback::with_limits(2, 1024);
        sb.push(b"a\nb\nc\n");
        let (data, total) = sb.snapshot();
        assert_eq!(data, b"b\nc\n");
        assert_eq!(total, 6, "offset counts every byte ever produced");
        assert_eq!(sb.lines(), 2);
    }

    #[test]
    fn drops_whole_lines_over_byte_limit() {
        let mut sb = Scrollback::with_limits(100, 8);
        sb.push(b"12345\n678\nab");
        assert_eq!(sb.snapshot().0, b"678\nab");
        assert!(sb.len() <= 8);
    }

    #[test]
    fn truncates_single_huge_line() {
        let mut sb = Scrollback::with_limits(100, 4);
        sb.push(b"abcdefgh");
        assert_eq!(sb.snapshot().0, b"efgh");
    }

    #[test]
    fn default_limits_hold_for_large_output() {
        let mut sb = Scrollback::default();
        for i in 0..20_000 {
            sb.push(format!("line {i}\n").as_bytes());
        }
        assert_eq!(sb.lines(), MAX_SCROLLBACK_LINES);
        assert!(sb.snapshot().0.starts_with(b"line 15000\n"));

        let mut sb = Scrollback::default();
        let line = vec![b'x'; 1023];
        for _ in 0..2_000 {
            sb.push(&line);
            sb.push(b"\n");
        }
        assert!(sb.len() <= MAX_SCROLLBACK_BYTES);
        assert!(!sb.is_empty());
    }
}
