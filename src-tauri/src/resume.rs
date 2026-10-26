//! What a terminal reopened after a restart runs again, from what ran in it
//! before. Coding agents come back in their own conversation: Claude Code
//! and Codex by the session id found in their session files, OpenCode with
//! its "continue the last session". Any other agent CLI is started again as
//! it was. Any other program is only typed at the prompt, without Enter: a
//! build, a deploy or a script must not run again by itself.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// A session file counts as the program's if it changed after the program
/// started (less this slack, for clocks and file times).
const START_SLACK: Duration = Duration::from_secs(5);

/// Agent CLIs that are started again without asking: they only wait for
/// the user's next prompt.
const AGENTS: [&str; 13] = [
    "claude",
    "codex",
    "opencode",
    "mimo",
    "gemini",
    "aider",
    "qwen",
    "crush",
    "goose",
    "amp",
    "cursor-agent",
    "copilot",
    "droid",
];

/// The line to put back in a reopened terminal, and whether to press Enter.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Restore {
    pub argv: Vec<String>,
    pub run: bool,
}

/// Which agent `argv` starts: the program's file name, or for an agent run
/// by node (an npm install), its package's.
pub(crate) fn agent_of(argv: &[String]) -> Option<&'static str> {
    let stem =
        |arg: &String| Path::new(arg).file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
    let program = stem(argv.first()?);
    if let Some(agent) = AGENTS.iter().find(|a| **a == program) {
        return Some(agent);
    }
    if program == "node" || program == "bun" {
        let script = argv.get(1)?.to_lowercase().replace('\\', "/");
        let by_package = [("claude-code", "claude"), ("@openai/codex", "codex"), ("opencode", "opencode"), ("@mimo-ai/", "mimo")];
        if let Some((_, agent)) = by_package.iter().find(|(package, _)| script.contains(package)) {
            return Some(agent);
        }
        return AGENTS.iter().find(|a| script.contains(&format!("/{a}"))).copied();
    }
    None
}

/// How many leading arguments are the program itself (`node cli.js` is two).
pub(crate) fn program_len(argv: &[String]) -> usize {
    let first = Path::new(&argv[0]).file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
    if (first == "node" || first == "bun") && argv.len() > 1 { 2 } else { 1 }
}

/// `args` without the options that pick a session (`flags`), and their
/// values (`with_value`).
fn without_session_args(args: &[String], flags: &[&str], with_value: &[&str]) -> Vec<String> {
    let mut kept = Vec::new();
    let mut iter = args.iter().peekable();
    while let Some(arg) = iter.next() {
        let name = arg.split('=').next().unwrap_or(arg);
        if with_value.contains(&name) {
            // `--resume id` takes the next argument unless it is an option.
            if !arg.contains('=') && iter.peek().is_some_and(|next| !next.starts_with('-')) {
                iter.next();
            }
        } else if !flags.contains(&name) {
            kept.push(arg.clone());
        }
    }
    kept
}

/// The agent a terminal runs (`"claude"`, `"codex"`) and the file it keeps
/// its current session in, found as for [`plan`].
pub fn session_file(
    argv: &[String],
    cwd: &Path,
    started: Option<SystemTime>,
    home: &Path,
    claimed: &mut HashSet<PathBuf>,
) -> Option<(&'static str, PathBuf)> {
    match agent_of(argv)? {
        "claude" => claude_session_file(home, cwd, started, claimed).map(|f| ("claude", f)),
        "codex" => codex_session_file(home, cwd, started, claimed).map(|f| ("codex", f)),
        _ => None,
    }
}

/// What to run again in a terminal whose foreground program was `argv`,
/// started at `started` in `cwd`. `claimed` holds the session files already
/// given to other terminals, so two agents in one folder keep their own.
pub fn plan(
    argv: &[String],
    cwd: Option<&Path>,
    started: Option<SystemTime>,
    home: &Path,
    claimed: &mut HashSet<PathBuf>,
) -> Option<Restore> {
    if argv.is_empty() {
        return None;
    }
    let split = program_len(argv);
    let (program, args) = argv.split_at(split);
    let mut line = program.to_vec();
    match agent_of(argv) {
        Some("claude") => {
            line.extend(without_session_args(args, &["-c", "--continue"], &["-r", "--resume", "--session-id"]));
            let session = cwd.and_then(|cwd| claude_session(home, cwd, started, claimed));
            match session {
                Some(id) => line.extend(["--resume".to_owned(), id]),
                None => line.push("--continue".to_owned()),
            }
        }
        Some("codex") => {
            let session = cwd.and_then(|cwd| codex_session(home, cwd, started, claimed));
            line.push("resume".to_owned());
            line.push(session.unwrap_or_else(|| "--last".to_owned()));
        }
        Some("opencode" | "mimo") => {
            line.extend(without_session_args(args, &["-c", "--continue"], &["-s", "--session"]));
            line.push("--continue".to_owned());
        }
        Some(_) => line.extend(args.iter().cloned()),
        None => return Some(Restore { argv: argv.to_vec(), run: false }),
    }
    Some(Restore { argv: line, run: true })
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// The newest file among `files` that changed since `started` and is not
/// claimed yet; claims it.
fn newest(files: Vec<PathBuf>, started: Option<SystemTime>, claimed: &mut HashSet<PathBuf>) -> Option<PathBuf> {
    let since = started.map(|t| t.checked_sub(START_SLACK).unwrap_or(t));
    let best = files
        .into_iter()
        .filter(|f| !claimed.contains(f))
        .filter_map(|f| modified(&f).map(|m| (m, f)))
        .filter(|(m, _)| since.is_none_or(|since| *m >= since))
        .max_by_key(|(m, _)| *m)
        .map(|(_, f)| f)?;
    claimed.insert(best.clone());
    Some(best)
}

/// Claude Code's folder for the sessions started in `cwd`: every character
/// that is not a letter or digit becomes `-` (`C:\Users\me` → `C--Users-me`).
fn claude_project_dir(home: &Path, cwd: &Path) -> PathBuf {
    let name: String = cwd.to_string_lossy().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    home.join(".claude").join("projects").join(name)
}

/// The session an agent running on its own screen in `cwd` since
/// `started` works on, found by its files (Claude Code and Codex), except
/// the files in `taken`.
pub fn running_session(
    agent: &str,
    cwd: &Path,
    started: Option<SystemTime>,
    home: &Path,
    taken: HashSet<PathBuf>,
) -> Option<String> {
    let mut claimed = taken;
    match agent {
        "claude" => claude_session(home, cwd, started, &mut claimed),
        "codex" => codex_session(home, cwd, started, &mut claimed),
        _ => None,
    }
}

/// The file an agent keeps the session `id` in, when it is one Godesk
/// reads ("claude", "codex") and the file exists.
pub fn known_session_file(agent: &str, id: &str, cwd: &Path, home: &Path) -> Option<PathBuf> {
    match agent {
        "claude" => Some(claude_project_dir(home, cwd).join(format!("{id}.jsonl"))).filter(|f| f.is_file()),
        "codex" => {
            let mut dirs = vec![home.join(".codex").join("sessions")];
            while let Some(dir) = dirs.pop() {
                for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        dirs.push(path);
                    } else if path.file_stem().is_some_and(|s| s.to_string_lossy().ends_with(id)) {
                        return Some(path);
                    }
                }
            }
            None
        }
        _ => None,
    }
}

fn claude_session_file(
    home: &Path,
    cwd: &Path,
    started: Option<SystemTime>,
    claimed: &mut HashSet<PathBuf>,
) -> Option<PathBuf> {
    let files = std::fs::read_dir(claude_project_dir(home, cwd))
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
        .collect();
    newest(files, started, claimed)
}

/// Codex keeps sessions as `sessions/YYYY/MM/DD/rollout-<time>-<id>.jsonl`,
/// whose first line records the folder they ran in.
fn codex_session_file(
    home: &Path,
    cwd: &Path,
    started: Option<SystemTime>,
    claimed: &mut HashSet<PathBuf>,
) -> Option<PathBuf> {
    let mut files = Vec::new();
    let mut dirs = vec![home.join(".codex").join("sessions")];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.file_name().is_some_and(|n| n.to_string_lossy().starts_with("rollout-")) {
                files.push(path);
            }
        }
    }
    let since = started.map(|t| t.checked_sub(START_SLACK).unwrap_or(t));
    let wanted = cwd.to_string_lossy().to_lowercase();
    let in_cwd = |file: &PathBuf| {
        use std::io::BufRead;
        let Ok(handle) = std::fs::File::open(file) else { return false };
        let mut first = String::new();
        let _ = std::io::BufReader::new(handle).read_line(&mut first);
        serde_json::from_str::<serde_json::Value>(&first)
            .ok()
            .and_then(|v| v["payload"]["cwd"].as_str().map(|c| c.to_lowercase() == wanted))
            .unwrap_or(false)
    };
    // Only recent files are opened to read their folder.
    let recent: Vec<PathBuf> = files
        .into_iter()
        .filter(|f| since.is_none_or(|since| modified(f).is_some_and(|m| m >= since)))
        .filter(in_cwd)
        .collect();
    newest(recent, started, claimed)
}

fn claude_session(
    home: &Path,
    cwd: &Path,
    started: Option<SystemTime>,
    claimed: &mut HashSet<PathBuf>,
) -> Option<String> {
    let file = claude_session_file(home, cwd, started, claimed)?;
    Some(file.file_stem()?.to_string_lossy().into_owned())
}

fn codex_session(
    home: &Path,
    cwd: &Path,
    started: Option<SystemTime>,
    claimed: &mut HashSet<PathBuf>,
) -> Option<String> {
    let file = codex_session_file(home, cwd, started, claimed)?;
    let stem = file.file_stem()?.to_string_lossy().into_owned();
    // The id is the trailing UUID: 36 characters.
    (stem.len() >= 36).then(|| stem[stem.len() - 36..].to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(args: &[&str]) -> Vec<String> {
        args.iter().map(|a| (*a).to_owned()).collect()
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn claude_comes_back_in_its_own_conversation() {
        let home = tempfile::tempdir().unwrap();
        let cwd = Path::new(r"C:\Users\me\project");
        let dir = home.path().join(".claude/projects/C--Users-me-project");
        write(&dir.join("older.jsonl"), "{}");
        std::thread::sleep(Duration::from_millis(20));
        write(&dir.join("6fb9df23-2fb2-4292-bb2a-d0248f4a60d4.jsonl"), "{}");
        let mut claimed = HashSet::new();

        let first = plan(&argv(&["claude.exe", "--model", "opus", "-c"]), Some(cwd), None, home.path(), &mut claimed);
        assert_eq!(
            first,
            Some(Restore {
                argv: argv(&["claude.exe", "--model", "opus", "--resume", "6fb9df23-2fb2-4292-bb2a-d0248f4a60d4"]),
                run: true
            })
        );
        // A second Claude in the same folder gets the other conversation.
        let second = plan(&argv(&["claude", "--resume", "x"]), Some(cwd), None, home.path(), &mut claimed).unwrap();
        assert_eq!(second.argv, argv(&["claude", "--resume", "older"]));
        // Without a conversation left, the most recent one in the folder.
        let third = plan(&argv(&["claude"]), Some(cwd), None, home.path(), &mut claimed).unwrap();
        assert_eq!(third.argv, argv(&["claude", "--continue"]));
    }

    #[test]
    fn codex_resumes_the_session_of_its_folder() {
        let home = tempfile::tempdir().unwrap();
        let meta = |cwd: &str| format!(r#"{{"type":"session_meta","payload":{{"cwd":{}}}}}"#, serde_json::json!(cwd));
        let sessions = home.path().join(".codex/sessions/2026/10/21");
        write(
            &sessions.join("rollout-2026-10-21T10-00-00-01a14283-4503-7e02-aee2-318be925e579.jsonl"),
            &meta(r"C:\work"),
        );
        write(
            &sessions.join("rollout-2026-10-21T11-00-00-01a14283-4503-7e02-aee2-000000000000.jsonl"),
            &meta(r"C:\other"),
        );
        let mut claimed = HashSet::new();
        let restore =
            plan(&argv(&[r"C:\bin\codex.exe", "--yolo"]), Some(Path::new(r"C:\Work")), None, home.path(), &mut claimed);
        assert_eq!(
            restore.unwrap().argv,
            argv(&[r"C:\bin\codex.exe", "resume", "01a14283-4503-7e02-aee2-318be925e579"])
        );
        let restore = plan(&argv(&["codex"]), Some(Path::new(r"C:\Work")), None, home.path(), &mut claimed);
        assert_eq!(restore.unwrap().argv, argv(&["codex", "resume", "--last"]));
    }

    #[test]
    fn other_agents_start_again_and_other_programs_wait_at_the_prompt() {
        let home = tempfile::tempdir().unwrap();
        let mut claimed = HashSet::new();
        let mut plan_of = |a: &[&str]| plan(&argv(a), None, None, home.path(), &mut claimed).unwrap();
        assert_eq!(
            plan_of(&["node", r"C:\npm\node_modules\opencode-ai\bin\opencode", "-s", "abc"]),
            Restore { argv: argv(&["node", r"C:\npm\node_modules\opencode-ai\bin\opencode", "--continue"]), run: true }
        );
        assert_eq!(plan_of(&["gemini.exe", "--yolo"]), Restore { argv: argv(&["gemini.exe", "--yolo"]), run: true });
        assert_eq!(plan_of(&["npm", "run", "deploy"]), Restore { argv: argv(&["npm", "run", "deploy"]), run: false });
    }
}
