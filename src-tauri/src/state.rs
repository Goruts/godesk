//! In-memory application model and every rule that keeps it consistent.
//! Nothing here touches Tauri or processes, so it is tested directly.

use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::links::{DETECTED_LINK_TTL_MS, LINK_ACTIVITY_THROTTLE_MS, Peer};
use crate::model::{AppSnapshot, Camera, Link, LinkKind, Rect, STATE_VERSION, TerminalWindow, Workspace};

const MAX_NAME_CHARS: usize = 64;

fn new_id() -> String {
    Uuid::new_v4().to_string()
}

fn clean_name(name: &str) -> AppResult<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(AppError::Invalid("name cannot be empty".into()));
    }
    Ok(name.chars().take(MAX_NAME_CHARS).collect())
}

fn new_workspace(name: String) -> Workspace {
    Workspace { id: new_id(), name, camera: Camera::default(), terminals: Vec::new(), links: Vec::new() }
}

/// Picks `"{prefix} N"` with the smallest N not already used.
fn next_name<'a>(prefix: &str, taken: impl Iterator<Item = &'a str> + Clone) -> String {
    (1..)
        .map(|n| format!("{prefix} {n}"))
        .find(|candidate| !taken.clone().any(|t| t == candidate))
        .expect("an unused name always exists")
}

/// Name for a copy of the terminal called `source`: its base name (without a
/// trailing number) plus the smallest unused N from 2, so "planner",
/// "planner 2" and "planner 3" all duplicate to the next free "planner N".
/// The base is shortened so the result still fits [`MAX_NAME_CHARS`].
fn duplicate_name<'a>(source: &str, taken: impl Iterator<Item = &'a str> + Clone) -> String {
    let base = match source.rsplit_once(' ') {
        Some((base, n)) if !base.trim().is_empty() && !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) => base,
        _ => source,
    };
    (2..)
        .map(|n| {
            let suffix = format!(" {n}");
            let room = MAX_NAME_CHARS - suffix.len();
            let base: String = base.chars().take(room).collect();
            format!("{}{suffix}", base.trim_end())
        })
        .find(|candidate| !taken.clone().any(|t| t == candidate))
        .expect("an unused name always exists")
}

/// Appends a terminal on top of the workspace's stack.
fn push_terminal(ws: &mut Workspace, name: String, rect: Rect) -> TerminalWindow {
    let rect = rect.sanitized();
    let terminal = TerminalWindow {
        id: new_id(),
        name,
        x: rect.x,
        y: rect.y,
        width: rect.width,
        height: rect.height,
        z: ws.terminals.iter().map(|t| t.z).max().map_or(1, |z| z + 1),
    };
    ws.terminals.push(terminal.clone());
    terminal
}

#[derive(Debug, Clone, PartialEq)]
pub struct Model {
    active_workspace_id: String,
    workspaces: Vec<Workspace>,
}

impl Default for Model {
    fn default() -> Self {
        let first = new_workspace("Workspace 1".into());
        Self { active_workspace_id: first.id.clone(), workspaces: vec![first] }
    }
}

impl Model {
    /// Builds a model from a loaded `state.json`, repairing anything that
    /// would break invariants (dangling links, invalid geometry, no active
    /// workspace, detected links that should never have been saved).
    pub fn from_snapshot(snapshot: AppSnapshot) -> Self {
        let mut workspaces = snapshot.workspaces;
        for ws in &mut workspaces {
            ws.camera = ws.camera.sanitized();
            for t in &mut ws.terminals {
                let rect = Rect { x: t.x, y: t.y, width: t.width, height: t.height }.sanitized();
                (t.x, t.y, t.width, t.height) = (rect.x, rect.y, rect.width, rect.height);
            }
            let ids: Vec<String> = ws.terminals.iter().map(|t| t.id.clone()).collect();
            ws.links.retain(|l| {
                l.kind == LinkKind::Manual
                    && l.source_id != l.target_id
                    && ids.contains(&l.source_id)
                    && ids.contains(&l.target_id)
            });
        }
        if workspaces.is_empty() {
            return Self::default();
        }
        let active_workspace_id = if workspaces.iter().any(|w| w.id == snapshot.active_workspace_id) {
            snapshot.active_workspace_id
        } else {
            workspaces[0].id.clone()
        };
        Self { active_workspace_id, workspaces }
    }

    /// Full model, including detected links, as sent to the frontend.
    pub fn snapshot(&self) -> AppSnapshot {
        AppSnapshot {
            version: STATE_VERSION,
            active_workspace_id: self.active_workspace_id.clone(),
            workspaces: self.workspaces.clone(),
        }
    }

    /// What is written to `state.json`: detected links are left out.
    pub fn persisted(&self) -> AppSnapshot {
        let mut snapshot = self.snapshot();
        for ws in &mut snapshot.workspaces {
            ws.links.retain(|l| l.kind == LinkKind::Manual);
        }
        snapshot
    }

    pub fn workspaces(&self) -> &[Workspace] {
        &self.workspaces
    }

    pub fn active_workspace_id(&self) -> &str {
        &self.active_workspace_id
    }

    fn workspace_mut(&mut self, id: &str) -> AppResult<&mut Workspace> {
        self.workspaces.iter_mut().find(|w| w.id == id).ok_or_else(|| AppError::workspace(id))
    }

    fn terminal_mut(&mut self, id: &str) -> AppResult<&mut TerminalWindow> {
        self.workspaces
            .iter_mut()
            .flat_map(|w| w.terminals.iter_mut())
            .find(|t| t.id == id)
            .ok_or_else(|| AppError::terminal(id))
    }

    /// Workspace that contains the given terminal.
    pub fn workspace_of(&self, terminal_id: &str) -> Option<&Workspace> {
        self.workspaces.iter().find(|w| w.terminals.iter().any(|t| t.id == terminal_id))
    }

    pub fn terminal(&self, id: &str) -> Option<&TerminalWindow> {
        self.workspaces.iter().flat_map(|w| &w.terminals).find(|t| t.id == id)
    }

    /// Terminals that can be linked with `terminal_id` (its workspace,
    /// including itself), plus that workspace's id.
    pub fn peers_of(&self, terminal_id: &str) -> Option<(&str, Vec<Peer<'_>>)> {
        let ws = self.workspace_of(terminal_id)?;
        let peers = ws.terminals.iter().map(|t| Peer { id: &t.id, name: &t.name }).collect();
        Some((&ws.id, peers))
    }

    pub fn links_of(&self, workspace_id: &str) -> Vec<Link> {
        self.workspaces.iter().find(|w| w.id == workspace_id).map(|w| w.links.clone()).unwrap_or_default()
    }

    pub fn create_workspace(&mut self, name: Option<&str>) -> AppResult<Workspace> {
        let name = match name {
            Some(name) => clean_name(name)?,
            None => next_name("Workspace", self.workspaces.iter().map(|w| w.name.as_str())),
        };
        let ws = new_workspace(name);
        self.workspaces.push(ws.clone());
        Ok(ws)
    }

    pub fn rename_workspace(&mut self, id: &str, name: &str) -> AppResult<()> {
        let name = clean_name(name)?;
        self.workspace_mut(id)?.name = name;
        Ok(())
    }

    /// Removes a workspace and returns the ids of its terminals, whose
    /// processes the caller must stop. The last workspace cannot be removed.
    pub fn delete_workspace(&mut self, id: &str) -> AppResult<Vec<String>> {
        let index = self.workspaces.iter().position(|w| w.id == id).ok_or_else(|| AppError::workspace(id))?;
        if self.workspaces.len() == 1 {
            return Err(AppError::Invalid("the last workspace cannot be deleted".into()));
        }
        let removed = self.workspaces.remove(index);
        if self.active_workspace_id == id {
            self.active_workspace_id = self.workspaces[index.min(self.workspaces.len() - 1)].id.clone();
        }
        Ok(removed.terminals.into_iter().map(|t| t.id).collect())
    }

    pub fn set_active_workspace(&mut self, id: &str) -> AppResult<()> {
        self.workspace_mut(id)?;
        self.active_workspace_id = id.to_owned();
        Ok(())
    }

    pub fn update_camera(&mut self, workspace_id: &str, camera: Camera) -> AppResult<()> {
        self.workspace_mut(workspace_id)?.camera = camera.sanitized();
        Ok(())
    }

    pub fn add_terminal(&mut self, workspace_id: &str, rect: Rect) -> AppResult<TerminalWindow> {
        let ws = self.workspace_mut(workspace_id)?;
        let name = next_name("Terminal", ws.terminals.iter().map(|t| t.name.as_str()));
        Ok(push_terminal(ws, name, rect))
    }

    /// Adds a copy of `source_id` at `rect`, in the same workspace and on
    /// top, with a unique name derived from the source's. Links are not
    /// copied.
    pub fn duplicate_terminal(&mut self, source_id: &str, rect: Rect) -> AppResult<TerminalWindow> {
        let ws = self
            .workspaces
            .iter_mut()
            .find(|w| w.terminals.iter().any(|t| t.id == source_id))
            .ok_or_else(|| AppError::terminal(source_id))?;
        let source = &ws.terminals.iter().find(|t| t.id == source_id).expect("found above").name;
        let name = duplicate_name(source, ws.terminals.iter().map(|t| t.name.as_str()));
        Ok(push_terminal(ws, name, rect))
    }

    pub fn rename_terminal(&mut self, id: &str, name: &str) -> AppResult<()> {
        let name = clean_name(name)?;
        self.terminal_mut(id)?.name = name;
        Ok(())
    }

    pub fn update_terminal_rect(&mut self, id: &str, rect: Rect) -> AppResult<Rect> {
        let rect = rect.sanitized();
        let t = self.terminal_mut(id)?;
        (t.x, t.y, t.width, t.height) = (rect.x, rect.y, rect.width, rect.height);
        Ok(rect)
    }

    /// Moves a terminal above every other terminal of its workspace and
    /// returns its new z.
    pub fn bring_to_front(&mut self, id: &str) -> AppResult<u32> {
        let ws = self
            .workspaces
            .iter_mut()
            .find(|w| w.terminals.iter().any(|t| t.id == id))
            .ok_or_else(|| AppError::terminal(id))?;
        let top_other = ws.terminals.iter().filter(|t| t.id != id).map(|t| t.z).max().unwrap_or(0);
        let t = ws.terminals.iter_mut().find(|t| t.id == id).expect("found above");
        if t.z <= top_other {
            t.z = top_other + 1;
        }
        Ok(t.z)
    }

    /// Removes a terminal and every link that touches it. Returns its
    /// workspace id.
    pub fn remove_terminal(&mut self, id: &str) -> AppResult<String> {
        let ws = self
            .workspaces
            .iter_mut()
            .find(|w| w.terminals.iter().any(|t| t.id == id))
            .ok_or_else(|| AppError::terminal(id))?;
        ws.terminals.retain(|t| t.id != id);
        ws.links.retain(|l| l.source_id != id && l.target_id != id);
        Ok(ws.id.clone())
    }

    /// Creates a manual link. Both terminals must exist and share a
    /// workspace. A detected link between the same pair becomes manual; an
    /// existing manual link is returned unchanged.
    pub fn add_manual_link(&mut self, source_id: &str, target_id: &str, now: u64) -> AppResult<(String, Link)> {
        if source_id == target_id {
            return Err(AppError::Invalid("a terminal cannot be linked to itself".into()));
        }
        let source_ws = self.workspace_of(source_id).ok_or_else(|| AppError::terminal(source_id))?.id.clone();
        let target_ws = self.workspace_of(target_id).ok_or_else(|| AppError::terminal(target_id))?.id.clone();
        if source_ws != target_ws {
            return Err(AppError::Invalid("linked terminals must belong to the same workspace".into()));
        }
        let ws = self.workspace_mut(&source_ws)?;
        if let Some(link) = ws.links.iter_mut().find(|l| l.source_id == source_id && l.target_id == target_id) {
            link.kind = LinkKind::Manual;
            return Ok((source_ws, link.clone()));
        }
        let link = Link {
            id: new_id(),
            source_id: source_id.to_owned(),
            target_id: target_id.to_owned(),
            kind: LinkKind::Manual,
            last_activity_at: now,
        };
        ws.links.push(link.clone());
        Ok((source_ws, link))
    }

    /// Removes a link of any kind. Returns its workspace id.
    pub fn remove_link(&mut self, link_id: &str) -> AppResult<String> {
        let ws = self
            .workspaces
            .iter_mut()
            .find(|w| w.links.iter().any(|l| l.id == link_id))
            .ok_or_else(|| AppError::NotFound(format!("link {link_id}")))?;
        ws.links.retain(|l| l.id != link_id);
        Ok(ws.id.clone())
    }

    /// Records activity from `source` to `target`, creating a detected link
    /// when none exists. Returns true when the frontend should be told: a
    /// link was created, or the refresh falls in a new
    /// [`LINK_ACTIVITY_THROTTLE_MS`] period. Steady traffic is therefore
    /// reported about once per period, so `lastActivityAt` on the frontend
    /// never lags by more than that.
    pub fn touch_link(&mut self, workspace_id: &str, source_id: &str, target_id: &str, now: u64) -> bool {
        let Ok(ws) = self.workspace_mut(workspace_id) else { return false };
        let both_exist = [source_id, target_id].iter().all(|id| ws.terminals.iter().any(|t| t.id == *id));
        if source_id == target_id || !both_exist {
            return false;
        }
        if let Some(link) = ws.links.iter_mut().find(|l| l.source_id == source_id && l.target_id == target_id) {
            let previous = link.last_activity_at;
            link.last_activity_at = previous.max(now);
            return link.last_activity_at / LINK_ACTIVITY_THROTTLE_MS != previous / LINK_ACTIVITY_THROTTLE_MS;
        }
        ws.links.push(Link {
            id: new_id(),
            source_id: source_id.to_owned(),
            target_id: target_id.to_owned(),
            kind: LinkKind::Detected,
            last_activity_at: now,
        });
        true
    }

    /// Drops detected links idle for longer than the TTL and returns the ids
    /// of the workspaces that changed.
    pub fn expire_detected_links(&mut self, now: u64) -> Vec<String> {
        let mut changed = Vec::new();
        for ws in &mut self.workspaces {
            let before = ws.links.len();
            ws.links.retain(|l| {
                l.kind == LinkKind::Manual || now.saturating_sub(l.last_activity_at) <= DETECTED_LINK_TTL_MS
            });
            if ws.links.len() != before {
                changed.push(ws.id.clone());
            }
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect() -> Rect {
        Rect { x: 0.0, y: 0.0, width: 600.0, height: 400.0 }
    }

    fn model_with_two_terminals() -> (Model, String, String, String) {
        let mut m = Model::default();
        let ws = m.active_workspace_id().to_owned();
        let a = m.add_terminal(&ws, rect()).unwrap().id;
        let b = m.add_terminal(&ws, rect()).unwrap().id;
        (m, ws, a, b)
    }

    #[test]
    fn default_model_has_one_active_workspace() {
        let m = Model::default();
        assert_eq!(m.workspaces().len(), 1);
        assert_eq!(m.active_workspace_id(), m.workspaces()[0].id);
    }

    #[test]
    fn terminals_get_unique_names_increasing_z_and_min_size() {
        let mut m = Model::default();
        let ws = m.active_workspace_id().to_owned();
        let a = m.add_terminal(&ws, Rect { x: 0.0, y: 0.0, width: 10.0, height: f64::NAN }).unwrap();
        let b = m.add_terminal(&ws, rect()).unwrap();
        assert_eq!((a.name.as_str(), b.name.as_str()), ("Terminal 1", "Terminal 2"));
        assert!(b.z > a.z);
        assert_eq!((a.width, a.height), (320.0, 200.0));
    }

    #[test]
    fn duplicates_are_numbered_from_the_source_base_name() {
        let (mut m, ws, a, b) = model_with_two_terminals();
        m.rename_terminal(&a, "planner").unwrap();
        let first = m.duplicate_terminal(&a, rect()).unwrap();
        let second = m.duplicate_terminal(&a, rect()).unwrap();
        let of_copy = m.duplicate_terminal(&first.id, rect()).unwrap();
        let names: Vec<_> = [&first, &second, &of_copy].iter().map(|t| t.name.clone()).collect();
        assert_eq!(names, ["planner 2", "planner 3", "planner 4"]);

        // "Terminal 2" duplicates to the next free "Terminal N".
        assert_eq!(m.duplicate_terminal(&b, rect()).unwrap().name, "Terminal 3");
        assert_eq!(m.workspace_of(&of_copy.id).unwrap().id, ws);
    }

    #[test]
    fn duplicate_lands_in_the_source_workspace_on_top_without_links() {
        let (mut m, ws, a, b) = model_with_two_terminals();
        m.add_manual_link(&a, &b, 0).unwrap();
        let other = m.create_workspace(None).unwrap().id;
        m.set_active_workspace(&other).unwrap();
        let copy = m.duplicate_terminal(&a, Rect { x: 700.0, y: 0.0, width: 600.0, height: 400.0 }).unwrap();
        let terminals = &m.workspace_of(&a).unwrap().terminals;
        assert_eq!(m.workspace_of(&copy.id).unwrap().id, ws);
        assert!(terminals.iter().all(|t| t.id == copy.id || t.z < copy.z));
        assert_eq!((copy.x, copy.width), (700.0, 600.0));
        assert!(m.links_of(&ws).iter().all(|l| l.source_id != copy.id && l.target_id != copy.id));
        assert_eq!(m.duplicate_terminal("nope", rect()).unwrap_err().to_string(), "terminal nope not found");
    }

    #[test]
    fn duplicate_names_stay_within_the_length_limit() {
        let long = "x".repeat(MAX_NAME_CHARS);
        let name = duplicate_name(&long, std::iter::empty());
        assert_eq!(name.chars().count(), MAX_NAME_CHARS);
        assert!(name.ends_with("x 2"));
        // A lone number or a trailing space is a name, not a suffix.
        assert_eq!(duplicate_name("2026", std::iter::empty()), "2026 2");
        assert_eq!(duplicate_name("v 1", ["v 2"].into_iter()), "v 3");
    }

    #[test]
    fn bring_to_front_raises_only_when_needed() {
        let (mut m, _, a, b) = model_with_two_terminals();
        let z = m.bring_to_front(&a).unwrap();
        assert!(z > m.terminal(&b).unwrap().z);
        assert_eq!(m.bring_to_front(&a).unwrap(), z, "already in front");
    }

    #[test]
    fn manual_link_requires_same_workspace_and_distinct_terminals() {
        let (mut m, _, a, b) = model_with_two_terminals();
        let other = m.create_workspace(None).unwrap();
        let c = m.add_terminal(&other.id, rect()).unwrap().id;

        assert!(matches!(m.add_manual_link(&a, &a, 0), Err(AppError::Invalid(_))));
        assert!(matches!(m.add_manual_link(&a, &c, 0), Err(AppError::Invalid(_))));
        assert!(matches!(m.add_manual_link(&a, "missing", 0), Err(AppError::NotFound(_))));

        let (_, link) = m.add_manual_link(&a, &b, 0).unwrap();
        let (_, again) = m.add_manual_link(&a, &b, 5).unwrap();
        assert_eq!(link.id, again.id, "creating the same link twice is idempotent");
    }

    #[test]
    fn manual_link_promotes_detected_link() {
        let (mut m, ws, a, b) = model_with_two_terminals();
        assert!(m.touch_link(&ws, &a, &b, 0));
        let (_, link) = m.add_manual_link(&a, &b, 1).unwrap();
        assert_eq!(link.kind, LinkKind::Manual);
        assert_eq!(m.links_of(&ws).len(), 1);
    }

    #[test]
    fn touch_link_creates_then_throttles_notifications() {
        let (mut m, ws, a, b) = model_with_two_terminals();
        assert!(m.touch_link(&ws, &a, &b, 1_000), "created");
        assert!(!m.touch_link(&ws, &a, &b, 1_500), "refreshed within throttle");
        assert_eq!(m.links_of(&ws)[0].last_activity_at, 1_500);
        assert!(m.touch_link(&ws, &a, &b, 2_600), "a new throttle period");
        assert!(!m.touch_link(&ws, &a, &b, 2_000), "an older timestamp changes nothing");
        assert_eq!(m.links_of(&ws)[0].last_activity_at, 2_600);
        assert!(!m.touch_link(&ws, &a, &a, 3_000), "no self links");
        assert!(!m.touch_link(&ws, &a, "missing", 3_000));
        assert_eq!(m.links_of(&ws).len(), 1);
    }

    #[test]
    fn steady_traffic_keeps_notifying_once_per_throttle_period() {
        let (mut m, ws, a, b) = model_with_two_terminals();
        let notified: Vec<u64> =
            (0..40).map(|i| 10_000 + i * 250).filter(|&now| m.touch_link(&ws, &a, &b, now)).collect();
        let gaps: Vec<u64> = notified.windows(2).map(|w| w[1] - w[0]).collect();
        assert_eq!(notified.len(), 10, "one creation plus one per elapsed period: {notified:?}");
        assert!(gaps.iter().all(|&gap| gap == LINK_ACTIVITY_THROTTLE_MS), "{gaps:?}");
    }

    #[test]
    fn detected_links_expire_but_manual_links_stay() {
        let (mut m, ws, a, b) = model_with_two_terminals();
        m.touch_link(&ws, &a, &b, 0);
        m.add_manual_link(&b, &a, 0).unwrap();
        assert!(m.expire_detected_links(DETECTED_LINK_TTL_MS).is_empty());
        assert_eq!(m.expire_detected_links(DETECTED_LINK_TTL_MS + 1), vec![ws.clone()]);
        let links = m.links_of(&ws);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].kind, LinkKind::Manual);
    }

    #[test]
    fn removing_a_terminal_removes_its_links() {
        let (mut m, ws, a, b) = model_with_two_terminals();
        m.add_manual_link(&a, &b, 0).unwrap();
        m.touch_link(&ws, &b, &a, 0);
        assert_eq!(m.remove_terminal(&a).unwrap(), ws);
        assert!(m.links_of(&ws).is_empty());
    }

    #[test]
    fn persisted_snapshot_keeps_only_manual_links() {
        let (mut m, ws, a, b) = model_with_two_terminals();
        m.add_manual_link(&a, &b, 0).unwrap();
        m.touch_link(&ws, &b, &a, 0);
        assert_eq!(m.snapshot().workspaces[0].links.len(), 2);
        let saved = m.persisted();
        assert_eq!(saved.workspaces[0].links.len(), 1);
        assert_eq!(saved.workspaces[0].links[0].kind, LinkKind::Manual);
    }

    #[test]
    fn workspace_lifecycle() {
        let mut m = Model::default();
        let first = m.active_workspace_id().to_owned();
        let second = m.create_workspace(None).unwrap();
        assert_eq!(second.name, "Workspace 2");
        m.rename_workspace(&second.id, "  Agents  ").unwrap();
        assert!(m.rename_workspace(&second.id, "   ").is_err());
        m.set_active_workspace(&second.id).unwrap();
        let t = m.add_terminal(&second.id, rect()).unwrap();

        assert_eq!(m.delete_workspace(&second.id).unwrap(), vec![t.id]);
        assert_eq!(m.active_workspace_id(), first, "active falls back to a remaining workspace");
        assert!(matches!(m.delete_workspace(&first), Err(AppError::Invalid(_))));
    }

    #[test]
    fn camera_is_clamped() {
        let mut m = Model::default();
        let ws = m.active_workspace_id().to_owned();
        m.update_camera(&ws, Camera { x: 10.0, y: f64::INFINITY, zoom: 50.0 }).unwrap();
        assert_eq!(m.workspaces()[0].camera, Camera { x: 10.0, y: 0.0, zoom: 4.0 });
    }

    #[test]
    fn from_snapshot_repairs_invalid_data() {
        let (mut m, ws, a, b) = model_with_two_terminals();
        m.add_manual_link(&a, &b, 0).unwrap();
        m.touch_link(&ws, &b, &a, 0);
        let mut snapshot = m.snapshot();
        snapshot.active_workspace_id = "gone".into();
        snapshot.workspaces[0].links.push(Link {
            id: "dangling".into(),
            source_id: a.clone(),
            target_id: "gone".into(),
            kind: LinkKind::Manual,
            last_activity_at: 0,
        });

        let repaired = Model::from_snapshot(snapshot);
        assert_eq!(repaired.active_workspace_id(), ws);
        let links = repaired.links_of(&ws);
        assert_eq!(links.len(), 1, "detected and dangling links are dropped");
        assert_eq!(links[0].kind, LinkKind::Manual);

        let empty = AppSnapshot { version: STATE_VERSION, active_workspace_id: String::new(), workspaces: vec![] };
        assert_eq!(Model::from_snapshot(empty).workspaces().len(), 1);
    }
}
