//! Shell sessions backed by real pseudo-terminals (`portable-pty`: Unix PTYs
//! and ConPTY on Windows).
//!
//! Every session runs three threads: a reader that appends output to the
//! session's [`Scrollback`] and forwards it to an [`OutputSink`], a writer
//! that drains queued input (so a slow child never blocks the caller), and a
//! waiter that reports the exit status.

use std::collections::HashMap;
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
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

#[derive(Debug, Clone, Default)]
pub struct PtyConfig {
    /// Initial working directory of every shell.
    pub home: PathBuf,
    /// Program and arguments to run instead of the user's default shell.
    pub shell_override: Option<Vec<String>>,
    /// Extra environment of every shell.
    pub env: Vec<(String, String)>,
    /// A folder put first on every shell's PATH.
    pub path_prefix: Option<PathBuf>,
}

pub struct SpawnRequest<'a> {
    pub terminal_id: &'a str,
    pub terminal_name: &'a str,
    pub workspace_id: &'a str,
    /// Where the shell starts; [`PtyConfig::home`] when `None`.
    pub cwd: Option<&'a Path>,
    /// Output shown before the shell's own: what the terminal showed before
    /// Godesk restarted.
    pub history: Option<&'a [u8]>,
    pub cols: u16,
    pub rows: u16,
}

/// What a terminal is doing, so a copy of it can do the same: the working
/// directory of its foreground program (or of its idle shell), and that
/// program's argv. Read from /proc on Linux and from the process table on
/// Windows (see [`crate::winproc`]); always empty on macOS.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Activity {
    pub cwd: Option<PathBuf>,
    pub program: Option<Vec<String>>,
}

struct Session {
    generation: u64,
    master: Mutex<Box<dyn MasterPty + Send>>,
    input: Sender<Vec<u8>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    /// The shell's pid, which on Unix is also its process group id.
    pid: Option<u32>,
    scrollback: Mutex<Scrollback>,
    alive: AtomicBool,

}

impl Session {
    /// Hangs up the shell: SIGHUP on Unix, which the shell forwards to its
    /// jobs, or process termination on Windows, where the programs it
    /// started are terminated first since nothing forwards a hangup to them.
    /// On Unix, a shell still alive after [`CLOSE_GRACE_PERIOD`] (one that
    /// ignores SIGHUP) is killed together with its foreground job, so it
    /// cannot outlive its window.
    fn terminate(self: Arc<Self>) {
        if !self.alive.load(Ordering::SeqCst) {
            return;
        }
        #[cfg(windows)]
        if let Some(pid) = self.pid {
            crate::winproc::kill_descendants(pid);
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
    /// Language of the shells, for lines typed into them.
    shell: ShellKind,
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

/// The command language of a shell, which decides how a program's argv is
/// typed into it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellKind {
    /// sh, bash, zsh, fish.
    Posix,
    /// Windows PowerShell or PowerShell 7.
    PowerShell,
    /// `cmd.exe`.
    Cmd,
}

impl ShellKind {
    /// The shell a program is, by file name. Anything unknown is taken for a
    /// POSIX shell (Git Bash, WSL, or any shell on Unix).
    fn of(program: &std::ffi::OsStr) -> Self {
        let name = Path::new(program).file_stem().map(|s| s.to_string_lossy().to_lowercase());
        match name.as_deref() {
            Some("powershell" | "pwsh") => Self::PowerShell,
            Some("cmd") => Self::Cmd,
            _ => Self::Posix,
        }
    }

    /// `argv` as one line this shell runs as that exact command, or `None`
    /// when it cannot be written safely (see the functions it calls).
    pub fn command_line(self, argv: &[String]) -> Option<String> {
        match self {
            Self::Posix => command_line(argv),
            Self::PowerShell => powershell_command_line(argv),
            Self::Cmd => cmd_command_line(argv),
        }
    }
}

/// Run by every default PowerShell before its first prompt. It keeps the
/// process working directory synced to the current location (so a duplicate
/// can read where the shell is), and defines the `z` jump: a zoxide-style
/// `z <query>` that asks Godesk for the best-matching folder and changes to
/// it. The user's own prompt (profile, oh-my-posh…) is kept.
/// Template for the hook above. The bridge endpoint (port and token) is filled
/// in per spawn by [`powershell_cwd_hook`] rather than read from the
/// environment: both change on every restart, and a terminal's environment can
/// hold stale values, whereas the hook is injected fresh each spawn. The
/// terminal id (stable per terminal) is still read from the environment and
/// marks a real Godesk terminal.
#[cfg(windows)]
const POWERSHELL_HOOK_TEMPLATE: &str = "$global:__godeskPrompt = $function:prompt
function global:prompt {
  if ($PWD.Provider.Name -eq 'FileSystem') { [Environment]::CurrentDirectory = $PWD.ProviderPath }
  & $global:__godeskPrompt
}
function global:z {
  if (-not $env:GODESK_TERMINAL_ID) { Write-Host 'z: not inside a Godesk terminal' -ForegroundColor Yellow; return }
  $query = ($args -join ' ').Trim()
  [Net.ServicePointManager]::Expect100Continue = $false
  try {
    $c = New-Object Net.WebClient
    $c.Headers.Add('X-Godesk-Token', '__GODESK_TOKEN__')
    $c.Headers.Add('X-Godesk-From', $env:GODESK_TERMINAL_ID)
    $c.Headers.Add('Content-Type', 'text/plain; charset=utf-8')
    $bytes = $c.UploadData(\"http://127.0.0.1:__GODESK_PORT__/dir\", 'POST', [Text.Encoding]::UTF8.GetBytes($query))
    Set-Location -LiteralPath ([Text.Encoding]::UTF8.GetString($bytes).Trim())
  } catch [Net.WebException] {
    $r = $_.Exception.Response
    if ($r) { Write-Host ((New-Object IO.StreamReader($r.GetResponseStream())).ReadToEnd()).Trim() -ForegroundColor Yellow }
    else { Write-Host \"z: $($_.Exception.Message)\" -ForegroundColor Yellow }
  }
}";

/// The PowerShell startup hook with the live bridge endpoint baked in.
#[cfg(windows)]
fn powershell_cwd_hook(port: &str, token: &str) -> String {
    POWERSHELL_HOOK_TEMPLATE.replace("__GODESK_PORT__", port).replace("__GODESK_TOKEN__", token)
}

/// The shell to launch: the user's default shell on Unix (login shell from
/// `$SHELL`, falling back to the passwd entry), PowerShell or `cmd.exe` on
/// Windows.
fn shell_command(config: &PtyConfig) -> (CommandBuilder, ShellKind) {
    if let Some(argv) = &config.shell_override {
        let mut cmd = CommandBuilder::new(&argv[0]);
        cmd.args(&argv[1..]);
        return (cmd, ShellKind::of(argv[0].as_ref()));
    }
    #[cfg(windows)]
    {
        let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        let powershell = PathBuf::from(system_root).join(r"System32\WindowsPowerShell\v1.0\powershell.exe");
        if powershell.exists() {
            use base64::Engine;
            // Bake the live bridge endpoint into the `z` hook, from the env Godesk
            // injects, so it works even if the shell's own environment is stale.
            let find = |key: &str| config.env.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str()).unwrap_or("");
            let hook = powershell_cwd_hook(find("GODESK_PORT"), find("GODESK_TOKEN"));
            let utf16: Vec<u8> = hook.encode_utf16().flat_map(u16::to_le_bytes).collect();
            let mut cmd = CommandBuilder::new(powershell);
            cmd.args(["-NoExit", "-EncodedCommand", &base64::engine::general_purpose::STANDARD.encode(utf16)]);
            return (cmd, ShellKind::PowerShell);
        }
        // new_default_prog() runs %ComSpec%.
        (CommandBuilder::new_default_prog(), ShellKind::Cmd)
    }
    #[cfg(not(windows))]
    (CommandBuilder::new_default_prog(), ShellKind::Posix)
}

fn pty_err(err: impl std::fmt::Display) -> AppError {
    AppError::Pty(err.to_string())
}

fn size(cols: u16, rows: u16) -> PtySize {
    PtySize { cols: cols.max(2), rows: rows.max(1), pixel_width: 0, pixel_height: 0 }
}

impl PtyManager {
    pub fn new(config: PtyConfig, sink: Arc<dyn OutputSink>) -> Self {
        let shell = shell_command(&config).1;
        Self {
            inner: Arc::new(Inner {
                shell,
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

        let (mut cmd, _) = shell_command(&self.inner.config);
        cmd.cwd(req.cwd.unwrap_or(&self.inner.config.home));
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        for (key, value) in self.environment(req.terminal_id, req.terminal_name, req.workspace_id) {
            cmd.env(key, value);
        }

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
            pid: child.process_id(),
            scrollback: Mutex::new({
                let mut scrollback = Scrollback::default();
                if let Some(history) = req.history {
                    scrollback.push(history);
                }
                scrollback
            }),
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

    /// What Godesk adds to the environment of a terminal's programs: which
    /// terminal they run in, how to reach the others (`godesk`), and the
    /// folder of the `godesk` command first on PATH. A terminal's agent gets
    /// it too, so it can use `godesk` like its shell.
    pub fn environment(
        &self,
        terminal_id: &str,
        terminal_name: &str,
        workspace_id: &str,
    ) -> Vec<(String, std::ffi::OsString)> {
        let mut env: Vec<(String, std::ffi::OsString)> = vec![
            ("GODESK_TERMINAL_ID".into(), terminal_id.into()),
            ("GODESK_TERMINAL_NAME".into(), terminal_name.into()),
            ("GODESK_WORKSPACE_ID".into(), workspace_id.into()),
        ];
        env.extend(self.inner.config.env.iter().map(|(key, value)| (key.clone(), value.into())));
        if let Some(dir) = &self.inner.config.path_prefix {
            let path = std::env::var_os("PATH").unwrap_or_default();
            match std::env::join_paths(std::iter::once(dir.clone()).chain(std::env::split_paths(&path))) {
                Ok(path) => env.push(("PATH".into(), path)),
                Err(err) => eprintln!("godesk: cannot put {} on PATH: {err}", dir.display()),
            }
        }
        env
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

    /// Adds output of Godesk's own to a terminal, as if its shell wrote it.
    pub fn inject(&self, terminal_id: &str, bytes: &[u8]) {
        let Ok(session) = self.session(terminal_id) else { return };
        let seq = session.scrollback.lock().expect("scrollback lock poisoned").push(bytes);
        self.inner.sink.on_output(terminal_id, bytes, seq);
    }

    /// Ends the program the terminal runs in its foreground, with what it
    /// started, and leaves the shell. False when there was none.
    pub fn stop_foreground(&self, terminal_id: &str) -> bool {
        let Ok(session) = self.session(terminal_id) else { return false };
        #[cfg(windows)]
        {
            let Some(job) = session.pid.and_then(crate::winproc::foreground_job) else { return false };
            crate::winproc::kill_tree(job);
            true
        }
        #[cfg(unix)]
        {
            let leader = session.master.lock().expect("master lock poisoned").process_group_leader();
            match (leader, session.pid.and_then(|pid| libc::pid_t::try_from(pid).ok())) {
                (Some(leader), Some(shell)) if leader != shell && leader > 1 => {
                    // SAFETY: kill(2) has no memory-safety preconditions; the
                    // negated id addresses the job's process group.
                    unsafe { libc::kill(-leader, libc::SIGTERM) };
                    true
                }
                _ => false,
            }
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = session;
            false
        }
    }

    /// Retained output, the offset it ends at, and whether the shell runs.
    pub fn snapshot(&self, terminal_id: &str) -> AppResult<(Vec<u8>, u64, bool)> {
        let session = self.session(terminal_id)?;
        let (data, seq) = session.scrollback.lock().expect("scrollback lock poisoned").snapshot();
        Ok((data, seq, session.alive.load(Ordering::SeqCst)))
    }

    /// Whether a process other than the shell owns the terminal's foreground
    /// process group, or on Windows, whether the shell has started a program
    /// that still runs.
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
        #[cfg(windows)]
        {
            Ok(session.pid.and_then(crate::winproc::foreground_program).is_some())
        }
    }

    /// What the terminal is running: the foreground program's argv and
    /// working directory, or the idle shell's working directory. Uses the
    /// same foreground test as [`Self::has_foreground_process`].
    pub fn activity(&self, terminal_id: &str) -> Activity {
        let Ok(session) = self.session(terminal_id) else {
            return Activity::default();
        };
        if !session.alive.load(Ordering::SeqCst) {
            return Activity::default();
        }
        #[cfg(target_os = "linux")]
        {
            let leader = session.master.lock().expect("master lock poisoned").process_group_leader();
            let shell = session.pid.and_then(|pid| libc::pid_t::try_from(pid).ok());
            match (leader, shell) {
                (Some(leader), Some(shell)) if leader != shell => {
                    // The job may have ended in between: then this is a shell.
                    let program = proc_cmdline(leader);
                    let cwd = proc_cwd(leader).or_else(|| proc_cwd(shell));
                    Activity { cwd, program }
                }
                (_, Some(shell)) => Activity { cwd: proc_cwd(shell), program: None },
                _ => Activity::default(),
            }
        }
        #[cfg(windows)]
        {
            let Some(shell) = session.pid else { return Activity::default() };
            match crate::winproc::foreground_program(shell) {
                // The program may have ended in between: then this is a shell.
                Some(program) => Activity {
                    cwd: crate::winproc::cwd(program).or_else(|| crate::winproc::cwd(shell)),
                    program: crate::winproc::command_line(program),
                },
                None => Activity { cwd: crate::winproc::cwd(shell), program: None },
            }
        }
        #[cfg(not(any(target_os = "linux", windows)))]
        {
            Activity::default()
        }
    }

    /// The process the terminal runs in its foreground, other than its shell.
    pub fn foreground_pid(&self, terminal_id: &str) -> Option<u32> {
        let session = self.session(terminal_id).ok()?;
        #[cfg(windows)]
        {
            session.pid.and_then(crate::winproc::foreground_program)
        }
        #[cfg(unix)]
        {
            let leader = session.master.lock().expect("master lock poisoned").process_group_leader()?;
            u32::try_from(leader).ok().filter(|pid| Some(*pid) != session.pid)
        }
    }

    /// `argv` as a line the terminals' shell runs (see [`ShellKind::command_line`]).
    pub fn command_line(&self, argv: &[String]) -> Option<String> {
        self.inner.shell.command_line(argv)
    }

    /// `(terminal id, shell pid)` of every running shell.
    pub fn shell_pids(&self) -> Vec<(String, u32)> {
        let sessions = self.inner.sessions.lock().expect("sessions lock poisoned");
        sessions
            .iter()
            .filter(|(_, s)| s.alive.load(Ordering::SeqCst))
            .filter_map(|(id, s)| s.pid.map(|pid| (id.clone(), pid)))
            .collect()
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

#[cfg(target_os = "linux")]
fn proc_cwd(pid: libc::pid_t) -> Option<PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/cwd")).ok().filter(|dir| dir.is_dir())
}

/// A process's argv. `None` if it is gone, unreadable or not UTF-8.
#[cfg(target_os = "linux")]
fn proc_cmdline(pid: libc::pid_t) -> Option<Vec<String>> {
    let raw = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    // Programs that rename themselves (node's process.title) pad with NULs.
    let end = raw.iter().rposition(|&b| b != 0)? + 1;
    raw[..end].split(|&b| b == 0).map(|arg| String::from_utf8(arg.to_vec()).ok()).collect()
}

/// `argv` as one line for a POSIX shell (sh, bash, zsh; fish reads it the
/// same way), each argument single-quoted unless it is plainly safe, so no
/// argument can expand or start another command. `None` when an argument
/// holds a control character: it would reach the shell's line editor as a
/// key press (Enter, Ctrl+C…) that no quoting can neutralize.
pub fn command_line(argv: &[String]) -> Option<String> {
    if argv.is_empty() || argv.iter().any(|arg| arg.chars().any(char::is_control)) {
        return None;
    }
    let quoted: Vec<String> = argv
        .iter()
        .map(|arg| {
            let safe = |c: char| c.is_ascii_alphanumeric() || "@%+:,./_-".contains(c);
            if !arg.is_empty() && arg.chars().all(safe) {
                arg.clone()
            } else {
                format!("'{}'", arg.replace('\'', r"'\''"))
            }
        })
        .collect();
    Some(quoted.join(" "))
}

/// `argv` as one PowerShell line: the call operator and each argument
/// single-quoted, where nothing expands. `None` for control characters (see
/// [`command_line`]), and for double quotes and empty arguments, which
/// Windows PowerShell 5.1 passes to programs mangled or not at all.
pub fn powershell_command_line(argv: &[String]) -> Option<String> {
    let refused = |arg: &String| arg.is_empty() || arg.contains('"') || arg.chars().any(char::is_control);
    if argv.is_empty() || argv.iter().any(refused) {
        return None;
    }
    // PowerShell also closes single-quoted strings on typographic quotes.
    let quoted: Vec<String> = argv
        .iter()
        .map(|arg| {
            let escaped: String = arg
                .chars()
                .flat_map(|c| {
                    let twice = matches!(c, '\'' | '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}');
                    std::iter::repeat_n(c, if twice { 2 } else { 1 })
                })
                .collect();
            format!("'{escaped}'")
        })
        .collect();
    Some(format!("& {}", quoted.join(" ")))
}

/// `argv` as one `cmd.exe` line, arguments double-quoted unless plainly
/// safe. `None` when an argument holds what cmd expands or cannot quote:
/// `"`, `%`, `!`, `^` or a control character.
pub fn cmd_command_line(argv: &[String]) -> Option<String> {
    let refused = |arg: &String| arg.chars().any(|c| "\"%!^".contains(c) || c.is_control());
    if argv.is_empty() || argv.iter().any(refused) {
        return None;
    }
    let quoted: Vec<String> = argv
        .iter()
        .map(|arg| {
            let safe = |c: char| c.is_ascii_alphanumeric() || "@+:,./\\_-=".contains(c);
            if !arg.is_empty() && arg.chars().all(safe) { arg.clone() } else { format!("\"{arg}\"") }
        })
        .collect();
    Some(quoted.join(" "))
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
        let config = PtyConfig {
            home: std::env::temp_dir(),
            shell_override: Some(vec!["/bin/sh".into()]),
            ..PtyConfig::default()
        };
        PtyManager::new(config, recorder)
    }

    fn spawn(pty: &PtyManager, id: &str) {
        let req = SpawnRequest {
            terminal_id: id,
            terminal_name: "Agent One",
            workspace_id: "ws",
            cwd: None,
            history: None,
            cols: 80,
            rows: 24,
        };
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
        let pty = PtyManager::new(
            PtyConfig { home: home.clone(), shell_override: None, ..PtyConfig::default() },
            recorder.clone(),
        );
        spawn(&pty, "t4");

        // `\r` is what the Enter key sends; line editors such as zle expect it.
        pty.write("t4", b"echo \"[$GODESK_TERMINAL_ID:$(pwd -P)]\"\r".to_vec()).unwrap();
        let expected = format!("[t4:{}]", home.canonicalize().unwrap().display());
        assert!(eventually(|| recorder.output_contains(&expected)));
        pty.close("t4");
    }

    fn argv(args: &[&str]) -> Vec<String> {
        args.iter().map(|&a| a.to_owned()).collect()
    }

    #[test]
    fn command_line_quotes_everything_a_shell_would_expand() {
        assert_eq!(command_line(&argv(&["claude", "--model", "opus-5.5"])).unwrap(), "claude --model opus-5.5");
        assert_eq!(
            command_line(&argv(&["codex", "", "fix it", "$(rm -rf ~)", "it's", "a;b", "~", "*", "=cmd"])).unwrap(),
            r#"codex '' 'fix it' '$(rm -rf ~)' 'it'\''s' 'a;b' '~' '*' '=cmd'"#
        );
    }

    #[test]
    fn command_line_refuses_control_characters_and_empty_argv() {
        assert_eq!(command_line(&[]), None);
        // Each would act as a key press in the shell's line editor.
        for arg in ["a\rb", "a\nb", "\u{3}", "\u{1b}[A", "\u{7f}"] {
            assert_eq!(command_line(&argv(&["vim", arg])), None, "{arg:?}");
        }
    }

    /// The quoted line, run by a real shell, gives back the same argv.
    #[test]
    fn command_line_round_trips_through_sh() {
        let original = argv(&["printf", "[%s]", "", "two words", "it's", "$HOME", "`id`", "\\", "\"q\""]);
        let line = command_line(&original).unwrap();
        let out = std::process::Command::new("/bin/sh").args(["-c", &line]).output().unwrap();
        assert_eq!(String::from_utf8(out.stdout).unwrap(), "[][two words][it's][$HOME][`id`][\\][\"q\"]");
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

#[cfg(test)]
mod quoting_tests {
    use super::*;

    fn argv(args: &[&str]) -> Vec<String> {
        args.iter().map(|&a| a.to_owned()).collect()
    }

    #[test]
    fn powershell_lines_call_single_quoted_arguments() {
        assert_eq!(
            powershell_command_line(&argv(&[r"C:\Program Files\nodejs\node.exe", "cli.js", "$env:X", "it's"])).unwrap(),
            r"& 'C:\Program Files\nodejs\node.exe' 'cli.js' '$env:X' 'it''s'"
        );
        assert_eq!(powershell_command_line(&argv(&["a", "it\u{2019}s"])).unwrap(), "& 'a' 'it\u{2019}\u{2019}s'");
        for refused in [&["a", ""][..], &["a", "say \"hi\""], &["a", "x\ry"], &[]] {
            assert_eq!(powershell_command_line(&argv(refused)), None, "{refused:?}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn the_z_hook_bakes_in_the_current_endpoint() {
        let hook = powershell_cwd_hook("54321", "deadbeefcafe");
        assert!(hook.contains("127.0.0.1:54321/dir"), "{hook}");
        assert!(hook.contains("'deadbeefcafe'"), "{hook}");
        // The endpoint no longer comes from the (possibly stale) environment.
        assert!(!hook.contains("$env:GODESK_PORT"), "the port must not come from the env");
        assert!(!hook.contains("$env:GODESK_TOKEN"), "the token must not come from the env");
        // The stable terminal id still gates it.
        assert!(hook.contains("$env:GODESK_TERMINAL_ID"), "{hook}");
        assert!(!hook.contains("__GODESK_"), "every placeholder is filled");
    }

    #[test]
    fn cmd_lines_quote_spaces_and_refuse_what_cmd_expands() {
        assert_eq!(
            cmd_command_line(&argv(&[r"C:\Program Files\x.exe", "--model=opus", "a&b"])).unwrap(),
            r#""C:\Program Files\x.exe" --model=opus "a&b""#
        );
        for refused in ["%PATH%", "!x!", "a^b", "\"q\"", "a\nb"] {
            assert_eq!(cmd_command_line(&argv(&["x", refused])), None, "{refused:?}");
        }
    }

    #[test]
    fn shell_kind_follows_the_program_name() {
        assert_eq!(
            ShellKind::of(r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe".as_ref()),
            ShellKind::PowerShell
        );
        assert_eq!(ShellKind::of("pwsh".as_ref()), ShellKind::PowerShell);
        assert_eq!(ShellKind::of(r"C:\Windows\system32\CMD.EXE".as_ref()), ShellKind::Cmd);
        assert_eq!(ShellKind::of("/bin/zsh".as_ref()), ShellKind::Posix);
    }
}

/// The default Windows shell in a real ConPTY: where it is after `cd`, what
/// it runs, and that the line a duplicate types starts the same program.
#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// Keeps the output, to show what the shell said when a check fails.
    #[derive(Default)]
    struct Transcript(Mutex<Vec<u8>>);

    impl OutputSink for Transcript {
        fn on_output(&self, _: &str, chunk: &[u8], _: u64) {
            self.0.lock().unwrap().extend_from_slice(chunk);
        }
        fn on_exit(&self, _: &str, _: Option<u32>) {}
    }

    impl Transcript {
        fn text(&self) -> String {
            String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
        }
    }

    fn eventually(condition: impl Fn() -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            if condition() {
                return true;
            }
            thread::sleep(Duration::from_millis(100));
        }
        false
    }

    /// ConPTY starts by asking for the cursor position and waits for the
    /// answer, which xterm.js gives in the app.
    fn spawn(pty: &PtyManager, transcript: &Transcript, id: &str, cwd: Option<&Path>) {
        let asked = transcript.text().matches("[6n").count();
        let req = SpawnRequest {
            terminal_id: id,
            terminal_name: id,
            workspace_id: "ws",
            cwd,
            history: None,
            cols: 120,
            rows: 30,
        };
        pty.spawn(&req).unwrap();
        assert!(eventually(|| transcript.text().matches("[6n").count() > asked));
        pty.write(id, b"[1;1R".to_vec()).unwrap();
    }

    /// Output that scrolls past the screen must reach the terminal as
    /// lines, or it never gets into xterm's scrollback. Windows 10's own
    /// ConPTY repaints only the screen when output comes faster than it
    /// renders, so lines in between are lost; the ConPTY sideloaded next to
    /// Godesk (`conpty.dll`, `OpenConsole.exe`) passes them through.
    #[test]
    #[ignore = "measures the ConPTY in use; run with and without conpty.dll beside the test binary"]
    fn fast_output_reaches_the_terminal_line_by_line() {
        let home = tempfile::tempdir().unwrap();
        let transcript = Arc::new(Transcript::default());
        let pty =
            PtyManager::new(PtyConfig { home: home.path().to_owned(), ..PtyConfig::default() }, transcript.clone());
        spawn(&pty, &transcript, "s1", None);
        // Split so that the echoed command line does not count as the output.
        pty.write("s1", b"1..2000 | % { 'line ' + $_ }; 'all' + '-done'\r".to_vec()).unwrap();
        assert!(eventually(|| transcript.text().contains("all-done")), "never finished");
        let text = crate::text::AnsiStripper::default().strip(&transcript.text());
        // Each line on its own, ended as a line ("line 1" alone is in "line 10");
        // the stripper turns line ends into newlines.
        let seen = (1..=2000).filter(|n| text.contains(&format!("line {n}\n"))).count();
        let sideloaded = std::env::current_exe().unwrap().with_file_name("conpty.dll").is_file();
        eprintln!("sideloaded conpty: {sideloaded}; lines received: {seen} of 2000");
        pty.close_all();
        assert_eq!(seen, 2000);
    }

    #[test]
    fn powershell_reports_its_location_and_program_for_a_duplicate() {
        let home = tempfile::tempdir().unwrap();
        let project = home.path().join("my project");
        std::fs::create_dir(&project).unwrap();
        let transcript = Arc::new(Transcript::default());
        let pty = PtyManager::new(
            PtyConfig { home: home.path().to_owned(), shell_override: None, ..PtyConfig::default() },
            transcript.clone(),
        );
        assert_eq!(pty.inner.shell, ShellKind::PowerShell);
        spawn(&pty, &transcript, "w1", None);

        pty.write("w1", b"Set-Location 'my project'\r".to_vec()).unwrap();
        assert!(
            eventually(|| pty.activity("w1").cwd.as_deref() == Some(project.as_path())),
            "{:?}
{}",
            pty.activity("w1"),
            transcript.text()
        );
        assert!(!pty.has_foreground_process("w1").unwrap());

        pty.write("w1", b"ping -n 60 127.0.0.1 > $null\r".to_vec()).unwrap();
        assert!(eventually(|| pty.activity("w1").program.is_some()));
        assert!(pty.has_foreground_process("w1").unwrap());
        let running = pty.activity("w1");
        assert_eq!(running.cwd.as_deref(), Some(project.as_path()));
        assert_eq!(running.program.as_ref().unwrap()[1..], ["-n", "60", "127.0.0.1"]);

        // What duplicate_terminal does: a shell in that folder, the line typed
        // before PowerShell has even shown a prompt.
        spawn(&pty, &transcript, "w2", running.cwd.as_deref());
        let line = pty.command_line(running.program.as_ref().unwrap()).unwrap();
        pty.write("w2", format!("{line}\r").into_bytes()).unwrap();
        assert!(
            eventually(|| pty.activity("w2") == running),
            "{:?}
{}",
            pty.activity("w2"),
            transcript.text()
        );

        // Closing a terminal stops what runs in it, not only the shell.
        let ping = crate::winproc::foreground_program(pty.session("w2").unwrap().pid.unwrap()).unwrap();
        pty.close_all();
        assert!(eventually(|| crate::winproc::cwd(ping).is_none()), "ping outlived its terminal");
    }
}
