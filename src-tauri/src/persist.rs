//! Loading and saving `state.json`. Saves are debounced on a background
//! thread and written atomically (temporary file + rename), so a crash never
//! leaves a half-written file behind.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::model::AppSnapshot;
use crate::state::Model;

pub const STATE_FILE: &str = "state.json";
/// Quiet period after the last change before the file is written.
pub const SAVE_DEBOUNCE: Duration = Duration::from_millis(400);

/// Reads the model from `path`. A missing file yields the default model. A
/// file that cannot be read or parsed is moved aside
/// (`state.json.corrupt-<ms>`), because the next save would otherwise
/// overwrite the user's data.
pub fn load(path: &Path) -> Model {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Model::default(),
        Err(err) => return set_aside(path, &err),
    };
    match serde_json::from_slice::<AppSnapshot>(&bytes) {
        Ok(snapshot) => Model::from_snapshot(snapshot),
        Err(err) => set_aside(path, &err),
    }
}

/// Moves an unusable state file out of the way and starts from scratch.
fn set_aside(path: &Path, reason: &dyn std::fmt::Display) -> Model {
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis());
    let backup = path.with_extension(format!("json.corrupt-{stamp}"));
    eprintln!("godesk: cannot load {} ({reason}); moving it to {}", path.display(), backup.display());
    if let Err(err) = fs::rename(path, &backup) {
        eprintln!("godesk: cannot back up invalid state: {err}");
    }
    Model::default()
}

/// Writes the snapshot atomically.
pub fn save(path: &Path, snapshot: &AppSnapshot) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_vec_pretty(snapshot).map_err(std::io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    {
        let mut file = fs::File::create(&tmp)?;
        file.write_all(&json)?;
        file.sync_all()?;
    }
    fs::rename(&tmp, path)
}

enum Message {
    Dirty,
    Flush(Sender<()>),
}

/// Handle to the background saver.
#[derive(Clone)]
pub struct Persister {
    tx: Sender<Message>,
}

impl Persister {
    pub fn spawn(path: PathBuf, model: Arc<Mutex<Model>>) -> Self {
        let (tx, rx) = mpsc::channel::<Message>();
        thread::Builder::new()
            .name("godesk-persist".into())
            .spawn(move || {
                let write = || {
                    let snapshot = model.lock().expect("model lock poisoned").persisted();
                    if let Err(err) = save(&path, &snapshot) {
                        eprintln!("godesk: cannot save {}: {err}", path.display());
                    }
                };
                while let Ok(message) = rx.recv() {
                    let mut message = message;
                    // Debounce: keep absorbing changes until things are quiet.
                    loop {
                        match message {
                            Message::Flush(done) => {
                                write();
                                let _ = done.send(());
                                break;
                            }
                            Message::Dirty => match rx.recv_timeout(SAVE_DEBOUNCE) {
                                Ok(next) => message = next,
                                Err(RecvTimeoutError::Timeout) => {
                                    write();
                                    break;
                                }
                                Err(RecvTimeoutError::Disconnected) => {
                                    write();
                                    return;
                                }
                            },
                        }
                    }
                }
            })
            .expect("cannot start the persistence thread");
        Self { tx }
    }

    /// Schedules a save after the debounce period.
    pub fn mark_dirty(&self) {
        let _ = self.tx.send(Message::Dirty);
    }

    /// Saves immediately and waits for the write to finish.
    pub fn flush(&self) {
        let (done_tx, done_rx) = mpsc::channel();
        if self.tx.send(Message::Flush(done_tx)).is_ok() {
            let _ = done_rx.recv_timeout(Duration::from_secs(5));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Rect;

    #[test]
    fn missing_file_gives_default_model() {
        let dir = tempfile::tempdir().unwrap();
        let model = load(&dir.path().join(STATE_FILE));
        assert_eq!(model.workspaces().len(), 1);
    }

    #[test]
    fn save_then_load_round_trips_without_detected_links() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join(STATE_FILE);
        let mut model = Model::default();
        let ws = model.active_workspace_id().to_owned();
        let rect = Rect { x: 10.0, y: 20.0, width: 500.0, height: 300.0 };
        let a = model.add_terminal(&ws, rect).unwrap().id;
        let b = model.add_terminal(&ws, rect).unwrap().id;
        model.add_manual_link(&a, &b, 1).unwrap();
        model.touch_link(&ws, &b, &a, 1);

        save(&path, &model.persisted()).unwrap();
        let loaded = load(&path);
        assert_eq!(loaded.persisted(), model.persisted());
        assert_eq!(loaded.links_of(&ws).len(), 1);
        assert!(!path.with_extension("json.tmp").exists());
    }

    #[test]
    fn corrupt_file_is_moved_aside() {
        let contents: [&[u8]; 2] = [b"{ not json", b"{\"version\": 1, \"name\": \"\xff\xfe\"}"];
        for content in contents {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join(STATE_FILE);
            fs::write(&path, content).unwrap();
            let model = load(&path);
            assert_eq!(model.workspaces().len(), 1);
            assert!(!path.exists(), "{content:?} must not stay where the next save overwrites it");
            let backups = fs::read_dir(dir.path()).unwrap().filter_map(Result::ok);
            assert!(backups.into_iter().any(|e| e.file_name().to_string_lossy().contains("corrupt")));
        }
    }

    #[test]
    fn persister_debounces_and_flushes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(STATE_FILE);
        let model = Arc::new(Mutex::new(Model::default()));
        let persister = Persister::spawn(path.clone(), model.clone());

        persister.mark_dirty();
        assert!(!path.exists(), "nothing is written before the debounce period");
        model.lock().unwrap().create_workspace(Some("Later")).unwrap();
        persister.flush();

        let loaded = load(&path);
        assert_eq!(loaded.workspaces().len(), 2);
    }
}
