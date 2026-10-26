//! Text helpers for PTY output, which arrives in arbitrary chunks so escape
//! sequences can be split across reads.

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ansi_stripper_removes_csi_and_osc_across_chunks() {
        let mut stripper = AnsiStripper::default();
        let mut out = stripper.strip("\u{1b}[1;3");
        out.push_str(&stripper.strip("2mred\u{1b}[0m \u{1b}]0;title\u{7}ok\r\n"));
        assert_eq!(out, "red ok\n\n");
    }
}
