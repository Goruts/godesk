//! Godesk backend: real shells on an infinite canvas of terminals.

mod agent_session;
mod agent_terminal;
mod agent_watch;
mod agents;
mod bridge;
mod commands;
mod core;
mod dirs;
pub mod embed;
pub mod error;
mod extract;
pub mod knowledge;
pub mod links;
pub mod model;
pub mod persist;
pub mod pinch;
pub mod pty;
mod resume;
mod search_index;
pub mod scrollback;
mod session;
pub mod state;
mod whisper;
pub mod text;
#[cfg(windows)]
pub mod winproc;

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use tauri::{Manager, RunEvent, WindowEvent};

use crate::core::{Core, CoreSink, Shared};
use crate::embed::Apps;
use crate::persist::{Persister, STATE_FILE};
use crate::pty::{PtyConfig, PtyManager};

/// How often what each terminal is doing is saved for a restart.
const SESSION_SAVE_INTERVAL: Duration = Duration::from_secs(15);
/// How often the agents' session files are read for tokens and subagents.
const AGENT_WATCH_INTERVAL: Duration = Duration::from_secs(2);
/// How often the shells are checked for an agent to show in Godesk's view.
/// Short, so the agent's own screen is closed before it draws much.
const AGENT_TAKEOVER_INTERVAL: Duration = Duration::from_millis(120);
/// How often the terminals' shells are checked for programs they started.
#[cfg(windows)]
const LAUNCH_WATCH_INTERVAL: Duration = Duration::from_millis(200);
/// Shows Godesk (maximized) from anywhere, or minimizes it when it is in front.
#[cfg(windows)]
const TOGGLE_SHORTCUT: &str = "ctrl+alt+f";
/// How often a shortcut held by another program is tried again.
#[cfg(windows)]
const SHORTCUT_RETRY: Duration = Duration::from_secs(5);

/// What [`TOGGLE_SHORTCUT`] does.
#[cfg(windows)]
fn toggle_main_window(app: &tauri::AppHandle) {
    let Some(window) = app.get_webview_window("main") else { return };
    let in_front = window.is_visible().unwrap_or(false)
        && !window.is_minimized().unwrap_or(false)
        && window.is_focused().unwrap_or(false);
    let result = if in_front {
        window.minimize()
    } else {
        window
            .unminimize()
            .and_then(|()| window.show())
            .and_then(|()| window.maximize())
            .and_then(|()| window.set_focus())
    };
    if let Err(err) = result {
        eprintln!("godesk: cannot toggle the window: {err}");
    }
}

fn setup(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let data_dir = app.path().app_data_dir()?;
    let state_path = data_dir.join(STATE_FILE);
    let home = app.path().home_dir().unwrap_or_else(|_| std::env::temp_dir());

    let model = Arc::new(Mutex::new(persist::load(&state_path)));
    let shared = Arc::new(Shared::new(
        Box::new(core::AsyncEmitter::new(app.handle().clone())),
        model.clone(),
        Persister::spawn(state_path, model),
    ));
    // Keep the agents' note up to date with Godesk's wording, but only when
    // the user already turned it on — never write it unasked.
    if agents::installed(&home) {
        if let Err(err) = agents::set(&home, true) {
            eprintln!("godesk: cannot refresh the agents' note: {err}");
        }
    }
    // The `godesk` command that lets terminals reach each other. Without it
    // (the port or the folder unavailable) the terminals still work.
    let bridge = bridge::bind(&data_dir).map_err(|err| eprintln!("godesk: no terminal bridge: {err}")).ok();
    let config = PtyConfig {
        home,
        env: bridge.as_ref().map(|(_, env)| env.vars.clone()).unwrap_or_default(),
        path_prefix: bridge.as_ref().map(|(_, env)| env.bin_dir.clone()),
        ..PtyConfig::default()
    };
    let ptys = PtyManager::new(config, Arc::new(CoreSink(shared.clone())));
    let core = Core {
        shared: shared.clone(),
        ptys,
        sessions: Arc::new(session::SessionStore::new(&data_dir)),
        home: app.path().home_dir().unwrap_or_else(|_| std::env::temp_dir()),
        agents: agent_session::AgentSessions::kept_in(data_dir.join("agents")),
        agent_view: Arc::new(std::sync::atomic::AtomicBool::new(true)),
        classic: Arc::default(),
        dirs: Arc::new(dirs::DirStore::load(data_dir.join("dirs.json"))),
        kb: knowledge::KbStore::new(data_dir.join("kb")),
        data_dir: data_dir.clone(),
    };
    if let Some((listener, env)) = bridge {
        let token = env.vars.iter().find(|(k, _)| k == "GODESK_TOKEN").map(|(_, v)| v.clone()).unwrap_or_default();
        let reach = bridge::Reach {
            ptys: core.ptys.clone(),
            agents: core.agents.clone(),
            agent_sink: core.agent_sink(),
            dirs: core.dirs.clone(),
            kb: core.kb.clone(),
        };
        bridge::serve(listener, token, shared.clone(), reach);
    }

    #[cfg(target_os = "linux")]
    if let Some(window) = app.get_webview_window("main") {
        pinch::attach(&window, shared.clone())?;
    }

    #[cfg(windows)]
    let main = app.get_webview_window("main").ok_or("no main window")?.hwnd()?.0 as isize;
    #[cfg(not(windows))]
    let main = 0;
    let apps = Apps::new(main, Box::new(core::AsyncEmitter::new(app.handle().clone())));
    app.manage(apps.clone());

    // Another program may hold the shortcut when Godesk starts: it is tried
    // again until it is free, and Godesk works without it meanwhile.
    #[cfg(windows)]
    {
        use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
        let handle = app.handle().clone();
        thread::Builder::new().name("godesk-shortcut".into()).spawn(move || {
            let mut reported = false;
            loop {
                let registered = handle.global_shortcut().on_shortcut(TOGGLE_SHORTCUT, |app, _, event| {
                    if event.state == ShortcutState::Pressed {
                        toggle_main_window(app);
                    }
                });
                match registered {
                    Ok(()) => return,
                    Err(err) if !reported => {
                        eprintln!("godesk: cannot register {TOGGLE_SHORTCUT} yet: {err}");
                        reported = true;
                    }
                    Err(_) => {}
                }
                thread::sleep(SHORTCUT_RETRY);
            }
        })?;
    }

    // Reopen every saved terminal where it was, with its output, and the
    // agent it showed with its conversation, in all workspaces: link
    // detection works in the hidden ones too.
    let terminal_ids: Vec<String> =
        shared.model().workspaces().iter().flat_map(|w| w.terminals.iter().map(|t| t.id.clone())).collect();
    for id in terminal_ids {
        if let Err(err) = core.restore_terminal(&id) {
            eprintln!("godesk: cannot start terminal {id}: {err}");
        }
        let agent = shared.model().terminal(&id).and_then(|t| t.agent.clone());
        if let Some(agent) = agent {
            core.reopen_agent(&id, &agent);
        }
    }
    let takeover = core.clone();
    thread::Builder::new().name("godesk-agent-takeover".into()).spawn(move || {
        let mut checked = std::collections::HashMap::new();
        loop {
            thread::sleep(AGENT_TAKEOVER_INTERVAL);
            takeover.take_over_agents(&mut checked);
        }
    })?;
    let watch = Arc::new(agent_watch::AgentWatch::new(core.home.clone()));
    app.manage(watch.clone());
    let watcher = core.clone();
    thread::Builder::new().name("godesk-agents".into()).spawn(move || {
        loop {
            thread::sleep(AGENT_WATCH_INTERVAL);
            if let Some(event) = watch.changed(watch.scan(&watcher.watched_agents())) {
                watcher.shared.emit(agent_watch::AGENTS_CHANGED_EVENT, event);
            }
        }
    })?;
    // Kept current while Godesk runs, so a crash or power cut loses little.
    let saved = core.clone();
    thread::Builder::new().name("godesk-sessions".into()).spawn(move || {
        loop {
            thread::sleep(SESSION_SAVE_INTERVAL);
            saved.save_sessions();
            // Learn the folders the terminals work in, for the folder palette
            // and the `z` jump, ranked like zoxide.
            saved.record_dirs();
        }
    })?;
    #[cfg(windows)]
    {
        let ptys = core.ptys.clone();
        thread::Builder::new().name("godesk-launches".into()).spawn(move || {
            loop {
                thread::sleep(LAUNCH_WATCH_INTERVAL);
                apps.capture_launches(&ptys.shell_pids());
            }
        })?;
    }
    #[cfg(not(windows))]
    drop(apps);
    app.manage(core);
    Ok(())
}

pub fn run() {
    let app = tauri::Builder::default()
        // WebKitGTK rejects navigator.clipboard even inside a key press, so
        // terminal copy/paste reads and writes the clipboard from Rust.
        .plugin(tauri_plugin_clipboard_manager::init())
        // Folder and file pickers for what to open on the canvas.
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .setup(setup)
        .invoke_handler(tauri::generate_handler![
            commands::get_state,
            commands::create_workspace,
            commands::rename_workspace,
            commands::delete_workspace,
            commands::set_active_workspace,
            commands::update_camera,
            commands::create_terminal,
            commands::create_terminal_in,
            commands::list_dirs,
            commands::whisper_ready,
            commands::whisper_can_stream,
            commands::whisper_install,
            commands::transcribe,
            commands::detach_agent,
            commands::set_agent_view,
            commands::agent_to_classic,
            commands::send_agent_message,
            commands::interrupt_agent,
            commands::restart_agent,
            commands::set_agent_model,
            commands::answer_agent_permission,
            commands::agent_output,
            commands::agent_folder,
            commands::duplicate_terminal,
            commands::close_terminal,
            commands::rename_terminal,
            commands::update_terminal_rect,
            commands::focus_terminal,
            commands::terminal_has_foreground_process,
            commands::attach_terminal,
            commands::restart_terminal,
            commands::write_terminal,
            commands::resize_terminal,
            commands::create_link,
            commands::delete_link,
            commands::create_knowledge_base,
            commands::remove_knowledge_base,
            commands::rename_knowledge_base,
            commands::update_knowledge_base_rect,
            commands::terminal_folder,
            commands::pick_path,
            commands::list_windows,
            commands::open_app,
            commands::place_apps,
            commands::focus_app,
            commands::close_app,
            commands::release_app,
            commands::set_capture_launches,
            commands::agent_notes_installed,
            commands::set_agent_notes,
            commands::running_programs,
            commands::get_agents,
            commands::prepare_close,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Godesk");

    app.run(|app, event| match event {
        // Embedded windows follow Godesk's window, and go back to the desktop
        // before it closes so that no program loses its window with it.
        RunEvent::WindowEvent { event, .. } => {
            let Some(apps) = app.try_state::<Apps>() else { return };
            match event {
                WindowEvent::Moved(_) | WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                    apps.replace_all();
                }
                // Closing may still be cancelled (see prepare_close).
                WindowEvent::Destroyed => apps.release_all(),
                _ => {}
            }
        }
        RunEvent::Exit => {
            if let Some(apps) = app.try_state::<Apps>() {
                apps.release_all();
            }
            if let Some(core) = app.try_state::<Core>() {
                core.save_sessions();
                core.shared.persister.flush();
                core.agents.close_all();
                core.ptys.close_all();
            }
            whisper::shutdown();
        }
        _ => {}
    });
}
