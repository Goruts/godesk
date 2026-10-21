//! Shell sessions backed by real pseudo-terminals (`portable-pty`: Unix PTYs
//! and ConPTY on Windows).
//!
//! Every session runs three threads: a reader that appends output to the
//! session's [`Scrollback`] and forwards it to an [`OutputSink`], a writer
//! that drains queued input (so a slow child never blocks the caller), and a
//! waiter that reports the exit status.

use std::collections::HashMap;
use std::io::{ErrorKind, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};

use crate::error::{AppError, AppResult};
use crate::scrollback::Scrollback;

pub const DEFAULT_COLS: u16 = 80;
pub const DEFAULT_ROWS: u16 = 24;
const READ_BUFFER_BYTES: usize = 16 * 1024;
/// How long a closed shell gets to exit after SIGHUP before it and its
/// foreground job are killed (Unix only).
pub const CLOSE_GRACE_PERIOD: Duration = Duration::from_secs(2);

/// Receives what sessions produce. Implemented by the Tauri layer (events +
/// link detection) and by tests.
pub trait OutputSink: Send + Sync + 'static {
    /// `seq` is the session's total output, in bytes, up to the end of `chunk`.
    fn on_output(&self, terminal_id: &str, chunk: &[u8], seq: u64);
    fn on_exit(&self, terminal_id: &str, exit_code: Option<u32>);
}

#[derive(Debug, Clone)]
pub struct PtyConfig {
    /// Initial working directory of every shell.
    pub home: PathBuf,
    /// Program and arguments to run instead of the user's default shell.
    pub shell_override: Option<Vec<String>>,
}

pub struct SpawnRequest<'a> {
    pub terminal_id: &'a str,
    pub terminal_name: &'a str,
    pub workspace_id: &'a str,
    pub cols: u16,
    pub rows: u16,
}

struct Session {
    generation: u64,
    master: Mutex<Box<dyn MasterPty + Send>>,
    input: Sender<Vec<u8>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    /// The shell's pid, which is also its process group id (Unix only).
    #[cfg(unix)]
    pid: Option<u32>,
    scrollback: Mutex<Scrollback>,
    alive: AtomicBool,
}

impl Session {
    /// Hangs up the shell: SIGHUP on Unix, which the shell forwards to its
    /// jobs, or process termination on Windows. On Unix, a shell still alive
    /// after [`CLOSE_GRACE_PERIOD`] (one that ignores SIGHUP) is killed
    /// together with its foreground job, so it cannot outlive its window.
    fn terminate(self: Arc<Self>) {
        if !self.alive.load(Ordering::SeqCst) {
            return;
        }
        let _ = self.killer.lock().expect("killer lock poisoned").kill();
        #[cfg(unix)]
        {
            let spawned = thread::Builder::new().name("pty-reap".into()).spawn(move || {
                let deadline = std::time::Instant::now() + CLOSE_GRACE_PERIOD;
                while self.alive.load(Ordering::SeqCst) {
                    if std::time::Instant::now() >= deadline {
                        self.kill_process_groups();
                        return;
                    }
                    thread::sleep(Duration::from_millis(50));
                }
            });
            if let Err(err) = spawned {
                eprintln!("godesk: cannot start pty reaper: {err}");
            }
        }
    }

    /// SIGKILLs the terminal's foreground process group, then the shell's
    /// own group (the shell leads its session, so its pid is its group id).
    /// Only called while the shell is unreaped, so its pid is still its own.
    #[cfg(unix)]
    fn kill_process_groups(&self) {
        let foreground = self.master.lock().expect("master lock poisoned").process_group_leader();
        let shell = self.pid.and_then(|pid| libc::pid_t::try_from(pid).ok());
        // SAFETY: getpgrp(2) has no preconditions and cannot fail.
        let own_group = unsafe { libc::getpgrp() };
        for group in [foreground, shell].into_iter().flatten() {
            // kill(0) and kill(-1) would address Godesk's group or every
            // process we may signal.
            if group > 1 && group != own_group {
                // SAFETY: kill(2) has no memory-safety preconditions; the
                // negated id addresses the whole process group.
                unsafe { libc::kill(-group, libc::SIGKILL) };
            }
        }
    }
}

struct Inner {
    config: PtyConfig,
    sink: Arc<dyn OutputSink>,
    sessions: Mutex<HashMap<String, Arc<Session>>>,
    next_generation: AtomicU64,
}

impl Inner {
    /// True while `generation` is still the live session of `terminal_id`;
    /// stale threads of a replaced session must stay silent.
    fn is_current(&self, terminal_id: &str, generation: u64) -> bool {
        self.sessions
            .lock()
            .expect("sessions lock poisoned")
            .get(terminal_id)
            .is_some_and(|s| s.generation == generation)
    }
}

#[derive(Clone)]
pub struct PtyManager {
    inner: Arc<Inner>,
}

/// The shell to launch: the user's default shell on Unix (login shell from
/// `$SHELL`, falling back to the passwd entry), PowerShell or `cmd.exe` on
/// Windows.
fn shell_command(config: &PtyConfig) -> CommandBuilder {
    if let Some(argv) = &config.shell_override {
        let mut cmd = CommandBuilder::new(&argv[0]);
        cmd.args(&argv[1..]);
        return cmd;
    }
    #[cfg(windows)]
    {
        let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        let powershell = PathBuf::from(system_root).join(r"System32\WindowsPowerShell\v1.0\powershell.exe");
        if powershell.exists() {
            return CommandBuilder::new(powershell);
        }
    }
    CommandBuilder::new_default_prog()
}

fn pty_err(err: impl std::fmt::Display) -> AppError {
    AppError::Pty(err.to_string())
}

fn size(cols: u16, rows: u16) -> PtySize {
    PtySize { cols: cols.max(2), rows: rows.max(1), pixel_width: 0, pixel_height: 0 }
}

impl PtyManager {
    pub fn new(config: PtyConfig, sink: Arc<dyn OutputSink>) -> Self {
        Self {
            inner: Arc::new(Inner {
                config,
                sink,
                sessions: Mutex::new(HashMap::new()),
                next_generation: AtomicU64::new(1),
            }),
        }
    }

    fn session(&self, terminal_id: &str) -> AppResult<Arc<Session>> {
        self.inner
            .sessions
            .lock()
            .expect("sessions lock poisoned")
            .get(terminal_id)
            .cloned()
            .ok_or_else(|| AppError::Pty(format!("no session for terminal {terminal_id}")))
    }

    pub fn has_session(&self, terminal_id: &str) -> bool {
        self.inner.sessions.lock().expect("sessions lock poisoned").contains_key(terminal_id)
    }

    /// Starts a shell for the terminal, replacing (and killing) any previous
    /// session it had.
    pub fn spawn(&self, req: &SpawnRequest) -> AppResult<()> {
        let pair = native_pty_system().openpty(size(req.cols, req.rows)).map_err(pty_err)?;

        let mut cmd = shell_command(&self.inner.config);
        cmd.cwd(&self.inner.config.home);
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        cmd.env("GODESK_TERMINAL_ID", req.terminal_id);
        cmd.env("GODESK_TERMINAL_NAME", req.terminal_name);
        cmd.env("GODESK_WORKSPACE_ID", req.workspace_id);

        let mut child = pair.slave.spawn_command(cmd).map_err(pty_err)?;
        // The child owns the slave side now; keeping it open here would stop
        // the reader from seeing EOF when the shell exits.
        drop(pair.slave);
        let reader = pair.master.try_clone_reader().map_err(pty_err)?;
        let writer = pair.master.take_writer().map_err(pty_err)?;
        let (input_tx, input_rx) = mpsc::channel::<Vec<u8>>();

        let generation = self.inner.next_generation.fetch_add(1, Ordering::SeqCst);
        let session = Arc::new(Session {
            generation,
            master: Mutex::new(pair.master),
            input: input_tx,
            killer: Mutex::new(child.clone_killer()),
            #[cfg(unix)]
            pid: child.process_id(),
            scrollback: Mutex::new(Scrollback::default()),
            alive: AtomicBool::new(true),
        });
        let id = req.terminal_id.to_owned();
        let previous = self.inner.sessions.lock().expect("sessions lock poisoned").insert(id.clone(), session.clone());
        if let Some(previous) = previous {
            previous.terminate();
        }

        spawn_writer(&id, writer, input_rx);
        spawn_reader(&id, reader, session.clone(), self.inner.clone());
        let inner = self.inner.clone();
        thread::Builder::new()
            .name(format!("pty-wait-{id}"))
            .spawn(move || {
                let code = child.wait().ok().map(|status| status.exit_code());
                session.alive.store(false, Ordering::SeqCst);
                if inner.is_current(&id, generation) {
                    inner.sink.on_exit(&id, code);
                }
            })
            .map_err(pty_err)?;
        Ok(())
    }

    /// Queues input for the shell.
    pub fn write(&self, terminal_id: &str, data: Vec<u8>) -> AppResult<()> {
        self.session(terminal_id)?.input.send(data).map_err(|_| AppError::Pty("the terminal has exited".into()))
    }

    pub fn resize(&self, terminal_id: &str, cols: u16, rows: u16) -> AppResult<()> {
        let session = self.session(terminal_id)?;
        let master = session.master.lock().expect("master lock poisoned");
        master.resize(size(cols, rows)).map_err(pty_err)
    }

    /// Retained output, the offset it ends at, and whether the shell runs.
    pub fn snapshot(&self, terminal_id: &str) -> AppResult<(Vec<u8>, u64, bool)> {
        let session = self.session(terminal_id)?;
        let (data, seq) = session.scrollback.lock().expect("scrollback lock poisoned").snapshot();
        Ok((data, seq, session.alive.load(Ordering::SeqCst)))
    }

    /// Whether a process other than the shell owns the terminal's foreground
    /// process group (Unix only; always false on Windows, where ConPTY does
    /// not expose it).
    pub fn has_foreground_process(&self, terminal_id: &str) -> AppResult<bool> {
        let session = self.session(terminal_id)?;
        if !session.alive.load(Ordering::SeqCst) {
            return Ok(false);
        }
        #[cfg(unix)]
        {
            let leader = session.master.lock().expect("master lock poisoned").process_group_leader();
            Ok(match (leader, session.pid) {
                (Some(leader), Some(shell)) => i64::from(leader) != i64::from(shell),
                _ => false,
            })
        }
        #[cfg(not(unix))]
        {
            Ok(false)
        }
    }

    /// Stops the shell (see [`Session::terminate`]) and forgets the session.
    pub fn close(&self, terminal_id: &str) {
        let session = self.inner.sessions.lock().expect("sessions lock poisoned").remove(terminal_id);
        if let Some(session) = session {
            session.terminate();
        }
    }

    pub fn close_all(&self) {
        let sessions: Vec<_> =
            self.inner.sessions.lock().expect("sessions lock poisoned").drain().map(|(_, s)| s).collect();
        for session in sessions {
            session.terminate();
        }
    }
}

fn spawn_writer(id: &str, mut writer: Box<dyn Write + Send>, input: mpsc::Receiver<Vec<u8>>) {
    let spawned = thread::Builder::new().name(format!("pty-write-{id}")).spawn(move || {
        for data in input {
            if writer.write_all(&data).and_then(|()| writer.flush()).is_err() {
                break;
            }
        }
    });
    if let Err(err) = spawned {
        eprintln!("godesk: cannot start pty writer: {err}");
    }
}

fn spawn_reader(id: &str, mut reader: Box<dyn Read + Send>, session: Arc<Session>, inner: Arc<Inner>) {
    let id = id.to_owned();
    let spawned = thread::Builder::new().name(format!("pty-read-{id}")).spawn(move || {
        let mut buf = vec![0u8; READ_BUFFER_BYTES];
        loop {
            match reader.read(&mut buf) {
                // A signal landing on this thread must not end the session's
                // output for good.
                Err(err) if err.kind() == ErrorKind::Interrupted => continue,
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let chunk = &buf[..n];
                    // The single reader thread appends and forwards in order,
                    // so `seq` grows monotonically across events.
                    let seq = session.scrollback.lock().expect("scrollback lock poisoned").push(chunk);
                    if !inner.is_current(&id, session.generation) {
                        break;
                    }
                    inner.sink.on_output(&id, chunk, seq);
                }
            }
        }
    });
    if let Err(err) = spawned {
        eprintln!("godesk: cannot start pty reader: {err}");
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[derive(Default)]
    struct Recorder {
        output: Mutex<Vec<u8>>,
        last_seq: Mutex<u64>,
        exits: Mutex<Vec<Option<u32>>>,
    }

    impl OutputSink for Recorder {
        fn on_output(&self, _: &str, chunk: &[u8], seq: u64) {
            let mut last = self.last_seq.lock().unwrap();
            assert!(seq > *last, "seq must grow");
            *last = seq;
            self.output.lock().unwrap().extend_from_slice(chunk);
        }

        fn on_exit(&self, _: &str, code: Option<u32>) {
            self.exits.lock().unwrap().push(code);
        }
    }

    impl Recorder {
        fn output_contains(&self, needle: &str) -> bool {
            String::from_utf8_lossy(&self.output.lock().unwrap()).contains(needle)
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

    fn manager(recorder: Arc<Recorder>) -> PtyManager {
        let config = PtyConfig { home: std::env::temp_dir(), shell_override: Some(vec!["/bin/sh".into()]) };
        PtyManager::new(config, recorder)
    }

    fn spawn(pty: &PtyManager, id: &str) {
        let req = SpawnRequest { terminal_id: id, terminal_name: "Agent One", workspace_id: "ws", cols: 80, rows: 24 };
        pty.spawn(&req).unwrap();
    }

    #[test]
    fn runs_a_shell_with_godesk_environment_and_reports_exit() {
        let recorder = Arc::new(Recorder::default());
        let pty = manager(recorder.clone());
        spawn(&pty, "t1");

        // The terminal echoes the typed line, so match only what the shell
        // expands: identity variables and the working directory.
        pty.write("t1", b"echo \"[$GODESK_TERMINAL_ID|$GODESK_TERMINAL_NAME|$(pwd -P)]\"\n".to_vec()).unwrap();
        let home = std::env::temp_dir().canonicalize().unwrap();
        let expected = format!("[t1|Agent One|{}]", home.display());
        assert!(eventually(|| recorder.output_contains(&expected)));

        let (data, seq, alive) = pty.snapshot("t1").unwrap();
        assert!(alive);
        assert!(seq >= *recorder.last_seq.lock().unwrap());
        assert!(String::from_utf8_lossy(&data).contains(&expected));

        pty.write("t1", b"exit 3\n".to_vec()).unwrap();
        assert!(eventually(|| !recorder.exits.lock().unwrap().is_empty()));
        assert_eq!(recorder.exits.lock().unwrap()[0], Some(3));
        assert!(!pty.snapshot("t1").unwrap().2);
    }

    #[test]
    fn detects_foreground_process_resizes_and_closes() {
        let recorder = Arc::new(Recorder::default());
        let pty = manager(recorder.clone());
        spawn(&pty, "t2");
        pty.resize("t2", 120, 40).unwrap();
        pty.write("t2", b"stty size\n".to_vec()).unwrap();
        assert!(eventually(|| recorder.output_contains("40 120")));
        assert!(!pty.has_foreground_process("t2").unwrap(), "idle shell");

        pty.write("t2", b"sleep 30\n".to_vec()).unwrap();
        assert!(eventually(|| pty.has_foreground_process("t2").unwrap_or(false)));

        pty.close("t2");
        assert!(!pty.has_session("t2"));
        assert!(pty.write("t2", b"x".to_vec()).is_err());
    }

    #[test]
    #[ignore = "starts the user's own login shell, which loads their rc files"]
    fn runs_the_default_shell_in_home() {
        let recorder = Arc::new(Recorder::default());
        let home = PathBuf::from(std::env::var_os("HOME").unwrap());
        let pty = PtyManager::new(PtyConfig { home: home.clone(), shell_override: None }, recorder.clone());
        spawn(&pty, "t4");

        // `\r` is what the Enter key sends; line editors such as zle expect it.
        pty.write("t4", b"echo \"[$GODESK_TERMINAL_ID:$(pwd -P)]\"\r".to_vec()).unwrap();
        let expected = format!("[t4:{}]", home.canonicalize().unwrap().display());
        assert!(eventually(|| recorder.output_contains(&expected)));
        pty.close("t4");
    }

    fn group_exists(group: libc::pid_t) -> bool {
        // SAFETY: signal 0 only checks that the process group exists.
        unsafe { libc::kill(-group, 0) == 0 }
    }

    #[test]
    fn close_kills_a_shell_and_foreground_job_that_ignore_hangup() {
        let recorder = Arc::new(Recorder::default());
        let pty = manager(recorder.clone());
        spawn(&pty, "t3");
        pty.write("t3", b"trap '' HUP; echo \"ignoring-$((1 + 1))\"\n".to_vec()).unwrap();
        assert!(eventually(|| recorder.output_contains("ignoring-2")));
        // `sleep` inherits the ignored SIGHUP.
        pty.write("t3", b"sleep 60\n".to_vec()).unwrap();
        assert!(eventually(|| pty.has_foreground_process("t3").unwrap_or(false)));

        let session = pty.session("t3").unwrap();
        let shell = session.pid.unwrap() as libc::pid_t;
        let job = session.master.lock().unwrap().process_group_leader().unwrap();
        assert_ne!(job, shell);

        pty.close("t3");
        assert!(eventually(|| !session.alive.load(Ordering::SeqCst)), "shell still running after close");
        assert!(eventually(|| !group_exists(job)), "foreground job still running after close");
    }
}
