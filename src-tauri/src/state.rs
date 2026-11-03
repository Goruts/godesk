//! In-memory application model and every rule that keeps it consistent.
//! Nothing here touches Tauri or processes, so it is tested directly.

use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::links::LINK_ACTIVITY_THROTTLE_MS;
use crate::model::{AppSnapshot, Camera, KnowledgeBase, Link, LinkKind, Rect, STATE_VERSION, TerminalWindow, Workspace};

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
    Workspace {
        id: new_id(),
        name,
        camera: Camera::default(),
        terminals: Vec::new(),
        links: Vec::new(),
        knowledge_bases: Vec::new(),
    }
}

/// Picks `"{prefix} N"` with the smallest N not already used.
fn next_name<'a>(prefix: &str, taken: impl Iterator<Item = &'a str> + Clone) -> String {
    (1..)
        .map(|n| format!("{prefix} {n}"))
        .find(|candidate| !taken.clone().any(|t| t == candidate))
        .expect("an unused name always exists")
}


/// Name for a copy of the terminal called `source`: its base name (without a/// trailing number) plus the smallest unused N from 2, so "planner",
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
        agent: None,
    };
    ws.terminals.push(terminal.clone());
    terminal
}

/// Appends a knowledge-base node on top of the workspace's stack. Its z is
/// taken over both terminals and knowledge bases, so nodes never share a z.
fn push_knowledge_base(ws: &mut Workspace, name: String, path: String, rect: Rect) -> KnowledgeBase {
    let rect = rect.sanitized();
    let z = ws
        .terminals
        .iter()
        .map(|t| t.z)
        .chain(ws.knowledge_bases.iter().map(|k| k.z))
        .max()
        .map_or(1, |z| z + 1);
    let kb = KnowledgeBase {
        id: new_id(),
        name,
        path,
        x: rect.x,
        y: rect.y,
        width: rect.width,
        height: rect.height,
        z,
    };
    ws.knowledge_bases.push(kb.clone());
    kb
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
            for k in &mut ws.knowledge_bases {
                let rect = Rect { x: k.x, y: k.y, width: k.width, height: k.height }.sanitized();
                (k.x, k.y, k.width, k.height) = (rect.x, rect.y, rect.width, rect.height);
            }
            // A link may join two terminals or a knowledge base and a terminal,
            // so both node kinds count as valid endpoints; otherwise every
            // knowledge-base arrow would be dropped on reload.
            let ids: Vec<String> = ws
                .terminals
                .iter()
                .map(|t| t.id.clone())
                .chain(ws.knowledge_bases.iter().map(|k| k.id.clone()))
                .collect();
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

    /// Workspace that contains the given node, whether it is a terminal or a
    /// knowledge base. Links may join either kind, so endpoint lookups use this.
    pub fn workspace_of_node(&self, node_id: &str) -> Option<&Workspace> {
        self.workspaces
            .iter()
            .find(|w| w.terminals.iter().any(|t| t.id == node_id) || w.knowledge_bases.iter().any(|k| k.id == node_id))
    }

    pub fn terminal(&self, id: &str) -> Option<&TerminalWindow> {
        self.workspaces.iter().flat_map(|w| &w.terminals).find(|t| t.id == id)
    }

    /// The knowledge-base node with this id, in any workspace.
    pub fn knowledge_base(&self, id: &str) -> Option<&KnowledgeBase> {
        self.workspaces.iter().flat_map(|w| &w.knowledge_bases).find(|k| k.id == id)
    }

    fn is_knowledge_base(&self, id: &str) -> bool {
        self.knowledge_base(id).is_some()
    }

    /// Every knowledge base of a workspace.
    pub fn knowledge_bases_of(&self, workspace_id: &str) -> Vec<KnowledgeBase> {
        self.workspaces.iter().find(|w| w.id == workspace_id).map(|w| w.knowledge_bases.clone()).unwrap_or_default()
    }

    /// The ids of the knowledge bases an arrow connects to this terminal.
    pub fn knowledge_bases_linked_to(&self, terminal_id: &str) -> Vec<KnowledgeBase> {
        let Some(ws) = self.workspace_of_node(terminal_id) else { return Vec::new() };
        ws.knowledge_bases
            .iter()
            .filter(|k| self.are_linked(terminal_id, &k.id))
            .cloned()
            .collect()
    }

    fn knowledge_base_mut(&mut self, id: &str) -> AppResult<&mut KnowledgeBase> {
        self.workspaces
            .iter_mut()
            .flat_map(|w| w.knowledge_bases.iter_mut())
            .find(|k| k.id == id)
            .ok_or_else(|| AppError::NotFound(format!("knowledge base {id}")))
    }

    /// Whether a manual link joins the two terminals, in either direction:
    /// an arrow is a two-way permission to talk, so the order does not matter.
    pub fn are_linked(&self, a: &str, b: &str) -> bool {
        let Some(ws) = self.workspace_of_node(a) else { return false };
        ws.links.iter().any(|l| {
            l.kind == LinkKind::Manual
                && ((l.source_id == a && l.target_id == b) || (l.source_id == b && l.target_id == a))
        })
    }

    /// The two endpoints of a link, by its id, in any workspace.
    pub fn link_endpoints(&self, link_id: &str) -> Option<(String, String)> {
        self.workspaces
            .iter()
            .flat_map(|w| &w.links)
            .find(|l| l.id == link_id)
            .map(|l| (l.source_id.clone(), l.target_id.clone()))
    }

    /// The ids of the terminals an arrow connects to this knowledge base.
    pub fn terminals_linked_to(&self, kb_id: &str) -> Vec<String> {
        let Some(ws) = self.workspace_of_node(kb_id) else { return Vec::new() };
        ws.terminals.iter().filter(|t| self.are_linked(&t.id, kb_id)).map(|t| t.id.clone()).collect()
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

    /// Shows `agent` (e.g. "claude") in the terminal instead of its shell,
    /// or the shell again with `None`. Returns the terminal as it is now.
    pub fn set_terminal_agent(&mut self, id: &str, agent: Option<&str>) -> AppResult<TerminalWindow> {
        let terminal = self.terminal_mut(id)?;
        terminal.agent = agent.map(str::to_owned);
        Ok(terminal.clone())
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

    /// Adds a knowledge-base node for `path` to a workspace. The name is
    /// cleaned and made unique among the workspace's knowledge bases.
    pub fn add_knowledge_base(
        &mut self,
        workspace_id: &str,
        name: &str,
        path: &str,
        rect: Rect,
    ) -> AppResult<KnowledgeBase> {
        let ws = self.workspace_mut(workspace_id)?;
        let base = clean_name(name).unwrap_or_else(|_| "Knowledge Base".to_owned());
        let name = if ws.knowledge_bases.iter().any(|k| k.name == base) {
            next_name(&base, ws.knowledge_bases.iter().map(|k| k.name.as_str()))
        } else {
            base
        };
        Ok(push_knowledge_base(ws, name, path.to_owned(), rect))
    }

    pub fn rename_knowledge_base(&mut self, id: &str, name: &str) -> AppResult<()> {
        let name = clean_name(name)?;
        self.knowledge_base_mut(id)?.name = name;
        Ok(())
    }

    pub fn update_knowledge_base_rect(&mut self, id: &str, rect: Rect) -> AppResult<Rect> {
        let rect = rect.sanitized();
        let k = self.knowledge_base_mut(id)?;
        (k.x, k.y, k.width, k.height) = (rect.x, rect.y, rect.width, rect.height);
        Ok(rect)
    }

    /// Removes a knowledge base and every link that touches it. Returns its
    /// workspace id.
    pub fn remove_knowledge_base(&mut self, id: &str) -> AppResult<String> {
        let ws = self
            .workspaces
            .iter_mut()
            .find(|w| w.knowledge_bases.iter().any(|k| k.id == id))
            .ok_or_else(|| AppError::NotFound(format!("knowledge base {id}")))?;
        ws.knowledge_bases.retain(|k| k.id != id);
        ws.links.retain(|l| l.source_id != id && l.target_id != id);
        Ok(ws.id.clone())
    }

    /// Creates a manual link. Both terminals must exist and share a
    /// workspace. A detected link between the same pair becomes manual; an
    /// existing manual link is returned unchanged.
    pub fn add_manual_link(&mut self, source_id: &str, target_id: &str, now: u64) -> AppResult<(String, Link)> {
        if source_id == target_id {
            return Err(AppError::Invalid("a node cannot be linked to itself".into()));
        }
        // Two knowledge bases have nothing to say to each other; a useful arrow
        // always has a terminal on at least one end.
        if self.is_knowledge_base(source_id) && self.is_knowledge_base(target_id) {
            return Err(AppError::Invalid("two knowledge bases cannot be linked".into()));
        }
        let source_ws = self.workspace_of_node(source_id).ok_or_else(|| AppError::NotFound(format!("node {source_id}")))?.id.clone();
        let target_ws = self.workspace_of_node(target_id).ok_or_else(|| AppError::NotFound(format!("node {target_id}")))?.id.clone();
        if source_ws != target_ws {
            return Err(AppError::Invalid("linked nodes must belong to the same workspace".into()));
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

    /// Lights up the manual link between `source` and `target` (in either
    /// direction) when they interact, so the arrow pulses with live traffic.
    /// Does not create anything: the bridge draws the arrow first, with
    /// [`Model::add_manual_link`], the first time one terminal names another. Returns
    /// true when the frontend should be told: the refresh falls in a new
    /// [`LINK_ACTIVITY_THROTTLE_MS`] period, so steady traffic is reported
    /// about once per period and `lastActivityAt` never lags by more than that.
    pub fn touch_link(&mut self, workspace_id: &str, source_id: &str, target_id: &str, now: u64) -> bool {
        let Ok(ws) = self.workspace_mut(workspace_id) else { return false };
        let Some(link) = ws.links.iter_mut().find(|l| {
            l.kind == LinkKind::Manual
                && ((l.source_id == source_id && l.target_id == target_id)
                    || (l.source_id == target_id && l.target_id == source_id))
        }) else {
            return false;
        };
        let previous = link.last_activity_at;
        link.last_activity_at = previous.max(now);
        link.last_activity_at / LINK_ACTIVITY_THROTTLE_MS != previous / LINK_ACTIVITY_THROTTLE_MS
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
    fn are_linked_is_bidirectional_and_needs_a_manual_arrow() {
        let (mut m, _, a, b) = model_with_two_terminals();
        assert!(!m.are_linked(&a, &b), "no arrow, no reach");
        m.add_manual_link(&a, &b, 0).unwrap();
        assert!(m.are_linked(&a, &b));
        assert!(m.are_linked(&b, &a), "an arrow is a two-way permission");
        let c = m.add_terminal(&m.active_workspace_id().to_owned(), rect()).unwrap().id;
        assert!(!m.are_linked(&a, &c), "unrelated terminal stays unreachable");
    }

    #[test]
    fn touch_link_only_refreshes_an_existing_arrow() {
        let (mut m, ws, a, b) = model_with_two_terminals();
        // Without an arrow there is nothing to light up, and nothing is created.
        assert!(!m.touch_link(&ws, &a, &b, 1_000));
        assert!(m.links_of(&ws).is_empty(), "traffic never creates a link");

        m.add_manual_link(&a, &b, 0).unwrap();
        assert!(m.touch_link(&ws, &a, &b, 1_000), "a new throttle period");
        assert!(!m.touch_link(&ws, &a, &b, 1_500), "refreshed within throttle");
        assert_eq!(m.links_of(&ws)[0].last_activity_at, 1_500);
        assert!(m.touch_link(&ws, &b, &a, 2_600), "the reverse direction lights the same arrow");
        assert!(!m.touch_link(&ws, &a, &b, 2_000), "an older timestamp changes nothing");
        assert_eq!(m.links_of(&ws)[0].last_activity_at, 2_600);
        assert_eq!(m.links_of(&ws).len(), 1);
    }

    #[test]
    fn steady_traffic_keeps_notifying_once_per_throttle_period() {
        let (mut m, ws, a, b) = model_with_two_terminals();
        m.add_manual_link(&a, &b, 0).unwrap();
        let notified: Vec<u64> =
            (0..40).map(|i| 10_000 + i * 250).filter(|&now| m.touch_link(&ws, &a, &b, now)).collect();
        let gaps: Vec<u64> = notified.windows(2).map(|w| w[1] - w[0]).collect();
        assert_eq!(notified.len(), 10, "one per elapsed period: {notified:?}");
        assert!(gaps.iter().all(|&gap| gap == LINK_ACTIVITY_THROTTLE_MS), "{gaps:?}");
    }

    #[test]
    fn removing_a_terminal_removes_its_links() {
        let (mut m, ws, a, b) = model_with_two_terminals();
        m.add_manual_link(&a, &b, 0).unwrap();
        assert_eq!(m.remove_terminal(&a).unwrap(), ws);
        assert!(m.links_of(&ws).is_empty());
    }

    #[test]
    fn a_knowledge_base_links_to_a_terminal_but_not_to_another_base() {
        let (mut m, ws, a, _b) = model_with_two_terminals();
        let kb = m.add_knowledge_base(&ws, "docs", "C:/docs", rect()).unwrap();
        assert_eq!(kb.name, "docs");
        // A terminal and a knowledge base can be linked, in either direction.
        assert!(m.add_manual_link(&kb.id, &a, 0).is_ok());
        assert!(m.are_linked(&a, &kb.id), "an arrow is two-way");
        assert!(m.knowledge_bases_linked_to(&a).iter().any(|k| k.id == kb.id));
        // Two knowledge bases have nothing to say to each other.
        let kb2 = m.add_knowledge_base(&ws, "notes", "C:/notes", rect()).unwrap();
        assert!(matches!(m.add_manual_link(&kb.id, &kb2.id, 0), Err(AppError::Invalid(_))));
        // A knowledge base cannot link to itself either.
        assert!(matches!(m.add_manual_link(&kb.id, &kb.id, 0), Err(AppError::Invalid(_))));
    }

    #[test]
    fn removing_a_knowledge_base_removes_its_links() {
        let (mut m, ws, a, _b) = model_with_two_terminals();
        let kb = m.add_knowledge_base(&ws, "docs", "C:/docs", rect()).unwrap();
        m.add_manual_link(&kb.id, &a, 0).unwrap();
        assert_eq!(m.remove_knowledge_base(&kb.id).unwrap(), ws);
        assert!(m.links_of(&ws).is_empty());
        assert!(m.knowledge_base(&kb.id).is_none());
    }

    #[test]
    fn a_knowledge_base_link_survives_a_reload() {
        let (mut m, ws, a, _b) = model_with_two_terminals();
        let kb = m.add_knowledge_base(&ws, "docs", "C:/docs", rect()).unwrap();
        m.add_manual_link(&kb.id, &a, 0).unwrap();
        let repaired = Model::from_snapshot(m.persisted());
        assert_eq!(repaired.knowledge_bases_of(&ws).len(), 1);
        assert!(repaired.are_linked(&a, &kb.id), "a KB-to-terminal arrow is not a dangling link");
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
        let mut snapshot = m.snapshot();
        snapshot.active_workspace_id = "gone".into();
        // A detected link from an older Godesk must not survive a reload.
        snapshot.workspaces[0].links.push(Link {
            id: "stale-detected".into(),
            source_id: b.clone(),
            target_id: a.clone(),
            kind: LinkKind::Detected,
            last_activity_at: 0,
        });
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
