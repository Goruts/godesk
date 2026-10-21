//! `#[tauri::command]` handlers. Every mutation goes through [`Model`] (which
//! validates it), schedules a save, and only then touches processes.
//!
//! [`Model`]: crate::state::Model

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use tauri::State;

use crate::core::{Core, now_ms};
use crate::error::{AppError, AppResult};
use crate::model::{AppSnapshot, AttachResult, Camera, Link, Rect, TerminalWindow, Workspace};
use crate::pty::{DEFAULT_COLS, DEFAULT_ROWS, SpawnRequest};

impl Core {
    fn mutate<T>(&self, change: impl FnOnce(&mut crate::state::Model) -> AppResult<T>) -> AppResult<T> {
        let result = change(&mut self.shared.model())?;
        self.shared.persister.mark_dirty();
        Ok(result)
    }

    /// Starts the shell of a terminal that exists in the model.
    pub fn spawn_terminal(&self, terminal_id: &str, cols: u16, rows: u16) -> AppResult<()> {
        let (name, workspace_id) = {
            let model = self.shared.model();
            let terminal = model.terminal(terminal_id).ok_or_else(|| AppError::terminal(terminal_id))?;
            let workspace = model.workspace_of(terminal_id).expect("terminal belongs to a workspace");
            (terminal.name.clone(), workspace.id.clone())
        };
        self.ptys.spawn(&SpawnRequest { terminal_id, terminal_name: &name, workspace_id: &workspace_id, cols, rows })
    }

    fn close_terminals(&self, terminal_ids: &[String]) {
        for id in terminal_ids {
            self.ptys.close(id);
            self.shared.forget_terminal(id);
        }
    }

    fn notify_links(&self, workspace_id: &str) {
        let links = self.shared.model().links_of(workspace_id);
        self.shared.emit_links(workspace_id, links);
    }
}

#[tauri::command]
pub fn get_state(core: State<'_, Core>) -> AppSnapshot {
    core.shared.model().snapshot()
}

#[tauri::command]
pub fn create_workspace(core: State<'_, Core>, name: Option<String>) -> AppResult<Workspace> {
    core.mutate(|m| m.create_workspace(name.as_deref()))
}

#[tauri::command]
pub fn rename_workspace(core: State<'_, Core>, workspace_id: String, name: String) -> AppResult<()> {
    core.mutate(|m| m.rename_workspace(&workspace_id, &name))
}

/// Deletes a workspace, stops its shells and returns the new active
/// workspace id.
#[tauri::command]
pub fn delete_workspace(core: State<'_, Core>, workspace_id: String) -> AppResult<String> {
    let (terminals, active) = core.mutate(|m| {
        let terminals = m.delete_workspace(&workspace_id)?;
        Ok((terminals, m.active_workspace_id().to_owned()))
    })?;
    core.close_terminals(&terminals);
    Ok(active)
}

#[tauri::command]
pub fn set_active_workspace(core: State<'_, Core>, workspace_id: String) -> AppResult<()> {
    core.mutate(|m| m.set_active_workspace(&workspace_id))
}

#[tauri::command]
pub fn update_camera(core: State<'_, Core>, workspace_id: String, camera: Camera) -> AppResult<()> {
    core.mutate(|m| m.update_camera(&workspace_id, camera))
}

/// Adds a terminal at `rect` (canvas coordinates) and starts its shell.
#[tauri::command]
pub fn create_terminal(core: State<'_, Core>, workspace_id: String, rect: Rect) -> AppResult<TerminalWindow> {
    let terminal = core.mutate(|m| m.add_terminal(&workspace_id, rect))?;
    if let Err(err) = core.spawn_terminal(&terminal.id, DEFAULT_COLS, DEFAULT_ROWS) {
        core.mutate(|m| m.remove_terminal(&terminal.id))?;
        return Err(err);
    }
    Ok(terminal)
}

/// Removes a terminal, its links, and stops its shell.
#[tauri::command]
pub fn close_terminal(core: State<'_, Core>, terminal_id: String) -> AppResult<()> {
    let workspace_id = core.mutate(|m| m.remove_terminal(&terminal_id))?;
    core.close_terminals(std::slice::from_ref(&terminal_id));
    core.notify_links(&workspace_id);
    Ok(())
}

#[tauri::command]
pub fn rename_terminal(core: State<'_, Core>, terminal_id: String, name: String) -> AppResult<()> {
    core.mutate(|m| m.rename_terminal(&terminal_id, &name))
}

/// Stores a new position/size and returns it after clamping.
#[tauri::command]
pub fn update_terminal_rect(core: State<'_, Core>, terminal_id: String, rect: Rect) -> AppResult<Rect> {
    core.mutate(|m| m.update_terminal_rect(&terminal_id, rect))
}

/// Brings a terminal to the front and returns its new z.
#[tauri::command]
pub fn focus_terminal(core: State<'_, Core>, terminal_id: String) -> AppResult<u32> {
    core.mutate(|m| m.bring_to_front(&terminal_id))
}

#[tauri::command]
pub fn terminal_has_foreground_process(core: State<'_, Core>, terminal_id: String) -> AppResult<bool> {
    if !core.ptys.has_session(&terminal_id) {
        return Ok(false);
    }
    core.ptys.has_foreground_process(&terminal_id)
}

/// Connects a view to a terminal: starts the shell if it is not running yet,
/// applies the view's size and returns the output to replay.
#[tauri::command]
pub fn attach_terminal(core: State<'_, Core>, terminal_id: String, cols: u16, rows: u16) -> AppResult<AttachResult> {
    if !core.ptys.has_session(&terminal_id) {
        core.spawn_terminal(&terminal_id, cols, rows)?;
    } else {
        let _ = core.ptys.resize(&terminal_id, cols, rows);
    }
    let (data, seq, alive) = core.ptys.snapshot(&terminal_id)?;
    Ok(AttachResult { data: BASE64.encode(data), seq, alive })
}

/// Replaces the terminal's shell with a fresh one.
#[tauri::command]
pub fn restart_terminal(core: State<'_, Core>, terminal_id: String, cols: u16, rows: u16) -> AppResult<()> {
    core.shared.forget_terminal(&terminal_id);
    core.spawn_terminal(&terminal_id, cols, rows)
}

/// Sends input (base64 of the raw bytes) to the terminal.
#[tauri::command]
pub fn write_terminal(core: State<'_, Core>, terminal_id: String, data: String) -> AppResult<()> {
    let bytes = BASE64.decode(data).map_err(|e| AppError::Invalid(format!("input is not base64: {e}")))?;
    core.shared.observe_input(&terminal_id, &bytes);
    core.ptys.write(&terminal_id, bytes)
}

#[tauri::command]
pub fn resize_terminal(core: State<'_, Core>, terminal_id: String, cols: u16, rows: u16) -> AppResult<()> {
    core.ptys.resize(&terminal_id, cols, rows)
}

/// Creates a manual link between two terminals of the same workspace.
#[tauri::command]
pub fn create_link(core: State<'_, Core>, source_id: String, target_id: String) -> AppResult<Link> {
    let (workspace_id, link) = core.mutate(|m| m.add_manual_link(&source_id, &target_id, now_ms()))?;
    core.notify_links(&workspace_id);
    Ok(link)
}

#[tauri::command]
pub fn delete_link(core: State<'_, Core>, link_id: String) -> AppResult<()> {
    let workspace_id = core.mutate(|m| m.remove_link(&link_id))?;
    core.notify_links(&workspace_id);
    Ok(())
}
