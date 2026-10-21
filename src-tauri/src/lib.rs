//! Godesk backend: real shells on an infinite canvas of terminals.

mod commands;
mod core;
pub mod error;
pub mod links;
pub mod model;
pub mod persist;
pub mod pinch;
pub mod pty;
pub mod scrollback;
pub mod state;
pub mod text;

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use tauri::{Manager, RunEvent};

use crate::core::{Core, CoreSink, Shared};
use crate::links::LinkDetector;
use crate::persist::{Persister, STATE_FILE};
use crate::pty::{DEFAULT_COLS, DEFAULT_ROWS, PtyConfig, PtyManager};

/// How often idle detected links are swept.
const LINK_SWEEP_INTERVAL: Duration = Duration::from_secs(1);

fn setup(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let state_path = app.path().app_data_dir()?.join(STATE_FILE);
    let home = app.path().home_dir().unwrap_or_else(|_| std::env::temp_dir());

    let model = Arc::new(Mutex::new(persist::load(&state_path)));
    let shared = Arc::new(Shared {
        events: Box::new(app.handle().clone()),
        persister: Persister::spawn(state_path, model.clone()),
        model,
        detector: Mutex::new(LinkDetector::default()),
    });
    let ptys = PtyManager::new(PtyConfig { home, shell_override: None }, Arc::new(CoreSink(shared.clone())));
    let core = Core { shared: shared.clone(), ptys };

    #[cfg(target_os = "linux")]
    if let Some(window) = app.get_webview_window("main") {
        pinch::attach(&window, shared.clone())?;
    }

    // Restore every saved terminal with a fresh shell, so link detection
    // works in all workspaces from the start, not only the visible one.
    let terminal_ids: Vec<String> =
        shared.model().workspaces().iter().flat_map(|w| w.terminals.iter().map(|t| t.id.clone())).collect();
    for id in terminal_ids {
        if let Err(err) = core.spawn_terminal(&id, DEFAULT_COLS, DEFAULT_ROWS) {
            eprintln!("godesk: cannot start terminal {id}: {err}");
        }
    }
    app.manage(core);

    thread::Builder::new().name("godesk-link-sweep".into()).spawn(move || {
        loop {
            thread::sleep(LINK_SWEEP_INTERVAL);
            shared.expire_links();
        }
    })?;
    Ok(())
}

pub fn run() {
    let app = tauri::Builder::default()
        .setup(setup)
        .invoke_handler(tauri::generate_handler![
            commands::get_state,
            commands::create_workspace,
            commands::rename_workspace,
            commands::delete_workspace,
            commands::set_active_workspace,
            commands::update_camera,
            commands::create_terminal,
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
        ])
        .build(tauri::generate_context!())
        .expect("error while building Godesk");

    app.run(|app, event| {
        if let RunEvent::Exit = event
            && let Some(core) = app.try_state::<Core>()
        {
            core.shared.persister.flush();
            core.ptys.close_all();
        }
    });
}
