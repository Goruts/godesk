//! Tells coding agents about Godesk's terminals, so that "ask Terminal 3"
//! means something to them: a short note, between markers, in the
//! instructions each agent reads in every project (Claude Code's
//! `~/.claude/CLAUDE.md`, Codex's `~/.codex/AGENTS.md`). Only the marked
//! block is ever written or removed; the rest of those files is the user's.

use std::path::{Path, PathBuf};

use crate::error::AppResult;

const BEGIN: &str = "<!-- godesk:begin (written by Godesk; turn it off in Godesk's ⋯ menu) -->";
const END: &str = "<!-- godesk:end -->";

const NOTE: &str = "## Godesk terminals

If the environment variable `GODESK_TERMINAL_ID` is set, you run in a terminal of Godesk, an infinite canvas of terminals. Each terminal has the name shown on its title bar (\"Terminal 3\", \"planner\"…). When the user names a terminal or an agent like that, they mean one of these terminals, not something in the code.

Your own terminal's name is in `$GODESK_TERMINAL_NAME` and its id in `$GODESK_TERMINAL_ID` — read them to know which terminal you are. When the user asks where you are or which terminal you run in, they mean this name, not your shell or working directory.

To talk to another terminal, run the real `godesk` command in your own shell — the same way you run any other command. It is a normal program on your PATH, not a tool call or an API: never invent a JSON message, a file, or any other channel to reach another terminal. There is no other way to reach one.

- `godesk list` — the other terminals of your workspace and what runs in each; `[connected]` marks those an arrow already joins to yours. Run this first when unsure of a name; names match without regard to case.
- `godesk send \"<terminal>\" \"<message>\"` — types the message into that terminal and presses Enter: this is how you ask the agent there to do something. The message is sent as one line.
- `godesk read \"<terminal>\" [lines]` — prints the end of its output, for example to see its answer.

A terminal can also be connected to a knowledge base: a folder of documents (notes, PDFs, spreadsheets, code…) drawn as its own node. When an arrow connects one to your terminal, you can read it:
- `godesk kb` — the knowledge bases connected to you and the files in each.
- `godesk kb read \"<file>\"` — print a document's text (use `godesk kb read \"<base>\" \"<file>\"` when several are connected). Godesk turns PDFs and Office files into text for you.
Read a knowledge base when it would help with the task. You can reach it only while its arrow is connected; once the arrow is removed, you can no longer read it.

You can reach any terminal of your own workspace by its name, with no arrow drawn first. When the user asks you to lead or coordinate other terminals, or a task is better split between them, act as the orchestrator: give each terminal its part with `godesk send`, check their progress with `godesk read`, and ask them to report back to you by your name. The first time you message or read a terminal, Godesk draws the arrow between your windows on its own, so the user sees who instructs whom, and the other terminal can answer you back. Terminals of other workspaces cannot be reached.";

/// The instruction files of the agents Godesk knows, under `home`.
fn files(home: &Path) -> [PathBuf; 2] {
    [home.join(".claude").join("CLAUDE.md"), home.join(".codex").join("AGENTS.md")]
}

/// `text` without Godesk's block (and the blank line before it).
fn without_note(text: &str) -> String {
    match (text.find(BEGIN), text.find(END)) {
        (Some(start), Some(end)) if end > start => {
            let after = &text[end + END.len()..];
            format!("{}{}", text[..start].trim_end_matches(['\r', '\n']), after.trim_start_matches(['\r', '\n']))
        }
        _ => text.to_owned(),
    }
}

/// Whether every agent's instructions carry the note.
pub fn installed(home: &Path) -> bool {
    files(home).iter().all(|file| std::fs::read_to_string(file).is_ok_and(|text| text.contains(BEGIN)))
}

/// Adds the note to each agent's instructions (replacing an older one), or
/// removes it, deleting a file that held nothing else.
pub fn set(home: &Path, on: bool) -> AppResult<()> {
    for file in files(home) {
        let current = std::fs::read_to_string(&file).unwrap_or_default();
        let rest = without_note(&current);
        if on {
            if let Some(dir) = file.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let separator = if rest.trim().is_empty() { "" } else { "\n\n" };
            std::fs::write(&file, format!("{}{separator}{BEGIN}\n{NOTE}\n{END}\n", rest.trim_end()))?;
        } else if current.contains(BEGIN) {
            if rest.trim().is_empty() {
                std::fs::remove_file(&file)?;
            } else {
                std::fs::write(&file, rest)?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds_and_removes_only_its_own_block() {
        let home = tempfile::tempdir().unwrap();
        let [claude, codex] = files(home.path());
        std::fs::create_dir_all(claude.parent().unwrap()).unwrap();
        std::fs::write(&claude, "# My rules\n\nBe brief.\n").unwrap();
        assert!(!installed(home.path()));

        set(home.path(), true).unwrap();
        set(home.path(), true).unwrap(); // a second time replaces, never duplicates
        assert!(installed(home.path()));
        let text = std::fs::read_to_string(&claude).unwrap();
        assert!(text.starts_with("# My rules\n\nBe brief.\n\n<!-- godesk:begin"));
        assert_eq!(text.matches(BEGIN).count(), 1);
        assert!(std::fs::read_to_string(&codex).unwrap().contains("godesk send"));

        set(home.path(), false).unwrap();
        assert!(!installed(home.path()));
        assert_eq!(std::fs::read_to_string(&claude).unwrap(), "# My rules\n\nBe brief.");
        assert!(!codex.exists(), "a file Godesk created for the note goes with it");
    }
}
