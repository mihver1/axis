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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaneKind {
    Shell,
    Agent,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PaneRecord {
    pub id: PaneId,
    pub title: String,
    pub kind: PaneKind,
    pub position: Point,
    pub size: Size,
}

#[derive(Debug, Default)]
pub struct Workdesk {
    next_pane_id: u64,
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
        let id = PaneId(self.next_pane_id);

        self.panes.push(PaneRecord {
            id,
            title: title.into(),
            kind,
            position,
            size,
        });

        id
    }

    pub fn panes(&self) -> &[PaneRecord] {
        &self.panes
    }
}
