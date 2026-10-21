//! Streaming text helpers used by link detection: PTY output arrives in
//! arbitrary chunks, so both UTF-8 sequences and escape sequences can be split
//! across reads.

/// Decodes UTF-8 incrementally, carrying an incomplete trailing sequence over
/// to the next chunk. Invalid bytes become U+FFFD.
#[derive(Debug, Default)]
pub struct Utf8Stream {
    carry: Vec<u8>,
}

impl Utf8Stream {
    pub fn decode(&mut self, bytes: &[u8]) -> String {
        let mut buf = std::mem::take(&mut self.carry);
        buf.extend_from_slice(bytes);
        let mut out = String::with_capacity(buf.len());
        let mut rest: &[u8] = &buf;
        loop {
            match std::str::from_utf8(rest) {
                Ok(valid) => {
                    out.push_str(valid);
                    break;
                }
                Err(err) => {
                    let (valid, after) = rest.split_at(err.valid_up_to());
                    // `valid_up_to` guarantees this prefix is valid UTF-8.
                    out.push_str(std::str::from_utf8(valid).unwrap_or_default());
                    match err.error_len() {
                        Some(len) => {
                            out.push(char::REPLACEMENT_CHARACTER);
                            rest = &after[len..];
                        }
                        None => {
                            // Incomplete sequence at the end: keep it for later.
                            self.carry = after.to_vec();
                            break;
                        }
                    }
                }
            }
        }
        out
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum AnsiState {
    #[default]
    Text,
    Escape,
    Csi,
    /// OSC, DCS, SOS, PM and APC strings, terminated by BEL or ST.
    String,
    StringEscape,
}

/// Removes terminal escape sequences and control characters, keeping
/// printable text, newlines and tabs. Carriage returns become newlines so
/// that redrawn lines stay separated.
#[derive(Debug, Default)]
pub struct AnsiStripper {
    state: AnsiState,
}

impl AnsiStripper {
    pub fn strip(&mut self, input: &str) -> String {
        let mut out = String::with_capacity(input.len());
        for ch in input.chars() {
            self.state = match self.state {
                AnsiState::Text => match ch {
                    '\u{1b}' => AnsiState::Escape,
                    '\n' | '\t' => {
                        out.push(ch);
                        AnsiState::Text
                    }
                    '\r' => {
                        out.push('\n');
                        AnsiState::Text
                    }
                    c if c.is_control() => AnsiState::Text,
                    c => {
                        out.push(c);
                        AnsiState::Text
                    }
                },
                AnsiState::Escape => match ch {
                    '[' => AnsiState::Csi,
                    ']' | 'P' | 'X' | '^' | '_' => AnsiState::String,
                    // Two-character sequences such as `ESC 7` or `ESC (B` are
                    // approximated by dropping the next character.
                    _ => AnsiState::Text,
                },
                AnsiState::Csi => {
                    if ('\u{40}'..='\u{7e}').contains(&ch) {
                        AnsiState::Text
                    } else {
                        AnsiState::Csi
                    }
                }
                AnsiState::String => match ch {
                    '\u{7}' => AnsiState::Text,
                    '\u{1b}' => AnsiState::StringEscape,
                    _ => AnsiState::String,
                },
                AnsiState::StringEscape => match ch {
                    '\\' => AnsiState::Text,
                    '\u{1b}' => AnsiState::StringEscape,
                    _ => AnsiState::String,
                },
            };
        }
        out
    }
}

/// Collapses every run of whitespace into a single space and lowercases the
/// text, so that wrapped or re-rendered output still matches.
pub fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut pending_space = false;
    for ch in text.chars() {
        if ch.is_whitespace() {
            pending_space = !out.is_empty();
        } else {
            if pending_space {
                out.push(' ');
                pending_space = false;
            }
            out.extend(ch.to_lowercase());
        }
    }
    out
}

fn is_word_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

/// Finds `needle` in `haystack` as a whole word (not preceded or followed by
/// an alphanumeric character or `_`). Only matches that end after byte offset
/// `min_end` count, which lets callers ignore text they already scanned.
pub fn contains_word(haystack: &str, needle: &str, min_end: usize) -> bool {
    if needle.is_empty() {
        return false;
    }
    let mut start = 0;
    while let Some(pos) = haystack[start..].find(needle) {
        let begin = start + pos;
        let end = begin + needle.len();
        let before_ok = haystack[..begin].chars().next_back().is_none_or(|c| !is_word_char(c));
        let after_ok = haystack[end..].chars().next().is_none_or(|c| !is_word_char(c));
        if before_ok && after_ok && end > min_end {
            return true;
        }
        // Advance by one character, staying on a char boundary.
        start = begin + haystack[begin..].chars().next().map_or(1, char::len_utf8);
    }
    false
}

/// Returns the suffix of `text` holding at most `max_bytes` bytes, cut on a
/// char boundary.
pub fn tail(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }
    let mut cut = text.len() - max_bytes;
    while !text.is_char_boundary(cut) {
        cut += 1;
    }
    &text[cut..]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_stream_joins_split_sequences() {
        let mut stream = Utf8Stream::default();
        let bytes = "héllo ✓".as_bytes();
        let split = bytes.len() - 2; // inside the 3-byte check mark
        let mut out = stream.decode(&bytes[..split]);
        out.push_str(&stream.decode(&bytes[split..]));
        assert_eq!(out, "héllo ✓");
    }

    #[test]
    fn utf8_stream_replaces_invalid_bytes() {
        let mut stream = Utf8Stream::default();
        assert_eq!(stream.decode(b"a\xffb"), "a\u{fffd}b");
    }

    #[test]
    fn ansi_stripper_removes_csi_and_osc_across_chunks() {
        let mut stripper = AnsiStripper::default();
        let mut out = stripper.strip("\u{1b}[1;3");
        out.push_str(&stripper.strip("2mred\u{1b}[0m \u{1b}]0;title\u{7}ok\r\n"));
        assert_eq!(out, "red ok\n\n");
    }

    #[test]
    fn normalize_collapses_whitespace_and_case() {
        assert_eq!(normalize("  Hello\n\n  World\t"), "hello world");
    }

    #[test]
    fn contains_word_respects_boundaries() {
        assert!(contains_word("ask terminal 2 now", "terminal 2", 0));
        assert!(!contains_word("ask terminal 21 now", "terminal 2", 0));
        assert!(!contains_word("subackend", "backend", 0));
        assert!(contains_word("@backend:", "backend", 0));
    }

    #[test]
    fn contains_word_ignores_matches_before_min_end() {
        let text = "backend ... later backend";
        assert!(contains_word(text, "backend", 10));
        assert!(!contains_word("backend ...", "backend", 10));
    }

    #[test]
    fn tail_cuts_on_char_boundary() {
        assert_eq!(tail("ab✓", 2), "");
        assert_eq!(tail("ab✓", 3), "✓");
        assert_eq!(tail("abc", 10), "abc");
    }
}
