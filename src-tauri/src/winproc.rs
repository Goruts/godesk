//! What a Windows shell is running, read from the process table and from the
//! processes' own memory, since ConPTY exposes neither a foreground process
//! group nor anything like `/proc`.
//!
//! The "foreground program" of a shell is its newest child, looking through
//! `cmd.exe /c` wrappers (the `.cmd` shims npm installs for `claude`, `codex`…)
//! to the program they start. The working directory and command line of a
//! process are read from its PEB, which needs no privilege for processes of
//! the same user.

use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;

use windows_sys::Wdk::System::Threading::{NtQueryInformationProcess, ProcessBasicInformation};
use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, HANDLE, INVALID_HANDLE_VALUE, LocalFree};
use windows_sys::Win32::System::Diagnostics::Debug::ReadProcessMemory;
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, PROCESS_BASIC_INFORMATION, PROCESS_QUERY_INFORMATION,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE, PROCESS_VM_READ, TerminateProcess,
};
use windows_sys::Win32::UI::Shell::CommandLineToArgvW;

/// Console hosts ConPTY may start beside a shell's programs; never a program.
const CONSOLE_HOSTS: [&str; 2] = ["conhost.exe", "openconsole.exe"];

struct Entry {
    pid: u32,
    parent: u32,
    exe: String,
}

/// An open process handle, closed on drop.
struct Process(HANDLE);

impl Process {
    fn open(pid: u32, access: u32) -> Option<Self> {
        // SAFETY: OpenProcess has no memory-safety preconditions.
        let handle = unsafe { OpenProcess(access, 0, pid) };
        (!handle.is_null()).then_some(Self(handle))
    }

    /// Creation time in 100 ns ticks since 1601.
    fn created(&self) -> Option<u64> {
        let mut times = [FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 }; 4];
        let [created, exited, kernel, user] = &mut times;
        // SAFETY: the handle is open and every out pointer is a valid FILETIME.
        let ok = unsafe { GetProcessTimes(self.0, created, exited, kernel, user) };
        (ok != 0).then(|| (u64::from(times[0].dwHighDateTime) << 32) | u64::from(times[0].dwLowDateTime))
    }

    /// Reads a `T` at `address` in the process's memory.
    fn read<T: Copy>(&self, address: usize) -> Option<T> {
        let mut value = std::mem::MaybeUninit::<T>::uninit();
        let mut read = 0usize;
        // SAFETY: the buffer is valid for size_of::<T>() bytes, and is only
        // assumed initialized when all of them were read.
        let ok =
            unsafe { ReadProcessMemory(self.0, address as _, value.as_mut_ptr().cast(), size_of::<T>(), &mut read) };
        // SAFETY: see above; `T` is plain old data (integers).
        (ok != 0 && read == size_of::<T>()).then(|| unsafe { value.assume_init() })
    }

    /// Reads a UNICODE_STRING (64-bit layout) at `address`.
    fn read_unicode_string(&self, address: usize) -> Option<String> {
        let length = usize::from(self.read::<u16>(address)?);
        let buffer = self.read::<u64>(address + 8)? as usize;
        if length == 0 || buffer == 0 || length % 2 != 0 {
            return None;
        }
        let mut units = vec![0u16; length / 2];
        let mut read = 0usize;
        // SAFETY: `units` is valid for `length` bytes.
        let ok = unsafe { ReadProcessMemory(self.0, buffer as _, units.as_mut_ptr().cast(), length, &mut read) };
        (ok != 0 && read == length).then(|| String::from_utf16_lossy(&units))
    }

    /// Address of the process's RTL_USER_PROCESS_PARAMETERS.
    fn parameters(&self) -> Option<usize> {
        // SAFETY: PROCESS_BASIC_INFORMATION is plain old data.
        let mut info: PROCESS_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
        let mut length = 0u32;
        // SAFETY: the buffer and its size match the information class.
        let status = unsafe {
            NtQueryInformationProcess(
                self.0,
                ProcessBasicInformation,
                (&raw mut info).cast(),
                size_of::<PROCESS_BASIC_INFORMATION>() as u32,
                &mut length,
            )
        };
        if status < 0 || info.PebBaseAddress.is_null() {
            return None;
        }
        // PEB.ProcessParameters, 64-bit layout.
        self.read::<u64>(info.PebBaseAddress as usize + 0x20).map(|p| p as usize).filter(|&p| p != 0)
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        // SAFETY: the handle is open and owned.
        unsafe { CloseHandle(self.0) };
    }
}

/// Every process, with its parent pid and lowercase image name.
fn processes() -> Vec<Entry> {
    // SAFETY: no memory-safety preconditions.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Vec::new();
    }
    let mut entries = Vec::new();
    // SAFETY: PROCESSENTRY32W is plain old data; dwSize is set as required.
    let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
    // SAFETY: the snapshot is open and `entry` is a valid PROCESSENTRY32W.
    let mut more = unsafe { Process32FirstW(snapshot, &mut entry) } != 0;
    while more {
        let name = &entry.szExeFile;
        let end = name.iter().position(|&c| c == 0).unwrap_or(name.len());
        entries.push(Entry {
            pid: entry.th32ProcessID,
            parent: entry.th32ParentProcessID,
            exe: String::from_utf16_lossy(&name[..end]).to_lowercase(),
        });
        // SAFETY: as above.
        more = unsafe { Process32NextW(snapshot, &mut entry) } != 0;
    }
    // SAFETY: the snapshot is open and owned.
    unsafe { CloseHandle(snapshot) };
    entries
}

fn created(pid: u32) -> Option<u64> {
    Process::open(pid, PROCESS_QUERY_LIMITED_INFORMATION)?.created()
}

/// When a process started.
pub fn started(pid: u32) -> Option<std::time::SystemTime> {
    // FILETIME counts 100 ns ticks from 1601; the Unix epoch is this many after.
    const UNIX_EPOCH_TICKS: u64 = 116_444_736_000_000_000;
    let ticks = created(pid)?.checked_sub(UNIX_EPOCH_TICKS)?;
    Some(std::time::UNIX_EPOCH + std::time::Duration::from_nanos(ticks * 100))
}

/// The live children of `parent`, newest first, without console hosts. A
/// recorded parent pid may since have been reused, so children older than
/// the parent are not its own.
fn children(all: &[Entry], parent: u32) -> Vec<&Entry> {
    let Some(born) = created(parent) else { return Vec::new() };
    let mut found: Vec<(u64, &Entry)> = all
        .iter()
        .filter(|e| e.parent == parent && e.pid != parent && !CONSOLE_HOSTS.contains(&e.exe.as_str()))
        .filter_map(|e| created(e.pid).filter(|&t| t >= born).map(|t| (t, e)))
        .collect();
    found.sort_by_key(|&(created, _)| std::cmp::Reverse(created));
    found.into_iter().map(|(_, e)| e).collect()
}

/// The program the shell `shell` runs in its foreground, if any: its newest
/// child, or what a `cmd.exe /c` wrapper among them starts.
pub fn foreground_program(shell: u32) -> Option<u32> {
    let all = processes();
    let mut current = children(&all, shell).first()?.pid;
    // A bounded walk: pid reuse cannot form a cycle past the time check, but
    // a malformed table must not hang the caller.
    for _ in 0..8 {
        let entry = all.iter().find(|e| e.pid == current)?;
        if entry.exe != "cmd.exe" || !command_line(current).is_some_and(|argv| is_wrapper(&argv)) {
            break;
        }
        match children(&all, current).first() {
            Some(child) => current = child.pid,
            None => break,
        }
    }
    Some(current)
}

/// The job the shell `shell` runs in its foreground: its newest child,
/// which may be a `cmd.exe /c` wrapper of the program (see
/// [`foreground_program`]).
pub fn foreground_job(shell: u32) -> Option<u32> {
    children(&processes(), shell).first().map(|e| e.pid)
}

/// The shells among `shells` that run a program.
pub fn busy(shells: &[u32]) -> std::collections::HashSet<u32> {
    let all = processes();
    all.iter()
        .filter(|e| shells.contains(&e.parent) && e.pid != e.parent && !CONSOLE_HOSTS.contains(&e.exe.as_str()))
        .map(|e| e.parent)
        .collect()
}

/// Terminates `pid` and every program it started.
pub fn kill_tree(pid: u32) {
    kill_descendants(pid);
    if let Some(process) = Process::open(pid, PROCESS_TERMINATE) {
        // SAFETY: the handle is open with PROCESS_TERMINATE.
        unsafe { TerminateProcess(process.0, 1) };
    }
}

/// `cmd.exe /c …` (or `/k`): a shell that only runs one command.
fn is_wrapper(argv: &[String]) -> bool {
    argv.iter().skip(1).any(|a| a.eq_ignore_ascii_case("/c"))
}

/// Every descendant of `root` with its lowercase image name, deepest first.
pub fn descendants(root: u32) -> Vec<(u32, String)> {
    let all = processes();
    let mut order: Vec<(u32, String)> = Vec::new();
    let mut stack = vec![root];
    while let Some(pid) = stack.pop() {
        for child in children(&all, pid) {
            if !order.iter().any(|(p, _)| *p == child.pid) && child.pid != root {
                order.push((child.pid, child.exe.clone()));
                stack.push(child.pid);
            }
        }
    }
    order.reverse();
    order
}

/// Terminates every program `shell` started, so a closed terminal does not
/// leave an agent running without a window. The shell itself is left to the
/// caller.
pub fn kill_descendants(shell: u32) {
    for (pid, _) in descendants(shell) {
        if let Some(process) = Process::open(pid, PROCESS_TERMINATE) {
            // SAFETY: the handle is open with PROCESS_TERMINATE.
            unsafe { TerminateProcess(process.0, 1) };
        }
    }
}

fn open_for_reading(pid: u32) -> Option<Process> {
    Process::open(pid, PROCESS_QUERY_INFORMATION | PROCESS_VM_READ)
}

/// The working directory of a process.
pub fn cwd(pid: u32) -> Option<PathBuf> {
    let process = open_for_reading(pid)?;
    // RTL_USER_PROCESS_PARAMETERS.CurrentDirectory.DosPath, 64-bit layout.
    let dir = process.read_unicode_string(process.parameters()? + 0x38)?;
    // Windows keeps a trailing separator, except that a root keeps it too.
    let trimmed = if dir.len() > 3 { dir.trim_end_matches('\\') } else { dir.as_str() };
    Some(PathBuf::from(trimmed)).filter(|d| d.is_dir())
}

/// The argv of a process, split the way the C runtime splits it.
pub fn command_line(pid: u32) -> Option<Vec<String>> {
    let process = open_for_reading(pid)?;
    // RTL_USER_PROCESS_PARAMETERS.CommandLine, 64-bit layout.
    let line = process.read_unicode_string(process.parameters()? + 0x70)?;
    split_command_line(&line)
}

fn split_command_line(line: &str) -> Option<Vec<String>> {
    let wide: Vec<u16> = line.encode_utf16().chain(std::iter::once(0)).collect();
    let mut count = 0i32;
    // SAFETY: `wide` is NUL-terminated; the result is freed with LocalFree.
    let argv = unsafe { CommandLineToArgvW(wide.as_ptr(), &mut count) };
    if argv.is_null() {
        return None;
    }
    let args = (0..usize::try_from(count).unwrap_or(0))
        .map(|i| {
            // SAFETY: CommandLineToArgvW returns `count` NUL-terminated strings.
            let arg = unsafe { *argv.add(i) };
            let len = (0..).take_while(|&j| unsafe { *arg.add(j) } != 0).count();
            // SAFETY: `arg` is valid for `len` units.
            OsString::from_wide(unsafe { std::slice::from_raw_parts(arg, len) }).into_string().ok()
        })
        .collect();
    // SAFETY: allocated by CommandLineToArgvW.
    unsafe { LocalFree(argv.cast()) };
    args
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    #[test]
    fn splits_command_lines_like_the_c_runtime() {
        let argv = split_command_line(r#""C:\Program Files\nodejs\node.exe" cli.js --model "opus 5" a\"b"#).unwrap();
        assert_eq!(argv, [r"C:\Program Files\nodejs\node.exe", "cli.js", "--model", "opus 5", "a\"b"]);
    }

    #[test]
    fn reads_the_cwd_and_argv_of_a_program_started_through_cmd_wrappers() {
        let dir = tempfile::tempdir().unwrap();
        // canonicalize() gives a \\?\ path; the PEB holds the plain one.
        let dir = dir.path().canonicalize().unwrap();
        let plain = PathBuf::from(dir.to_string_lossy().trim_start_matches(r"\\?\"));
        // The outer cmd stands for the shell; the inner one is a wrapper, as
        // npm's .cmd shims are, so the program is ping.
        let mut shell = Command::new("cmd.exe")
            .args(["/d", "/c", "cmd.exe", "/d", "/c", "ping", "-n", "30", "127.0.0.1"])
            .current_dir(&plain)
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let found = loop {
            let program = foreground_program(shell.id());
            if let Some(pid) = program.filter(|&pid| command_line(pid).is_some_and(|argv| !is_wrapper(&argv))) {
                break pid;
            }
            assert!(Instant::now() < deadline, "ping never showed up under the wrappers");
            std::thread::sleep(Duration::from_millis(50));
        };
        assert_eq!(command_line(found).unwrap()[1..], ["-n", "30", "127.0.0.1"]);
        assert_eq!(cwd(found).unwrap(), plain);

        // Termination is asynchronous.
        kill_descendants(shell.id());
        let deadline = Instant::now() + Duration::from_secs(10);
        while cwd(found).is_some() {
            assert!(Instant::now() < deadline, "ping outlived kill_descendants");
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = shell.kill();
        let _ = shell.wait();
    }
}
