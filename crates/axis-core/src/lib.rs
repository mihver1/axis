pub mod agent;
pub mod agent_history;
pub mod automation;
pub mod paths;
pub mod review;
pub mod terminal;
pub mod workdesk;
pub mod worktree;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Size {
    pub width: f32,
    pub height: f32,
}

impl Size {
    pub const fn new(width: f32, height: f32) -> Self {
        Self { width, height }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct PaneId(u64);

impl PaneId {
    pub const fn new(raw: u64) -> Self {
        Self(raw)
    }

    pub const fn raw(self) -> u64 {
        self.0
    }
}

/// Identifies a surface within a pane stack in the spatial UI; agent sessions may attach to one.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct SurfaceId(u64);

impl SurfaceId {
    pub const fn new(raw: u64) -> Self {
        Self(raw)
    }

    pub const fn raw(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SurfaceKind {
    Shell,
    Agent,
    Browser,
    Editor,
}

pub type PaneKind = SurfaceKind;

impl SurfaceKind {
    pub const fn is_terminal(&self) -> bool {
        matches!(self, Self::Shell | Self::Agent)
    }

    pub const fn default_title_label(&self) -> &'static str {
        match self {
            Self::Shell => "Shell",
            Self::Agent => "Agent",
            Self::Browser => "Browser",
            Self::Editor => "Editor",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SurfaceRecord {
    pub id: SurfaceId,
    pub title: String,
    pub kind: SurfaceKind,
    pub browser_url: Option<String>,
    pub editor_file_path: Option<String>,
    pub dirty: bool,
}

impl SurfaceRecord {
    pub fn new(id: SurfaceId, title: impl Into<String>, kind: SurfaceKind) -> Self {
        Self {
            id,
            title: title.into(),
            kind,
            browser_url: None,
            editor_file_path: None,
            dirty: false,
        }
    }

    pub fn browser(id: SurfaceId, title: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            id,
            title: title.into(),
            kind: SurfaceKind::Browser,
            browser_url: Some(url.into()),
            editor_file_path: None,
            dirty: false,
        }
    }

    pub fn editor(
        id: SurfaceId,
        title: impl Into<String>,
        file_path: impl Into<String>,
        dirty: bool,
    ) -> Self {
        Self {
            id,
            title: title.into(),
            kind: SurfaceKind::Editor,
            browser_url: None,
            editor_file_path: Some(file_path.into()),
            dirty,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PaneRecord {
    pub id: PaneId,
    pub title: String,
    pub kind: PaneKind,
    pub position: Point,
    pub size: Size,
    pub active_surface_id: SurfaceId,
    pub surfaces: Vec<SurfaceRecord>,
    pub stack_title: Option<String>,
}

impl PaneRecord {
    pub fn new(
        id: PaneId,
        position: Point,
        size: Size,
        surface: SurfaceRecord,
        stack_title: Option<String>,
    ) -> Self {
        let active_surface_id = surface.id;
        let title = surface.title.clone();
        let kind = surface.kind.clone();
        Self {
            id,
            title,
            kind,
            position,
            size,
            active_surface_id,
            surfaces: vec![surface],
            stack_title,
        }
    }

    pub fn active_surface(&self) -> Option<&SurfaceRecord> {
        self.surfaces
            .iter()
            .find(|surface| surface.id == self.active_surface_id)
            .or_else(|| self.surfaces.first())
    }

    pub fn active_surface_mut(&mut self) -> Option<&mut SurfaceRecord> {
        let active_surface_id = self.active_surface_id;
        let index = self
            .surfaces
            .iter()
            .position(|surface| surface.id == active_surface_id)
            .unwrap_or(0);
        self.surfaces.get_mut(index)
    }

    pub fn surface(&self, surface_id: SurfaceId) -> Option<&SurfaceRecord> {
        self.surfaces
            .iter()
            .find(|surface| surface.id == surface_id)
    }

    pub fn surface_mut(&mut self, surface_id: SurfaceId) -> Option<&mut SurfaceRecord> {
        self.surfaces
            .iter_mut()
            .find(|surface| surface.id == surface_id)
    }

    pub fn focus_surface(&mut self, surface_id: SurfaceId) -> bool {
        if self.surface(surface_id).is_none() {
            return false;
        }
        if self.active_surface_id == surface_id {
            return false;
        }
        self.active_surface_id = surface_id;
        self.sync_from_active_surface();
        true
    }

    pub fn push_surface(&mut self, surface: SurfaceRecord, make_active: bool) {
        if self.surfaces.len() == 1 && self.stack_title.is_none() {
            self.stack_title = Some(self.title.clone());
        }
        let surface_id = surface.id;
        self.surfaces.push(surface);
        if make_active {
            self.active_surface_id = surface_id;
        }
        self.sync_from_active_surface();
    }

    pub fn remove_surface(&mut self, surface_id: SurfaceId) -> Option<SurfaceRecord> {
        let index = self
            .surfaces
            .iter()
            .position(|surface| surface.id == surface_id)?;
        let removed = self.surfaces.remove(index);
        if self.surfaces.is_empty() {
            return Some(removed);
        }
        if self.active_surface_id == surface_id {
            let fallback_index = index.saturating_sub(1).min(self.surfaces.len() - 1);
            self.active_surface_id = self.surfaces[fallback_index].id;
        }
        self.sync_from_active_surface();
        if self.surfaces.len() == 1 {
            self.stack_title = None;
        }
        Some(removed)
    }

    pub fn next_surface_id(&self, backwards: bool) -> Option<SurfaceId> {
        if self.surfaces.len() <= 1 {
            return None;
        }
        let current_index = self
            .surfaces
            .iter()
            .position(|surface| surface.id == self.active_surface_id)
            .unwrap_or(0);
        let next_index = if backwards {
            if current_index == 0 {
                self.surfaces.len() - 1
            } else {
                current_index - 1
            }
        } else {
            (current_index + 1) % self.surfaces.len()
        };
        Some(self.surfaces[next_index].id)
    }

    pub fn stack_display_title(&self) -> &str {
        self.stack_title.as_deref().unwrap_or_else(|| {
            self.active_surface()
                .map(|surface| surface.title.as_str())
                .unwrap_or(self.title.as_str())
        })
    }

    pub fn sync_from_active_surface(&mut self) {
        if self.surfaces.is_empty() {
            return;
        }
        let index = self
            .surfaces
            .iter()
            .position(|surface| surface.id == self.active_surface_id)
            .unwrap_or(0);
        let surface = &self.surfaces[index];
        self.active_surface_id = surface.id;
        self.title = surface.title.clone();
        self.kind = surface.kind.clone();
    }
}

#[derive(Debug, Default)]
pub struct Workdesk {
    next_pane_id: u64,
    next_surface_id: u64,
    panes: Vec<PaneRecord>,
}

impl Workdesk {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_pane(
        &mut self,
        title: impl Into<String>,
        kind: PaneKind,
        position: Point,
        size: Size,
    ) -> PaneId {
        self.next_pane_id += 1;
        self.next_surface_id += 1;
        let id = PaneId(self.next_pane_id);
        let surface = SurfaceRecord::new(SurfaceId(self.next_surface_id), title.into(), kind);

        self.panes
            .push(PaneRecord::new(id, position, size, surface, None));

        id
    }

    pub fn panes(&self) -> &[PaneRecord] {
        &self.panes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pane_stack_title_snapshots_when_second_surface_is_added() {
        let mut pane = PaneRecord::new(
            PaneId::new(1),
            Point::new(0.0, 0.0),
            Size::new(640.0, 480.0),
            SurfaceRecord::new(SurfaceId::new(1), "Build Shell", SurfaceKind::Shell),
            None,
        );

        pane.push_surface(
            SurfaceRecord::new(SurfaceId::new(2), "Implement Agent", SurfaceKind::Agent),
            true,
        );

        assert_eq!(pane.stack_title.as_deref(), Some("Build Shell"));
        assert_eq!(pane.stack_display_title(), "Build Shell");
        assert_eq!(pane.title, "Implement Agent");
        assert_eq!(pane.active_surface_id, SurfaceId::new(2));
        assert_eq!(pane.surfaces.len(), 2);
    }

    #[test]
    fn pane_stack_title_clears_when_stack_returns_to_single_surface() {
        let mut pane = PaneRecord::new(
            PaneId::new(1),
            Point::new(0.0, 0.0),
            Size::new(640.0, 480.0),
            SurfaceRecord::new(SurfaceId::new(1), "Build Shell", SurfaceKind::Shell),
            None,
        );
        pane.push_surface(
            SurfaceRecord::new(SurfaceId::new(2), "Implement Agent", SurfaceKind::Agent),
            true,
        );

        let removed = pane.remove_surface(SurfaceId::new(2));

        assert!(removed.is_some());
        assert_eq!(pane.surfaces.len(), 1);
        assert_eq!(pane.stack_title, None);
        assert_eq!(pane.stack_display_title(), "Build Shell");
        assert_eq!(pane.title, "Build Shell");
        assert_eq!(pane.active_surface_id, SurfaceId::new(1));
    }
}
