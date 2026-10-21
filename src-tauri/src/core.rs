//! Glue between the model, the PTY sessions, link detection and the
//! frontend. Lock order is always `model` before `detector`.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Runtime};

use crate::links::{LinkDetector, Peer};
use crate::model::{Link, LinksChangedEvent, PtyExitEvent, PtyOutputEvent};
use crate::persist::Persister;
use crate::pty::{OutputSink, PtyManager};
use crate::state::Model;

pub const LINKS_CHANGED_EVENT: &str = "links-changed";

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

/// State shared by commands and PTY threads.
pub struct Shared {
    pub events: Box<dyn Events>,
    pub model: Arc<Mutex<Model>>,
    pub detector: Mutex<LinkDetector>,
    pub persister: Persister,
}

impl Shared {
    pub fn model(&self) -> MutexGuard<'_, Model> {
        self.model.lock().expect("model lock poisoned")
    }

    fn detector(&self) -> MutexGuard<'_, LinkDetector> {
        self.detector.lock().expect("detector lock poisoned")
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

    /// Runs a detector pass for `terminal_id` and records the resulting
    /// links. `pair` maps each peer id the detector returns to
    /// `(source, target)`.
    fn detect(
        &self,
        terminal_id: &str,
        run: impl FnOnce(&mut LinkDetector, &[Peer], u64) -> Vec<String>,
        pair: impl Fn(&str) -> (String, String),
    ) {
        let now = now_ms();
        let mut model = self.model();
        let (workspace_id, peers) = match model.peers_of(terminal_id) {
            Some((ws, peers)) => (ws.to_owned(), run(&mut self.detector(), &peers, now)),
            None => return,
        };
        let mut changed = false;
        for peer in peers {
            let (source, target) = pair(&peer);
            changed |= model.touch_link(&workspace_id, &source, &target, now);
        }
        if changed {
            let links = model.links_of(&workspace_id);
            drop(model);
            self.emit_links(&workspace_id, links);
        }
    }

    /// Output from `source` that mentions a peer links source → peer.
    pub fn observe_output(&self, source: &str, chunk: &[u8]) {
        self.detect(
            source,
            |detector, peers, now| detector.observe_output(source, chunk, peers, now),
            |peer| (source.to_owned(), peer.to_owned()),
        );
    }

    /// Input to `target` that a peer just produced links peer → target.
    pub fn observe_input(&self, target: &str, data: &[u8]) {
        self.detect(
            target,
            |detector, peers, now| detector.observe_input(target, data, peers, now),
            |peer| (peer.to_owned(), target.to_owned()),
        );
    }

    pub fn forget_terminal(&self, terminal_id: &str) {
        self.detector().forget(terminal_id);
    }

    /// Removes idle detected links and notifies the affected workspaces.
    pub fn expire_links(&self) {
        let mut model = self.model();
        let changed: Vec<_> = model
            .expire_detected_links(now_ms())
            .into_iter()
            .map(|ws| {
                let links = model.links_of(&ws);
                (ws, links)
            })
            .collect();
        drop(model);
        for (ws, links) in changed {
            self.emit_links(&ws, links);
        }
    }
}

/// Forwards PTY output to the frontend (one event name per terminal) and to
/// link detection.
pub struct CoreSink(pub Arc<Shared>);

impl OutputSink for CoreSink {
    fn on_output(&self, terminal_id: &str, chunk: &[u8], seq: u64) {
        self.0.emit(&pty_output_event(terminal_id), PtyOutputEvent { data: BASE64.encode(chunk), seq });
        self.0.observe_output(terminal_id, chunk);
    }

    fn on_exit(&self, terminal_id: &str, exit_code: Option<u32>) {
        self.0.emit(&pty_exit_event(terminal_id), PtyExitEvent { exit_code });
    }
}

/// Managed Tauri state.
pub struct Core {
    pub shared: Arc<Shared>,
    pub ptys: PtyManager,
}

#[cfg(test)]
mod tests {
    //! End-to-end backend pipeline: real shells, the real sink, link
    //! detection and the model, with a recorder in place of the Tauri app.

    use std::thread;
    use std::time::{Duration, Instant};

    use super::*;
    use crate::model::{LinkKind, Rect};
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

    fn has_link(shared: &Shared, ws: &str, source: &str, target: &str) -> bool {
        shared
            .model()
            .links_of(ws)
            .iter()
            .any(|l| l.source_id == source && l.target_id == target && l.kind == LinkKind::Detected)
    }

    #[test]
    fn shell_output_and_relayed_input_create_detected_links() {
        let dir = tempfile::tempdir().unwrap();
        let recorder = Arc::new(Recorder::default());
        let model = Arc::new(Mutex::new(Model::default()));
        let shared = Arc::new(Shared {
            events: Box::new(recorder.clone()),
            persister: Persister::spawn(dir.path().join("state.json"), model.clone()),
            model,
            detector: Mutex::new(LinkDetector::default()),
        });
        let config = PtyConfig { home: dir.path().to_owned(), shell_override: Some(vec!["/bin/sh".into()]) };
        let ptys = PtyManager::new(config, Arc::new(CoreSink(shared.clone())));

        let rect = Rect { x: 0.0, y: 0.0, width: 640.0, height: 400.0 };
        let (ws, planner, coder) = {
            let mut m = shared.model();
            let ws = m.active_workspace_id().to_owned();
            let planner = m.add_terminal(&ws, rect).unwrap().id;
            let coder = m.add_terminal(&ws, rect).unwrap().id;
            m.rename_terminal(&planner, "planner").unwrap();
            m.rename_terminal(&coder, "coder").unwrap();
            (ws, planner, coder)
        };
        for (id, name) in [(&planner, "planner"), (&coder, "coder")] {
            let req = SpawnRequest { terminal_id: id, terminal_name: name, workspace_id: &ws, cols: 80, rows: 24 };
            ptys.spawn(&req).unwrap();
        }

        // The shell sees its own identity, and output arrives as base64 events.
        ptys.write(&planner, b"echo \"me=$GODESK_TERMINAL_NAME\"\n".to_vec()).unwrap();
        assert!(eventually(|| recorder.output_of(&planner).contains("me=planner")));
        assert!(shared.model().links_of(&ws).is_empty(), "mentioning itself links nothing");

        // planner's output names coder: planner -> coder.
        ptys.write(&planner, b"echo handing the parser to coder\n".to_vec()).unwrap();
        assert!(eventually(|| has_link(&shared, &ws, &planner, &coder)));
        let events = recorder.payloads(LINKS_CHANGED_EVENT);
        assert_eq!(events.last().unwrap()["workspaceId"], ws.as_str());

        // coder prints a line and the user pastes it into planner: coder -> planner.
        ptys.write(&coder, b"echo result-7f3a9c ready\n".to_vec()).unwrap();
        assert!(eventually(|| recorder.output_of(&coder).contains("result-7f3a9c ready")));
        let pasted = b"result-7f3a9c ready".to_vec();
        shared.observe_input(&planner, &pasted);
        ptys.write(&planner, pasted).unwrap();
        assert!(has_link(&shared, &ws, &coder, &planner));

        // Every output event carries a growing byte count.
        let seqs: Vec<u64> =
            recorder.payloads(&pty_output_event(&coder)).iter().map(|p| p["seq"].as_u64().unwrap()).collect();
        assert!(seqs.windows(2).all(|w| w[0] < w[1]));

        ptys.close_all();
    }
}
