//! `#[tauri::command]` handlers. Every mutation goes through [`Model`] (which
//! validates it), schedules a save, and only then touches processes.
//!
//! [`Model`]: crate::state::Model

use std::collections::HashSet;
use std::path::Path;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use tauri::{AppHandle, Manager, State};
use tauri_plugin_dialog::DialogExt;

use crate::core::{Core, now_ms};
use crate::embed::Apps;
use crate::error::{AppError, AppResult};
use crate::model::{
    AppLaunch, AppPlacement, AppSnapshot, AttachResult, Camera, EmbeddedApp, KnowledgeBase, Link, NativeWindow, Rect,
    TerminalWindow, Workspace,
};
use crate::agent_terminal::{AgentKind, Launch};
use crate::pty::{DEFAULT_COLS, DEFAULT_ROWS, SpawnRequest};

/// A terminal now shows its agent, or its shell again.
pub const TERMINAL_CHANGED_EVENT: &str = "terminal-changed";

/// Written to a terminal whose agent's own screen was closed under it, so
/// the shell gets the terminal back as it expects it: attributes reset, the
/// main screen, the cursor shown, and no mouse, focus or paste reporting.
const SCREEN_RESET: &[u8] =
    b"\x1b[0m\x1b[?1049l\x1b[?25h\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l\x1b[?1004l\x1b[?2004l\r\n";

impl Core {
    fn mutate<T>(&self, change: impl FnOnce(&mut crate::state::Model) -> AppResult<T>) -> AppResult<T> {
        let result = change(&mut self.shared.model())?;
        self.shared.persister.mark_dirty();
        Ok(result)
    }

    /// Starts the shell of a terminal that exists in the model.
    pub fn spawn_terminal(&self, terminal_id: &str, cols: u16, rows: u16) -> AppResult<()> {
        self.spawn_terminal_in(terminal_id, None, None, cols, rows)
    }

    /// Like [`Self::spawn_terminal`], starting in `cwd` instead of home and
    /// showing `history` first.
    fn spawn_terminal_in(
        &self,
        terminal_id: &str,
        cwd: Option<&Path>,
        history: Option<&[u8]>,
        cols: u16,
        rows: u16,
    ) -> AppResult<()> {
        let (name, workspace_id) = {
            let model = self.shared.model();
            let terminal = model.terminal(terminal_id).ok_or_else(|| AppError::terminal(terminal_id))?;
            let workspace = model.workspace_of(terminal_id).expect("terminal belongs to a workspace");
            (terminal.name.clone(), workspace.id.clone())
        };
        self.ptys.spawn(&SpawnRequest {
            terminal_id,
            terminal_name: &name,
            workspace_id: &workspace_id,
            cwd,
            history,
            cols,
            rows,
        })
    }

    /// Adds a copy of `source_id` at `rect` (see [`duplicate_terminal`]).
    pub(crate) fn duplicate_terminal(&self, source_id: &str, rect: Rect) -> AppResult<TerminalWindow> {
        let terminal = self.mutate(|m| m.duplicate_terminal(source_id, rect))?;
        // A source that shows an agent is copied as that agent, in its
        // folder, with a conversation of its own.
        let shown = self.agents.agent(source_id).zip(self.agents.launch(source_id));
        let activity = self.ptys.activity(source_id);
        let cwd = if shown.is_some() { self.agents.cwd(source_id).or(activity.cwd.clone()) } else { activity.cwd.clone() };
        if let Err(err) = self.spawn_terminal_in(&terminal.id, cwd.as_deref(), None, DEFAULT_COLS, DEFAULT_ROWS) {
            self.mutate(|m| m.remove_terminal(&terminal.id))?;
            return Err(err);
        }
        if let Some((agent, launch)) = shown {
            // If the agent cannot start, the copy is still a working shell.
            return Ok(self.attach_agent(&terminal.id, agent, launch, cwd.as_deref()).unwrap_or_else(|err| {
                eprintln!("godesk: cannot open {} in terminal {}: {err}", agent.program(), terminal.id);
                terminal
            }));
        }
        // Typed into the new shell, so the program runs inside a shell like
        // the source's and the shell stays when it exits. The tty holds the
        // line until the shell is ready to read it. If that fails the copy
        // is still a working shell, so it is returned rather than lost.
        if let Some(line) = activity.program.as_deref().and_then(|argv| self.ptys.command_line(argv))
            && let Err(err) = self.ptys.write(&terminal.id, format!("{line}\r").into_bytes())
        {
            eprintln!("godesk: cannot start the program in terminal {}: {err}", terminal.id);
        }
        Ok(terminal)
    }

    fn close_terminals(&self, terminal_ids: &[String]) {
        for id in terminal_ids {
            self.ptys.close(id);
            self.sessions.forget(id);
            self.agents.close(id);
        }
    }

    /// The sink that carries what an agent writes to the frontend.
    pub fn agent_sink(&self) -> std::sync::Arc<dyn crate::agent_session::AgentSink> {
        std::sync::Arc::new(crate::core::AgentEvents { shared: self.shared.clone(), sessions: self.agents.clone() })
    }

    /// The environment a terminal's agent gets: its shell's (see
    /// [`crate::pty::PtyManager::environment`]).
    fn agent_env(&self, terminal_id: &str) -> Vec<(String, std::ffi::OsString)> {
        let (name, workspace_id) = {
            let model = self.shared.model();
            let name = model.terminal(terminal_id).map(|t| t.name.clone()).unwrap_or_default();
            let workspace = model.workspace_of(terminal_id).map(|w| w.id.clone()).unwrap_or_default();
            (name, workspace)
        };
        self.ptys.environment(terminal_id, &name, &workspace_id)
    }

    /// Shows `agent` in the terminal instead of its shell, working in `cwd`
    /// as `launch` says. The shell stays, under it, for when the user goes
    /// back to it.
    pub(crate) fn attach_agent(
        &self,
        terminal_id: &str,
        agent: AgentKind,
        launch: Launch,
        cwd: Option<&Path>,
    ) -> AppResult<TerminalWindow> {
        let prompt = launch.prompt.clone();
        let sink = self.agent_sink();
        self.agents.spawn(terminal_id, agent.program(), cwd, launch, self.agent_env(terminal_id), sink.clone())?;
        let terminal = match self.mutate(|m| m.set_terminal_agent(terminal_id, Some(agent.program()))) {
            Ok(terminal) => terminal,
            Err(err) => {
                self.agents.close(terminal_id);
                return Err(err);
            }
        };
        self.shared.emit(TERMINAL_CHANGED_EVENT, &terminal);
        if let Some(prompt) = prompt
            && let Err(err) = self.agents.send(terminal_id, &prompt, sink)
        {
            eprintln!("godesk: cannot give the agent of terminal {terminal_id} its prompt: {err}");
        }
        Ok(terminal)
    }

    /// Closes the terminal's agent and shows its shell again.
    pub(crate) fn detach_agent(&self, terminal_id: &str) -> AppResult<TerminalWindow> {
        self.agents.close(terminal_id);
        let terminal = self.mutate(|m| m.set_terminal_agent(terminal_id, None))?;
        // A terminal that only ever showed an agent gets its shell now.
        if !self.ptys.has_session(terminal_id) {
            self.spawn_terminal(terminal_id, DEFAULT_COLS, DEFAULT_ROWS)?;
        }
        self.shared.emit(TERMINAL_CHANGED_EVENT, &terminal);
        Ok(terminal)
    }

    /// Closes Godesk's view of the terminal's agent and opens the agent on
    /// its own screen in the shell, in the same conversation. Godesk leaves
    /// it there until it exits.
    pub(crate) fn to_classic(&self, terminal_id: &str) -> AppResult<TerminalWindow> {
        let agent = self.agents.agent(terminal_id).ok_or_else(|| AppError::Invalid("no agent is shown here".into()))?;
        let launch = self.agents.launch(terminal_id).unwrap_or_default();
        let session = self.agents.session_id(terminal_id);
        let argv = crate::agent_terminal::tui_argv(agent, &launch, session.as_deref());
        self.classic.lock().expect("classic lock poisoned").insert(terminal_id.to_owned(), std::time::Instant::now());
        let terminal = self.detach_agent(terminal_id)?;
        if let Some(line) = self.ptys.command_line(&argv) {
            self.ptys.write(terminal_id, format!("{line}\r").into_bytes())?;
        }
        Ok(terminal)
    }

    /// Whether the terminal's agent was sent to its own screen and still
    /// runs there (`running`), so it is left alone. Once it has exited,
    /// after a moment for it to start, the terminal is like any other.
    fn left_on_own_screen(&self, terminal_id: &str, running: bool) -> bool {
        /// How long an agent typed into a shell may take to start.
        const START_GRACE: std::time::Duration = std::time::Duration::from_secs(10);
        let mut classic = self.classic.lock().expect("classic lock poisoned");
        match classic.get(terminal_id) {
            Some(_) if running => true,
            Some(since) if since.elapsed() < START_GRACE => true,
            Some(_) => {
                classic.remove(terminal_id);
                false
            }
            None => false,
        }
    }

    /// The session files of the agents Godesk shows, which no agent on its
    /// own screen can be working on.
    fn shown_session_files(&self) -> std::collections::HashSet<std::path::PathBuf> {
        self.agents
            .open_terminals()
            .into_iter()
            .filter_map(|id| {
                let agent = self.agents.agent(&id)?;
                let session = self.agents.session_id(&id)?;
                crate::resume::known_session_file(agent.program(), &session, &self.agents.cwd(&id)?, &self.home)
            })
            .collect()
    }

    /// Takes over the agents the shells start: a shell whose foreground
    /// program is an agent Godesk draws (`claude`, `codex`, `opencode`,
    /// `mimo`, only opening a conversation) has it closed, before it draws
    /// much of its own screen, and the terminal shows Godesk's view of it
    /// instead, in the same folder and conversation. However the command
    /// was written (typed, from history, pasted, sent with `godesk send`),
    /// what counts is the program that runs.
    ///
    /// `checked` holds, by terminal, the foreground program last found not
    /// to be one, so a long-running program is looked at once.
    pub fn take_over_agents(&self, checked: &mut std::collections::HashMap<String, u32>) {
        if !self.agent_view.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        let shells = self.ptys.shell_pids();
        // One look at the process table finds the shells that run anything.
        #[cfg(windows)]
        let busy = crate::winproc::busy(&shells.iter().map(|(_, pid)| *pid).collect::<Vec<_>>());
        checked.retain(|id, _| shells.iter().any(|(shell, _)| shell == id));
        for (terminal_id, _shell) in shells {
            #[cfg(windows)]
            if !busy.contains(&_shell) {
                checked.remove(&terminal_id);
                self.left_on_own_screen(&terminal_id, false);
                continue;
            }
            if self.agents.is_open(&terminal_id) {
                continue;
            }
            let program = self.ptys.foreground_pid(&terminal_id);
            if self.left_on_own_screen(&terminal_id, program.is_some()) {
                continue;
            }
            let Some(program) = program else {
                checked.remove(&terminal_id);
                continue;
            };
            if checked.get(&terminal_id) == Some(&program) {
                continue;
            }
            let activity = self.ptys.activity(&terminal_id);
            let Some((agent, mut launch)) = activity.program.as_deref().and_then(crate::agent_terminal::launch_of)
            else {
                checked.insert(terminal_id, program);
                continue;
            };
            // An agent that ran on its own screen for a while (the view was
            // just turned on) goes on with the conversation it was in.
            #[cfg(windows)]
            let started = crate::winproc::started(program);
            #[cfg(not(windows))]
            let started: Option<std::time::SystemTime> = None;
            let ran_a_while = started
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|ran| ran > std::time::Duration::from_secs(5));
            if ran_a_while
                && launch.session.is_none()
                && !launch.continue_last
                && let Some(cwd) = activity.cwd.as_deref()
            {
                launch.session = crate::resume::running_session(
                    agent.program(),
                    cwd,
                    started,
                    &self.home,
                    self.shown_session_files(),
                );
            }
            // Closed only when Godesk can start it: otherwise it runs as it is.
            if !crate::agent_terminal::installed(agent, &launch) {
                checked.insert(terminal_id, program);
                continue;
            }
            if !self.ptys.stop_foreground(&terminal_id) {
                continue;
            }
            self.ptys.inject(&terminal_id, SCREEN_RESET);
            if let Err(err) = self.attach_agent(&terminal_id, agent, launch, activity.cwd.as_deref()) {
                let line = format!("\x1b[31mgodesk: cannot open {} here: {err}\x1b[0m\r\n", agent.program());
                self.ptys.inject(&terminal_id, line.as_bytes());
            }
        }
    }

    /// Opens again the agent a terminal showed when Godesk closed, with its
    /// conversation. A terminal whose agent cannot start shows its shell.
    pub fn reopen_agent(&self, terminal_id: &str, agent: &str) {
        let reopened = match self.agents.reopen(terminal_id, self.agent_env(terminal_id), self.agent_sink()) {
            Ok(true) => Ok(()),
            // Kept by an earlier Godesk without its conversation: a new one.
            Ok(false) => match AgentKind::from_name(agent) {
                Some(kind) => {
                    let cwd = self.ptys.activity(terminal_id).cwd;
                    let env = self.agent_env(terminal_id);
                    self.agents.spawn(terminal_id, kind.program(), cwd.as_deref(), Launch::default(), env, self.agent_sink())
                }
                None => Err(AppError::Invalid(format!("unknown agent: {agent}"))),
            },
            Err(err) => Err(err),
        };
        if let Err(err) = reopened {
            eprintln!("godesk: cannot open the agent of terminal {terminal_id} again: {err}");
            self.agents.close(terminal_id);
            let _ = self.mutate(|m| m.set_terminal_agent(terminal_id, None));
        }
    }

    /// Records what every terminal is doing (see [`crate::session`]), so a
    /// restart reopens it there, with its output and its agent.
    pub fn save_sessions(&self) {
        let ids: Vec<String> =
            self.shared.model().workspaces().iter().flat_map(|w| w.terminals.iter().map(|t| t.id.clone())).collect();
        let mut claimed = HashSet::new();
        for id in &ids {
            let Ok((output, _, alive)) = self.ptys.snapshot(id) else { continue };
            let previous = self.sessions.load(id).0;
            let saved = if alive {
                let activity = self.ptys.activity(id);
                #[cfg(windows)]
                let started = self.ptys.foreground_pid(id).and_then(crate::winproc::started);
                #[cfg(not(windows))]
                let started = None;
                let restore = activity.program.as_deref().and_then(|argv| {
                    crate::resume::plan(argv, activity.cwd.as_deref(), started, &self.home, &mut claimed)
                });
                crate::session::Saved { cwd: activity.cwd.or(previous.cwd), restore }
            } else {
                crate::session::Saved { restore: None, ..previous }
            };
            if let Err(err) = self.sessions.save(id, &saved, &output) {
                eprintln!("godesk: cannot save terminal {id}: {err}");
            }
        }
        let ids = ids.into_iter().collect();
        self.agents.prune(&ids);
        self.sessions.prune(&ids);
    }

    /// Records the folder each terminal is working in, so the folder palette
    /// and the `z` jump learn where you spend time. Saved when it changed.
    pub fn record_dirs(&self) {
        let ids: Vec<String> =
            self.shared.model().workspaces().iter().flat_map(|w| w.terminals.iter().map(|t| t.id.clone())).collect();
        for id in ids {
            if let Some(cwd) = self.agents.cwd(&id).or_else(|| self.ptys.activity(&id).cwd) {
                self.dirs.add(&cwd);
            }
        }
        self.dirs.save_if_dirty();
    }

    /// The terminals that run a program, for [`crate::agent_watch`].
    pub fn watched_agents(&self) -> Vec<crate::agent_watch::Watched> {
        let ids: Vec<String> =
            self.shared.model().workspaces().iter().flat_map(|w| w.terminals.iter().map(|t| t.id.clone())).collect();
        ids.into_iter()
            .filter_map(|terminal_id| {
                // An agent shown in Godesk's view: count it by the session
                // file Godesk knows. If that file is not resolved yet (the
                // agent has not reported its id), fall back to finding it by
                // program and folder, as a plain shell agent is found, so its
                // tokens still show instead of dropping the terminal.
                if let Some(agent) = self.agents.agent(&terminal_id) {
                    let cwd = self.agents.cwd(&terminal_id)?;
                    let label = if agent == AgentKind::Claude { "claude" } else { "codex" };
                    let file = self
                        .agents
                        .session_id(&terminal_id)
                        .and_then(|id| crate::resume::known_session_file(agent.program(), &id, &cwd, &self.home));
                    return Some(crate::agent_watch::Watched {
                        argv: vec![agent.program().to_owned()],
                        cwd,
                        started: None,
                        session: file.map(|file| (label, file)),
                        terminal_id,
                    });
                }
                let activity = self.ptys.activity(&terminal_id);
                #[cfg(windows)]
                let started = self.ptys.foreground_pid(&terminal_id).and_then(crate::winproc::started);
                #[cfg(not(windows))]
                let started = None;
                Some(crate::agent_watch::Watched {
                    argv: activity.program?,
                    cwd: activity.cwd?,
                    started,
                    session: None,
                    terminal_id,
                })
            })
            .collect()
    }

    /// Starts a terminal's shell where it was before Godesk restarted, with
    /// the output it showed and, typed in, what ran in it (see
    /// [`crate::resume`]). A folder since removed falls back to home.
    pub fn restore_terminal(&self, terminal_id: &str) -> AppResult<()> {
        let (saved, output) = self.sessions.load(terminal_id);
        let cwd = saved.cwd.filter(|dir| dir.is_dir());
        let history = output.map(|mut bytes| {
            bytes.extend_from_slice(crate::session::RESTORED_MARKER);
            bytes
        });
        self.spawn_terminal_in(terminal_id, cwd.as_deref(), history.as_deref(), DEFAULT_COLS, DEFAULT_ROWS)?;
        if let Some(restore) = saved.restore
            && let Some(line) = self.ptys.command_line(&restore.argv)
        {
            let line = if restore.run { format!("{line}\r") } else { line };
            self.ptys.write(terminal_id, line.into_bytes())?;
        }
        Ok(())
    }

    fn notify_links(&self, workspace_id: &str) {
        let links = self.shared.model().links_of(workspace_id);
        self.shared.emit_links(workspace_id, links);
    }

    fn notify_knowledge_bases(&self, workspace_id: &str) {
        let knowledge_bases = self.shared.model().knowledge_bases_of(workspace_id);
        self.shared.emit_knowledge_bases(workspace_id, knowledge_bases);
    }

    /// `(knowledge base id, terminal id)` when these two endpoints are exactly a
    /// knowledge base and a terminal; otherwise `None`.
    fn kb_and_terminal(&self, a: &str, b: &str) -> Option<(String, String)> {
        let model = self.shared.model();
        if model.knowledge_base(a).is_some() && model.terminal(b).is_some() {
            Some((a.to_owned(), b.to_owned()))
        } else if model.knowledge_base(b).is_some() && model.terminal(a).is_some() {
            Some((b.to_owned(), a.to_owned()))
        } else {
            None
        }
    }

    /// Pre-indexes a knowledge base on a background thread so the first search
    /// over it is instant instead of paying for the walk and text extraction.
    /// Called when the base's node is created and when it is connected to a
    /// terminal; it is best-effort and idempotent, so a repeat call is cheap.
    fn warm_knowledge_base(&self, kb_id: &str, path: &str) {
        let kb = self.kb.clone();
        let (kb_id, path) = (kb_id.to_owned(), path.to_owned());
        let spawned = std::thread::Builder::new()
            .name("godesk-kb-warm".into())
            .spawn(move || kb.warm(&kb_id, Path::new(&path)));
        if let Err(err) = spawned {
            eprintln!("godesk: cannot pre-index a knowledge base: {err}");
        }
    }

    /// Tells a terminal's agent, if it shows one, that a knowledge base is now
    /// readable by it. The note rides along with the agent's next prompt.
    fn announce_kb_connected(&self, kb_id: &str, terminal_id: &str) {
        if !self.agents.is_open(terminal_id) {
            return;
        }
        let Some(kb) = self.shared.model().knowledge_base(kb_id).cloned() else { return };
        let count = self.kb.index(&kb.id, Path::new(&kb.path)).entries.len();
        let text = format!(
            "[Godesk] Knowledge base \"{}\" is now connected to this terminal ({count} file(s) available to you). \
             Run `godesk kb` to see its files and `godesk kb read <file>` to read one. \
             You can read it only while the arrow stays connected.",
            kb.name
        );
        let _ = self.agents.inject_context(terminal_id, kb_id, &text, self.agent_sink());
    }

    /// Tells a terminal's agent that a knowledge base is no longer readable. If
    /// the connection note was never delivered, it is dropped instead.
    fn announce_kb_disconnected(&self, kb_id: &str, kb_name: &str, terminal_id: &str) {
        if !self.agents.is_open(terminal_id) {
            return;
        }
        if self.agents.retract_context(terminal_id, kb_id) {
            return;
        }
        let text = format!(
            "[Godesk] Knowledge base \"{kb_name}\" is no longer connected to this terminal; you can no longer read it."
        );
        let _ = self.agents.inject_context(terminal_id, kb_id, &text, self.agent_sink());
    }
}

#[tauri::command]
pub fn get_state(core: State<'_, Core>) -> AppSnapshot {
    core.shared.model().snapshot()
}

#[tauri::command]
pub fn create_workspace(core: State<'_, Core>, name: Option<String>) -> AppResult<Workspace> {
    core.mutate(|m| m.create_workspace(name.as_deref()))
}

#[tauri::command]
pub fn rename_workspace(core: State<'_, Core>, workspace_id: String, name: String) -> AppResult<()> {
    core.mutate(|m| m.rename_workspace(&workspace_id, &name))
}

/// Deletes a workspace, stops its shells and returns the new active
/// workspace id.
#[tauri::command]
pub fn delete_workspace(core: State<'_, Core>, workspace_id: String) -> AppResult<String> {
    let (terminals, active) = core.mutate(|m| {
        let terminals = m.delete_workspace(&workspace_id)?;
        Ok((terminals, m.active_workspace_id().to_owned()))
    })?;
    core.close_terminals(&terminals);
    Ok(active)
}

#[tauri::command]
pub fn set_active_workspace(core: State<'_, Core>, workspace_id: String) -> AppResult<()> {
    core.mutate(|m| m.set_active_workspace(&workspace_id))
}

#[tauri::command]
pub fn update_camera(core: State<'_, Core>, workspace_id: String, camera: Camera) -> AppResult<()> {
    core.mutate(|m| m.update_camera(&workspace_id, camera))
}

/// Adds a terminal at `rect` (canvas coordinates) and starts its shell.
#[tauri::command]
pub async fn create_terminal(core: State<'_, Core>, workspace_id: String, rect: Rect) -> AppResult<TerminalWindow> {
    let core = core.inner().clone();
    // Starting a shell is slow (on Windows it spawns a ConPTY), so it does not
    // block the webview while it waits.
    tauri::async_runtime::spawn_blocking(move || {
        let terminal = core.mutate(|m| m.add_terminal(&workspace_id, rect))?;
        if let Err(err) = core.spawn_terminal(&terminal.id, DEFAULT_COLS, DEFAULT_ROWS) {
            let _ = core.mutate(|m| m.remove_terminal(&terminal.id));
            return Err(err);
        }
        Ok(terminal)
    })
    .await
    .map_err(|e| AppError::Invalid(e.to_string()))?
}

/// Adds a terminal at `rect` whose shell starts in `cwd` (the folder palette
/// uses this to drop a terminal straight into a frecent folder). A folder that
/// no longer exists falls back to home.
#[tauri::command]
pub async fn create_terminal_in(
    core: State<'_, Core>,
    workspace_id: String,
    rect: Rect,
    cwd: String,
) -> AppResult<TerminalWindow> {
    let core = core.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let dir = std::path::PathBuf::from(&cwd);
        let folder = dir.is_dir().then_some(dir.as_path());
        let terminal = core.mutate(|m| m.add_terminal(&workspace_id, rect))?;
        if let Err(err) = core.spawn_terminal_in(&terminal.id, folder, None, DEFAULT_COLS, DEFAULT_ROWS) {
            let _ = core.mutate(|m| m.remove_terminal(&terminal.id));
            return Err(err);
        }
        if folder.is_some() {
            core.dirs.add(&dir);
        }
        Ok(terminal)
    })
    .await
    .map_err(|e| AppError::Invalid(e.to_string()))?
}

/// The folders matching `query`, most frecent first (see [`crate::dirs`]).
#[tauri::command]
pub fn list_dirs(core: State<'_, Core>, query: String) -> Vec<String> {
    core.dirs.matches(&query, 50)
}

/// Whether the offline voice model and engine are installed (see [`crate::whisper`]).
#[tauri::command]
pub fn whisper_ready(core: State<'_, Core>) -> bool {
    crate::whisper::ready(&core.data_dir)
}

/// Whether live (streaming) dictation is available (the persistent server).
#[tauri::command]
pub fn whisper_can_stream(core: State<'_, Core>) -> bool {
    crate::whisper::can_stream(&core.data_dir)
}

/// Downloads the offline voice model and engine, emitting `whisper-progress`.
#[tauri::command]
pub async fn whisper_install(core: State<'_, Core>) -> AppResult<()> {
    let core = core.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        crate::whisper::install(&core.data_dir, &|progress| core.shared.emit("whisper-progress", progress))
    })
    .await
    .map_err(|e| AppError::Invalid(e.to_string()))?
}

/// Transcribes a 16 kHz mono WAV (base64) offline in `lang` (e.g. "es", or
/// "auto"), returning the text.
#[tauri::command]
pub async fn transcribe(core: State<'_, Core>, wav: String, lang: String) -> AppResult<String> {
    let bytes = BASE64.decode(wav).map_err(|e| AppError::Invalid(format!("audio is not base64: {e}")))?;
    let core = core.inner().clone();
    tauri::async_runtime::spawn_blocking(move || crate::whisper::transcribe(&core.data_dir, &bytes, &lang))
        .await
        .map_err(|e| AppError::Invalid(e.to_string()))?
}

/// Opens a copy of a terminal: same workspace, a name derived from the
/// source's, and a shell in the source's working directory that starts the
/// program the source runs in its foreground, if any. Links are not copied.
/// Directory and program are read on Linux and Windows; on macOS the copy
/// is a shell in home.
#[tauri::command]
pub fn duplicate_terminal(core: State<'_, Core>, terminal_id: String, rect: Rect) -> AppResult<TerminalWindow> {
    core.duplicate_terminal(&terminal_id, rect)
}

/// Closes the agent a terminal shows and shows its shell again.
#[tauri::command]
pub fn detach_agent(core: State<'_, Core>, terminal_id: String) -> AppResult<TerminalWindow> {
    core.detach_agent(&terminal_id)
}

/// Whether an agent started in a shell is shown by Godesk (the default) or
/// runs on its own screen.
/// Turned off, every agent Godesk shows goes back to its own screen, in the
/// same conversation; turned on, the agents running on their own screens
/// come to Godesk's view.
#[tauri::command]
pub fn set_agent_view(core: State<'_, Core>, on: bool) {
    core.agent_view.store(on, std::sync::atomic::Ordering::Relaxed);
    if on {
        // Taken over as soon as the takeover looks, however they started.
        core.classic.lock().expect("classic lock poisoned").clear();
        return;
    }
    for terminal_id in core.agents.open_terminals() {
        if let Err(err) = core.to_classic(&terminal_id) {
            eprintln!("godesk: cannot open the agent of terminal {terminal_id} on its own screen: {err}");
        }
    }
}

/// Opens the agent of one terminal on its own screen, in the same
/// conversation, instead of Godesk's view.
#[tauri::command]
pub fn agent_to_classic(core: State<'_, Core>, terminal_id: String) -> AppResult<TerminalWindow> {
    core.to_classic(&terminal_id)
}

/// Sends what the user wrote to the agent of a terminal.
#[tauri::command]
pub fn send_agent_message(core: State<'_, Core>, terminal_id: String, text: String) -> AppResult<()> {
    core.agents.send(&terminal_id, &text, core.agent_sink())
}

/// Stops the turn the agent of a terminal is working on.
#[tauri::command]
pub fn interrupt_agent(core: State<'_, Core>, terminal_id: String) -> AppResult<()> {
    core.agents.interrupt(&terminal_id, core.agent_sink())
}

/// Starts the agent of a terminal again, on the same conversation.
#[tauri::command]
pub fn restart_agent(core: State<'_, Core>, terminal_id: String) -> AppResult<()> {
    core.agents.restart(&terminal_id, core.agent_sink())
}

/// Answers a permission the agent of a terminal asked for: `"allow"`,
/// `"always"` or `"deny"`.
#[tauri::command]
pub fn answer_agent_permission(
    core: State<'_, Core>,
    terminal_id: String,
    request_id: String,
    decision: String,
) -> AppResult<()> {
    core.agents.answer(&terminal_id, &request_id, &decision, core.agent_sink())
}

/// Changes the model the agent of a terminal works with; `None` goes back
/// to the agent's default.
#[tauri::command]
pub fn set_agent_model(core: State<'_, Core>, terminal_id: String, model: Option<String>) -> AppResult<()> {
    core.agents.set_model(&terminal_id, model, core.agent_sink())
}

/// What the agent wrote so far, to draw the conversation again, and how
/// many lines that is.
#[tauri::command]
pub fn agent_output(core: State<'_, Core>, terminal_id: String) -> serde_json::Value {
    let (text, lines) = core.agents.history(&terminal_id);
    serde_json::json!({ "text": text, "lines": lines })
}

/// Where the agent works.
#[tauri::command]
pub fn agent_folder(core: State<'_, Core>, terminal_id: String) -> Option<String> {
    core.agents.cwd(&terminal_id).map(|dir| dir.to_string_lossy().into_owned())
}

/// Removes a terminal, its links, and stops its shell and its agent.
#[tauri::command]
pub fn close_terminal(core: State<'_, Core>, terminal_id: String) -> AppResult<()> {
    let workspace_id = core.mutate(|m| m.remove_terminal(&terminal_id))?;
    core.close_terminals(std::slice::from_ref(&terminal_id));
    core.notify_links(&workspace_id);
    Ok(())
}

#[tauri::command]
pub fn rename_terminal(core: State<'_, Core>, terminal_id: String, name: String) -> AppResult<()> {
    core.mutate(|m| m.rename_terminal(&terminal_id, &name))
}

/// Stores a new position/size and returns it after clamping.
#[tauri::command]
pub fn update_terminal_rect(core: State<'_, Core>, terminal_id: String, rect: Rect) -> AppResult<Rect> {
    core.mutate(|m| m.update_terminal_rect(&terminal_id, rect))
}

/// Brings a terminal to the front and returns its new z.
#[tauri::command]
pub fn focus_terminal(core: State<'_, Core>, terminal_id: String) -> AppResult<u32> {
    core.mutate(|m| m.bring_to_front(&terminal_id))
}

#[tauri::command]
pub fn terminal_has_foreground_process(core: State<'_, Core>, terminal_id: String) -> AppResult<bool> {
    if !core.ptys.has_session(&terminal_id) {
        return Ok(false);
    }
    core.ptys.has_foreground_process(&terminal_id)
}

/// Connects a view to a terminal: starts the shell if it is not running yet,
/// applies the view's size and returns the output to replay.
#[tauri::command]
pub fn attach_terminal(core: State<'_, Core>, terminal_id: String, cols: u16, rows: u16) -> AppResult<AttachResult> {
    if !core.ptys.has_session(&terminal_id) {
        core.spawn_terminal(&terminal_id, cols, rows)?;
    } else {
        let _ = core.ptys.resize(&terminal_id, cols, rows);
    }
    let (data, seq, alive) = core.ptys.snapshot(&terminal_id)?;
    Ok(AttachResult { data: BASE64.encode(data), seq, alive })
}

/// Replaces the terminal's shell with a fresh one.
#[tauri::command]
pub fn restart_terminal(core: State<'_, Core>, terminal_id: String, cols: u16, rows: u16) -> AppResult<()> {
    core.spawn_terminal(&terminal_id, cols, rows)
}

/// Sends input (base64 of the raw bytes) to the terminal.
#[tauri::command]
pub fn write_terminal(core: State<'_, Core>, terminal_id: String, data: String) -> AppResult<()> {
    let bytes = BASE64.decode(data).map_err(|e| AppError::Invalid(format!("input is not base64: {e}")))?;
    core.ptys.write(&terminal_id, bytes)
}

#[tauri::command]
pub fn resize_terminal(core: State<'_, Core>, terminal_id: String, cols: u16, rows: u16) -> AppResult<()> {
    core.ptys.resize(&terminal_id, cols, rows)
}

/// Creates a manual link between two terminals of the same workspace.
#[tauri::command]
pub fn create_link(core: State<'_, Core>, source_id: String, target_id: String) -> AppResult<Link> {
    let (workspace_id, link) = core.mutate(|m| m.add_manual_link(&source_id, &target_id, now_ms()))?;
    core.notify_links(&workspace_id);
    // Connecting a knowledge base to a terminal gives its agent the base.
    if let Some((kb, terminal)) = core.kb_and_terminal(&source_id, &target_id) {
        core.announce_kb_connected(&kb, &terminal);
        // Make sure its index is ready for the questions that tend to follow a
        // fresh connection, even if the node existed before this launch. The
        // path is read in its own statement so the model guard is not held
        // across the call (see `bridge::touch_node`).
        let path = core.shared.model().knowledge_base(&kb).map(|k| k.path.clone());
        if let Some(path) = path {
            core.warm_knowledge_base(&kb, &path);
        }
    }
    Ok(link)
}

#[tauri::command]
pub fn delete_link(core: State<'_, Core>, link_id: String) -> AppResult<()> {
    let endpoints = core.shared.model().link_endpoints(&link_id);
    let workspace_id = core.mutate(|m| m.remove_link(&link_id))?;
    core.notify_links(&workspace_id);
    if let Some((a, b)) = endpoints
        && let Some((kb, terminal)) = core.kb_and_terminal(&a, &b)
    {
        let name = core.shared.model().knowledge_base(&kb).map(|k| k.name.clone()).unwrap_or_default();
        core.announce_kb_disconnected(&kb, &name, &terminal);
    }
    Ok(())
}

/// Adds a knowledge-base node for `path` (a folder) to a workspace. The node's
/// name is the folder's own name, made unique in the workspace.
#[tauri::command]
pub fn create_knowledge_base(
    core: State<'_, Core>,
    workspace_id: String,
    path: String,
    rect: Rect,
) -> AppResult<KnowledgeBase> {
    let dir = std::path::PathBuf::from(&path);
    if !dir.is_dir() {
        return Err(AppError::Invalid(format!("{path} is not a folder")));
    }
    let name =
        dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "Knowledge Base".to_owned());
    let kb = core.mutate(|m| m.add_knowledge_base(&workspace_id, &name, &path, rect))?;
    core.notify_knowledge_bases(&workspace_id);
    // Index it now, in the background, so the first search is already warm by
    // the time the user connects it to a terminal and asks a question.
    core.warm_knowledge_base(&kb.id, &path);
    Ok(kb)
}

/// Removes a knowledge-base node and every arrow touching it.
#[tauri::command]
pub fn remove_knowledge_base(core: State<'_, Core>, knowledge_base_id: String) -> AppResult<()> {
    // Agents that could read this base are about to lose it; tell them first.
    let (name, terminals) = {
        let model = core.shared.model();
        let name = model.knowledge_base(&knowledge_base_id).map(|k| k.name.clone()).unwrap_or_default();
        (name, model.terminals_linked_to(&knowledge_base_id))
    };
    let workspace_id = core.mutate(|m| m.remove_knowledge_base(&knowledge_base_id))?;
    core.kb.forget(&knowledge_base_id);
    for terminal in &terminals {
        core.announce_kb_disconnected(&knowledge_base_id, &name, terminal);
    }
    core.notify_knowledge_bases(&workspace_id);
    // Arrows that touched the node went with it.
    core.notify_links(&workspace_id);
    Ok(())
}

#[tauri::command]
pub fn rename_knowledge_base(core: State<'_, Core>, knowledge_base_id: String, name: String) -> AppResult<()> {
    let workspace_id = core.mutate(|m| {
        m.rename_knowledge_base(&knowledge_base_id, &name)?;
        Ok(m.workspace_of_node(&knowledge_base_id).map(|w| w.id.clone()).unwrap_or_default())
    })?;
    core.notify_knowledge_bases(&workspace_id);
    Ok(())
}

/// Moves or resizes a knowledge-base node. Committed once on pointer-up, like a
/// terminal's rect, so no event is emitted per drag frame.
#[tauri::command]
pub fn update_knowledge_base_rect(core: State<'_, Core>, knowledge_base_id: String, rect: Rect) -> AppResult<Rect> {
    core.mutate(|m| m.update_knowledge_base_rect(&knowledge_base_id, rect))
}

/// The folder a terminal's program (or its idle shell) is in, where known.
#[tauri::command]
pub fn terminal_folder(core: State<'_, Core>, terminal_id: String) -> Option<String> {
    core.ptys.activity(&terminal_id).cwd.map(|dir| dir.to_string_lossy().into_owned())
}

/// Asks for a folder (or a file); `None` when the person cancels.
#[tauri::command]
pub async fn pick_path(app: AppHandle, folder: bool) -> AppResult<Option<String>> {
    let picked = tauri::async_runtime::spawn_blocking(move || {
        let mut dialog = app.dialog().file();
        if let Some(window) = app.get_webview_window("main") {
            dialog = dialog.set_parent(&window);
        }
        if folder { dialog.blocking_pick_folder() } else { dialog.blocking_pick_file() }
    })
    .await
    .map_err(|e| AppError::Invalid(e.to_string()))?;
    Ok(picked.and_then(|path| path.into_path().ok()).map(|path| path.to_string_lossy().into_owned()))
}

/// Open windows of other programs that can be brought onto the canvas.
#[tauri::command]
pub fn list_windows(apps: State<'_, Apps>) -> Vec<NativeWindow> {
    apps.list_windows()
}

/// Starts a program (or takes an open window) and brings its window onto the
/// canvas, hidden until `place_apps` puts it on its card. Resolves once the
/// window has appeared.
#[tauri::command]
pub async fn open_app(apps: State<'_, Apps>, launch: AppLaunch) -> AppResult<EmbeddedApp> {
    let apps = apps.inner().clone();
    tauri::async_runtime::spawn_blocking(move || apps.open(&launch))
        .await
        .map_err(|e| AppError::Invalid(e.to_string()))?
}

/// Moves embedded windows over their cards.
#[tauri::command]
pub fn place_apps(apps: State<'_, Apps>, placements: Vec<AppPlacement>) {
    apps.place(placements);
}

#[tauri::command]
pub fn focus_app(apps: State<'_, Apps>, app_id: String) {
    apps.focus(&app_id);
}

/// Asks the program to close its window; an `app-changed` event follows once
/// it has.
#[tauri::command]
pub fn close_app(apps: State<'_, Apps>, app_id: String) -> AppResult<()> {
    apps.close(&app_id)
}

/// Whether programs started in terminals (`code .`, `explorer .`, `notepad`)
/// come onto the canvas beside their terminal.
#[tauri::command]
pub fn set_capture_launches(apps: State<'_, Apps>, on: bool) {
    apps.set_capture(on);
}

/// Whether Claude Code and Codex are told about Godesk's terminals (see
/// [`crate::agents`]).
#[tauri::command]
pub fn agent_notes_installed(app: AppHandle) -> bool {
    app.path().home_dir().is_ok_and(|home| crate::agents::installed(&home))
}

#[tauri::command]
pub fn set_agent_notes(app: AppHandle, on: bool) -> AppResult<()> {
    let home = app.path().home_dir().map_err(|e| AppError::Invalid(e.to_string()))?;
    crate::agents::set(&home, on)
}

/// What the agents in the terminals are doing: tokens and subagents.
#[tauri::command]
pub fn get_agents(
    watch: State<'_, std::sync::Arc<crate::agent_watch::AgentWatch>>,
) -> crate::model::AgentsChangedEvent {
    watch.current()
}

/// How many terminals run a program, to warn before closing Godesk.
#[tauri::command]
pub fn running_programs(core: State<'_, Core>) -> u32 {
    let ids: Vec<String> =
        core.shared.model().workspaces().iter().flat_map(|w| w.terminals.iter().map(|t| t.id.clone())).collect();
    ids.iter().filter(|id| core.ptys.has_foreground_process(id).unwrap_or(false)).count() as u32
}

/// Called before the window closes: records every terminal for the next
/// start, and gives other programs' windows back to the desktop.
#[tauri::command]
pub fn prepare_close(core: State<'_, Core>, apps: State<'_, Apps>) {
    core.save_sessions();
    apps.release_all();
    // The agents are processes of their own: they go with the app, and
    // their conversations are kept for the next start.
    core.agents.close_all();
}

/// Gives the window back to the desktop.
#[tauri::command]
pub fn release_app(apps: State<'_, Apps>, app_id: String) -> AppResult<()> {
    apps.release(&app_id)
}
