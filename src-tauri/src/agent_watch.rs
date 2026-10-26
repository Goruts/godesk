//! What the coding agents in the terminals are doing, read from their own
//! session files (the same ones [`crate::resume`] finds): the tokens each
//! session has used, and for Claude Code the subagents it started, each with
//! its own transcript (`<session>/subagents/agent-<id>.jsonl` and
//! `.meta.json`). Files are read incrementally: only what was appended since
//! the last look.

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

use crate::model::{AgentsChangedEvent, Subagent, TerminalAgent, TokenUsage};

pub const AGENTS_CHANGED_EVENT: &str = "agents-changed";
/// Transcript lines kept per subagent.
const MAX_LINES: usize = 200;
/// Longest transcript line shown.
const LINE_CHARS: usize = 240;
/// Input fields that say best what a tool call does, in order of preference.
const TOOL_SUMMARY_FIELDS: [&str; 8] =
    ["command", "file_path", "pattern", "path", "url", "query", "description", "prompt"];

/// A terminal to look at: its id, its foreground program's argv, its folder,
/// and when that program started; or, for an agent Godesk shows, the agent
/// and the file of its session, which Godesk knows.
pub struct Watched {
    pub terminal_id: String,
    pub argv: Vec<String>,
    pub cwd: PathBuf,
    pub started: Option<SystemTime>,
    pub session: Option<(&'static str, PathBuf)>,
}

#[derive(Default)]
struct FileState {
    offset: u64,
    /// A line not yet ended when last read.
    partial: Vec<u8>,
    /// Claude Code: usage per message id. One message spans several entries,
    /// each repeating its usage; the last one is final.
    by_message: HashMap<String, TokenUsage>,
    /// Codex: the running total it records itself.
    total: TokenUsage,
    lines: VecDeque<String>,
}

impl FileState {
    fn tokens(&self) -> TokenUsage {
        let mut tokens = self.total;
        for usage in self.by_message.values() {
            tokens += *usage;
        }
        tokens
    }

    fn push_line(&mut self, line: String) {
        let line = line.trim().to_owned();
        if line.is_empty() {
            return;
        }
        let shortened = if line.chars().count() > LINE_CHARS {
            format!("{}…", line.chars().take(LINE_CHARS).collect::<String>())
        } else {
            line
        };
        self.lines.push_back(shortened);
        while self.lines.len() > MAX_LINES {
            self.lines.pop_front();
        }
    }

    /// One JSON line of a Claude Code or Codex session file.
    fn take(&mut self, entry: &Value) {
        // Codex: `{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":…}}}`
        let payload = &entry["payload"];
        if payload["type"] == "token_count" {
            let usage = &payload["info"]["total_token_usage"];
            if let (Some(input), Some(output)) = (usage["input_tokens"].as_u64(), usage["output_tokens"].as_u64()) {
                self.total = TokenUsage { input, output };
            }
            return;
        }
        let message = &entry["message"];
        match entry["type"].as_str() {
            Some("assistant") => {
                let usage = &message["usage"];
                if let Some(id) = message["id"].as_str() {
                    let read = ["input_tokens", "cache_creation_input_tokens", "cache_read_input_tokens"]
                        .iter()
                        .filter_map(|field| usage[field].as_u64())
                        .sum();
                    let written = usage["output_tokens"].as_u64().unwrap_or(0);
                    self.by_message.insert(id.to_owned(), TokenUsage { input: read, output: written });
                }
                for block in message["content"].as_array().into_iter().flatten() {
                    match block["type"].as_str() {
                        Some("text") => {
                            for line in block["text"].as_str().unwrap_or_default().lines() {
                                self.push_line(line.to_owned());
                            }
                        }
                        Some("tool_use") => self.push_line(tool_line(block)),
                        _ => {}
                    }
                }
            }
            // The person's (or for a subagent, its parent's) words; tool
            // results are not shown.
            Some("user") => {
                if let Some(text) = message["content"].as_str() {
                    let first = text.lines().find(|l| !l.trim().is_empty()).unwrap_or_default();
                    self.push_line(format!("› {first}"));
                }
            }
            _ => {}
        }
    }
}

/// `▸ Bash: npm test` for a tool call.
fn tool_line(block: &Value) -> String {
    let name = block["name"].as_str().unwrap_or("tool");
    let input = &block["input"];
    let summary = TOOL_SUMMARY_FIELDS
        .iter()
        .find_map(|field| input[field].as_str())
        .map(|s| s.lines().next().unwrap_or_default().to_owned())
        .unwrap_or_default();
    if summary.is_empty() { format!("▸ {name}") } else { format!("▸ {name}: {summary}") }
}

fn millis(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

pub struct AgentWatch {
    home: PathBuf,
    files: Mutex<HashMap<PathBuf, FileState>>,
    last: Mutex<Option<AgentsChangedEvent>>,
}

impl AgentWatch {
    pub fn new(home: PathBuf) -> Self {
        Self { home, files: Mutex::new(HashMap::new()), last: Mutex::new(None) }
    }

    /// Reads what was appended to `file` since the last look; returns its
    /// tokens and transcript.
    fn read(files: &mut HashMap<PathBuf, FileState>, file: &Path) -> (TokenUsage, Vec<String>) {
        let state = files.entry(file.to_owned()).or_default();
        if let Ok(mut handle) = std::fs::File::open(file) {
            let length = handle.metadata().map(|m| m.len()).unwrap_or(0);
            // Rewritten from the start (shorter than read so far): read it again.
            if length < state.offset {
                *state = FileState::default();
            }
            let mut fresh = Vec::new();
            if handle.seek(SeekFrom::Start(state.offset)).is_ok() && handle.read_to_end(&mut fresh).is_ok() {
                state.offset += fresh.len() as u64;
                let mut pending = std::mem::take(&mut state.partial);
                pending.extend_from_slice(&fresh);
                let complete = pending.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
                state.partial = pending.split_off(complete);
                for line in pending.split(|&b| b == b'\n').filter(|l| !l.is_empty()) {
                    if let Ok(entry) = serde_json::from_slice::<Value>(line) {
                        state.take(&entry);
                    }
                }
            }
        }
        (state.tokens(), state.lines.iter().cloned().collect())
    }

    /// The subagents of the Claude Code session kept in `session`.
    fn subagents(files: &mut HashMap<PathBuf, FileState>, session: &Path) -> Vec<Subagent> {
        let dir = session.with_extension("").join("subagents");
        let mut found: Vec<Subagent> = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
            .filter_map(|transcript| {
                let stem = transcript.file_stem()?.to_string_lossy().into_owned();
                let meta: Value = std::fs::read(dir.join(format!("{stem}.meta.json")))
                    .ok()
                    .and_then(|b| serde_json::from_slice(&b).ok())
                    .unwrap_or_default();
                let updated_at = std::fs::metadata(&transcript).and_then(|m| m.modified()).map_or(0, millis);
                let (tokens, lines) = Self::read(files, &transcript);
                Some(Subagent {
                    id: stem.trim_start_matches("agent-").to_owned(),
                    kind: meta["agentType"].as_str().unwrap_or("agent").to_owned(),
                    description: meta["description"].as_str().unwrap_or_default().to_owned(),
                    finished: meta["finished"].as_bool().unwrap_or(false),
                    tokens,
                    lines,
                    updated_at,
                })
            })
            .collect();
        found.sort_by_key(|s| s.updated_at);
        found
    }

    /// Looks at every agent the terminals run.
    pub fn scan(&self, watched: &[Watched]) -> AgentsChangedEvent {
        let mut files = self.files.lock().expect("agent files lock poisoned");
        let before: HashSet<PathBuf> = files.keys().cloned().collect();
        let mut seen = HashSet::new();
        let mut claimed = HashSet::new();
        let mut event = AgentsChangedEvent::default();
        for w in watched {
            let found = match &w.session {
                Some(known) => Some(known.clone()),
                None => crate::resume::session_file(&w.argv, &w.cwd, w.started, &self.home, &mut claimed),
            };
            let Some((agent, session)) = found else { continue };
            claimed.insert(session.clone());
            let (mut tokens, _) = Self::read(&mut files, &session);
            seen.insert(session.clone());
            let subagents = if agent == "claude" { Self::subagents(&mut files, &session) } else { Vec::new() };
            for sub in &subagents {
                tokens += sub.tokens;
            }
            event.total += tokens;
            event.terminals.push(TerminalAgent {
                terminal_id: w.terminal_id.clone(),
                agent: agent.to_owned(),
                tokens,
                subagents,
            });
        }
        // Forget sessions no terminal runs any more (their subagents too).
        let subagent_dirs: Vec<PathBuf> = seen.iter().map(|s| s.with_extension("").join("subagents")).collect();
        for gone in before {
            if !seen.contains(&gone) && !subagent_dirs.iter().any(|d| gone.starts_with(d)) {
                files.remove(&gone);
            }
        }
        event
    }

    /// The last event returned, for a view that starts after it was sent.
    pub fn current(&self) -> AgentsChangedEvent {
        self.last.lock().expect("agent event lock poisoned").clone().unwrap_or_default()
    }

    /// `event`, if it differs from the last one returned.
    pub fn changed(&self, event: AgentsChangedEvent) -> Option<AgentsChangedEvent> {
        let mut last = self.last.lock().expect("agent event lock poisoned");
        if last.as_ref() == Some(&event) {
            return None;
        }
        *last = Some(event.clone());
        Some(event)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn append(path: &Path, lines: &[Value]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path).unwrap();
        for line in lines {
            writeln!(file, "{line}").unwrap();
        }
    }

    fn assistant(id: &str, output: u64, content: Value) -> Value {
        serde_json::json!({"type": "assistant", "message": {"id": id, "content": content, "usage": {
            "input_tokens": 2, "cache_creation_input_tokens": 100, "cache_read_input_tokens": 1000, "output_tokens": output}}})
    }

    #[test]
    fn counts_claude_tokens_with_subagents_and_shows_their_work() {
        let home = tempfile::tempdir().unwrap();
        let cwd = PathBuf::from(r"C:\work");
        let session = home.path().join(".claude/projects/C--work/s1.jsonl");
        // One message in two entries: counted once, with its final usage.
        append(
            &session,
            &[
                serde_json::json!({"type": "user", "message": {"content": "build it"}}),
                assistant("m1", 10, serde_json::json!([{"type": "text", "text": "On it."}])),
                assistant(
                    "m1",
                    50,
                    serde_json::json!([{"type": "tool_use", "name": "Agent", "input": {"description": "Explore"}}]),
                ),
            ],
        );
        let subagents = home.path().join(".claude/projects/C--work/s1/subagents");
        append(
            &subagents.join("agent-a1.jsonl"),
            &[
                serde_json::json!({"type": "user", "message": {"content": "Find the tests\nin src"}}),
                assistant(
                    "x1",
                    5,
                    serde_json::json!([{"type": "tool_use", "name": "Bash", "input": {"command": "ls src\nmore"}}]),
                ),
            ],
        );
        std::fs::write(
            subagents.join("agent-a1.meta.json"),
            r#"{"agentType":"Explore","description":"Find tests","finished":false}"#,
        )
        .unwrap();

        let watch = AgentWatch::new(home.path().to_owned());
        let watched = [Watched {
            terminal_id: "t1".into(),
            argv: vec!["claude".into()],
            cwd: cwd.clone(),
            started: None,
            session: None,
        }];
        let event = watch.scan(&watched);
        let terminal = &event.terminals[0];
        assert_eq!(terminal.agent, "claude");
        let sub = &terminal.subagents[0];
        assert_eq!(
            (sub.id.as_str(), sub.kind.as_str(), sub.description.as_str(), sub.finished),
            ("a1", "Explore", "Find tests", false)
        );
        assert_eq!(sub.lines, ["› Find the tests", "▸ Bash: ls src"]);
        assert_eq!(sub.tokens, TokenUsage { input: 1102, output: 5 });
        assert_eq!(terminal.tokens, TokenUsage { input: 2204, output: 55 });
        assert_eq!(event.total, terminal.tokens);
        assert!(watch.changed(event.clone()).is_some());
        assert!(watch.changed(watch.scan(&watched)).is_none(), "nothing new, nothing to send");

        // Only what is appended is read, and it adds up.
        append(&session, &[assistant("m2", 7, serde_json::json!([]))]);
        assert_eq!(watch.scan(&watched).total, TokenUsage { input: 3306, output: 62 });

        // An agent Godesk shows is found by its session, whatever runs.
        let shown = [Watched {
            terminal_id: "t2".into(),
            argv: Vec::new(),
            cwd: cwd.clone(),
            started: None,
            session: Some(("claude", session.clone())),
        }];
        let event = watch.scan(&shown);
        assert_eq!(event.terminals[0].terminal_id, "t2");
        assert_eq!(event.terminals[0].subagents.len(), 1);
    }

    #[test]
    fn takes_the_running_total_codex_records() {
        let home = tempfile::tempdir().unwrap();
        let file = home
            .path()
            .join(".codex/sessions/2026/10/21/rollout-2026-10-21T10-00-00-01a14283-4503-7e02-aee2-318be925e579.jsonl");
        let count = |input: u64, output: u64| serde_json::json!({"type": "event_msg", "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": input, "output_tokens": output}}}});
        append(
            &file,
            &[
                serde_json::json!({"type": "session_meta", "payload": {"cwd": r"C:\work"}}),
                count(100, 10),
                count(250, 30),
            ],
        );
        let watch = AgentWatch::new(home.path().to_owned());
        let watched = [Watched {
            terminal_id: "t1".into(),
            argv: vec!["codex".into()],
            cwd: r"C:\work".into(),
            started: None,
            session: None,
        }];
        assert_eq!(watch.scan(&watched).total, TokenUsage { input: 250, output: 30 });
    }
}

/// Reads this machine's real Claude Code sessions (read-only), to check the
/// format still matches. Run with `--ignored --nocapture`.
#[cfg(test)]
mod real_data {
    use super::*;

    #[test]
    #[ignore = "reads the real ~/.claude of this machine"]
    fn reads_real_claude_sessions() {
        let home = PathBuf::from(std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).unwrap());
        let cwd = PathBuf::from(std::env::var("GODESK_TEST_CWD").unwrap_or_else(|_| home.to_string_lossy().into()));
        if let Ok(session) = std::env::var("GODESK_TEST_SESSION") {
            let mut files = HashMap::new();
            let (tokens, _) = AgentWatch::read(&mut files, Path::new(&session));
            eprintln!("session file: {tokens:?}");
            for sub in AgentWatch::subagents(&mut files, Path::new(&session)) {
                eprintln!(
                    "  {} {:?} finished={} {:?} lines={} first={:?}",
                    sub.kind,
                    sub.description,
                    sub.finished,
                    sub.tokens,
                    sub.lines.len(),
                    sub.lines.iter().take(2).collect::<Vec<_>>()
                );
            }
            return;
        }
        let watch = AgentWatch::new(home);
        let event = watch.scan(&[Watched {
            terminal_id: "t".into(),
            argv: vec!["claude".into()],
            cwd,
            started: None,
            session: None,
        }]);
        for terminal in &event.terminals {
            eprintln!("{} session: {:?} with {} subagents", terminal.agent, terminal.tokens, terminal.subagents.len());
            for sub in &terminal.subagents {
                eprintln!(
                    "  {} {:?} finished={} {:?} lines={}",
                    sub.kind,
                    sub.description,
                    sub.finished,
                    sub.tokens,
                    sub.lines.len()
                );
                for line in sub.lines.iter().take(3) {
                    eprintln!("     {line}");
                }
            }
        }
    }
}
