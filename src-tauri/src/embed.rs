//! Windows of other programs (VS Code, File Explorer, documents…) shown on
//! the canvas. Windows only; elsewhere every call says so.
//!
//! A window is not made a child of Godesk's: the main window is transparent
//! (DWM blur-behind), so a child drawn with GDI would show through, and the
//! webview would cover it. It stays a top-level window, without its frame,
//! that Godesk owns: it stays above Godesk, hides while Godesk is minimized
//! and leaves the taskbar. The frontend reports where its card is, and the
//! window is moved there and clipped to the part of the card that is on
//! screen and not under the page's own overlays. Closing Godesk hands every
//! window back to the desktop as it was.

use crate::core::Events;
use crate::error::{AppError, AppResult};
use crate::model::{AppLaunch, AppPlacement, EmbeddedApp, NativeWindow};

pub const APP_CHANGED_EVENT: &str = "app-changed";
pub const APP_CAPTURED_EVENT: &str = "app-captured";
pub const APP_MOVED_EVENT: &str = "app-moved";
pub const APP_PREVIEW_EVENT: &str = "app-preview";

#[cfg(windows)]
pub use imp::Apps;
#[cfg(not(windows))]
pub use unsupported::Apps;

#[cfg(not(windows))]
mod unsupported {
    use super::*;

    fn unsupported<T>() -> AppResult<T> {
        Err(AppError::Invalid("Bringing other programs onto the canvas is only available on Windows.".into()))
    }

    /// Stand-in for platforms without embedding.
    #[derive(Clone)]
    pub struct Apps;

    impl Apps {
        pub fn new(_main: isize, _events: Box<dyn Events>) -> Self {
            Self
        }
        pub fn list_windows(&self) -> Vec<NativeWindow> {
            Vec::new()
        }
        pub fn open(&self, _launch: &AppLaunch) -> AppResult<EmbeddedApp> {
            unsupported()
        }
        pub fn place(&self, _placements: Vec<AppPlacement>) {}
        pub fn focus(&self, _id: &str) {}
        pub fn close(&self, _id: &str) -> AppResult<()> {
            unsupported()
        }
        pub fn release(&self, _id: &str) -> AppResult<()> {
            unsupported()
        }
        pub fn release_all(&self) {}
        pub fn replace_all(&self) {}
        pub fn set_capture(&self, _on: bool) {}
    }
}

#[cfg(windows)]
mod imp {
    use std::collections::{HashMap, HashSet};
    use std::path::PathBuf;
    use std::process::Command;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex, MutexGuard};
    use std::thread;
    use std::time::{Duration, Instant};

    use windows_sys::Win32::Foundation::{CloseHandle, HWND, LPARAM, POINT, RECT, WPARAM};
    use windows_sys::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
    use windows_sys::Win32::Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CAPTUREBLT, ClientToScreen, CombineRgn, CreateCompatibleBitmap,
        CreateCompatibleDC, CreateRectRgn, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetDIBits, RGN_DIFF,
        ReleaseDC, SRCCOPY, SelectObject, SetWindowRgn,
    };
    use windows_sys::Win32::System::Threading::{
        GetProcessId, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
    };
    use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
    use windows_sys::Win32::UI::Shell::{SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GUI_INMOVESIZE, GUITHREADINFO, GW_OWNER, GWL_EXSTYLE, GWL_STYLE, GWLP_HWNDPARENT, GetClassNameW,
        GetClientRect, GetForegroundWindow, GetGUIThreadInfo, GetWindow, GetWindowLongPtrW, GetWindowRect,
        GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, HWND_TOP, IsHungAppWindow, IsIconic, IsWindow,
        IsWindowVisible, IsZoomed, PostMessageW, SW_HIDE, SW_RESTORE, SW_SHOWNA, SW_SHOWNORMAL, SWP_ASYNCWINDOWPOS,
        SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOOWNERZORDER, SWP_NOSIZE, SWP_NOZORDER, SWP_SHOWWINDOW,
        SetForegroundWindow, SetWindowLongPtrW, SetWindowPos, ShowWindow, WM_CLOSE, WS_CAPTION, WS_EX_APPWINDOW,
        WS_EX_DLGMODALFRAME, WS_EX_TOOLWINDOW, WS_EX_WINDOWEDGE, WS_MAXIMIZEBOX, WS_MINIMIZEBOX, WS_SYSMENU,
        WS_THICKFRAME,
    };

    use super::*;
    use crate::model::{AppCapturedEvent, AppChangedEvent, AppMovedEvent, AppPreviewEvent, Rect};

    /// How long a launched program gets to show its window.
    const LAUNCH_TIMEOUT: Duration = Duration::from_secs(20);
    const LAUNCH_POLL: Duration = Duration::from_millis(150);
    /// How long a launch waits for a new window before taking the program's
    /// window that came to the front instead: VS Code, asked for a folder it
    /// already has open, brings that window forward rather than making one.
    const REUSE_GRACE: Duration = Duration::from_secs(2);
    /// How long after a launch a program may swap its window for another
    /// one, which then takes the card: VS Code restarting to apply an
    /// update, a splash or restored window closing as the real one opens.
    const HANDOVER_PERIOD: Duration = Duration::from_secs(10);
    /// How often embedded windows are checked for a new title, an exit, or
    /// being moved by their own frame.
    const WATCH_INTERVAL: Duration = Duration::from_millis(150);
    /// How long a placement may take to land (posted to a program that hangs,
    /// or adjusted by the program) before the window counts as elsewhere.
    const PLACEMENT_SETTLE: Duration = Duration::from_millis(400);
    /// Widest preview of a hidden window, in pixels: it is shown shrunk.
    const PREVIEW_MAX_WIDTH: u32 = 960;
    /// The desktop and taskbar are windows too, never ones to bring in.
    const SHELL_CLASSES: [&str; 4] = ["Progman", "WorkerW", "Shell_TrayWnd", "Shell_SecondaryTrayWnd"];

    /// A window handle as an integer: `HWND` is a raw pointer, which is
    /// neither `Send` nor `Sync`.
    type Hwnd = isize;

    fn hwnd(h: Hwnd) -> HWND {
        h as HWND
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// What the window was before it came onto the canvas.
    struct Original {
        style: isize,
        ex_style: isize,
        owner: isize,
        rect: RECT,
    }

    /// Screen rectangle and clip region last applied, to skip repeats.
    type Applied = Option<((i32, i32, i32, i32), Vec<(i32, i32, i32, i32)>)>;

    /// A launched program's window may still be replaced (see [`HANDOVER_PERIOD`]).
    struct Handover {
        until: Instant,
        /// Windows open when it was brought in; a replacement is not one of them.
        known: HashSet<Hwnd>,
    }

    struct App {
        hwnd: Hwnd,
        original: Original,
        title: String,
        program: String,
        placement: Option<AppPlacement>,
        applied: Applied,
        /// When `applied` was last sent to the window.
        applied_at: Option<Instant>,
        /// The person is moving or resizing the window by its own frame.
        dragging: bool,
        /// A rectangle the window keeps instead of its card's (a minimum
        /// size), already pushed back once.
        drift: Option<(i32, i32, i32, i32)>,
        handover: Option<Handover>,
    }

    /// What [`Apps::capture_launches`] saw at its last look.
    #[derive(Default)]
    struct Capture {
        /// Whether the sets below hold a first look yet.
        primed: bool,
        /// Processes already seen under each terminal's shell.
        seen: HashMap<String, HashSet<u32>>,
        /// Program windows that were open (see [`is_app_window`]).
        windows: HashSet<Hwnd>,
    }

    struct Inner {
        main: Hwnd,
        apps: Mutex<HashMap<String, App>>,
        events: Box<dyn Events>,
        capture_on: AtomicBool,
        capture: Mutex<Capture>,
    }

    impl Inner {
        fn apps(&self) -> MutexGuard<'_, HashMap<String, App>> {
            self.apps.lock().expect("apps lock poisoned")
        }

        fn capture(&self) -> MutexGuard<'_, Capture> {
            self.capture.lock().expect("capture lock poisoned")
        }

        fn emit(&self, event: &str, payload: impl serde::Serialize) {
            match serde_json::to_value(payload) {
                Ok(payload) => self.events.send(event, payload),
                Err(err) => eprintln!("godesk: cannot serialize {event}: {err}"),
            }
        }
    }

    /// The embedded windows, keyed by the id the frontend knows them by.
    #[derive(Clone)]
    pub struct Apps {
        inner: Arc<Inner>,
    }

    fn window_text(h: Hwnd) -> String {
        // SAFETY: plain queries on a window handle; the buffer is sized from
        // the reported length plus the NUL.
        unsafe {
            let len = GetWindowTextLengthW(hwnd(h));
            if len <= 0 {
                return String::new();
            }
            let mut buf = vec![0u16; len as usize + 1];
            let copied = GetWindowTextW(hwnd(h), buf.as_mut_ptr(), buf.len() as i32);
            String::from_utf16_lossy(&buf[..copied.max(0) as usize])
        }
    }

    fn class_name(h: Hwnd) -> String {
        let mut buf = [0u16; 256];
        // SAFETY: the buffer and its length match.
        let len = unsafe { GetClassNameW(hwnd(h), buf.as_mut_ptr(), buf.len() as i32) };
        String::from_utf16_lossy(&buf[..len.max(0) as usize])
    }

    fn process_id(h: Hwnd) -> u32 {
        let mut pid = 0;
        // SAFETY: `pid` is a valid out pointer.
        unsafe { GetWindowThreadProcessId(hwnd(h), &mut pid) };
        pid
    }

    /// The file name of a window's program, such as `Code.exe`.
    fn program_name(h: Hwnd) -> String {
        // SAFETY: the handle is checked and closed; the buffer and its length match.
        unsafe {
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, process_id(h));
            if process.is_null() {
                return String::new();
            }
            let mut buf = [0u16; 1024];
            let mut len = buf.len() as u32;
            let ok = QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len);
            CloseHandle(process);
            if ok == 0 {
                return String::new();
            }
            let path = PathBuf::from(String::from_utf16_lossy(&buf[..len as usize]));
            path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
        }
    }

    fn style(h: Hwnd) -> isize {
        // SAFETY: plain query.
        unsafe { GetWindowLongPtrW(hwnd(h), GWL_STYLE) }
    }

    fn ex_style(h: Hwnd) -> isize {
        // SAFETY: plain query.
        unsafe { GetWindowLongPtrW(hwnd(h), GWL_EXSTYLE) }
    }

    /// A program's main window that a person would call "a window": shown,
    /// titled, framed (not a splash screen), unowned, not a tool palette and
    /// not hidden on another virtual desktop. Never Godesk's own.
    fn is_app_window(h: Hwnd) -> bool {
        // SAFETY: plain queries; `cloaked` is a valid out buffer of its size.
        unsafe {
            if IsWindowVisible(hwnd(h)) == 0 || !GetWindow(hwnd(h), GW_OWNER).is_null() {
                return false;
            }
            let mut cloaked = 0u32;
            let cloak_query = DwmGetWindowAttribute(
                hwnd(h),
                DWMWA_CLOAKED as u32,
                (&raw mut cloaked).cast(),
                size_of::<u32>() as u32,
            );
            if cloak_query == 0 && cloaked != 0 {
                return false;
            }
        }
        let framed = style(h) & (WS_CAPTION | WS_THICKFRAME) as isize != 0;
        framed
            && ex_style(h) & WS_EX_TOOLWINDOW as isize == 0
            && process_id(h) != std::process::id()
            && !window_text(h).is_empty()
            && !SHELL_CLASSES.contains(&class_name(h).as_str())
    }

    fn top_level_windows() -> Vec<Hwnd> {
        unsafe extern "system" fn collect(h: HWND, list: LPARAM) -> windows_sys::core::BOOL {
            // SAFETY: `list` is the Vec passed below, alive for the call.
            unsafe { (*(list as *mut Vec<Hwnd>)).push(h as Hwnd) };
            1
        }
        let mut list: Vec<Hwnd> = Vec::new();
        // SAFETY: the callback only pushes into `list`, which outlives the call.
        unsafe { EnumWindows(Some(collect), (&raw mut list) as LPARAM) };
        list
    }

    /// VS Code's own executable: next to the `bin\code.cmd` on PATH, or in
    /// one of its install locations.
    fn vscode_exe() -> Option<PathBuf> {
        let on_path = std::env::var_os("PATH")
            .into_iter()
            .flat_map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
            .filter(|dir| dir.join("code.cmd").is_file())
            .filter_map(|bin| bin.parent().map(|dir| dir.join("Code.exe")));
        let installs = [("LOCALAPPDATA", r"Programs\Microsoft VS Code"), ("ProgramFiles", "Microsoft VS Code")]
            .into_iter()
            .filter_map(|(var, dir)| std::env::var_os(var).map(|root| PathBuf::from(root).join(dir).join("Code.exe")));
        on_path.chain(installs).find(|exe| exe.is_file())
    }

    /// Opens `path` with the program Windows associates with it, returning
    /// that program's pid when Windows started one.
    fn shell_open(path: &str) -> AppResult<Option<u32>> {
        let verb = wide("open");
        let file = wide(path);
        // SAFETY: SHELLEXECUTEINFOW is plain old data; cbSize is set and the
        // strings outlive the call. The process handle is closed.
        unsafe {
            let mut info: SHELLEXECUTEINFOW = std::mem::zeroed();
            info.cbSize = size_of::<SHELLEXECUTEINFOW>() as u32;
            info.fMask = SEE_MASK_NOCLOSEPROCESS;
            info.lpVerb = verb.as_ptr();
            info.lpFile = file.as_ptr();
            info.nShow = SW_SHOWNORMAL;
            if ShellExecuteExW(&mut info) == 0 {
                return Err(AppError::Io(std::io::Error::last_os_error()));
            }
            if info.hProcess.is_null() {
                return Ok(None);
            }
            let pid = GetProcessId(info.hProcess);
            CloseHandle(info.hProcess);
            Ok((pid != 0).then_some(pid))
        }
    }

    /// Starts what `launch` names; the pid of the started program if it is
    /// known. VS Code and Explorer hand off to a process already running, so
    /// for them the window is found by appearing, not by pid.
    fn start(launch: &AppLaunch) -> AppResult<Option<u32>> {
        match launch {
            AppLaunch::VsCode { folder } => {
                let exe = vscode_exe().ok_or_else(|| {
                    AppError::Invalid(
                        "VS Code was not found. Install it, or make sure `code` works in a terminal.".into(),
                    )
                })?;
                let mut cmd = Command::new(exe);
                cmd.arg("--new-window");
                cmd.args(folder);
                cmd.spawn()?;
                Ok(None)
            }
            AppLaunch::Explorer { folder } => {
                let windows =
                    std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
                let folder = folder.clone().map(PathBuf::from).or_else(home).unwrap_or_else(|| PathBuf::from("."));
                Command::new(windows.join("explorer.exe")).arg(folder).spawn()?;
                Ok(None)
            }
            AppLaunch::Open { path } => shell_open(path),
            AppLaunch::Window { .. } => Ok(None),
        }
    }

    fn home() -> Option<PathBuf> {
        std::env::var_os("USERPROFILE").map(PathBuf::from)
    }

    /// How a started program's window is recognized.
    #[derive(Default)]
    struct Expected {
        /// The started process, when Windows says which it is.
        pid: Option<u32>,
        /// The program's file name, when it is known by name.
        program: Option<&'static str>,
        /// The name of the folder it was asked to open: VS Code titles its
        /// windows "… - <folder> - Visual Studio Code".
        folder: Option<String>,
    }

    impl Expected {
        fn of(launch: &AppLaunch, pid: Option<u32>) -> Self {
            let folder_name = |folder: &Option<String>| {
                folder.as_deref().and_then(|f| PathBuf::from(f).file_name().map(|n| n.to_string_lossy().into_owned()))
            };
            match launch {
                AppLaunch::VsCode { folder } => Self { pid, program: Some("Code.exe"), folder: folder_name(folder) },
                AppLaunch::Explorer { .. } => Self { pid, program: Some("explorer.exe"), folder: None },
                AppLaunch::Open { .. } | AppLaunch::Window { .. } => Self { pid, ..Self::default() },
            }
        }

        fn is_program(&self, h: Hwnd) -> bool {
            self.pid.is_some_and(|pid| process_id(h) == pid)
                || self.program.is_some_and(|name| program_name(h).eq_ignore_ascii_case(name))
        }

        fn shows_folder(&self, h: Hwnd) -> bool {
            let Some(folder) = &self.folder else { return false };
            let title = window_text(h);
            title.contains(&format!(" - {folder} - ")) || title.starts_with(&format!("{folder} - "))
        }
    }

    /// The first window that was not open before and belongs to a program,
    /// preferring the started one's. A program asked for something it
    /// already shows (VS Code and a folder it has open) brings that window
    /// forward instead of making one: after [`REUSE_GRACE`] without a new
    /// window, the program's window that came to the front, or that shows
    /// the folder, is taken. `free` excludes windows already on the canvas.
    fn wait_for_window(before: &HashSet<Hwnd>, expected: &Expected, free: impl Fn(Hwnd) -> bool) -> Option<Hwnd> {
        let started = Instant::now();
        // A window already in front did not come forward for this launch.
        // SAFETY: plain query.
        let front_before = unsafe { GetForegroundWindow() } as Hwnd;
        while started.elapsed() < LAUNCH_TIMEOUT {
            let fresh: Vec<Hwnd> = top_level_windows()
                .into_iter()
                .filter(|&h| !before.contains(&h) && free(h) && is_app_window(h))
                .collect();
            let own = fresh.iter().find(|&&h| expected.is_program(h));
            if let Some(&h) = own.or(fresh.first()) {
                return Some(h);
            }
            if started.elapsed() >= REUSE_GRACE {
                // SAFETY: plain query.
                let front = Some(unsafe { GetForegroundWindow() } as Hwnd).filter(|&h| h != front_before).unwrap_or(0);
                let reused = std::iter::once(front)
                    .chain(top_level_windows().into_iter().filter(|&h| expected.shows_folder(h)))
                    .find(|&h| h != 0 && free(h) && is_app_window(h) && expected.is_program(h));
                if reused.is_some() {
                    return reused;
                }
            }
            thread::sleep(LAUNCH_POLL);
        }
        None
    }

    /// The folder VS Code's command line (`Code.exe [cli.js] <path> …`) asks
    /// for, resolved against the process's working directory.
    fn vscode_target(pid: u32) -> Option<String> {
        let argv = crate::winproc::command_line(pid)?;
        let arg = argv.iter().skip(1).find(|a| !a.starts_with('-') && !a.to_lowercase().ends_with("cli.js"))?;
        let path = crate::winproc::cwd(pid).map_or_else(|| PathBuf::from(arg), |cwd| cwd.join(arg));
        let path = std::fs::canonicalize(&path).unwrap_or(path);
        let dir = if path.is_file() { path.parent()?.to_owned() } else { path };
        Some(dir.to_string_lossy().trim_start_matches(r"\\?\").to_owned())
    }

    /// Makes `h` frameless and owned by `main`, hidden until placed, and
    /// returns what it was before.
    fn take(main: Hwnd, h: Hwnd) -> AppResult<Original> {
        // SAFETY: plain window calls on a live handle; `rect` is a valid
        // out pointer.
        unsafe {
            if IsIconic(hwnd(h)) != 0 || IsZoomed(hwnd(h)) != 0 {
                ShowWindow(hwnd(h), SW_RESTORE);
            }
            let mut rect = RECT { left: 0, top: 0, right: 0, bottom: 0 };
            GetWindowRect(hwnd(h), &mut rect);
            let original = Original {
                style: style(h),
                ex_style: ex_style(h),
                owner: GetWindowLongPtrW(hwnd(h), GWLP_HWNDPARENT),
                rect,
            };
            // Windows refuses silently for elevated programs: the owner is
            // read back rather than the call's result trusted.
            SetWindowLongPtrW(hwnd(h), GWLP_HWNDPARENT, main);
            if GetWindowLongPtrW(hwnd(h), GWLP_HWNDPARENT) != main {
                return Err(AppError::Invalid(
                    "Windows refused: that program runs as administrator, and Godesk does not.".into(),
                ));
            }
            let frame = (WS_CAPTION | WS_THICKFRAME | WS_MINIMIZEBOX | WS_MAXIMIZEBOX | WS_SYSMENU) as isize;
            SetWindowLongPtrW(hwnd(h), GWL_STYLE, original.style & !frame);
            let ex_frame = (WS_EX_APPWINDOW | WS_EX_WINDOWEDGE | WS_EX_DLGMODALFRAME) as isize;
            SetWindowLongPtrW(hwnd(h), GWL_EXSTYLE, original.ex_style & !ex_frame);
            // Hidden and shown again, so the taskbar drops its button.
            ShowWindow(hwnd(h), SW_HIDE);
            SetWindowPos(
                hwnd(h),
                std::ptr::null_mut(),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
            );
            Ok(original)
        }
    }

    fn parse_handle(handle: &str) -> AppResult<Hwnd> {
        handle.parse::<Hwnd>().map_err(|_| AppError::Invalid(format!("not a window handle: {handle}")))
    }

    impl Apps {
        /// Embeds under `main`, Godesk's own window, and reports changes
        /// through `events`.
        pub fn new(main: isize, events: Box<dyn Events>) -> Self {
            let apps = Self {
                inner: Arc::new(Inner {
                    main,
                    apps: Mutex::new(HashMap::new()),
                    events,
                    capture_on: AtomicBool::new(true),
                    capture: Mutex::new(Capture::default()),
                }),
            };
            let watched = apps.clone();
            let spawned = thread::Builder::new().name("godesk-apps".into()).spawn(move || {
                loop {
                    thread::sleep(WATCH_INTERVAL);
                    watched.watch();
                }
            });
            if let Err(err) = spawned {
                eprintln!("godesk: cannot watch embedded windows: {err}");
            }
            apps
        }

        /// Forgets windows their program closed, and passes on new titles. A
        /// launched program that replaces its window soon after (see
        /// [`HANDOVER_PERIOD`]) keeps its card, with the new window in it.
        fn watch(&self) {
            let mut changes = Vec::new();
            let mut moves = Vec::new();
            let mut apps = self.inner.apps();
            let mut embedded: HashSet<Hwnd> = apps.values().map(|a| a.hwnd).collect();
            apps.retain(|id, app| {
                // SAFETY: plain query; any value is a valid argument.
                if unsafe { IsWindow(hwnd(app.hwnd)) } == 0 {
                    let Some(handover) = app.handover.as_ref().filter(|h| Instant::now() < h.until) else {
                        changes.push(AppChangedEvent { id: id.clone(), title: app.title.clone(), closed: true });
                        return false;
                    };
                    let replacement = top_level_windows().into_iter().find(|&h| {
                        !handover.known.contains(&h)
                            && !embedded.contains(&h)
                            && is_app_window(h)
                            && program_name(h).eq_ignore_ascii_case(&app.program)
                    });
                    // Until one shows, the card waits with its window hidden.
                    if let Some(h) = replacement
                        && let Ok(original) = take(self.inner.main, h)
                    {
                        embedded.insert(h);
                        app.hwnd = h;
                        app.original = original;
                        app.applied = None;
                        self.apply(app);
                    }
                    return true;
                }
                let title = window_text(app.hwnd);
                if title != app.title && !title.is_empty() {
                    app.title = title.clone();
                    changes.push(AppChangedEvent { id: id.clone(), title, closed: false });
                }
                if let Some(rect) = self.follow(app) {
                    moves.push(AppMovedEvent { id: id.clone(), rect });
                }
                true
            });
            drop(apps);
            for change in changes {
                self.inner.emit(APP_CHANGED_EVENT, change);
            }
            for moved in moves {
                self.inner.emit(APP_MOVED_EVENT, moved);
            }
        }

        /// Keeps a window and its card together when the window moves by
        /// itself. Moved or resized by its own frame (VS Code's title bar
        /// and edges), the card follows: the new rectangle is returned, in
        /// client pixels, once the move ends. Only the window's own
        /// move/size loop counts: a card being dragged in Godesk, with the
        /// window catching up, is not the window moving. Maximized or minimized by
        /// the program, or kept at another size (a minimum), it goes back
        /// on its card.
        fn follow(&self, app: &mut App) -> Option<Rect> {
            let h = hwnd(app.hwnd);
            let ((x, y, width, height), _) = app.applied.as_ref()?;
            // SAFETY: plain window queries; `rect` and `gui` are valid out pointers.
            unsafe {
                let mut gui: GUITHREADINFO = std::mem::zeroed();
                gui.cbSize = size_of::<GUITHREADINFO>() as u32;
                let thread = GetWindowThreadProcessId(h, std::ptr::null_mut());
                if GetGUIThreadInfo(thread, &mut gui) != 0 && gui.flags & GUI_INMOVESIZE != 0 && gui.hwndMoveSize == h {
                    app.dragging = true;
                    return None;
                }
                if !app.dragging && app.applied_at.is_some_and(|at| at.elapsed() < PLACEMENT_SETTLE) {
                    return None;
                }
                if IsZoomed(h) != 0 || IsIconic(h) != 0 {
                    ShowWindow(h, SW_RESTORE);
                    app.applied = None;
                    self.apply(app);
                    return None;
                }
                let mut rect = RECT { left: 0, top: 0, right: 0, bottom: 0 };
                GetWindowRect(h, &mut rect);
                let actual = (rect.left, rect.top, rect.right - rect.left, rect.bottom - rect.top);
                if actual == (*x, *y, *width, *height) {
                    app.dragging = false;
                    app.drift = None;
                    return None;
                }
                if std::mem::take(&mut app.dragging) {
                    let main = hwnd(self.inner.main);
                    let scale = f64::from(GetDpiForWindow(main).max(96)) / 96.0;
                    let mut origin = POINT { x: 0, y: 0 };
                    ClientToScreen(main, &mut origin);
                    // Taken as placed, so the card's next placement moves nothing.
                    if let Some((screen, _)) = app.applied.as_mut() {
                        *screen = actual;
                    }
                    return Some(Rect {
                        x: f64::from(actual.0 - origin.x) / scale,
                        y: f64::from(actual.1 - origin.y) / scale,
                        width: f64::from(actual.2) / scale,
                        height: f64::from(actual.3) / scale,
                    });
                }
                if app.drift != Some(actual) {
                    app.drift = Some(actual);
                    app.applied = None;
                    self.apply(app);
                }
                None
            }
        }

        /// Whether programs started in terminals come onto the canvas.
        pub fn set_capture(&self, on: bool) {
            self.inner.capture_on.store(on, Ordering::SeqCst);
            // Turned back on, it starts from what is open then.
            *self.inner.capture() = Capture::default();
        }

        /// Brings onto the canvas what the terminals' shells (`(terminal id,
        /// shell pid)`) start: a window of one of their processes, or VS
        /// Code and File Explorer, which hand off to a process already
        /// running and are recognized by being started. Called often; the
        /// first call only takes note of what is already there.
        pub fn capture_launches(&self, shells: &[(String, u32)]) {
            if !self.inner.capture_on.load(Ordering::SeqCst) {
                return;
            }
            // Only windows ready to show count as seen: one still being
            // created (hidden, untitled) is looked at again next time.
            let windows: HashSet<Hwnd> = top_level_windows().into_iter().filter(|&h| is_app_window(h)).collect();
            let mut owners: HashMap<u32, String> = HashMap::new();
            let mut launches = Vec::new();
            let fresh: Vec<(Hwnd, String)> = {
                let mut capture = self.inner.capture();
                capture.seen.retain(|id, _| shells.iter().any(|(t, _)| t == id));
                for (terminal, shell) in shells {
                    let first = !capture.seen.contains_key(terminal);
                    let seen = capture.seen.entry(terminal.clone()).or_default();
                    for (pid, exe) in crate::winproc::descendants(*shell) {
                        owners.insert(pid, terminal.clone());
                        if seen.insert(pid) && !first && matches!(exe.as_str(), "code.exe" | "explorer.exe") {
                            launches.push((terminal.clone(), pid, exe));
                        }
                    }
                }
                let fresh = if capture.primed {
                    windows
                        .difference(&capture.windows)
                        .filter_map(|&h| owners.get(&process_id(h)).map(|t| (h, t.clone())))
                        .collect()
                } else {
                    Vec::new()
                };
                capture.windows = windows;
                capture.primed = true;
                fresh
            };
            for (h, terminal) in fresh {
                if let Ok(app) = self.adopt(h) {
                    self.inner.emit(APP_CAPTURED_EVENT, AppCapturedEvent { app, terminal_id: terminal });
                }
            }
            for (terminal, pid, exe) in launches {
                let apps = self.clone();
                let spawned = thread::Builder::new().name("godesk-capture".into()).spawn(move || {
                    let expected = if exe == "code.exe" {
                        let folder = vscode_target(pid);
                        Expected::of(&AppLaunch::VsCode { folder }, None)
                    } else {
                        Expected::of(&AppLaunch::Explorer { folder: None }, None)
                    };
                    let before: HashSet<Hwnd> = apps.inner.capture().windows.clone();
                    if let Ok(app) = apps.wait_and_adopt(&before, &expected) {
                        apps.inner.emit(APP_CAPTURED_EVENT, AppCapturedEvent { app, terminal_id: terminal });
                    }
                });
                if let Err(err) = spawned {
                    eprintln!("godesk: cannot follow a program started in a terminal: {err}");
                }
            }
        }

        /// Waits for a started program's window (see [`wait_for_window`]) and
        /// brings it in, letting the program swap it soon after.
        fn wait_and_adopt(&self, before: &HashSet<Hwnd>, expected: &Expected) -> AppResult<EmbeddedApp> {
            let free = |h: Hwnd| !self.inner.apps().values().any(|a| a.hwnd == h);
            let window = wait_for_window(before, expected, free).ok_or_else(|| {
                AppError::Invalid(
                    "No window appeared. If the program was already open, bring its window in from the list.".into(),
                )
            })?;
            let app = self.adopt(window)?;
            let known = top_level_windows().into_iter().collect();
            if let Some(adopted) = self.inner.apps().get_mut(&app.id) {
                adopted.handover = Some(Handover { until: Instant::now() + HANDOVER_PERIOD, known });
            }
            Ok(app)
        }

        /// Open windows that can be brought onto the canvas, front to back.
        pub fn list_windows(&self) -> Vec<NativeWindow> {
            let embedded: HashSet<Hwnd> = self.inner.apps().values().map(|a| a.hwnd).collect();
            top_level_windows()
                .into_iter()
                .filter(|h| !embedded.contains(h) && is_app_window(*h))
                .map(|h| NativeWindow { handle: h.to_string(), title: window_text(h), program: program_name(h) })
                .collect()
        }

        /// Starts what `launch` names, waits for its window and brings it in
        /// (or brings in the window it names). Blocks while the program starts.
        pub fn open(&self, launch: &AppLaunch) -> AppResult<EmbeddedApp> {
            if let AppLaunch::Window { handle } = launch {
                return self.adopt(parse_handle(handle)?);
            }
            let before: HashSet<Hwnd> = top_level_windows().into_iter().collect();
            let pid = start(launch)?;
            self.wait_and_adopt(&before, &Expected::of(launch, pid))
        }

        /// Takes `h` off the desktop: frameless, owned by Godesk, hidden until
        /// the frontend places it.
        fn adopt(&self, h: Hwnd) -> AppResult<EmbeddedApp> {
            if !is_app_window(h) {
                return Err(AppError::Invalid("That window is gone or cannot be brought onto the canvas.".into()));
            }
            if self.inner.apps().values().any(|a| a.hwnd == h) {
                return Err(AppError::Invalid("That window is already on the canvas.".into()));
            }
            let original = take(self.inner.main, h)?;
            let app =
                EmbeddedApp { id: uuid::Uuid::new_v4().to_string(), title: window_text(h), program: program_name(h) };
            self.inner.apps().insert(
                app.id.clone(),
                App {
                    hwnd: h,
                    original,
                    title: app.title.clone(),
                    program: app.program.clone(),
                    placement: None,
                    applied: None,
                    applied_at: None,
                    dragging: false,
                    drift: None,
                    handover: None,
                },
            );
            Ok(app)
        }

        /// Moves each window over its card (see [`AppPlacement`]).
        pub fn place(&self, placements: Vec<AppPlacement>) {
            let mut apps = self.inner.apps();
            for placement in placements {
                if let Some(app) = apps.get_mut(&placement.id) {
                    app.placement = Some(placement);
                    self.apply(app);
                }
            }
        }

        /// Places every window again, after Godesk's own window moved or
        /// changed size or scale.
        pub fn replace_all(&self) {
            for app in self.inner.apps().values_mut() {
                app.applied = None;
                self.apply(app);
            }
        }

        fn apply(&self, app: &mut App) {
            let h = hwnd(app.hwnd);
            let main = hwnd(self.inner.main);
            let Some(placement) = &app.placement else { return };
            // SAFETY: plain window calls; every out pointer is valid. A region
            // given to SetWindowRgn belongs to the system, any other is deleted.
            unsafe {
                let mut client = RECT { left: 0, top: 0, right: 0, bottom: 0 };
                GetClientRect(main, &mut client);
                let scale = f64::from(GetDpiForWindow(main).max(96)) / 96.0;
                let px = |v: f64| (v * scale).round() as i32;
                let to_px = |r: &Rect| (px(r.x), px(r.y), px(r.x + r.width), px(r.y + r.height));
                let (left, top, right, bottom) = to_px(&placement.rect);
                // The part of the card inside Godesk's window, relative to the card.
                let shown = (left.max(0), top.max(0), right.min(client.right), bottom.min(client.bottom));
                if !placement.visible || IsIconic(main) != 0 || shown.0 >= shown.2 || shown.1 >= shown.3 {
                    let was_shown = app.applied.take().is_some();
                    if was_shown || IsWindowVisible(h) != 0 {
                        // On screen this last moment: its picture stands in for it
                        // on the card while it is hidden.
                        // Read from the screen, so only while Godesk (or the window)
                        // is in front: anything covering it would be pictured instead.
                        let front = GetForegroundWindow();
                        if was_shown && IsWindowVisible(h) != 0 && IsIconic(main) == 0 && (front == main || front == h)
                        {
                            self.send_preview(placement.id.clone(), h);
                        }
                        ShowWindow(h, SW_HIDE);
                    }
                    return;
                }
                let mut origin = POINT { x: 0, y: 0 };
                ClientToScreen(main, &mut origin);
                let screen = (origin.x + left, origin.y + top, right - left, bottom - top);
                let mut clip = vec![(shown.0 - left, shown.1 - top, shown.2 - left, shown.3 - top)];
                clip.extend(placement.cutouts.iter().map(|c| {
                    let (l, t, r, b) = to_px(c);
                    (l - left, t - top, r - left, b - top)
                }));
                let key = (screen, clip);
                if app.applied.as_ref() == Some(&key) {
                    return;
                }
                let ((x, y, width, height), clip) = &key;
                let region = CreateRectRgn(clip[0].0, clip[0].1, clip[0].2, clip[0].3);
                for &(l, t, r, b) in &clip[1..] {
                    let cut = CreateRectRgn(l, t, r, b);
                    CombineRgn(region, region, cut, RGN_DIFF);
                    DeleteObject(cut);
                }
                if SetWindowRgn(h, region, 1) == 0 {
                    DeleteObject(region);
                }
                SetWindowPos(
                    h,
                    std::ptr::null_mut(),
                    *x,
                    *y,
                    *width,
                    *height,
                    // Sent, so the window keeps up with its card; posted to a
                    // program that hangs, so that it cannot hang Godesk.
                    if IsHungAppWindow(h) != 0 { SWP_ASYNCWINDOWPOS } else { 0 }
                        | SWP_NOACTIVATE
                        | SWP_NOZORDER
                        | SWP_NOOWNERZORDER
                        | SWP_SHOWWINDOW,
                );
                app.applied = Some(key);
                app.applied_at = Some(Instant::now());
            }
        }

        /// Pictures the window from the screen (it must be showing), and sends
        /// the picture as a PNG data URL from another thread.
        fn send_preview(&self, id: String, h: HWND) {
            let Some((width, height, rgb)) = capture(h) else { return };
            let inner = self.inner.clone();
            let _ = thread::Builder::new().name("godesk-preview".into()).spawn(move || {
                let mut png = Vec::new();
                let mut encoder = png::Encoder::new(&mut png, width, height);
                encoder.set_color(png::ColorType::Rgb);
                encoder.set_depth(png::BitDepth::Eight);
                let written = encoder.write_header().and_then(|mut writer| writer.write_image_data(&rgb));
                if written.is_ok() {
                    use base64::Engine;
                    let image =
                        format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(&png));
                    inner.emit(APP_PREVIEW_EVENT, AppPreviewEvent { id, image });
                }
            });
        }

        /// Raises the window above the other embedded ones and gives it the
        /// keyboard.
        pub fn focus(&self, id: &str) {
            if let Some(app) = self.inner.apps().get(id) {
                // SAFETY: plain window calls.
                unsafe {
                    SetWindowPos(hwnd(app.hwnd), HWND_TOP, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE);
                    SetForegroundWindow(hwnd(app.hwnd));
                }
            }
        }

        /// Asks the program to close the window, as its own × would; it may
        /// ask to save first. The card goes once the window has.
        pub fn close(&self, id: &str) -> AppResult<()> {
            let mut apps = self.inner.apps();
            let app = apps.get_mut(id).ok_or_else(|| AppError::NotFound(format!("app {id}")))?;
            // Closed on purpose: the card goes with the window, no replacement awaited.
            app.handover = None;
            // SAFETY: posting a message has no memory-safety preconditions.
            unsafe { PostMessageW(hwnd(app.hwnd), WM_CLOSE, 0 as WPARAM, 0 as LPARAM) };
            Ok(())
        }

        /// Gives the window back to the desktop as it was before.
        pub fn release(&self, id: &str) -> AppResult<()> {
            let app = self.inner.apps().remove(id).ok_or_else(|| AppError::NotFound(format!("app {id}")))?;
            restore(&app);
            Ok(())
        }

        /// Gives every window back, so none is lost with Godesk.
        pub fn release_all(&self) {
            let apps: Vec<App> = self.inner.apps().drain().map(|(_, app)| app).collect();
            for app in &apps {
                restore(app);
            }
        }
    }

    /// What `h` shows on screen, as RGB rows, shrunk to at most
    /// [`PREVIEW_MAX_WIDTH`]. Read from the screen rather than asked of the
    /// window: VS Code and other GPU-drawn programs answer PrintWindow blank.
    fn capture(h: HWND) -> Option<(u32, u32, Vec<u8>)> {
        // SAFETY: GDI objects are created, used and released here; the
        // buffer holds exactly width × height 32-bit pixels.
        unsafe {
            let mut rect = RECT { left: 0, top: 0, right: 0, bottom: 0 };
            GetWindowRect(h, &mut rect);
            let (width, height) = (rect.right - rect.left, rect.bottom - rect.top);
            if width <= 0 || height <= 0 {
                return None;
            }
            let screen = GetDC(std::ptr::null_mut());
            let memory = CreateCompatibleDC(screen);
            let bitmap = CreateCompatibleBitmap(screen, width, height);
            let previous = SelectObject(memory, bitmap);
            let copied = BitBlt(memory, 0, 0, width, height, screen, rect.left, rect.top, SRCCOPY | CAPTUREBLT);
            let mut info: BITMAPINFO = std::mem::zeroed();
            info.bmiHeader = BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                // Negative: rows top to bottom.
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                ..std::mem::zeroed()
            };
            let mut bgra = vec![0u8; (width * height * 4) as usize];
            let rows = GetDIBits(memory, bitmap, 0, height as u32, bgra.as_mut_ptr().cast(), &mut info, DIB_RGB_COLORS);
            SelectObject(memory, previous);
            DeleteObject(bitmap);
            DeleteDC(memory);
            ReleaseDC(std::ptr::null_mut(), screen);
            if copied == 0 || rows == 0 {
                return None;
            }
            Some(shrink(&bgra, width as u32, height as u32))
        }
    }

    /// BGRA pixels to RGB, each output pixel the average of the block it covers.
    fn shrink(bgra: &[u8], width: u32, height: u32) -> (u32, u32, Vec<u8>) {
        let factor = width.div_ceil(PREVIEW_MAX_WIDTH).max(1);
        let (out_w, out_h) = (width / factor, height / factor);
        let mut rgb = Vec::with_capacity((out_w * out_h * 3) as usize);
        for oy in 0..out_h {
            for ox in 0..out_w {
                let mut sum = [0u32; 3];
                for dy in 0..factor {
                    for dx in 0..factor {
                        let i = (((oy * factor + dy) * width + ox * factor + dx) * 4) as usize;
                        sum[0] += u32::from(bgra[i + 2]);
                        sum[1] += u32::from(bgra[i + 1]);
                        sum[2] += u32::from(bgra[i]);
                    }
                }
                let n = factor * factor;
                rgb.extend(sum.map(|c| (c / n) as u8));
            }
        }
        (out_w, out_h, rgb)
    }

    fn restore(app: &App) {
        let h = hwnd(app.hwnd);
        let o = &app.original;
        // SAFETY: plain window calls; a null region removes the clipping.
        unsafe {
            if IsWindow(h) == 0 {
                return;
            }
            SetWindowRgn(h, std::ptr::null_mut(), 1);
            SetWindowLongPtrW(h, GWLP_HWNDPARENT, o.owner);
            SetWindowLongPtrW(h, GWL_STYLE, o.style);
            SetWindowLongPtrW(h, GWL_EXSTYLE, o.ex_style);
            // Hidden and shown again, so the taskbar shows its button again.
            ShowWindow(h, SW_HIDE);
            SetWindowPos(
                h,
                HWND_TOP,
                o.rect.left,
                o.rect.top,
                o.rect.right - o.rect.left,
                o.rect.bottom - o.rect.top,
                SWP_FRAMECHANGED | SWP_NOACTIVATE,
            );
            ShowWindow(h, SW_SHOWNA);
        }
    }
}

/// Real programs started from a real ConPTY shell come onto the canvas.
/// Ignored: it opens Notepad and a VS Code window on the desktop.
#[cfg(all(test, windows))]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use super::*;
    use crate::pty::{OutputSink, PtyConfig, PtyManager, SpawnRequest};

    #[derive(Default)]
    struct Recorder(Mutex<Vec<(String, serde_json::Value)>>);

    impl Events for Arc<Recorder> {
        fn send(&self, event: &str, payload: serde_json::Value) {
            self.0.lock().unwrap().push((event.to_owned(), payload));
        }
    }

    #[derive(Default)]
    struct Output(Mutex<Vec<u8>>);

    impl OutputSink for Output {
        fn on_output(&self, _: &str, chunk: &[u8], _: u64) {
            self.0.lock().unwrap().extend_from_slice(chunk);
        }
        fn on_exit(&self, _: &str, _: Option<u32>) {}
    }

    /// `(program, app id)` of every window brought in, in order.
    fn captured(recorder: &Recorder) -> Vec<(String, String)> {
        let events = recorder.0.lock().unwrap();
        events
            .iter()
            .filter(|(e, _)| e == APP_CAPTURED_EVENT)
            .map(|(_, p)| {
                let app = &p["app"];
                (app["program"].as_str().unwrap().to_owned(), app["id"].as_str().unwrap().to_owned())
            })
            .collect()
    }

    #[test]
    #[ignore = "opens Notepad and VS Code on the desktop"]
    fn programs_started_in_a_terminal_come_onto_the_canvas() {
        let project = tempfile::tempdir().unwrap();
        let output = Arc::new(Output::default());
        let ptys = PtyManager::new(
            PtyConfig { home: project.path().to_owned(), shell_override: None, ..PtyConfig::default() },
            output.clone(),
        );
        let req = SpawnRequest {
            terminal_id: "t1",
            terminal_name: "t1",
            workspace_id: "ws",
            cwd: None,
            history: None,
            cols: 120,
            rows: 30,
        };
        ptys.spawn(&req).unwrap();
        let recorder = Arc::new(Recorder::default());
        let apps = Apps::new(0, Box::new(recorder.clone()));
        // ConPTY waits for the cursor position, which xterm.js answers in the app.
        let deadline = Instant::now() + Duration::from_secs(20);
        while !String::from_utf8_lossy(&output.0.lock().unwrap()).contains("\x1b[6n") {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(50));
        }
        ptys.write("t1", b"\x1b[1;1R".to_vec()).unwrap();
        apps.capture_launches(&ptys.shell_pids());

        let wait_for = |program: &str, times: usize| {
            let deadline = Instant::now() + Duration::from_secs(30);
            while captured(&recorder).iter().filter(|(p, _)| p.eq_ignore_ascii_case(program)).count() < times {
                assert!(Instant::now() < deadline, "{program} never came onto the canvas: {:?}", captured(&recorder));
                apps.capture_launches(&ptys.shell_pids());
                std::thread::sleep(Duration::from_millis(200));
            }
        };
        ptys.write("t1", b"notepad\r".to_vec()).unwrap();
        wait_for("notepad.exe", 1);
        ptys.write("t1", b"code .\r".to_vec()).unwrap();
        wait_for("Code.exe", 1);

        // Asked again for the folder it has open, VS Code reuses that window
        // rather than making one. Given back to the desktop first, the window
        // comes onto the canvas again.
        let (_, vscode) = captured(&recorder).pop().unwrap();
        apps.release(&vscode).unwrap();
        ptys.write("t1", b"code .\r".to_vec()).unwrap();
        wait_for("Code.exe", 2);

        // Closed: Notepad's empty document, and the VS Code window this test
        // opened on its temporary folder.
        let ids: Vec<String> = captured(&recorder).into_iter().map(|(_, id)| id).collect();
        for id in ids.iter().filter(|id| **id != vscode) {
            apps.close(id).unwrap();
        }
        std::thread::sleep(Duration::from_secs(1));
        apps.release_all();
        ptys.close_all();
    }
}
