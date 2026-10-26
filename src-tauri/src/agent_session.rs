//! The agent's own session.
//!
//! An agent terminal is not a terminal: the agent runs in it in its
//! machine-readable mode — JSON events, one per line — over pipes, not over a
//! console. (A console would not do: an agent asked for `--print` reads its
//! prompt from a pipe and refuses a terminal.)
//!
//! Godesk keeps the session: it starts the agent, sends what the user
//! writes, stops a turn when asked, and closes the agent when the window goes
//! away. Besides the agent's own lines, it writes a few of its own, all with
//! a `godesk.` type, so the conversation can be drawn again from the history
//! alone:
//! - `{"type":"godesk.prompt","text":…}` what the user wrote;
//! - `{"type":"godesk.interrupt"}` the user stopped the turn;
//! - `{"type":"godesk.permission","request_id":…,"decision":…}` the user
//!   answered a permission the agent asked for ("allow", "always", "deny");
//! - `{"type":"godesk.turn_end","code":…}` a one-turn agent finished;
//! - `{"type":"godesk.restart"}` the agent was started again (with
//!   `"reopened":true` when Godesk itself started again).
//!
//! Each carries `at`, in milliseconds since the epoch, so a turn's length
//! can be told again later.
//!
//! With a folder to keep them in, sessions outlive Godesk: each terminal's
//! agent, folder and conversation id go in `<terminal id>.json` and what the
//! agent wrote in `<terminal id>.jsonl`, so a restart shows the conversation
//! again and continues it.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::agent_terminal::{self, AgentKind, Launch};
use crate::error::{AppError, AppResult};

/// How often a running agent is checked for its end.
const EXIT_POLL: Duration = Duration::from_millis(150);

/// What an agent session needs to tell the frontend.
pub trait AgentSink: Send + Sync + 'static {
    /// A line the agent wrote (its protocol, an error on its stderr as
    /// `! …`, or one of Godesk's own `godesk.` events).
    fn on_line(&self, terminal_id: &str, line: &str);
    /// The agent is gone (a persistent agent only: a one-turn agent ends
    /// each turn with `godesk.turn_end` instead).
    fn on_exit(&self, terminal_id: &str, code: Option<i32>);
}

/// A running agent process.
struct Process {
    child: Arc<Mutex<Child>>,
    /// Open for a persistent agent; a one-turn agent's input is closed once
    /// the prompt is written.
    stdin: Option<ChildStdin>,
    #[cfg_attr(not(windows), allow(dead_code))]
    pid: u32,
}

struct Session {
    kind: AgentKind,
    /// How the user started it: the model, the conversation to continue.
    launch: Launch,
    /// Where the agent works.
    cwd: Option<PathBuf>,
    /// What it wrote, to show the conversation again when the window reopens.
    history: String,
    /// How many lines `history` holds.
    lines: u64,
    /// The agent's own session, to continue it on the next turn.
    session: Option<String>,
    process: Option<Process>,
    /// Where what the agent writes is kept, when sessions are kept.
    log: Option<File>,
    /// Added to the agent's environment: its terminal's (see
    /// [`crate::pty::PtyManager::environment`]).
    env: Vec<(String, std::ffi::OsString)>,
    /// Permissions the agent asked for and waits on, by request id: what
    /// the tool would do and the rules that would allow it from now on.
    pending: HashMap<String, (serde_json::Value, serde_json::Value)>,
    /// Context notes (a connected knowledge base, say) waiting to be prefixed
    /// to the user's next prompt, each tagged by what it is about so it can be
    /// retracted. Prefixing, rather than sending on its own, avoids starting a
    /// turn the user did not ask for.
    pending_context: Vec<(String, String)>,
}

impl Session {
    fn new(kind: AgentKind, cwd: Option<&Path>, launch: Launch) -> Self {
        Self {
            kind,
            launch,
            cwd: cwd.map(Path::to_path_buf),
            history: String::new(),
            lines: 0,
            session: None,
            process: None,
            log: None,
            env: Vec::new(),
            pending: HashMap::new(),
            pending_context: Vec::new(),
        }
    }

    fn saved(&self) -> Saved {
        let launch = Launch { prompt: None, continue_last: false, session: None, ..self.launch.clone() };
        Saved { agent: self.kind.program().to_owned(), cwd: self.cwd.clone(), session: self.session.clone(), launch }
    }
}

/// What is kept of a terminal's agent between runs of Godesk.
#[derive(Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct Saved {
    agent: String,
    cwd: Option<PathBuf>,
    session: Option<String>,
    launch: Launch,
}

/// The agents running in the terminals, by terminal id.
#[derive(Clone, Default)]
pub struct AgentSessions {
    inner: Arc<Mutex<HashMap<String, Session>>>,
    /// Where sessions are kept between runs; nowhere when `None`.
    dir: Option<PathBuf>,
}

impl AgentSessions {
    /// Sessions kept in `dir` between runs of Godesk.
    pub fn kept_in(dir: PathBuf) -> Self {
        Self { dir: Some(dir), ..Self::default() }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Session>> {
        self.inner.lock().expect("agent sessions lock poisoned")
    }

    /// The files a terminal's session is kept in: its description and its
    /// history. Ids are generated (UUIDs); anything else has none.
    fn files(&self, terminal_id: &str) -> Option<(PathBuf, PathBuf)> {
        let dir = self.dir.as_ref()?;
        let valid = !terminal_id.is_empty() && terminal_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
        valid.then(|| (dir.join(format!("{terminal_id}.json")), dir.join(format!("{terminal_id}.jsonl"))))
    }

    /// Writes down which agent a terminal runs, where, and its conversation.
    fn save(&self, terminal_id: &str, saved: &Saved) {
        let Some((meta, _)) = self.files(terminal_id) else { return };
        let written = serde_json::to_vec(saved).map_err(std::io::Error::other).and_then(|bytes| {
            std::fs::create_dir_all(meta.parent().expect("kept in a folder"))?;
            let partial = meta.with_extension("partial");
            std::fs::write(&partial, bytes)?;
            std::fs::rename(&partial, &meta)
        });
        if let Err(err) = written {
            eprintln!("godesk: cannot keep the agent of terminal {terminal_id}: {err}");
        }
    }

    /// Opens `agent`'s session in `terminal_id`, in `cwd`, as `launch` says,
    /// with `env` added to its environment. A persistent agent starts now; a
    /// one-turn agent starts with the first message.
    pub fn spawn(
        &self,
        terminal_id: &str,
        agent: &str,
        cwd: Option<&Path>,
        launch: Launch,
        env: Vec<(String, std::ffi::OsString)>,
        sink: Arc<dyn AgentSink>,
    ) -> AppResult<()> {
        let kind = AgentKind::from_name(agent).ok_or_else(|| AppError::Invalid(format!("unknown agent: {agent}")))?;
        if !agent_terminal::installed(kind, &launch) {
            return Err(AppError::Invalid(format!("{agent} is not installed, or not on PATH")));
        }
        self.close(terminal_id);
        let mut session = Session::new(kind, cwd, launch);
        session.env = env;
        // The conversation to continue, until the agent names its own.
        session.session = session.launch.session.clone();
        self.save(terminal_id, &session.saved());
        self.lock().insert(terminal_id.to_owned(), session);
        if kind.persistent() {
            self.start(terminal_id, None, &sink)?;
        }
        Ok(())
    }

    /// Opens again the session a terminal had when Godesk closed, with what
    /// the agent wrote then. False when there is none to open.
    pub fn reopen(
        &self,
        terminal_id: &str,
        env: Vec<(String, std::ffi::OsString)>,
        sink: Arc<dyn AgentSink>,
    ) -> AppResult<bool> {
        let Some((meta, log)) = self.files(terminal_id) else { return Ok(false) };
        let Some(saved) = std::fs::read(&meta).ok().and_then(|b| serde_json::from_slice::<Saved>(&b).ok()) else {
            return Ok(false);
        };
        let kind = AgentKind::from_name(&saved.agent)
            .ok_or_else(|| AppError::Invalid(format!("unknown agent: {}", saved.agent)))?;
        let mut session = Session::new(kind, saved.cwd.as_deref().filter(|dir| dir.is_dir()), saved.launch);
        session.session = saved.session;
        session.env = env;
        session.history = std::fs::read_to_string(&log).unwrap_or_default();
        if !session.history.is_empty() && !session.history.ends_with('\n') {
            session.history.push('\n'); // A line cut short by a crash.
        }
        session.lines = session.history.lines().count() as u64;
        self.lock().insert(terminal_id.to_owned(), session);
        if kind.persistent() {
            self.start(terminal_id, None, &sink)?;
        }
        sink.on_line(terminal_id, &serde_json::json!({ "type": "godesk.restart", "reopened": true, "at": now() }).to_string());
        Ok(true)
    }

    /// Starts the agent's process, writing `prompt` to it and closing its
    /// input when given (a one-turn agent).
    fn start(&self, terminal_id: &str, prompt: Option<&str>, sink: &Arc<dyn AgentSink>) -> AppResult<()> {
        let mut sessions = self.lock();
        let session = sessions.get_mut(terminal_id).ok_or_else(|| AppError::Pty("the agent is not open".into()))?;
        if session.process.is_some() {
            return Err(AppError::Invalid("the agent is still working; stop it first".into()));
        }
        let kind = session.kind;
        // A new process has asked for nothing yet.
        session.pending.clear();
        let argv = agent_terminal::argv(kind, session.session.as_deref(), &session.launch);
        let mut command = Command::new(&argv[0]);
        command.args(&argv[1..]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        if let Some(dir) = &session.cwd {
            command.current_dir(dir);
        }
        command.envs(session.env.iter().map(|(key, value)| (key, value)));
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // No console window of its own: Godesk draws the agent.
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = command.spawn().map_err(|e| AppError::Pty(format!("cannot start {}: {e}", argv[0])))?;
        let mut stdin = child.stdin.take();
        let stdout = child.stdout.take().ok_or_else(|| AppError::Pty("the agent has no output".into()))?;
        let stderr = child.stderr.take();
        if let Some(prompt) = prompt {
            if let Some(mut input) = stdin.take() {
                // Dropping the input ends it: the agent reads one prompt.
                input.write_all(prompt.as_bytes()).map_err(|e| AppError::Pty(e.to_string()))?;
            }
        }
        let pid = child.id();
        let child = Arc::new(Mutex::new(child));
        session.process = Some(Process { child: child.clone(), stdin, pid });
        drop(sessions);

        // What the agent writes, line by line; the session it names is
        // remembered, to continue it.
        let id = terminal_id.to_owned();
        let out_sink = sink.clone();
        let sessions = self.clone();
        let reader = std::thread::Builder::new()
            .name(format!("agent-out-{id}"))
            .spawn(move || {
                for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                    if let Some((request, asked)) = kind.permission_in(&line)
                        && let Some(session) = sessions.lock().get_mut(&id)
                    {
                        session.pending.insert(request, asked);
                    }
                    if let Some(found) = kind.session_in(&line) {
                        let changed = sessions.lock().get_mut(&id).filter(|s| s.session.as_ref() != Some(&found)).map(|s| {
                            s.session = Some(found);
                            s.saved()
                        });
                        if let Some(saved) = changed {
                            sessions.save(&id, &saved);
                        }
                    }
                    out_sink.on_line(&id, &line);
                }
            })
            .ok();

        // Its errors say why it stopped. A persistent agent's are shown as
        // they come; a one-turn agent's only when the turn fails, since a
        // turn that worked also writes progress there.
        let id = terminal_id.to_owned();
        let err_sink = sink.clone();
        let errors = std::thread::Builder::new()
            .name(format!("agent-err-{id}"))
            .spawn(move || {
                let mut kept = Vec::new();
                if let Some(stderr) = stderr {
                    for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                        if line.trim().is_empty() {
                            continue;
                        }
                        if kind.persistent() {
                            err_sink.on_line(&id, &format!("! {line}"));
                        } else {
                            kept.push(line);
                        }
                    }
                }
                kept
            })
            .ok();

        // The end of the process. The lock is only held to look, so the
        // agent can be written to and stopped meanwhile.
        let id = terminal_id.to_owned();
        let sink = sink.clone();
        let sessions = self.clone();
        std::thread::Builder::new()
            .name(format!("agent-wait-{id}"))
            .spawn(move || {
                let code = loop {
                    match child.lock().expect("agent child lock poisoned").try_wait() {
                        Ok(Some(status)) => break status.code(),
                        Ok(None) => {}
                        Err(_) => break None,
                    }
                    std::thread::sleep(EXIT_POLL);
                };
                // Everything the agent wrote comes before its end.
                if let Some(reader) = reader {
                    let _ = reader.join();
                }
                let kept = errors.and_then(|h: JoinHandle<Vec<String>>| h.join().ok()).unwrap_or_default();
                let mine = {
                    let mut all = sessions.lock();
                    match all.get_mut(&id) {
                        Some(session) if session.process.as_ref().is_some_and(|p| Arc::ptr_eq(&p.child, &child)) => {
                            session.process = None;
                            true
                        }
                        _ => false,
                    }
                };
                if !mine {
                    return; // Closed, or started again: that one reports.
                }
                if kind.persistent() {
                    sink.on_exit(&id, code);
                } else {
                    if code != Some(0) {
                        for line in kept.iter().rev().take(20).rev() {
                            sink.on_line(&id, &format!("! {line}"));
                        }
                    }
                    sink.on_line(&id, &serde_json::json!({ "type": "godesk.turn_end", "code": code, "at": now() }).to_string());
                }
            })
            .ok();
        Ok(())
    }

    /// Sends what the user wrote to the agent, prefixed with any context notes
    /// that were waiting (a knowledge base that was connected, say).
    pub fn send(&self, terminal_id: &str, text: &str, sink: Arc<dyn AgentSink>) -> AppResult<()> {
        let (kind, preface) = {
            let mut sessions = self.lock();
            let session = sessions.get_mut(terminal_id).ok_or_else(|| AppError::Pty("the agent is not open".into()))?;
            (session.kind, std::mem::take(&mut session.pending_context))
        };
        let full = if preface.is_empty() {
            text.to_owned()
        } else {
            let notes = preface.iter().map(|(_, note)| note.as_str()).collect::<Vec<_>>().join("\n");
            format!("{notes}\n\n{text}")
        };
        let message = kind.message(&full);
        // The prompt comes first in the conversation, before any answer. Only
        // the user's own words are shown; the context preface rides along to the
        // agent unseen.
        sink.on_line(terminal_id, &serde_json::json!({ "type": "godesk.prompt", "text": text, "at": now() }).to_string());
        if kind.persistent() {
            let mut sessions = self.lock();
            let process = sessions
                .get_mut(terminal_id)
                .and_then(|s| s.process.as_mut())
                .and_then(|p| p.stdin.as_mut())
                .ok_or_else(|| AppError::Pty("the agent is not running".into()))?;
            writeln!(process, "{message}").map_err(|e| AppError::Pty(e.to_string()))?;
            process.flush().map_err(|e| AppError::Pty(e.to_string()))?;
        } else {
            self.start(terminal_id, Some(&message), &sink)?;
        }
        Ok(())
    }

    /// Remembers a context note to prefix to the agent's next prompt, tagged by
    /// what it is about (`tag`), and records it in the conversation. Does not
    /// start a turn: the note reaches the agent with the next message the user
    /// (or another terminal) sends.
    pub fn inject_context(&self, terminal_id: &str, tag: &str, text: &str, sink: Arc<dyn AgentSink>) -> AppResult<()> {
        {
            let mut sessions = self.lock();
            let session = sessions.get_mut(terminal_id).ok_or_else(|| AppError::Pty("the agent is not open".into()))?;
            session.pending_context.push((tag.to_owned(), text.to_owned()));
        }
        sink.on_line(terminal_id, &serde_json::json!({ "type": "godesk.context", "text": text, "at": now() }).to_string());
        Ok(())
    }

    /// Drops any still-undelivered context notes with this tag. Returns whether
    /// one was removed: if so the agent never saw the note, so there is nothing
    /// to walk back.
    pub fn retract_context(&self, terminal_id: &str, tag: &str) -> bool {
        let mut sessions = self.lock();
        let Some(session) = sessions.get_mut(terminal_id) else { return false };
        let before = session.pending_context.len();
        session.pending_context.retain(|(t, _)| t != tag);
        before != session.pending_context.len()
    }

    /// Stops the turn the agent is working on. A persistent agent is asked
    /// and keeps its session; a one-turn agent's process is ended.
    pub fn interrupt(&self, terminal_id: &str, sink: Arc<dyn AgentSink>) -> AppResult<()> {
        let mut sessions = self.lock();
        let session = sessions.get_mut(terminal_id).ok_or_else(|| AppError::Pty("the agent is not open".into()))?;
        let Some(process) = session.process.as_mut() else { return Ok(()) };
        match (session.kind.interrupt_message(), process.stdin.as_mut()) {
            (Some(message), Some(stdin)) => {
                writeln!(stdin, "{message}").map_err(|e| AppError::Pty(e.to_string()))?;
                stdin.flush().map_err(|e| AppError::Pty(e.to_string()))?;
            }
            _ => kill(process),
        }
        drop(sessions);
        sink.on_line(terminal_id, &serde_json::json!({ "type": "godesk.interrupt", "at": now() }).to_string());
        Ok(())
    }

    /// Starts a persistent agent again after it stopped, on the same
    /// session, so the conversation goes on.
    pub fn restart(&self, terminal_id: &str, sink: Arc<dyn AgentSink>) -> AppResult<()> {
        let kind = self.kind(terminal_id).ok_or_else(|| AppError::Pty("the agent is not open".into()))?;
        if let Some(mut process) = self.lock().get_mut(terminal_id).and_then(|s| s.process.take()) {
            kill(&mut process);
        }
        if kind.persistent() {
            self.start(terminal_id, None, &sink)?;
        }
        sink.on_line(terminal_id, &serde_json::json!({ "type": "godesk.restart", "at": now() }).to_string());
        Ok(())
    }

    /// Answers a permission the agent asked for: `"allow"` this once,
    /// `"always"` (this once and, as the agent suggested, from now on), or
    /// `"deny"`.
    pub fn answer(&self, terminal_id: &str, request_id: &str, decision: &str, sink: Arc<dyn AgentSink>) -> AppResult<()> {
        {
            let mut sessions = self.lock();
            let session = sessions.get_mut(terminal_id).ok_or_else(|| AppError::Pty("the agent is not open".into()))?;
            let (input, rules) = session
                .pending
                .remove(request_id)
                .ok_or_else(|| AppError::Invalid("the agent no longer waits for this answer".into()))?;
            let answer = agent_terminal::permission_answer(request_id, decision, input, rules)
                .ok_or_else(|| AppError::Invalid(format!("unknown answer: {decision}")))?;
            let stdin = session
                .process
                .as_mut()
                .and_then(|p| p.stdin.as_mut())
                .ok_or_else(|| AppError::Pty("the agent is not running".into()))?;
            writeln!(stdin, "{answer}").map_err(|e| AppError::Pty(e.to_string()))?;
            stdin.flush().map_err(|e| AppError::Pty(e.to_string()))?;
        }
        sink.on_line(
            terminal_id,
            &serde_json::json!({ "type": "godesk.permission", "request_id": request_id, "decision": decision, "at": now() })
                .to_string(),
        );
        Ok(())
    }

    /// Changes the model the agent works with, from the next turn on (a
    /// persistent agent starts again, on the same conversation). `None`
    /// goes back to the agent's own default.
    pub fn set_model(&self, terminal_id: &str, model: Option<String>, sink: Arc<dyn AgentSink>) -> AppResult<()> {
        let (kind, saved, running) = {
            let mut sessions = self.lock();
            let session = sessions.get_mut(terminal_id).ok_or_else(|| AppError::Pty("the agent is not open".into()))?;
            session.launch.model = model.filter(|m| !m.trim().is_empty()).map(|m| m.trim().to_owned());
            (session.kind, session.saved(), session.process.is_some())
        };
        self.save(terminal_id, &saved);
        if kind.persistent() && running {
            self.restart(terminal_id, sink)?;
        }
        Ok(())
    }

    fn kind(&self, terminal_id: &str) -> Option<AgentKind> {
        self.lock().get(terminal_id).map(|s| s.kind)
    }

    /// What the agent wrote so far, to draw the conversation again, and how
    /// many lines that is.
    pub fn history(&self, terminal_id: &str) -> (String, u64) {
        self.lock().get(terminal_id).map(|s| (s.history.clone(), s.lines)).unwrap_or_default()
    }

    /// Where the agent works.
    pub fn cwd(&self, terminal_id: &str) -> Option<PathBuf> {
        self.lock().get(terminal_id).and_then(|s| s.cwd.clone())
    }

    /// Keeps what the agent wrote, so the window can show it again (after a
    /// restart too), and returns the line's number (from 1).
    pub fn remember(&self, terminal_id: &str, line: &str) -> u64 {
        let log = self.files(terminal_id).map(|(_, log)| log);
        let mut sessions = self.lock();
        let Some(session) = sessions.get_mut(terminal_id) else { return 0 };
        session.history.push_str(line);
        session.history.push('\n');
        session.lines += 1;
        if session.log.is_none()
            && let Some(path) = log
        {
            session.log = std::fs::OpenOptions::new().create(true).append(true).open(path).ok();
        }
        if let Some(file) = session.log.as_mut()
            && writeln!(file, "{line}").is_err()
        {
            session.log = None;
        }
        session.lines
    }

    /// Whether a terminal shows an agent.
    pub fn is_open(&self, terminal_id: &str) -> bool {
        self.lock().contains_key(terminal_id)
    }

    /// Shows `kind` in a terminal without starting it, for tests of what
    /// reaches it.
    #[cfg(test)]
    pub fn open_idle(&self, terminal_id: &str, kind: AgentKind) {
        self.lock().insert(terminal_id.to_owned(), Session::new(kind, None, Launch::default()));
    }

    /// How the agent of a terminal was started, for another like it: the
    /// program, the model and the permissions, without its conversation.
    pub fn launch(&self, terminal_id: &str) -> Option<Launch> {
        self.lock().get(terminal_id).map(|s| s.saved().launch)
    }

    /// The agent's own session (to continue it, or to read its files), once
    /// it named it.
    pub fn session_id(&self, terminal_id: &str) -> Option<String> {
        self.lock().get(terminal_id).and_then(|s| s.session.clone())
    }

    /// The terminals that show an agent.
    pub fn open_terminals(&self) -> Vec<String> {
        self.lock().keys().cloned().collect()
    }

    /// The agent a terminal shows, if any.
    pub fn agent(&self, terminal_id: &str) -> Option<AgentKind> {
        self.kind(terminal_id)
    }

    /// The end of the conversation, as text: the last `lines` lines.
    pub fn transcript(&self, terminal_id: &str, lines: usize) -> Option<String> {
        let (kind, history) = {
            let sessions = self.lock();
            let session = sessions.get(terminal_id)?;
            (session.kind, session.history.clone())
        };
        let text: Vec<String> = history
            .lines()
            .filter_map(|line| agent_terminal::transcript_line(kind, line))
            .flat_map(|entry| entry.lines().map(str::to_owned).collect::<Vec<_>>())
            .collect();
        Some(text[text.len().saturating_sub(lines.max(1))..].join("\n"))
    }

    /// Stops the agent of a terminal and forgets its conversation: the
    /// terminal is a shell again, or gone.
    pub fn close(&self, terminal_id: &str) {
        let session = self.lock().remove(terminal_id);
        if let Some(mut process) = session.and_then(|s| s.process) {
            kill(&mut process);
        }
        if let Some((meta, log)) = self.files(terminal_id) {
            let _ = std::fs::remove_file(meta);
            let _ = std::fs::remove_file(log);
        }
    }

    /// Removes the kept sessions of terminals that no longer exist.
    pub fn prune(&self, keep: &std::collections::HashSet<String>) {
        let Some(dir) = &self.dir else { return };
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            let id = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            if !keep.contains(&id) {
                let _ = std::fs::remove_file(path);
            }
        }
    }

    /// Stops every agent, keeping their conversations for the next start.
    pub fn close_all(&self) {
        let sessions: Vec<Session> = self.lock().drain().map(|(_, s)| s).collect();
        for mut process in sessions.into_iter().filter_map(|s| s.process) {
            kill(&mut process);
        }
    }
}

/// Milliseconds since the epoch.
fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// Ends an agent and whatever it started (`cmd` → `node` → the agent, for
/// an agent installed with npm on Windows).
fn kill(process: &mut Process) {
    process.stdin = None;
    #[cfg(windows)]
    crate::winproc::kill_descendants(process.pid);
    let _ = process.child.lock().expect("agent child lock poisoned").kill();
}

#[cfg(test)]
mod kept_tests {
    use super::*;

    #[derive(Default)]
    struct Collect(Mutex<Vec<String>>);

    impl AgentSink for Collect {
        fn on_line(&self, _: &str, line: &str) {
            self.0.lock().unwrap().push(line.to_owned());
        }
        fn on_exit(&self, _: &str, _: Option<i32>) {}
    }

    /// What a one-turn agent wrote and the conversation it was in come back
    /// with the next run of Godesk, until the terminal stops showing it.
    #[test]
    fn a_conversation_outlives_godesk() {
        let dir = tempfile::tempdir().unwrap();
        let sessions = AgentSessions::kept_in(dir.path().to_owned());
        let launch = Launch { model: Some("gpt-6-luna".into()), ..Launch::default() };
        sessions.lock().insert("t-1".into(), Session::new(AgentKind::Codex, Some(dir.path()), launch));
        sessions.save("t-1", &sessions.lock()["t-1"].saved());
        sessions.lock().get_mut("t-1").unwrap().session = Some("01a1".into());
        sessions.save("t-1", &sessions.lock()["t-1"].saved());
        assert_eq!(sessions.remember("t-1", r#"{"type":"godesk.prompt","text":"hola"}"#), 1);
        assert_eq!(sessions.remember("t-1", r#"{"type":"thread.started","thread_id":"01a1"}"#), 2);
        sessions.close_all();

        let again = AgentSessions::kept_in(dir.path().to_owned());
        let lines = Arc::new(Collect::default());
        assert!(again.reopen("t-1", Vec::new(), lines.clone()).unwrap());
        let (history, count) = again.history("t-1");
        assert_eq!(count, 2);
        assert!(history.contains("hola"));
        {
            let sessions = again.lock();
            let session = &sessions["t-1"];
            assert_eq!(
                (session.session.as_deref(), session.launch.model.as_deref()),
                (Some("01a1"), Some("gpt-6-luna"))
            );
            assert_eq!(session.cwd.as_deref(), Some(dir.path()));
        }
        assert!(lines.0.lock().unwrap()[0].contains("godesk.restart"));

        again.close("t-1");
        assert!(!AgentSessions::kept_in(dir.path().to_owned()).reopen("t-1", Vec::new(), lines).unwrap());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}

#[cfg(all(test, windows))]
mod tests {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    use super::*;

    #[derive(Default)]
    struct Collect(Mutex<Vec<String>>);

    impl AgentSink for Collect {
        fn on_line(&self, _: &str, line: &str) {
            self.0.lock().unwrap().push(line.to_owned());
        }
        fn on_exit(&self, _: &str, _: Option<i32>) {}
    }

    fn wait_for(lines: &Collect, what: &str) -> bool {
        let deadline = Instant::now() + Duration::from_secs(120);
        while Instant::now() < deadline {
            if lines.0.lock().unwrap().iter().any(|l| l.contains(what)) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        false
    }

    /// The real agent, in a session of its own: what the user writes reaches
    /// it and what it answers comes back. Needs Claude Code and a network.
    #[test]
    #[ignore = "starts the real agent"]
    fn the_agent_answers_what_the_user_writes() {
        let lines = Arc::new(Collect::default());
        let sessions = AgentSessions::default();
        sessions.spawn("a1", "claude", None, Launch::default(), Vec::new(), lines.clone()).unwrap();
        sessions.send("a1", "di solo: hola", lines.clone()).unwrap();
        assert!(wait_for(&lines, "\"type\":\"assistant\""), "the agent never answered: {:?}", lines.0.lock().unwrap());
        sessions.close_all();
    }

    /// A command that needs a permission is asked for, allowed from Godesk,
    /// and run. Needs Claude Code and a network.
    #[test]
    #[ignore = "starts the real agent"]
    fn a_permission_allowed_in_godesk_lets_the_command_run() {
        let lines = Arc::new(Collect::default());
        let sessions = AgentSessions::default();
        sessions.spawn("p1", "claude", None, Launch::default(), Vec::new(), lines.clone()).unwrap();
        sessions
            .send("p1", "Run this exact command with the Bash tool and nothing else: whoami /user", lines.clone())
            .unwrap();
        assert!(wait_for(&lines, "can_use_tool"), "no permission asked: {:?}", lines.0.lock().unwrap());
        let request = sessions.lock()["p1"].pending.keys().next().cloned().unwrap();
        sessions.answer("p1", &request, "allow", lines.clone()).unwrap();
        assert!(wait_for(&lines, "\"type\":\"result\""));
        let all = lines.0.lock().unwrap().join("\n");
        assert!(all.contains("godesk.permission"));
        assert!(all.contains("\"decision\":\"accept\""), "the command was not run as allowed: {all}");
        sessions.close_all();
    }

    /// A one-turn agent answers, ends its turn, and continues its session.
    /// Needs OpenCode and a network.
    #[test]
    #[ignore = "starts the real agent"]
    fn a_one_turn_agent_continues_its_session() {
        let lines = Arc::new(Collect::default());
        let sessions = AgentSessions::default();
        sessions.spawn("o1", "opencode", None, Launch::default(), Vec::new(), lines.clone()).unwrap();
        sessions.send("o1", "di solo: uno", lines.clone()).unwrap();
        assert!(wait_for(&lines, "godesk.turn_end"), "no end of turn: {:?}", lines.0.lock().unwrap());
        assert!(sessions.lock().get("o1").unwrap().session.is_some());
        lines.0.lock().unwrap().clear();
        sessions.send("o1", "di solo: dos", lines.clone()).unwrap();
        assert!(wait_for(&lines, "godesk.turn_end"));
        sessions.close_all();
    }
}
