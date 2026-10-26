//! What each terminal was doing, kept on disk so a restart brings it back:
//! its folder, the output it showed, and what to run again in it (see
//! [`crate::resume`]). One `<terminal id>.json` and `<terminal id>.log` per
//! terminal under `<app data>/sessions`, written while Godesk runs and when
//! it closes; `state.json` keeps only the canvas.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::resume::Restore;

/// Written after the restored output, so the earlier session reads as such:
/// attributes reset, the main screen back, the cursor shown.
pub const RESTORED_MARKER: &[u8] =
    b"\x1b[0m\x1b[?1049l\x1b[?25h\r\n\x1b[2m-- Godesk reopened this terminal; earlier output above --\x1b[0m\r\n";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Saved {
    pub cwd: Option<PathBuf>,
    pub restore: Option<Restore>,
}

pub struct SessionStore {
    dir: PathBuf,
}

impl SessionStore {
    pub fn new(data_dir: &Path) -> Self {
        Self { dir: data_dir.join("sessions") }
    }

    fn files(&self, id: &str) -> (PathBuf, PathBuf) {
        (self.dir.join(format!("{id}.json")), self.dir.join(format!("{id}.log")))
    }

    /// Ids are generated (UUIDs); anything else is not a file name of ours.
    fn valid(id: &str) -> bool {
        !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    }

    pub fn save(&self, id: &str, saved: &Saved, output: &[u8]) -> std::io::Result<()> {
        if !Self::valid(id) {
            return Ok(());
        }
        std::fs::create_dir_all(&self.dir)?;
        let (meta, log) = self.files(id);
        // Written aside and renamed, so a crash mid-write keeps the last good copy.
        for (path, bytes) in
            [(&meta, serde_json::to_vec(saved).map_err(std::io::Error::other)?), (&log, output.to_vec())]
        {
            let partial = path.with_extension("partial");
            std::fs::write(&partial, bytes)?;
            std::fs::rename(&partial, path)?;
        }
        Ok(())
    }

    pub fn load(&self, id: &str) -> (Saved, Option<Vec<u8>>) {
        if !Self::valid(id) {
            return (Saved::default(), None);
        }
        let (meta, log) = self.files(id);
        let saved = std::fs::read(meta).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        (saved, std::fs::read(log).ok().filter(|b| !b.is_empty()))
    }

    pub fn forget(&self, id: &str) {
        if Self::valid(id) {
            let (meta, log) = self.files(id);
            let _ = std::fs::remove_file(meta);
            let _ = std::fs::remove_file(log);
        }
    }

    /// Removes the files of terminals that no longer exist.
    pub fn prune(&self, keep: &HashSet<String>) {
        for entry in std::fs::read_dir(&self.dir).into_iter().flatten().flatten() {
            let path = entry.path();
            let id = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            if !keep.contains(&id) {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_loads_and_forgets_a_terminal() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path());
        let saved = Saved {
            cwd: Some(PathBuf::from(r"C:\work")),
            restore: Some(Restore { argv: vec!["claude".into(), "--continue".into()], run: true }),
        };
        store.save("t-1", &saved, b"hello").unwrap();
        store.save("t-2", &Saved::default(), b"").unwrap();
        assert_eq!(store.load("t-1"), (saved, Some(b"hello".to_vec())));
        assert_eq!(store.load("t-2"), (Saved::default(), None));
        assert_eq!(store.load("../escape"), (Saved::default(), None));

        store.prune(&HashSet::from(["t-1".to_owned()]));
        assert_eq!(store.load("t-2").0, Saved::default());
        store.forget("t-1");
        assert_eq!(store.load("t-1"), (Saved::default(), None));
    }
}

/// A real Windows shell, saved and reopened by a new backend, as across a
/// restart of Godesk.
#[cfg(all(test, windows))]
mod restart_tests {
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use super::*;
    use crate::core::{Core, CoreSink, Events, Shared};
    use crate::model::Rect;
    use crate::persist::Persister;
    use crate::pty::{PtyConfig, PtyManager};
    use crate::state::Model;
    use crate::text::AnsiStripper;

    struct Quiet;

    impl Events for Quiet {
        fn send(&self, _: &str, _: serde_json::Value) {}
    }

    fn backend(data: &Path, home: &Path, model: Model) -> Core {
        let model = Arc::new(Mutex::new(model));
        let shared =
            Arc::new(Shared::new(Box::new(Quiet), model.clone(), Persister::spawn(data.join("state.json"), model)));
        let config = PtyConfig { home: home.to_owned(), ..PtyConfig::default() };
        let ptys = PtyManager::new(config, Arc::new(CoreSink(shared.clone())));
        Core {
            shared,
            ptys,
            sessions: Arc::new(SessionStore::new(data)),
            home: home.to_owned(),
            agents: crate::agent_session::AgentSessions::kept_in(data.join("agents")),
            agent_view: Arc::new(std::sync::atomic::AtomicBool::new(true)),
            classic: Arc::default(),
            dirs: Arc::new(crate::dirs::DirStore::load(data.join("dirs.json"))),
            kb: crate::knowledge::KbStore::new(data.join("kb")),
            data_dir: data.to_owned(),
        }
    }

    fn eventually(mut condition: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            if condition() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        false
    }

    fn raw(core: &Core, id: &str) -> String {
        String::from_utf8_lossy(&core.ptys.snapshot(id).unwrap().0).into_owned()
    }

    fn text(core: &Core, id: &str) -> String {
        AnsiStripper::default().strip(&raw(core, id))
    }

    /// ConPTY waits for the cursor position, which xterm.js answers in the app.
    fn answer_cursor_query(core: &Core, id: &str, asked_before: usize) {
        assert!(eventually(|| raw(core, id).matches("\x1b[6n").count() > asked_before));
        core.ptys.write(id, b"\x1b[1;1R".to_vec()).unwrap();
    }

    #[test]
    fn a_terminal_reopens_in_its_folder_with_its_output_and_its_program_typed() {
        let data = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let project = home.path().join("project");
        std::fs::create_dir(&project).unwrap();
        let rect = Rect { x: 0.0, y: 0.0, width: 640.0, height: 400.0 };

        let before = backend(data.path(), home.path(), Model::default());
        let id = {
            let mut m = before.shared.model();
            let ws = m.active_workspace_id().to_owned();
            m.add_terminal(&ws, rect).unwrap().id
        };
        let model = before.shared.model().clone();
        before.restore_terminal(&id).unwrap();
        answer_cursor_query(&before, &id, 0);
        before.ptys.write(&id, b"Set-Location project; 'before-' + 'restart'\r".to_vec()).unwrap();
        assert!(eventually(|| text(&before, &id).contains("before-restart\n")));
        before.ptys.write(&id, b"ping -n 60 127.0.0.1\r".to_vec()).unwrap();
        assert!(eventually(|| before.ptys.has_foreground_process(&id).unwrap_or(false)));
        before.save_sessions();
        before.ptys.close_all();

        let after = backend(data.path(), home.path(), model);
        after.restore_terminal(&id).unwrap();
        let asked = raw(&after, &id).matches("\x1b[6n").count();
        answer_cursor_query(&after, &id, asked);
        let shown = text(&after, &id);
        assert!(shown.contains("before-restart"), "earlier output is shown again");
        assert!(shown.contains("Godesk reopened this terminal"));
        assert!(
            eventually(|| after.ptys.activity(&id).cwd.as_deref() == Some(project.as_path())),
            "{:?}",
            after.ptys.activity(&id)
        );
        // ping is not an agent: typed at the prompt, waiting for Enter.
        assert!(eventually(|| text(&after, &id).ends_with("127.0.0.1'")), "{}", text(&after, &id));
        std::thread::sleep(Duration::from_secs(1));
        assert!(!after.ptys.has_foreground_process(&id).unwrap(), "ping must not run again by itself");
        after.ptys.close_all();
    }

    /// An agent the shell starts is closed and shown in the same terminal,
    /// in the shell's folder; going back leaves the shell as it was.
    #[test]
    fn an_agent_started_in_a_shell_is_shown_in_the_same_terminal() {
        let data = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let project = home.path().join("project");
        std::fs::create_dir(&project).unwrap();
        // A stand-in for the agent: any program called claude that waits.
        let system = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        let fake = project.join("claude.exe");
        std::fs::copy(Path::new(&system).join(r"System32\cmd.exe"), &fake).unwrap();
        let rect = Rect { x: 0.0, y: 0.0, width: 640.0, height: 400.0 };

        let core = backend(data.path(), home.path(), Model::default());
        let id = {
            let mut m = core.shared.model();
            let ws = m.active_workspace_id().to_owned();
            m.add_terminal(&ws, rect).unwrap().id
        };
        core.restore_terminal(&id).unwrap();
        answer_cursor_query(&core, &id, 0);
        core.ptys.write(&id, format!("Set-Location project; & '{}'\r", fake.display()).into_bytes()).unwrap();
        assert!(eventually(|| core.ptys.has_foreground_process(&id).unwrap_or(false)));

        let mut checked = std::collections::HashMap::new();
        let shown = || core.shared.model().terminal(&id).and_then(|t| t.agent.clone());
        assert!(eventually(|| {
            core.take_over_agents(&mut checked);
            shown().is_some()
        }));
        assert_eq!(shown().as_deref(), Some("claude"));
        assert!(core.agents.is_open(&id));
        assert_eq!(core.agents.cwd(&id).as_deref(), Some(project.as_path()));
        assert!(eventually(|| !core.ptys.has_foreground_process(&id).unwrap_or(true)), "the agent's own screen is closed");

        // A copy shows the same agent, in the same folder, with a
        // conversation of its own.
        let copy = core.duplicate_terminal(&id, rect).unwrap();
        assert_eq!(copy.agent.as_deref(), Some("claude"));
        assert!(core.agents.is_open(&copy.id));
        assert_eq!(core.agents.cwd(&copy.id).as_deref(), Some(project.as_path()));
        assert_eq!(core.agents.launch(&copy.id).and_then(|l| l.program), Some(vec![fake.to_string_lossy().into_owned()]));
        core.detach_agent(&copy.id).unwrap();

        // On its own screen again, on the same command line; Godesk leaves
        // it there while it runs.
        core.to_classic(&id).unwrap();
        assert_eq!(shown(), None);
        assert!(eventually(|| core.ptys.has_foreground_process(&id).unwrap_or(false)));
        for _ in 0..5 {
            core.take_over_agents(&mut checked);
            std::thread::sleep(Duration::from_millis(100));
        }
        assert_eq!(shown(), None, "an agent sent to its own screen stays there");
        assert!(core.ptys.stop_foreground(&id));
        assert!(eventually(|| !core.ptys.has_foreground_process(&id).unwrap_or(true)));

        core.attach_agent(&id, crate::agent_terminal::AgentKind::Claude, crate::agent_terminal::Launch {
            program: Some(vec![fake.to_string_lossy().into_owned()]),
            ..Default::default()
        }, Some(project.as_path())).unwrap();
        core.detach_agent(&id).unwrap();
        assert_eq!(shown(), None);
        assert!(!core.agents.is_open(&id));
        core.ptys.write(&id, b"'still-' + 'here'\r".to_vec()).unwrap();
        assert!(eventually(|| text(&core, &id).contains("still-here\n")), "{}", text(&core, &id));
        core.ptys.close_all();
    }
}
