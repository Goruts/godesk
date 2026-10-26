//! Glue between the model, the PTY sessions and the frontend.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Runtime};

use crate::model::{KnowledgeBase, KnowledgeBasesChangedEvent, Link, LinksChangedEvent, PtyExitEvent, PtyOutputEvent};
use crate::persist::Persister;
use crate::pty::{OutputSink, PtyManager};
use crate::state::Model;

/// How often buffered PTY output is flushed to the frontend. Small enough to
/// stay imperceptible while typing, large enough to collapse a burst of tiny
/// ConPTY reads into a single event.
const OUTPUT_FLUSH_MS: u64 = 6;

/// PTY output waiting to be sent for one terminal: the bytes buffered since
/// the last flush and the running byte count at their end.
#[derive(Default)]
struct PendingOutput {
    bytes: Vec<u8>,
    seq: u64,
}

/// Coalesces PTY output so a burst of small reads becomes a few Tauri events
/// instead of hundreds. ConPTY splits output into many little chunks, and one
/// IPC event per chunk is what used to jank the webview thread while typing.
/// Output is still delivered whole and in order; only the event count drops.
#[derive(Default)]
struct OutputBuffers {
    pending: Mutex<HashMap<String, PendingOutput>>,
    flushing: AtomicBool,
}

pub const LINKS_CHANGED_EVENT: &str = "links-changed";
pub const KNOWLEDGE_BASES_CHANGED_EVENT: &str = "knowledge-bases-changed";

pub fn pty_output_event(terminal_id: &str) -> String {
    format!("pty-output-{terminal_id}")
}

pub fn pty_exit_event(terminal_id: &str) -> String {
    format!("pty-exit-{terminal_id}")
}

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// Where backend events go: the Tauri app, or a recorder in tests.
pub trait Events: Send + Sync + 'static {
    fn send(&self, event: &str, payload: serde_json::Value);
}

impl<R: Runtime> Events for AppHandle<R> {
    fn send(&self, event: &str, payload: serde_json::Value) {
        if let Err(err) = Emitter::emit(self, event, payload) {
            eprintln!("godesk: cannot emit {event}: {err}");
        }
    }
}

/// Delivers events to the webview without ever blocking the caller.
///
/// Tauri's `emit` runs on the calling thread, and while the webview is busy —
/// exactly when the user is dragging or resizing on the canvas — it can stall
/// until the webview is free again. Backend worker threads must never wait on
/// that: a bridge request that emits (e.g. `godesk kb` lighting its arrow via
/// [`Shared::emit_links`]) would otherwise hang until the agent's `godesk`
/// command times out. Here every event is handed to one background thread that
/// emits them in order, so producers return at once and a busy webview only
/// delays delivery, never the Rust side.
pub struct AsyncEmitter {
    tx: std::sync::mpsc::Sender<(String, serde_json::Value)>,
}

impl AsyncEmitter {
    pub fn new<R: Runtime>(handle: AppHandle<R>) -> Self {
        let (tx, rx) = std::sync::mpsc::channel::<(String, serde_json::Value)>();
        let spawned = thread::Builder::new().name("godesk-emit".into()).spawn(move || {
            for (event, payload) in rx {
                if let Err(err) = Emitter::emit(&handle, &event, payload) {
                    eprintln!("godesk: cannot emit {event}: {err}");
                }
            }
        });
        if let Err(err) = spawned {
            eprintln!("godesk: cannot start the event sender: {err}");
        }
        Self { tx }
    }
}

impl Events for AsyncEmitter {
    fn send(&self, event: &str, payload: serde_json::Value) {
        // The receiver is gone only once the app is shutting down, when a
        // dropped event no longer matters.
        let _ = self.tx.send((event.to_owned(), payload));
    }
}

/// State shared by commands and PTY threads.
pub struct Shared {
    pub events: Box<dyn Events>,
    pub model: Arc<Mutex<Model>>,
    pub persister: Persister,
    output: OutputBuffers,
}

impl Shared {
    pub fn new(events: Box<dyn Events>, model: Arc<Mutex<Model>>, persister: Persister) -> Self {
        Self { events, model, persister, output: OutputBuffers::default() }
    }

    pub fn model(&self) -> MutexGuard<'_, Model> {
        self.model.lock().expect("model lock poisoned")
    }

    pub(crate) fn emit(&self, event: &str, payload: impl Serialize) {
        match serde_json::to_value(payload) {
            Ok(value) => self.events.send(event, value),
            Err(err) => eprintln!("godesk: cannot serialize {event}: {err}"),
        }
    }

    pub fn emit_links(&self, workspace_id: &str, links: Vec<Link>) {
        self.emit(LINKS_CHANGED_EVENT, LinksChangedEvent { workspace_id: workspace_id.to_owned(), links });
    }

    pub fn emit_knowledge_bases(&self, workspace_id: &str, knowledge_bases: Vec<KnowledgeBase>) {
        self.emit(
            KNOWLEDGE_BASES_CHANGED_EVENT,
            KnowledgeBasesChangedEvent { workspace_id: workspace_id.to_owned(), knowledge_bases },
        );
    }

    /// Buffers a PTY chunk for `terminal_id` and makes sure the flush loop is
    /// running, so bursts of tiny reads reach the frontend as a few events.
    fn push_output(self: &Arc<Self>, terminal_id: &str, chunk: &[u8], seq: u64) {
        {
            let mut pending = self.output.pending.lock().expect("output lock poisoned");
            let entry = pending.entry(terminal_id.to_owned()).or_default();
            entry.bytes.extend_from_slice(chunk);
            entry.seq = seq;
        }
        if !self.output.flushing.swap(true, Ordering::AcqRel) {
            let shared = self.clone();
            if thread::Builder::new().name("godesk-output-flush".into()).spawn(move || shared.flush_loop()).is_err() {
                // Without a flush thread, never hold output back: clear the
                // flag so a later chunk retries, and send what is buffered now.
                self.output.flushing.store(false, Ordering::Release);
                self.flush_output();
            }
        }
    }

    fn flush_loop(&self) {
        loop {
            thread::sleep(Duration::from_millis(OUTPUT_FLUSH_MS));
            self.flush_output();
        }
    }

    /// Emits one `pty-output` event per terminal with everything buffered for
    /// it, in order, carrying the byte count at the end of the batch.
    fn flush_output(&self) {
        let drained: Vec<(String, PendingOutput)> = {
            let mut pending = self.output.pending.lock().expect("output lock poisoned");
            pending.drain().filter(|(_, p)| !p.bytes.is_empty()).collect()
        };
        for (id, out) in drained {
            self.emit(&pty_output_event(&id), PtyOutputEvent { data: BASE64.encode(&out.bytes), seq: out.seq });
        }
    }
}

/// Forwards PTY output to the frontend, coalesced per terminal.
pub struct CoreSink(pub Arc<Shared>);

impl OutputSink for CoreSink {
    fn on_output(&self, terminal_id: &str, chunk: &[u8], seq: u64) {
        self.0.push_output(terminal_id, chunk, seq);
    }

    fn on_exit(&self, terminal_id: &str, exit_code: Option<u32>) {
        // Nothing buffered may follow an exit unseen: flush, then signal.
        self.0.flush_output();
        self.0.emit(&pty_exit_event(terminal_id), PtyExitEvent { exit_code });
    }
}

/// Managed Tauri state.
#[derive(Clone)]
pub struct Core {
    pub shared: Arc<Shared>,
    pub ptys: PtyManager,
    /// What each terminal was doing, for a restart.
    pub sessions: Arc<crate::session::SessionStore>,
    /// The user's home, where agents keep their sessions.
    pub home: std::path::PathBuf,
    /// The coding agents the terminals show (see [`crate::agent_session`]).
    pub agents: crate::agent_session::AgentSessions,
    /// Whether an agent started in a shell is shown by Godesk instead of
    /// running on its own screen.
    pub agent_view: Arc<std::sync::atomic::AtomicBool>,
    /// Terminals whose agent the user sent back to its own screen, and
    /// when: left alone while it runs there.
    pub classic: Arc<Mutex<std::collections::HashMap<String, std::time::Instant>>>,
    /// The folders the terminals work in, ranked by frecency (see [`crate::dirs`]).
    pub dirs: Arc<crate::dirs::DirStore>,
    /// Knowledge bases' indexes and extracted text (see [`crate::knowledge`]).
    pub kb: crate::knowledge::KbStore,
    /// The app's data folder (where the offline voice model is kept).
    pub data_dir: std::path::PathBuf,
}

/// Tells the frontend what the agents write and when they stop.
pub struct AgentEvents {
    pub shared: Arc<Shared>,
    pub sessions: crate::agent_session::AgentSessions,
}

impl crate::agent_session::AgentSink for AgentEvents {
    fn on_line(&self, terminal_id: &str, line: &str) {
        // Numbered, so a window that reads the history meanwhile skips what
        // it already has.
        let seq = self.sessions.remember(terminal_id, line);
        self.shared.emit(&format!("agent-line-{terminal_id}"), serde_json::json!({ "seq": seq, "line": line }));
    }

    fn on_exit(&self, terminal_id: &str, code: Option<i32>) {
        self.shared.emit(&format!("agent-exit-{terminal_id}"), code);
    }
}

#[cfg(all(test, unix))]
mod tests {
    //! End-to-end backend pipeline: real shells, the real sink and the model,
    //! with a recorder in place of the Tauri app.

    use std::thread;
    use std::time::{Duration, Instant};

    use super::*;
    use crate::model::Rect;
    use crate::pty::{PtyConfig, SpawnRequest};

    #[derive(Default)]
    struct Recorder(Mutex<Vec<(String, serde_json::Value)>>);

    impl Events for Arc<Recorder> {
        fn send(&self, event: &str, payload: serde_json::Value) {
            self.0.lock().unwrap().push((event.to_owned(), payload));
        }
    }

    impl Recorder {
        fn payloads(&self, event: &str) -> Vec<serde_json::Value> {
            self.0.lock().unwrap().iter().filter(|(e, _)| e == event).map(|(_, p)| p.clone()).collect()
        }

        fn output_of(&self, terminal_id: &str) -> String {
            let bytes: Vec<u8> = self
                .payloads(&pty_output_event(terminal_id))
                .iter()
                .flat_map(|p| BASE64.decode(p["data"].as_str().unwrap()).unwrap())
                .collect();
            String::from_utf8_lossy(&bytes).into_owned()
        }
    }

    fn eventually(condition: impl Fn() -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if condition() {
                return true;
            }
            thread::sleep(Duration::from_millis(20));
        }
        false
    }

    /// The backend with `/bin/sh` shells that start in `home`, and a
    /// recorder in place of the Tauri app.
    fn backend(home: &std::path::Path) -> (Arc<Recorder>, Arc<Shared>, PtyManager) {
        let recorder = Arc::new(Recorder::default());
        let model = Arc::new(Mutex::new(Model::default()));
        let shared = Arc::new(Shared::new(
            Box::new(recorder.clone()),
            model.clone(),
            Persister::spawn(home.join("state.json"), model),
        ));
        let config =
            PtyConfig { home: home.to_owned(), shell_override: Some(vec!["/bin/sh".into()]), ..PtyConfig::default() };
        let ptys = PtyManager::new(config, Arc::new(CoreSink(shared.clone())));
        (recorder, shared, ptys)
    }

    #[test]
    fn shell_output_arrives_as_ordered_coalesced_events() {
        let dir = tempfile::tempdir().unwrap();
        let (recorder, shared, ptys) = backend(dir.path());

        let rect = Rect { x: 0.0, y: 0.0, width: 640.0, height: 400.0 };
        let (ws, planner) = {
            let mut m = shared.model();
            let ws = m.active_workspace_id().to_owned();
            let planner = m.add_terminal(&ws, rect).unwrap().id;
            m.rename_terminal(&planner, "planner").unwrap();
            (ws, planner)
        };
        let req = SpawnRequest {
            terminal_id: &planner,
            terminal_name: "planner",
            workspace_id: &ws,
            cwd: None,
            history: None,
            cols: 80,
            rows: 24,
        };
        ptys.spawn(&req).unwrap();

        // The shell sees its own identity, and its output arrives as base64
        // events flushed by the coalescer.
        ptys.write(&planner, b"echo \"me=$GODESK_TERMINAL_NAME\"\n".to_vec()).unwrap();
        assert!(eventually(|| recorder.output_of(&planner).contains("me=planner")));

        // Each coalesced event carries the growing byte count at its end.
        let seqs: Vec<u64> =
            recorder.payloads(&pty_output_event(&planner)).iter().map(|p| p["seq"].as_u64().unwrap()).collect();
        assert!(!seqs.is_empty());
        assert!(seqs.windows(2).all(|w| w[0] < w[1]), "{seqs:?}");

        ptys.close_all();
    }

    /// A copy starts where the source is and runs what it runs: first an
    /// idle shell that changed directory, then a foreground program whose
    /// arguments need quoting. Directory and argv come from /proc.
    #[cfg(target_os = "linux")]
    #[test]
    fn duplicate_reopens_the_source_directory_and_foreground_program() {
        use crate::pty::Activity;

        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("my project");
        std::fs::create_dir(&project).unwrap();
        let project = project.canonicalize().unwrap();
        let (_, shared, ptys) = backend(dir.path());
        let core = Core {
            shared: shared.clone(),
            ptys: ptys.clone(),
            sessions: Arc::new(crate::session::SessionStore::new(dir.path())),
            home: dir.path().to_owned(),
            agents: crate::agent_session::AgentSessions::kept_in(dir.path().join("agents")),
            agent_view: Arc::new(std::sync::atomic::AtomicBool::new(true)),
            classic: Arc::default(),
        };

        let rect = Rect { x: 0.0, y: 0.0, width: 640.0, height: 400.0 };
        let source = {
            let mut m = shared.model();
            let ws = m.active_workspace_id().to_owned();
            let id = m.add_terminal(&ws, rect).unwrap().id;
            m.rename_terminal(&id, "planner").unwrap();
            id
        };
        core.spawn_terminal(&source, 80, 24).unwrap();

        ptys.write(&source, b"cd 'my project'\n".to_vec()).unwrap();
        assert!(eventually(|| ptys.activity(&source).cwd.as_deref() == Some(project.as_path())));
        let shell_copy = core.duplicate_terminal(&source, rect).unwrap();
        assert_eq!(shell_copy.name, "planner 2");
        let idle_shell = Activity { cwd: Some(project.clone()), program: None };
        assert!(eventually(|| ptys.activity(&shell_copy.id) == idle_shell));

        // `; :` keeps sh from exec'ing sleep, so sh itself, with these
        // arguments, leads the foreground job.
        ptys.write(&source, b"sh -c 'sleep 30; :' x \"it's \\$HOME; \\`id\\`\"\n".to_vec()).unwrap();
        assert!(eventually(|| ptys.has_foreground_process(&source).unwrap_or(false)));
        let running = ptys.activity(&source);
        let argv = ["sh", "-c", "sleep 30; :", "x", "it's $HOME; `id`"].map(String::from).to_vec();
        assert_eq!(running, Activity { cwd: Some(project.clone()), program: Some(argv) });

        let program_copy = core.duplicate_terminal(&source, rect).unwrap();
        assert_eq!(program_copy.name, "planner 3");
        assert!(eventually(|| ptys.activity(&program_copy.id) == running));

        assert!(core.duplicate_terminal("missing", rect).is_err());
        ptys.close_all();
    }
}
