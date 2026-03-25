use canvas_core::{PaneId, PaneKind, PaneRecord, Point as WorkdeskPoint, Size as WorkdeskSize};
use canvas_terminal::{
    ghostty_build_info, grid_size_for_pane, spawn_terminal_session, TerminalColor, TerminalRow,
    TerminalSession, TerminalSnapshot,
};
use gpui::{
    div, font, prelude::*, px, rgb, rgba, size, App, Application, Bounds, ClipboardItem, Context,
    FocusHandle, FontStyle, FontWeight, KeyDownEvent, KeybindingKeystroke, Keystroke,
    MagnifyGestureEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels,
    ScrollWheelEvent, SharedString, SmartMagnifyGestureEvent, StrikethroughStyle, StyledText,
    SwipeGestureEvent, TextRun, Timer, TouchEvent, TouchPhase, UnderlineStyle, Window,
    WindowBounds, WindowOptions,
};
use serde::{Deserialize, Serialize};
use std::{
    cmp::Ordering,
    collections::{BTreeMap, HashMap},
    fs,
    path::PathBuf,
    time::{Duration, Instant},
};

const MIN_ZOOM: f32 = 0.5;
const MAX_ZOOM: f32 = 2.5;
const SCROLL_WHEEL_LINE_HEIGHT: f32 = 20.0;
const SCROLL_ZOOM_SENSITIVITY: f32 = 0.0025;
const PINCH_MIN_ZOOM_FACTOR: f32 = 0.01;
const SWIPE_PAN_VIEWPORT_FRACTION: f32 = 0.35;
const SMART_MAGNIFY_RESET_EPSILON: f32 = 0.08;
const THREE_FINGER_PAN_SURFACE_SCALE: f32 = 1.15;
const THREE_FINGER_PAN_MIN_DELTA_PIXELS: f32 = 0.5;
const THREE_FINGER_SWIPE_SUPPRESSION: Duration = Duration::from_millis(250);
const GRID_STEP_WORLD: f32 = 160.0;
const MIN_PANE_WIDTH: f32 = 320.0;
const MIN_PANE_HEIGHT: f32 = 220.0;
const DEFAULT_SHELL_SIZE: WorkdeskSize = WorkdeskSize::new(920.0, 560.0);
const DEFAULT_AGENT_SIZE: WorkdeskSize = WorkdeskSize::new(720.0, 420.0);
const DEFAULT_WORKDESK_SUMMARY: &str = "Empty desk. Add shells or agents when you need them.";
const SIDEBAR_WIDTH: f32 = 268.0;
const WORKDESK_MENU_WIDTH: f32 = 208.0;
const CURSOR_BLINK_INTERVAL: Duration = Duration::from_millis(530);
const TERMINAL_SELECTION_BG: u32 = 0x2d5b88;
const TERMINAL_SELECTION_FG: u32 = 0xf4f8fb;
const TERMINAL_BODY_INSET: f32 = 12.0;
const SESSION_SAVE_DEBOUNCE: Duration = Duration::from_millis(240);
const GRID_ACTIVE_MARGIN_X: f32 = 84.0;
const GRID_ACTIVE_MARGIN_TOP: f32 = 54.0;
const GRID_ACTIVE_MARGIN_BOTTOM: f32 = 96.0;
const GRID_HINT_WIDTH: f32 = 170.0;
const GRID_HINT_HEIGHT: f32 = 88.0;
const GRID_DIRECTION_EPSILON: f32 = 24.0;
const EXPOSE_MARGIN_X: f32 = 58.0;
const EXPOSE_MARGIN_TOP: f32 = 64.0;
const EXPOSE_MARGIN_BOTTOM: f32 = 120.0;
const SPLIT_MARGIN_X: f32 = 28.0;
const SPLIT_MARGIN_TOP: f32 = 28.0;
const SPLIT_MARGIN_BOTTOM: f32 = 96.0;
const SPLIT_GAP: f32 = 14.0;
const SHORTCUT_PANEL_WIDTH: f32 = 540.0;
const SHORTCUT_PANEL_MARGIN: f32 = 20.0;

#[derive(Clone)]
struct WorkdeskState {
    name: String,
    summary: String,
    panes: Vec<PaneRecord>,
    terminals: HashMap<PaneId, TerminalSession>,
    terminal_revisions: HashMap<PaneId, u64>,
    terminal_views: HashMap<PaneId, TerminalViewState>,
    next_pane_serial: u64,
    layout_mode: LayoutMode,
    grid_layout: GridLayoutState,
    camera: WorkdeskPoint,
    zoom: f32,
    active_pane: Option<PaneId>,
    drag_state: DragState,
    runtime_notice: Option<SharedString>,
}

struct CanvasShell {
    workdesks: Vec<WorkdeskState>,
    active_workdesk: usize,
    workdesk_menu: Option<WorkdeskContextMenu>,
    focus_handle: FocusHandle,
    ghostty_vendor_dir: SharedString,
    ghostty_status: SharedString,
    shortcuts: ShortcutMap,
    shortcut_editor: ShortcutEditorState,
    cursor_blink_visible: bool,
    last_cursor_blink_at: Instant,
    persist_generation: u64,
    touchpad_pan_state: Option<TouchpadPanState>,
    last_touchpad_pan_end: Option<Instant>,
}

#[derive(Clone, Debug)]
struct TouchpadPanState {
    touch_ids: Vec<u64>,
    last_centroid: gpui::Point<f32>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct ShortcutEditorState {
    open: bool,
    recording: Option<ShortcutAction>,
}

#[derive(Clone, Copy, Debug)]
struct WorkdeskContextMenu {
    index: usize,
    position: gpui::Point<Pixels>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum DragState {
    Idle,
    Panning {
        last_mouse: gpui::Point<Pixels>,
    },
    MovingPane {
        pane_id: PaneId,
        last_mouse: gpui::Point<Pixels>,
    },
    ResizingPane {
        pane_id: PaneId,
        last_mouse: gpui::Point<Pixels>,
    },
    SelectingTerminal {
        pane_id: PaneId,
        metrics: TerminalFrameMetrics,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LayoutMode {
    Free,
    Grid,
    #[allow(dead_code)]
    ClassicSplit,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct GridLayoutState {
    expose_open: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum GridDirection {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ShortcutGroup {
    Workspace,
    Layout,
    View,
    Terminal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum ShortcutAction {
    ToggleShortcutPanel,
    SpawnShellPane,
    SpawnAgentPane,
    CloseActivePane,
    SpawnWorkdesk,
    SelectPreviousWorkdesk,
    SelectNextWorkdesk,
    LayoutFree,
    LayoutGrid,
    LayoutSplit,
    ToggleGridExpose,
    NavigateLeft,
    NavigateRight,
    NavigateUp,
    NavigateDown,
    FitPanes,
    ResetView,
    ZoomIn,
    ZoomOut,
    TerminalCopySelection,
    TerminalPaste,
    TerminalSelectAll,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ShortcutBinding {
    keystroke: KeybindingKeystroke,
}

#[derive(Clone, Debug)]
struct ShortcutMap {
    bindings: HashMap<ShortcutAction, Option<ShortcutBinding>>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct PersistedShortcutConfig {
    #[serde(default)]
    bindings: BTreeMap<String, Option<String>>,
}

#[derive(Clone, Copy, Debug, Default)]
struct GridNeighbors {
    left: Option<PaneId>,
    right: Option<PaneId>,
    up: Option<PaneId>,
    down: Option<PaneId>,
    left_count: usize,
    right_count: usize,
    up_count: usize,
    down_count: usize,
}

#[derive(Clone, Debug, Default)]
struct GridProjection {
    neighbors: HashMap<PaneId, GridNeighbors>,
    order: Vec<PaneId>,
}

#[derive(Clone, Copy, Debug)]
struct GridDirectionHint {
    pane_id: PaneId,
    count: usize,
}

#[derive(Clone, Copy, Debug)]
struct PaneViewportFrame {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    zoom: f32,
    allow_layout_drag: bool,
}

#[derive(Clone, Copy, Debug)]
struct ExposeLayoutFrame {
    left: f32,
    top: f32,
    scale: f32,
    min_x: f32,
    min_y: f32,
}

#[derive(Clone, Copy, Debug)]
struct SplitRect {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PersistedSession {
    active_workdesk: usize,
    workdesks: Vec<PersistedWorkdesk>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PersistedWorkdesk {
    name: String,
    summary: String,
    panes: Vec<PersistedPane>,
    layout_mode: PersistedLayoutMode,
    camera: PersistedPoint,
    zoom: f32,
    active_pane: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PersistedPane {
    id: u64,
    title: String,
    kind: PersistedPaneKind,
    position: PersistedPoint,
    size: PersistedSize,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum PersistedLayoutMode {
    Free,
    Grid,
    ClassicSplit,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum PersistedPaneKind {
    Shell,
    Agent,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
struct PersistedPoint {
    x: f32,
    y: f32,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
struct PersistedSize {
    width: f32,
    height: f32,
}

#[derive(Clone, Default)]
struct TerminalViewState {
    selection: Option<TerminalSelection>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TerminalSelection {
    anchor: TerminalCell,
    focus: TerminalCell,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TerminalCell {
    row: usize,
    col: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct TerminalFrameMetrics {
    body_origin: gpui::Point<Pixels>,
    cell_width: f32,
    cell_height: f32,
    cols: u16,
    rows: u16,
}

impl TerminalFrameMetrics {
    fn cell_at(self, position: gpui::Point<Pixels>) -> TerminalCell {
        let max_col = usize::from(self.cols.saturating_sub(1));
        let max_row = usize::from(self.rows.saturating_sub(1));
        let col = (((f32::from(position.x) - f32::from(self.body_origin.x)) / self.cell_width)
            .floor()
            .max(0.0) as usize)
            .min(max_col);
        let row = (((f32::from(position.y) - f32::from(self.body_origin.y)) / self.cell_height)
            .floor()
            .max(0.0) as usize)
            .min(max_row);
        TerminalCell { row, col }
    }
}

impl TerminalCell {
    fn compare(self, other: Self) -> Ordering {
        (self.row, self.col).cmp(&(other.row, other.col))
    }
}

impl TerminalSelection {
    fn ordered(self) -> (TerminalCell, TerminalCell) {
        if self.anchor.compare(self.focus).is_le() {
            (self.anchor, self.focus)
        } else {
            (self.focus, self.anchor)
        }
    }

    fn contains(self, cell: TerminalCell) -> bool {
        let (start, end) = self.ordered();
        cell.compare(start).is_ge() && cell.compare(end).is_le()
    }
}

impl LayoutMode {
    fn label(self) -> &'static str {
        match self {
            Self::Free => "Free",
            Self::Grid => "Grid",
            Self::ClassicSplit => "Classic split",
        }
    }
}

impl From<LayoutMode> for PersistedLayoutMode {
    fn from(value: LayoutMode) -> Self {
        match value {
            LayoutMode::Free => Self::Free,
            LayoutMode::Grid => Self::Grid,
            LayoutMode::ClassicSplit => Self::ClassicSplit,
        }
    }
}

impl From<PersistedLayoutMode> for LayoutMode {
    fn from(value: PersistedLayoutMode) -> Self {
        match value {
            PersistedLayoutMode::Free => Self::Free,
            PersistedLayoutMode::Grid => Self::Grid,
            PersistedLayoutMode::ClassicSplit => Self::ClassicSplit,
        }
    }
}

impl From<&PaneKind> for PersistedPaneKind {
    fn from(value: &PaneKind) -> Self {
        match value {
            PaneKind::Shell => Self::Shell,
            PaneKind::Agent => Self::Agent,
        }
    }
}

impl From<PersistedPaneKind> for PaneKind {
    fn from(value: PersistedPaneKind) -> Self {
        match value {
            PersistedPaneKind::Shell => Self::Shell,
            PersistedPaneKind::Agent => Self::Agent,
        }
    }
}

impl From<WorkdeskPoint> for PersistedPoint {
    fn from(value: WorkdeskPoint) -> Self {
        Self {
            x: value.x,
            y: value.y,
        }
    }
}

impl From<PersistedPoint> for WorkdeskPoint {
    fn from(value: PersistedPoint) -> Self {
        Self::new(value.x, value.y)
    }
}

impl From<WorkdeskSize> for PersistedSize {
    fn from(value: WorkdeskSize) -> Self {
        Self {
            width: value.width,
            height: value.height,
        }
    }
}

impl From<PersistedSize> for WorkdeskSize {
    fn from(value: PersistedSize) -> Self {
        Self::new(value.width, value.height)
    }
}

impl GridDirection {
    fn label(self) -> &'static str {
        match self {
            Self::Left => "Left",
            Self::Right => "Right",
            Self::Up => "Up",
            Self::Down => "Down",
        }
    }

    fn glyph(self) -> &'static str {
        match self {
            Self::Left => "←",
            Self::Right => "→",
            Self::Up => "↑",
            Self::Down => "↓",
        }
    }
}

const SHORTCUT_ACTIONS: [ShortcutAction; 22] = [
    ShortcutAction::ToggleShortcutPanel,
    ShortcutAction::SpawnShellPane,
    ShortcutAction::SpawnAgentPane,
    ShortcutAction::CloseActivePane,
    ShortcutAction::SpawnWorkdesk,
    ShortcutAction::SelectPreviousWorkdesk,
    ShortcutAction::SelectNextWorkdesk,
    ShortcutAction::LayoutFree,
    ShortcutAction::LayoutGrid,
    ShortcutAction::LayoutSplit,
    ShortcutAction::ToggleGridExpose,
    ShortcutAction::NavigateLeft,
    ShortcutAction::NavigateRight,
    ShortcutAction::NavigateUp,
    ShortcutAction::NavigateDown,
    ShortcutAction::FitPanes,
    ShortcutAction::ResetView,
    ShortcutAction::ZoomIn,
    ShortcutAction::ZoomOut,
    ShortcutAction::TerminalCopySelection,
    ShortcutAction::TerminalPaste,
    ShortcutAction::TerminalSelectAll,
];

impl ShortcutGroup {
    fn label(self) -> &'static str {
        match self {
            Self::Workspace => "Workspace",
            Self::Layout => "Layout",
            Self::View => "View",
            Self::Terminal => "Terminal",
        }
    }

    fn summary(self) -> &'static str {
        match self {
            Self::Workspace => "Create panes, move between desks, and open the shortcut drawer.",
            Self::Layout => "Switch layout modes and move focus in directional layouts.",
            Self::View => "Control the freeform camera without touching the mouse.",
            Self::Terminal => "Use familiar clipboard and selection actions in the active pane.",
        }
    }
}

impl ShortcutAction {
    fn all() -> &'static [Self] {
        &SHORTCUT_ACTIONS
    }

    fn slug(self) -> &'static str {
        match self {
            Self::ToggleShortcutPanel => "toggle-shortcut-panel",
            Self::SpawnShellPane => "spawn-shell-pane",
            Self::SpawnAgentPane => "spawn-agent-pane",
            Self::CloseActivePane => "close-active-pane",
            Self::SpawnWorkdesk => "spawn-workdesk",
            Self::SelectPreviousWorkdesk => "select-previous-workdesk",
            Self::SelectNextWorkdesk => "select-next-workdesk",
            Self::LayoutFree => "layout-free",
            Self::LayoutGrid => "layout-grid",
            Self::LayoutSplit => "layout-split",
            Self::ToggleGridExpose => "toggle-grid-expose",
            Self::NavigateLeft => "navigate-left",
            Self::NavigateRight => "navigate-right",
            Self::NavigateUp => "navigate-up",
            Self::NavigateDown => "navigate-down",
            Self::FitPanes => "fit-panes",
            Self::ResetView => "reset-view",
            Self::ZoomIn => "zoom-in",
            Self::ZoomOut => "zoom-out",
            Self::TerminalCopySelection => "terminal-copy-selection",
            Self::TerminalPaste => "terminal-paste",
            Self::TerminalSelectAll => "terminal-select-all",
        }
    }

    fn from_slug(value: &str) -> Option<Self> {
        Self::all()
            .iter()
            .copied()
            .find(|action| action.slug() == value)
    }

    fn group(self) -> ShortcutGroup {
        match self {
            Self::ToggleShortcutPanel
            | Self::SpawnShellPane
            | Self::SpawnAgentPane
            | Self::CloseActivePane
            | Self::SpawnWorkdesk
            | Self::SelectPreviousWorkdesk
            | Self::SelectNextWorkdesk => ShortcutGroup::Workspace,
            Self::LayoutFree
            | Self::LayoutGrid
            | Self::LayoutSplit
            | Self::ToggleGridExpose
            | Self::NavigateLeft
            | Self::NavigateRight
            | Self::NavigateUp
            | Self::NavigateDown => ShortcutGroup::Layout,
            Self::FitPanes | Self::ResetView | Self::ZoomIn | Self::ZoomOut => ShortcutGroup::View,
            Self::TerminalCopySelection | Self::TerminalPaste | Self::TerminalSelectAll => {
                ShortcutGroup::Terminal
            }
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::ToggleShortcutPanel => "Open shortcut drawer",
            Self::SpawnShellPane => "New shell pane",
            Self::SpawnAgentPane => "New agent pane",
            Self::CloseActivePane => "Close active pane",
            Self::SpawnWorkdesk => "New workdesk",
            Self::SelectPreviousWorkdesk => "Previous workdesk",
            Self::SelectNextWorkdesk => "Next workdesk",
            Self::LayoutFree => "Free layout",
            Self::LayoutGrid => "Grid layout",
            Self::LayoutSplit => "Split layout",
            Self::ToggleGridExpose => "Toggle grid expose",
            Self::NavigateLeft => "Focus pane left",
            Self::NavigateRight => "Focus pane right",
            Self::NavigateUp => "Focus pane up",
            Self::NavigateDown => "Focus pane down",
            Self::FitPanes => "Fit panes in view",
            Self::ResetView => "Reset zoom to 1:1",
            Self::ZoomIn => "Zoom in",
            Self::ZoomOut => "Zoom out",
            Self::TerminalCopySelection => "Copy terminal selection",
            Self::TerminalPaste => "Paste into terminal",
            Self::TerminalSelectAll => "Select all terminal text",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::ToggleShortcutPanel => {
                "Open the drawer where shortcuts can be reviewed and remapped."
            }
            Self::SpawnShellPane => "Create a new shell pane near the viewport center.",
            Self::SpawnAgentPane => "Create a new agent pane near the viewport center.",
            Self::CloseActivePane => "Close the focused pane and its live terminal session.",
            Self::SpawnWorkdesk => "Create a fresh workdesk and switch focus to it.",
            Self::SelectPreviousWorkdesk => {
                "Move focus to the workdesk on the left side of the rail."
            }
            Self::SelectNextWorkdesk => "Move focus to the workdesk on the right side of the rail.",
            Self::LayoutFree => "Return to the freeform canvas with draggable panes.",
            Self::LayoutGrid => "Focus one pane at a time with directional neighbors.",
            Self::LayoutSplit => "Tile panes into a split overview.",
            Self::ToggleGridExpose => "Open or close the overview strip for grid mode.",
            Self::NavigateLeft => "Move active focus to the nearest pane on the left.",
            Self::NavigateRight => "Move active focus to the nearest pane on the right.",
            Self::NavigateUp => "Move active focus to the nearest pane above.",
            Self::NavigateDown => "Move active focus to the nearest pane below.",
            Self::FitPanes => "Scale and center the freeform canvas around every pane.",
            Self::ResetView => "Reset the freeform camera to 100% zoom and the origin.",
            Self::ZoomIn => "Zoom the freeform canvas in around the viewport center.",
            Self::ZoomOut => "Zoom the freeform canvas out around the viewport center.",
            Self::TerminalCopySelection => "Copy the current terminal selection to the clipboard.",
            Self::TerminalPaste => "Paste clipboard text into the active terminal pane.",
            Self::TerminalSelectAll => "Select every visible terminal cell in the active pane.",
        }
    }

    fn default_binding(self) -> Option<&'static str> {
        match self {
            Self::ToggleShortcutPanel => Some("cmd-/"),
            Self::SpawnShellPane => Some("cmd-shift-n"),
            Self::SpawnAgentPane => Some("cmd-alt-n"),
            Self::CloseActivePane => Some("cmd-shift-w"),
            Self::SpawnWorkdesk => Some("cmd-shift-d"),
            Self::SelectPreviousWorkdesk => Some("cmd-alt-["),
            Self::SelectNextWorkdesk => Some("cmd-alt-]"),
            Self::LayoutFree => Some("cmd-shift-f"),
            Self::LayoutGrid => Some("cmd-shift-g"),
            Self::LayoutSplit => Some("cmd-shift-s"),
            Self::ToggleGridExpose => Some("cmd-shift-e"),
            Self::NavigateLeft => Some("cmd-shift-left"),
            Self::NavigateRight => Some("cmd-shift-right"),
            Self::NavigateUp => Some("cmd-shift-up"),
            Self::NavigateDown => Some("cmd-shift-down"),
            Self::FitPanes => Some("cmd-shift-0"),
            Self::ResetView => Some("cmd-0"),
            Self::ZoomIn => Some("cmd-="),
            Self::ZoomOut => Some("cmd--"),
            Self::TerminalCopySelection => Some("cmd-c"),
            Self::TerminalPaste => Some("cmd-v"),
            Self::TerminalSelectAll => Some("cmd-a"),
        }
    }
}

impl ShortcutBinding {
    fn parse(source: &str) -> Result<Self, String> {
        let parsed = Keystroke::parse(source).map_err(|error| error.to_string())?;
        Self::from_keystroke(parsed)
            .ok_or_else(|| format!("`{source}` is not a supported shortcut chord"))
    }

    fn from_keystroke(keystroke: Keystroke) -> Option<Self> {
        if !can_bind_shortcut_keystroke(&keystroke) {
            return None;
        }

        let mut keystroke = KeybindingKeystroke::from_keystroke(keystroke);
        keystroke.remove_key_char();
        Some(Self { keystroke })
    }

    fn matches(&self, event: &KeyDownEvent) -> bool {
        event.keystroke.should_match(&self.keystroke)
    }

    fn display_label(&self) -> String {
        self.keystroke.to_string()
    }

    fn serialized(&self) -> String {
        self.keystroke.unparse()
    }
}

impl Default for ShortcutMap {
    fn default() -> Self {
        let mut bindings = HashMap::new();
        for action in ShortcutAction::all() {
            let binding = action.default_binding().map(|source| {
                ShortcutBinding::parse(source).unwrap_or_else(|error| {
                    panic!(
                        "invalid default shortcut `{source}` for {}: {error}",
                        action.slug()
                    )
                })
            });
            bindings.insert(*action, binding);
        }

        Self { bindings }
    }
}

impl ShortcutMap {
    fn binding(&self, action: ShortcutAction) -> Option<&ShortcutBinding> {
        self.bindings
            .get(&action)
            .and_then(|binding| binding.as_ref())
    }

    fn display_label(&self, action: ShortcutAction) -> String {
        self.binding(action)
            .map(ShortcutBinding::display_label)
            .unwrap_or_else(|| "Unassigned".to_string())
    }

    fn matching_action(&self, event: &KeyDownEvent) -> Option<ShortcutAction> {
        ShortcutAction::all().iter().copied().find(|action| {
            self.binding(*action)
                .is_some_and(|binding| binding.matches(event))
        })
    }

    fn clear(&mut self, action: ShortcutAction) {
        self.bindings.insert(action, None);
    }

    fn set_binding(
        &mut self,
        action: ShortcutAction,
        binding: ShortcutBinding,
    ) -> Option<ShortcutAction> {
        let normalized = binding.serialized();
        let displaced = ShortcutAction::all().iter().copied().find(|candidate| {
            *candidate != action
                && self
                    .binding(*candidate)
                    .is_some_and(|existing| existing.serialized() == normalized)
        });

        if let Some(candidate) = displaced {
            self.clear(candidate);
        }

        self.bindings.insert(action, Some(binding));
        displaced
    }

    fn reset_binding(&mut self, action: ShortcutAction) -> Option<ShortcutAction> {
        if let Some(default) = action.default_binding() {
            let binding = ShortcutBinding::parse(default).unwrap_or_else(|error| {
                panic!(
                    "invalid default shortcut `{default}` for {}: {error}",
                    action.slug()
                )
            });
            self.set_binding(action, binding)
        } else {
            self.clear(action);
            None
        }
    }

    fn reset_all(&mut self) {
        *self = Self::default();
    }

    fn persisted_config(&self) -> PersistedShortcutConfig {
        let mut bindings = BTreeMap::new();
        for action in ShortcutAction::all() {
            bindings.insert(
                action.slug().to_string(),
                self.binding(*action).map(ShortcutBinding::serialized),
            );
        }
        PersistedShortcutConfig { bindings }
    }

    fn from_persisted(config: PersistedShortcutConfig) -> (Self, Vec<String>) {
        let mut shortcuts = Self::default();
        let mut warnings = Vec::new();

        for (action_slug, binding_source) in config.bindings {
            let Some(action) = ShortcutAction::from_slug(&action_slug) else {
                warnings.push(format!("ignored unknown shortcut action `{action_slug}`"));
                continue;
            };

            match binding_source {
                Some(source) => match ShortcutBinding::parse(&source) {
                    Ok(binding) => {
                        let displaced = shortcuts.set_binding(action, binding);
                        if let Some(displaced) = displaced {
                            warnings.push(format!(
                                "{} took over {} from {}",
                                action.label(),
                                shortcuts.display_label(action),
                                displaced.label(),
                            ));
                        }
                    }
                    Err(error) => warnings.push(format!(
                        "ignored invalid shortcut for {}: {error}",
                        action.label()
                    )),
                },
                None => shortcuts.clear(action),
            }
        }

        (shortcuts, warnings)
    }
}

impl GridNeighbors {
    fn primary(self, direction: GridDirection) -> Option<PaneId> {
        match direction {
            GridDirection::Left => self.left,
            GridDirection::Right => self.right,
            GridDirection::Up => self.up,
            GridDirection::Down => self.down,
        }
    }

    fn count(self, direction: GridDirection) -> usize {
        match direction {
            GridDirection::Left => self.left_count,
            GridDirection::Right => self.right_count,
            GridDirection::Up => self.up_count,
            GridDirection::Down => self.down_count,
        }
    }

    fn hint(self, direction: GridDirection) -> Option<GridDirectionHint> {
        self.primary(direction).map(|pane_id| GridDirectionHint {
            pane_id,
            count: self.count(direction),
        })
    }
}

impl PaneViewportFrame {
    fn from_free(pane: &PaneRecord, workdesk: &WorkdeskState) -> Self {
        Self {
            x: workdesk.camera.x + pane.position.x * workdesk.zoom,
            y: workdesk.camera.y + pane.position.y * workdesk.zoom,
            width: pane.size.width * workdesk.zoom,
            height: pane.size.height * workdesk.zoom,
            zoom: workdesk.zoom,
            allow_layout_drag: true,
        }
    }

    fn for_grid(pane: &PaneRecord, viewport_width: f32, viewport_height: f32) -> Self {
        let available_width =
            (viewport_width - SIDEBAR_WIDTH - GRID_ACTIVE_MARGIN_X * 2.0).max(MIN_PANE_WIDTH);
        let available_height =
            (viewport_height - GRID_ACTIVE_MARGIN_TOP - GRID_ACTIVE_MARGIN_BOTTOM)
                .max(MIN_PANE_HEIGHT);
        let zoom = (available_width / pane.size.width)
            .min(available_height / pane.size.height)
            .clamp(0.72, 1.35);
        let width = pane.size.width * zoom;
        let height = pane.size.height * zoom;
        let x = SIDEBAR_WIDTH + GRID_ACTIVE_MARGIN_X + (available_width - width).max(0.0) * 0.5;
        let y = GRID_ACTIVE_MARGIN_TOP + (available_height - height).max(0.0) * 0.5;

        Self {
            x,
            y,
            width,
            height,
            zoom,
            allow_layout_drag: false,
        }
    }

    fn for_split(pane: &PaneRecord, rect: SplitRect) -> Self {
        let zoom = (rect.width / pane.size.width)
            .min(rect.height / pane.size.height)
            .clamp(0.52, 1.08);

        Self {
            x: rect.x,
            y: rect.y,
            width: rect.width,
            height: rect.height,
            zoom,
            allow_layout_drag: false,
        }
    }
}

impl WorkdeskState {
    fn new(name: impl Into<String>, summary: impl Into<String>, panes: Vec<PaneRecord>) -> Self {
        let next_pane_serial = panes.iter().map(|pane| pane.id.raw()).max().unwrap_or(0) + 1;
        let active_pane = panes.last().map(|pane| pane.id);

        Self {
            name: name.into(),
            summary: summary.into(),
            panes,
            terminals: HashMap::new(),
            terminal_revisions: HashMap::new(),
            terminal_views: HashMap::new(),
            next_pane_serial,
            layout_mode: LayoutMode::Free,
            grid_layout: GridLayoutState::default(),
            camera: WorkdeskPoint::new(0.0, 0.0),
            zoom: 1.0,
            active_pane,
            drag_state: DragState::Idle,
            runtime_notice: None,
        }
    }

    fn sync_terminal_revisions(&mut self) -> bool {
        let mut changed = false;

        for (pane_id, terminal) in &self.terminals {
            let revision = terminal.revision();
            if self.terminal_revisions.get(pane_id).copied() != Some(revision) {
                self.terminal_revisions.insert(*pane_id, revision);
                changed = true;
            }
        }

        self.terminal_revisions
            .retain(|pane_id, _| self.terminals.contains_key(pane_id));
        self.terminal_views
            .retain(|pane_id, _| self.terminals.contains_key(pane_id));

        changed
    }

    fn attach_terminal_session(
        &mut self,
        pane_id: PaneId,
        kind: &PaneKind,
        title: &str,
        size: WorkdeskSize,
    ) {
        match spawn_terminal_session(kind, title, size) {
            Ok(session) => {
                self.terminal_revisions.insert(pane_id, session.revision());
                self.terminals.insert(pane_id, session);
                self.terminal_views.entry(pane_id).or_default();
            }
            Err(error) => {
                self.runtime_notice = Some(SharedString::from(format!(
                    "terminal boot failed for {title}: {error}"
                )));
            }
        }
    }

    fn pan_by_screen_delta(&mut self, delta: gpui::Point<Pixels>) {
        self.camera.x += f32::from(delta.x);
        self.camera.y += f32::from(delta.y);
    }

    fn move_pane_by_screen_delta(&mut self, pane_id: PaneId, delta: gpui::Point<Pixels>) {
        if let Some(pane) = self.panes.iter_mut().find(|pane| pane.id == pane_id) {
            pane.position.x += f32::from(delta.x) / self.zoom;
            pane.position.y += f32::from(delta.y) / self.zoom;
        }
    }

    fn resize_pane_by_screen_delta(&mut self, pane_id: PaneId, delta: gpui::Point<Pixels>) {
        if let Some(pane) = self.panes.iter_mut().find(|pane| pane.id == pane_id) {
            pane.size.width =
                (pane.size.width + f32::from(delta.x) / self.zoom).max(MIN_PANE_WIDTH);
            pane.size.height =
                (pane.size.height + f32::from(delta.y) / self.zoom).max(MIN_PANE_HEIGHT);

            if let Some(terminal) = self.terminals.get(&pane_id) {
                if let Err(error) = terminal.resize(grid_size_for_pane(pane.size)) {
                    self.runtime_notice = Some(SharedString::from(format!(
                        "terminal resize failed for {}: {}",
                        pane.title, error
                    )));
                }
            }
        }
    }

    fn zoom_about_screen_position(
        &mut self,
        position: gpui::Point<Pixels>,
        zoom_factor: f32,
    ) -> bool {
        let world_anchor = WorkdeskPoint::new(
            (f32::from(position.x) - self.camera.x) / self.zoom,
            (f32::from(position.y) - self.camera.y) / self.zoom,
        );
        let new_zoom = (self.zoom * zoom_factor).clamp(MIN_ZOOM, MAX_ZOOM);

        if (new_zoom - self.zoom).abs() < f32::EPSILON {
            return false;
        }

        self.zoom = new_zoom;
        self.camera.x = f32::from(position.x) - world_anchor.x * self.zoom;
        self.camera.y = f32::from(position.y) - world_anchor.y * self.zoom;
        true
    }

    fn focus_pane(&mut self, pane_id: PaneId) {
        self.active_pane = Some(pane_id);
        self.bring_pane_to_front(pane_id);
    }

    fn bring_pane_to_front(&mut self, pane_id: PaneId) {
        if let Some(index) = self.panes.iter().position(|pane| pane.id == pane_id) {
            if index + 1 == self.panes.len() {
                return;
            }

            let pane = self.panes.remove(index);
            self.panes.push(pane);
        }
    }

    fn drag_status(&self) -> String {
        match self.drag_state {
            DragState::Idle => "Idle".to_string(),
            DragState::Panning { .. } => "Panning workdesk".to_string(),
            DragState::MovingPane { pane_id, .. } => format!("Dragging pane #{}", pane_id.raw()),
            DragState::ResizingPane { pane_id, .. } => format!("Resizing pane #{}", pane_id.raw()),
            DragState::SelectingTerminal { pane_id, .. } => {
                format!("Selecting pane #{}", pane_id.raw())
            }
        }
    }

    fn active_pane_title(&self) -> String {
        self.active_pane
            .and_then(|pane_id| self.panes.iter().find(|pane| pane.id == pane_id))
            .map(|pane| pane.title.clone())
            .unwrap_or_else(|| "None".to_string())
    }

    fn clear_selection(&mut self, pane_id: PaneId) {
        if let Some(view) = self.terminal_views.get_mut(&pane_id) {
            view.selection = None;
        }
    }

    fn clear_all_selections(&mut self) {
        for view in self.terminal_views.values_mut() {
            view.selection = None;
        }
    }

    fn begin_selection(&mut self, pane_id: PaneId, cell: TerminalCell) {
        self.terminal_views.entry(pane_id).or_default().selection = Some(TerminalSelection {
            anchor: cell,
            focus: cell,
        });
    }

    fn update_selection(&mut self, pane_id: PaneId, cell: TerminalCell) {
        if let Some(selection) = self
            .terminal_views
            .entry(pane_id)
            .or_default()
            .selection
            .as_mut()
        {
            selection.focus = cell;
        }
    }
}

impl PersistedSession {
    fn from_shell(shell: &CanvasShell) -> Self {
        Self {
            active_workdesk: shell
                .active_workdesk
                .min(shell.workdesks.len().saturating_sub(1)),
            workdesks: shell
                .workdesks
                .iter()
                .map(PersistedWorkdesk::from_state)
                .collect(),
        }
    }

    fn into_runtime(self) -> Option<(Vec<WorkdeskState>, usize)> {
        let workdesks = self
            .workdesks
            .into_iter()
            .map(PersistedWorkdesk::into_state)
            .collect::<Vec<_>>();

        if workdesks.is_empty() {
            return None;
        }

        let active_workdesk = self.active_workdesk.min(workdesks.len().saturating_sub(1));
        Some((workdesks, active_workdesk))
    }
}

impl PersistedWorkdesk {
    fn from_state(state: &WorkdeskState) -> Self {
        Self {
            name: state.name.clone(),
            summary: state.summary.clone(),
            panes: state
                .panes
                .iter()
                .map(|pane| PersistedPane {
                    id: pane.id.raw(),
                    title: pane.title.clone(),
                    kind: PersistedPaneKind::from(&pane.kind),
                    position: PersistedPoint::from(pane.position),
                    size: PersistedSize::from(pane.size),
                })
                .collect(),
            layout_mode: state.layout_mode.into(),
            camera: state.camera.into(),
            zoom: state.zoom,
            active_pane: state.active_pane.map(PaneId::raw),
        }
    }

    fn into_state(self) -> WorkdeskState {
        let panes = self
            .panes
            .into_iter()
            .map(|pane| PaneRecord {
                id: PaneId::new(pane.id),
                title: pane.title,
                kind: pane.kind.into(),
                position: pane.position.into(),
                size: pane.size.into(),
            })
            .collect::<Vec<_>>();

        let mut state = WorkdeskState::new(self.name, self.summary, panes);
        state.layout_mode = self.layout_mode.into();
        state.camera = self.camera.into();
        state.zoom = self.zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        state.active_pane = self
            .active_pane
            .map(PaneId::new)
            .filter(|pane_id| state.panes.iter().any(|pane| pane.id == *pane_id));
        state.grid_layout = GridLayoutState::default();
        state.drag_state = DragState::Idle;
        state.runtime_notice = None;
        state
    }
}

impl CanvasShell {
    fn new(
        workdesks: Vec<WorkdeskState>,
        active_workdesk: usize,
        shortcuts: ShortcutMap,
        boot_notice: Option<SharedString>,
        focus_handle: FocusHandle,
        ghostty_vendor_dir: SharedString,
        ghostty_status: SharedString,
    ) -> Self {
        let clamped_active_workdesk = active_workdesk.min(workdesks.len().saturating_sub(1));
        let mut shell = Self {
            workdesks,
            active_workdesk: clamped_active_workdesk,
            workdesk_menu: None,
            focus_handle,
            ghostty_vendor_dir,
            ghostty_status,
            shortcuts,
            shortcut_editor: ShortcutEditorState::default(),
            cursor_blink_visible: true,
            last_cursor_blink_at: Instant::now(),
            persist_generation: 0,
            touchpad_pan_state: None,
            last_touchpad_pan_end: None,
        };

        if let Some(notice) = boot_notice {
            if let Some(workdesk) = shell.workdesks.get_mut(shell.active_workdesk) {
                workdesk.runtime_notice = Some(notice);
            }
        }

        for workdesk in &mut shell.workdesks {
            boot_workdesk_terminals(workdesk);
        }

        shell
    }

    fn active_workdesk(&self) -> &WorkdeskState {
        &self.workdesks[self.active_workdesk]
    }

    fn active_workdesk_mut(&mut self) -> &mut WorkdeskState {
        &mut self.workdesks[self.active_workdesk]
    }

    fn shortcut_label(&self, action: ShortcutAction) -> String {
        self.shortcuts.display_label(action)
    }

    fn set_runtime_notice(&mut self, message: impl Into<String>) {
        if let Some(workdesk) = self.workdesks.get_mut(self.active_workdesk) {
            workdesk.runtime_notice = Some(SharedString::from(message.into()));
        }
    }

    fn close_shortcut_panel(&mut self, cx: &mut Context<Self>) {
        if !self.shortcut_editor.open && self.shortcut_editor.recording.is_none() {
            return;
        }

        self.shortcut_editor.open = false;
        self.shortcut_editor.recording = None;
        cx.notify();
    }

    fn toggle_shortcut_panel(&mut self, cx: &mut Context<Self>) {
        if self.shortcut_editor.open {
            self.close_shortcut_panel(cx);
            return;
        }

        self.shortcut_editor.open = true;
        self.shortcut_editor.recording = None;
        cx.notify();
    }

    fn begin_shortcut_recording(&mut self, action: ShortcutAction, cx: &mut Context<Self>) {
        self.shortcut_editor.open = true;
        self.shortcut_editor.recording = Some(action);
        cx.notify();
    }

    fn clear_shortcut_binding(&mut self, action: ShortcutAction, cx: &mut Context<Self>) {
        self.shortcuts.clear(action);
        self.shortcut_editor.recording = None;
        self.save_shortcuts_with_notice(format!("Cleared shortcut for {}", action.label()), cx);
    }

    fn assign_shortcut_binding(
        &mut self,
        action: ShortcutAction,
        binding: ShortcutBinding,
        cx: &mut Context<Self>,
    ) {
        let displaced = self.shortcuts.set_binding(action, binding);
        self.shortcut_editor.recording = None;

        let mut message = format!("{} is now {}", action.label(), self.shortcut_label(action));
        if let Some(displaced) = displaced {
            message.push_str(&format!("; cleared {}", displaced.label()));
        }

        self.save_shortcuts_with_notice(message, cx);
    }

    fn reset_shortcut_binding(&mut self, action: ShortcutAction, cx: &mut Context<Self>) {
        let displaced = self.shortcuts.reset_binding(action);
        self.shortcut_editor.recording = None;

        let mut message = format!(
            "Reset {} to {}",
            action.label(),
            self.shortcut_label(action)
        );
        if let Some(displaced) = displaced {
            message.push_str(&format!("; cleared {}", displaced.label()));
        }

        self.save_shortcuts_with_notice(message, cx);
    }

    fn reset_all_shortcuts(&mut self, cx: &mut Context<Self>) {
        self.shortcuts.reset_all();
        self.shortcut_editor.recording = None;
        self.save_shortcuts_with_notice("Restored default shortcuts", cx);
    }

    fn save_shortcuts_with_notice(&mut self, message: impl Into<String>, cx: &mut Context<Self>) {
        match self.persist_shortcuts_now() {
            Ok(()) => self.set_runtime_notice(message.into()),
            Err(error) => {
                self.set_runtime_notice(format!("shortcut save failed: {error}"));
            }
        }
        cx.notify();
    }

    fn persist_shortcuts_now(&self) -> Result<(), String> {
        let shortcut_path = shortcut_file_path();
        let Some(shortcut_dir) = shortcut_path.parent() else {
            return Err("invalid shortcut path".to_string());
        };

        fs::create_dir_all(shortcut_dir)
            .map_err(|error| format!("create {}: {error}", shortcut_dir.display()))?;
        let payload = serde_json::to_vec_pretty(&self.shortcuts.persisted_config())
            .map_err(|error| format!("serialize shortcuts: {error}"))?;
        fs::write(&shortcut_path, payload)
            .map_err(|error| format!("write {}: {error}", shortcut_path.display()))?;
        Ok(())
    }

    fn cycle_workdesk(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.workdesks.is_empty() {
            return;
        }

        let len = self.workdesks.len() as isize;
        let next = (self.active_workdesk as isize + delta).rem_euclid(len) as usize;
        self.select_workdesk(next, cx);
    }

    fn request_persist(&mut self, cx: &mut Context<Self>) {
        self.persist_generation += 1;
        let requested_generation = self.persist_generation;

        cx.spawn(async move |this, cx| {
            Timer::after(SESSION_SAVE_DEBOUNCE).await;

            let _ = this.update(cx, |this, _cx| {
                if this.persist_generation != requested_generation {
                    return;
                }

                if let Err(error) = this.persist_session_now() {
                    if let Some(workdesk) = this.workdesks.get_mut(this.active_workdesk) {
                        workdesk.runtime_notice =
                            Some(SharedString::from(format!("session save failed: {error}")));
                    }
                }
            });
        })
        .detach();
    }

    fn persist_session_now(&mut self) -> Result<(), String> {
        let session = PersistedSession::from_shell(self);
        let session_path = session_file_path();
        let Some(session_dir) = session_path.parent() else {
            return Err("invalid session path".to_string());
        };

        fs::create_dir_all(session_dir)
            .map_err(|error| format!("create {}: {error}", session_dir.display()))?;
        let payload = serde_json::to_vec_pretty(&session)
            .map_err(|error| format!("serialize session: {error}"))?;
        fs::write(&session_path, payload)
            .map_err(|error| format!("write {}: {error}", session_path.display()))?;
        Ok(())
    }

    fn active_grid_pane_id(&self) -> Option<PaneId> {
        let workdesk = self.active_workdesk();
        workdesk
            .active_pane
            .filter(|pane_id| workdesk.panes.iter().any(|pane| pane.id == *pane_id))
            .or_else(|| workdesk.panes.last().map(|pane| pane.id))
    }

    fn set_layout_mode(&mut self, layout_mode: LayoutMode, cx: &mut Context<Self>) {
        let desk = self.active_workdesk_mut();
        if desk.layout_mode == layout_mode {
            return;
        }

        desk.layout_mode = layout_mode;
        desk.drag_state = DragState::Idle;
        desk.grid_layout.expose_open = false;

        if desk.active_pane.is_none() {
            desk.active_pane = desk.panes.last().map(|pane| pane.id);
        }

        self.request_persist(cx);
        cx.notify();
    }

    fn set_grid_expose(&mut self, open: bool, cx: &mut Context<Self>) {
        let desk = self.active_workdesk_mut();
        if desk.layout_mode != LayoutMode::Grid || desk.grid_layout.expose_open == open {
            return;
        }

        desk.grid_layout.expose_open = open;
        desk.drag_state = DragState::Idle;
        cx.notify();
    }

    fn toggle_grid_expose(&mut self, cx: &mut Context<Self>) {
        let open = !self.active_workdesk().grid_layout.expose_open;
        self.set_grid_expose(open, cx);
    }

    fn activate_grid_pane(&mut self, pane_id: PaneId, close_expose: bool, cx: &mut Context<Self>) {
        let desk = self.active_workdesk_mut();
        desk.focus_pane(pane_id);
        if close_expose {
            desk.grid_layout.expose_open = false;
        }
        self.cursor_blink_visible = true;
        self.last_cursor_blink_at = Instant::now();
        self.request_persist(cx);
        cx.notify();
    }

    fn navigate_layout(
        &mut self,
        direction: GridDirection,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let layout_mode = self.active_workdesk().layout_mode;
        if !matches!(layout_mode, LayoutMode::Grid | LayoutMode::ClassicSplit) {
            return;
        }

        let Some(active_pane_id) = self.active_grid_pane_id() else {
            return;
        };
        let projection = match layout_mode {
            LayoutMode::Grid => grid_projection(&self.active_workdesk().panes),
            LayoutMode::ClassicSplit => {
                let viewport = window.window_bounds().get_bounds();
                let frames = split_layout_frames(
                    &self.active_workdesk().panes,
                    self.active_grid_pane_id(),
                    f32::from(viewport.size.width),
                    f32::from(viewport.size.height),
                );
                directional_projection_for_frames(&frames)
            }
            LayoutMode::Free => return,
        };
        let Some(target) = projection
            .neighbors
            .get(&active_pane_id)
            .copied()
            .and_then(|neighbors| neighbors.primary(direction))
        else {
            return;
        };

        self.activate_grid_pane(target, false, cx);
    }

    fn handle_shortcut_recording(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        let Some(action) = self.shortcut_editor.recording else {
            return false;
        };
        let keystroke = &event.keystroke;

        if keystroke.key == "escape" && !keystroke.modifiers.modified() {
            self.shortcut_editor.recording = None;
            cx.notify();
            return true;
        }

        if matches!(keystroke.key.as_str(), "backspace" | "delete")
            && !keystroke.modifiers.modified()
        {
            self.clear_shortcut_binding(action, cx);
            return true;
        }

        let Some(binding) = ShortcutBinding::from_keystroke(keystroke.clone()) else {
            self.set_runtime_notice(
                "Shortcut capture expects a modified chord or a non-text key. Esc cancels; Delete clears.",
            );
            cx.notify();
            return true;
        };

        self.assign_shortcut_binding(action, binding, cx);
        true
    }

    fn execute_shortcut_action(
        &mut self,
        action: ShortcutAction,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> bool {
        match action {
            ShortcutAction::ToggleShortcutPanel => {
                self.toggle_shortcut_panel(cx);
                true
            }
            ShortcutAction::SpawnShellPane => {
                self.spawn_pane(PaneKind::Shell, window, cx);
                true
            }
            ShortcutAction::SpawnAgentPane => {
                self.spawn_pane(PaneKind::Agent, window, cx);
                true
            }
            ShortcutAction::CloseActivePane => {
                let Some(pane_id) = self.active_workdesk().active_pane else {
                    return false;
                };
                self.close_pane(pane_id, cx);
                true
            }
            ShortcutAction::SpawnWorkdesk => {
                self.spawn_workdesk(cx);
                true
            }
            ShortcutAction::SelectPreviousWorkdesk => {
                self.cycle_workdesk(-1, cx);
                true
            }
            ShortcutAction::SelectNextWorkdesk => {
                self.cycle_workdesk(1, cx);
                true
            }
            ShortcutAction::LayoutFree => {
                self.set_layout_mode(LayoutMode::Free, cx);
                true
            }
            ShortcutAction::LayoutGrid => {
                self.set_layout_mode(LayoutMode::Grid, cx);
                true
            }
            ShortcutAction::LayoutSplit => {
                self.set_layout_mode(LayoutMode::ClassicSplit, cx);
                true
            }
            ShortcutAction::ToggleGridExpose => {
                if self.active_workdesk().layout_mode != LayoutMode::Grid {
                    return false;
                }
                self.toggle_grid_expose(cx);
                true
            }
            ShortcutAction::NavigateLeft => {
                if !matches!(
                    self.active_workdesk().layout_mode,
                    LayoutMode::Grid | LayoutMode::ClassicSplit
                ) {
                    return false;
                }
                self.navigate_layout(GridDirection::Left, window, cx);
                true
            }
            ShortcutAction::NavigateRight => {
                if !matches!(
                    self.active_workdesk().layout_mode,
                    LayoutMode::Grid | LayoutMode::ClassicSplit
                ) {
                    return false;
                }
                self.navigate_layout(GridDirection::Right, window, cx);
                true
            }
            ShortcutAction::NavigateUp => {
                if !matches!(
                    self.active_workdesk().layout_mode,
                    LayoutMode::Grid | LayoutMode::ClassicSplit
                ) {
                    return false;
                }
                self.navigate_layout(GridDirection::Up, window, cx);
                true
            }
            ShortcutAction::NavigateDown => {
                if !matches!(
                    self.active_workdesk().layout_mode,
                    LayoutMode::Grid | LayoutMode::ClassicSplit
                ) {
                    return false;
                }
                self.navigate_layout(GridDirection::Down, window, cx);
                true
            }
            ShortcutAction::FitPanes => {
                if self.active_workdesk().layout_mode != LayoutMode::Free {
                    return false;
                }
                self.fit_to_panes(window, cx);
                true
            }
            ShortcutAction::ResetView => {
                if self.active_workdesk().layout_mode != LayoutMode::Free {
                    return false;
                }
                self.reset_view(cx);
                true
            }
            ShortcutAction::ZoomIn => {
                if self.active_workdesk().layout_mode != LayoutMode::Free {
                    return false;
                }
                self.zoom_about_viewport_center(1.15, window, cx);
                true
            }
            ShortcutAction::ZoomOut => {
                if self.active_workdesk().layout_mode != LayoutMode::Free {
                    return false;
                }
                self.zoom_about_viewport_center(1.0 / 1.15, window, cx);
                true
            }
            ShortcutAction::TerminalCopySelection
            | ShortcutAction::TerminalPaste
            | ShortcutAction::TerminalSelectAll => {
                self.execute_terminal_shortcut_action(action, cx)
            }
        }
    }

    fn start_terminal_refresh_loop(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| loop {
            Timer::after(Duration::from_millis(33)).await;

            if this
                .update(cx, |this, cx| {
                    let blink_changed = this.tick_cursor_blink();
                    if this.sync_terminal_revisions() || blink_changed {
                        cx.notify();
                    }
                })
                .is_err()
            {
                break;
            }
        })
        .detach();
    }

    fn sync_terminal_revisions(&mut self) -> bool {
        self.workdesks
            .iter_mut()
            .any(WorkdeskState::sync_terminal_revisions)
    }

    fn tick_cursor_blink(&mut self) -> bool {
        if self.last_cursor_blink_at.elapsed() < CURSOR_BLINK_INTERVAL {
            return false;
        }

        self.last_cursor_blink_at = Instant::now();
        self.cursor_blink_visible = !self.cursor_blink_visible;
        true
    }

    fn viewport_center(&self, window: &Window) -> gpui::Point<Pixels> {
        let viewport = window.window_bounds().get_bounds();
        let visible_width = (f32::from(viewport.size.width) - SIDEBAR_WIDTH).max(1.0);
        gpui::point(px(SIDEBAR_WIDTH + visible_width * 0.5), viewport.center().y)
    }

    fn screen_to_world(&self, position: gpui::Point<Pixels>) -> WorkdeskPoint {
        let workdesk = self.active_workdesk();
        WorkdeskPoint::new(
            (f32::from(position.x) - workdesk.camera.x) / workdesk.zoom,
            (f32::from(position.y) - workdesk.camera.y) / workdesk.zoom,
        )
    }

    fn zoom_about_screen_position(
        &mut self,
        position: gpui::Point<Pixels>,
        zoom_factor: f32,
    ) -> bool {
        self.active_workdesk_mut()
            .zoom_about_screen_position(position, zoom_factor)
    }

    fn zoom_about_viewport_center(
        &mut self,
        zoom_factor: f32,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        if self.zoom_about_screen_position(self.viewport_center(window), zoom_factor) {
            cx.notify();
        }
    }

    fn reset_view(&mut self, cx: &mut Context<Self>) {
        let workdesk = self.active_workdesk_mut();
        workdesk.camera = WorkdeskPoint::new(0.0, 0.0);
        workdesk.zoom = 1.0;
        workdesk.drag_state = DragState::Idle;
        self.request_persist(cx);
        cx.notify();
    }

    fn fit_to_panes(&mut self, window: &Window, cx: &mut Context<Self>) {
        if self.active_workdesk().panes.is_empty() {
            self.reset_view(cx);
            return;
        }

        let mut min_x = f32::INFINITY;
        let mut min_y = f32::INFINITY;
        let mut max_x = f32::NEG_INFINITY;
        let mut max_y = f32::NEG_INFINITY;

        for pane in &self.active_workdesk().panes {
            min_x = min_x.min(pane.position.x);
            min_y = min_y.min(pane.position.y);
            max_x = max_x.max(pane.position.x + pane.size.width);
            max_y = max_y.max(pane.position.y + pane.size.height);
        }

        let viewport = window.window_bounds().get_bounds();
        let viewport_width = f32::from(viewport.size.width);
        let viewport_height = f32::from(viewport.size.height);
        let margin = 96.0;
        let content_width = (max_x - min_x).max(1.0);
        let content_height = (max_y - min_y).max(1.0);
        let available_width = (viewport_width - SIDEBAR_WIDTH - margin * 2.0).max(1.0);
        let available_height = (viewport_height - margin * 2.0).max(1.0);
        let fitted_zoom = (available_width / content_width)
            .min(available_height / content_height)
            .clamp(MIN_ZOOM, MAX_ZOOM);
        let content_center = WorkdeskPoint::new((min_x + max_x) * 0.5, (min_y + max_y) * 0.5);

        let viewport_center = self.viewport_center(window);
        let workdesk = self.active_workdesk_mut();
        workdesk.zoom = fitted_zoom;
        workdesk.camera.x = f32::from(viewport_center.x) - content_center.x * workdesk.zoom;
        workdesk.camera.y = f32::from(viewport_center.y) - content_center.y * workdesk.zoom;
        self.request_persist(cx);
        cx.notify();
    }

    fn spawn_pane(&mut self, kind: PaneKind, window: &Window, cx: &mut Context<Self>) {
        let size = match kind {
            PaneKind::Shell => DEFAULT_SHELL_SIZE,
            PaneKind::Agent => DEFAULT_AGENT_SIZE,
        };
        let base_label = match kind {
            PaneKind::Shell => "Shell",
            PaneKind::Agent => "Agent",
        };
        let world_center = self.screen_to_world(self.viewport_center(window));
        let desk = self.active_workdesk_mut();
        let pane_id = PaneId::new(desk.next_pane_serial);
        desk.next_pane_serial += 1;
        let cascade = 36.0 * (desk.panes.len() % 6) as f32;
        let position = if desk.layout_mode != LayoutMode::Free {
            desk.active_pane
                .and_then(|active_pane_id| {
                    desk.panes
                        .iter()
                        .find(|pane| pane.id == active_pane_id)
                        .map(|pane| {
                            WorkdeskPoint::new(
                                pane.position.x + pane.size.width + 96.0,
                                pane.position.y + 42.0 * ((desk.panes.len() % 3) as f32),
                            )
                        })
                })
                .unwrap_or_else(|| {
                    WorkdeskPoint::new(
                        world_center.x - size.width * 0.5 + cascade,
                        world_center.y - size.height * 0.5 + cascade,
                    )
                })
        } else {
            WorkdeskPoint::new(
                world_center.x - size.width * 0.5 + cascade,
                world_center.y - size.height * 0.5 + cascade,
            )
        };
        let title = format!("{base_label} {}", pane_id.raw());

        desk.panes.push(PaneRecord {
            id: pane_id,
            title: title.clone(),
            kind: kind.clone(),
            position,
            size,
        });
        desk.attach_terminal_session(pane_id, &kind, &title, size);
        desk.focus_pane(pane_id);
        desk.drag_state = DragState::Idle;
        self.request_persist(cx);
        cx.notify();
    }

    fn close_pane(&mut self, pane_id: PaneId, cx: &mut Context<Self>) {
        let desk = self.active_workdesk_mut();
        desk.panes.retain(|pane| pane.id != pane_id);

        if let Some(terminal) = desk.terminals.remove(&pane_id) {
            terminal.close();
        }
        desk.terminal_revisions.remove(&pane_id);
        desk.terminal_views.remove(&pane_id);

        if desk.active_pane == Some(pane_id) {
            desk.active_pane = desk.panes.last().map(|pane| pane.id);
        }

        if matches!(
            desk.drag_state,
            DragState::MovingPane { pane_id: active, .. }
                | DragState::ResizingPane { pane_id: active, .. }
                | DragState::SelectingTerminal { pane_id: active, .. }
                if active == pane_id
        ) {
            desk.drag_state = DragState::Idle;
        }

        self.request_persist(cx);
        cx.notify();
    }

    fn end_touchpad_pan(&mut self) {
        if self.touchpad_pan_state.take().is_some() {
            self.last_touchpad_pan_end = Some(Instant::now());
        }
    }

    fn on_touch_event(&mut self, event: &TouchEvent, window: &Window, cx: &mut Context<Self>) {
        if !matches!(self.active_workdesk().drag_state, DragState::Idle)
            || self.active_workdesk().layout_mode != LayoutMode::Free
        {
            self.end_touchpad_pan();
            return;
        }

        if event.touches.len() != 3 {
            self.end_touchpad_pan();
            return;
        }

        let Some(centroid) = event.centroid() else {
            self.end_touchpad_pan();
            return;
        };

        let touch_ids = event
            .touches
            .iter()
            .map(|touch| touch.id)
            .collect::<Vec<_>>();
        let viewport = window.window_bounds().get_bounds();
        let usable_width = (f32::from(viewport.size.width) - SIDEBAR_WIDTH).max(220.0);
        let usable_height = f32::from(viewport.size.height).max(220.0);
        let mut pan_delta = None;

        match &mut self.touchpad_pan_state {
            Some(state) if state.touch_ids == touch_ids => {
                let delta = centroid - state.last_centroid;
                let screen_delta = gpui::point(
                    px(delta.x * usable_width * THREE_FINGER_PAN_SURFACE_SCALE),
                    px(-delta.y * usable_height * THREE_FINGER_PAN_SURFACE_SCALE),
                );

                state.last_centroid = centroid;

                if matches!(event.touch_phase, TouchPhase::Moved)
                    && (f32::from(screen_delta.x).abs() >= THREE_FINGER_PAN_MIN_DELTA_PIXELS
                        || f32::from(screen_delta.y).abs() >= THREE_FINGER_PAN_MIN_DELTA_PIXELS)
                {
                    pan_delta = Some(screen_delta);
                }
            }
            _ => {
                self.touchpad_pan_state = Some(TouchpadPanState {
                    touch_ids,
                    last_centroid: centroid,
                });
            }
        }

        if let Some(screen_delta) = pan_delta {
            self.active_workdesk_mut().pan_by_screen_delta(screen_delta);
            self.request_persist(cx);
            cx.notify();
        }
    }

    fn should_suppress_swipe(&self) -> bool {
        self.last_touchpad_pan_end
            .is_some_and(|ended_at| ended_at.elapsed() <= THREE_FINGER_SWIPE_SUPPRESSION)
    }

    fn handles_canvas_gesture(&self, position: gpui::Point<Pixels>) -> bool {
        f32::from(position.x) > SIDEBAR_WIDTH
            && matches!(self.active_workdesk().drag_state, DragState::Idle)
            && self.active_workdesk().layout_mode == LayoutMode::Free
    }

    fn on_magnify_gesture(&mut self, event: &MagnifyGestureEvent, cx: &mut Context<Self>) {
        if !self.handles_canvas_gesture(event.position) || !event.magnification.is_finite() {
            return;
        }

        let zoom_factor = (1.0 + event.magnification).max(PINCH_MIN_ZOOM_FACTOR);
        if self.zoom_about_screen_position(event.position, zoom_factor) {
            self.request_persist(cx);
            cx.notify();
        }
    }

    fn on_swipe_gesture(
        &mut self,
        event: &SwipeGestureEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        if self.should_suppress_swipe() {
            return;
        }

        if !matches!(event.touch_phase, TouchPhase::Ended)
            || !self.handles_canvas_gesture(event.position)
        {
            return;
        }

        if event.delta.x == 0.0 && event.delta.y == 0.0 {
            return;
        }

        let viewport = window.window_bounds().get_bounds();
        let step_x = (f32::from(viewport.size.width) - SIDEBAR_WIDTH).max(220.0)
            * SWIPE_PAN_VIEWPORT_FRACTION;
        let step_y = f32::from(viewport.size.height).max(220.0) * SWIPE_PAN_VIEWPORT_FRACTION;
        self.active_workdesk_mut().pan_by_screen_delta(gpui::point(
            px(-event.delta.x * step_x),
            px(-event.delta.y * step_y),
        ));
        self.request_persist(cx);
        cx.notify();
    }

    fn on_smart_magnify_gesture(
        &mut self,
        event: &SmartMagnifyGestureEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        if !self.handles_canvas_gesture(event.position) {
            return;
        }

        let workdesk = self.active_workdesk();
        let near_reset = (workdesk.zoom - 1.0).abs() <= SMART_MAGNIFY_RESET_EPSILON
            && workdesk.camera.x.abs() <= 24.0
            && workdesk.camera.y.abs() <= 24.0;

        if near_reset {
            self.fit_to_panes(window, cx);
        } else {
            self.reset_view(cx);
        }
    }

    fn on_trackpad_scroll(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
        if !matches!(self.active_workdesk().drag_state, DragState::Idle) {
            return;
        }

        if self.active_workdesk().layout_mode != LayoutMode::Free {
            return;
        }

        let delta = event.delta.pixel_delta(px(SCROLL_WHEEL_LINE_HEIGHT));
        let dominant_axis = if delta.y.abs() > delta.x.abs() {
            delta.y
        } else {
            delta.x
        };

        let did_change = if event.modifiers.platform {
            let zoom_factor = (-f32::from(dominant_axis) * SCROLL_ZOOM_SENSITIVITY).exp();
            self.zoom_about_screen_position(event.position, zoom_factor)
        } else {
            self.active_workdesk_mut()
                .pan_by_screen_delta(gpui::point(-delta.x, -delta.y));
            true
        };

        if did_change {
            self.request_persist(cx);
            cx.notify();
        }
    }

    fn on_canvas_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.active_workdesk().layout_mode == LayoutMode::Grid
            && self.active_workdesk().grid_layout.expose_open
        {
            self.set_grid_expose(false, cx);
            window.focus(&self.focus_handle);
            return;
        }

        let desk = self.active_workdesk_mut();
        desk.clear_all_selections();
        desk.drag_state = if desk.layout_mode == LayoutMode::Free {
            desk.active_pane = None;
            DragState::Panning {
                last_mouse: event.position,
            }
        } else {
            DragState::Idle
        };
        window.focus(&self.focus_handle);
        cx.notify();
    }

    fn on_canvas_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !event.dragging() {
            return;
        }

        match self.active_workdesk().drag_state {
            DragState::Idle => {}
            DragState::Panning { last_mouse } => {
                let delta = event.position - last_mouse;
                let desk = self.active_workdesk_mut();
                desk.pan_by_screen_delta(delta);
                desk.drag_state = DragState::Panning {
                    last_mouse: event.position,
                };
                cx.notify();
            }
            DragState::MovingPane {
                pane_id,
                last_mouse,
            } => {
                let delta = event.position - last_mouse;
                let desk = self.active_workdesk_mut();
                desk.move_pane_by_screen_delta(pane_id, delta);
                desk.drag_state = DragState::MovingPane {
                    pane_id,
                    last_mouse: event.position,
                };
                cx.notify();
            }
            DragState::ResizingPane {
                pane_id,
                last_mouse,
            } => {
                let delta = event.position - last_mouse;
                let desk = self.active_workdesk_mut();
                desk.resize_pane_by_screen_delta(pane_id, delta);
                desk.drag_state = DragState::ResizingPane {
                    pane_id,
                    last_mouse: event.position,
                };
                cx.notify();
            }
            DragState::SelectingTerminal { pane_id, metrics } => {
                let cell = metrics.cell_at(event.position);
                let desk = self.active_workdesk_mut();
                desk.update_selection(pane_id, cell);
                cx.notify();
            }
        }
    }

    fn on_canvas_mouse_up(
        &mut self,
        _event: &MouseUpEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let desk = self.active_workdesk_mut();
        let should_persist = matches!(
            desk.drag_state,
            DragState::Panning { .. }
                | DragState::MovingPane { .. }
                | DragState::ResizingPane { .. }
        );
        if !matches!(desk.drag_state, DragState::Idle) {
            desk.drag_state = DragState::Idle;
            if should_persist {
                self.request_persist(cx);
            }
            cx.notify();
        }
    }

    fn handle_terminal_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.handle_shortcut_recording(event, cx) {
            cx.stop_propagation();
            return;
        }

        if self.shortcut_editor.open {
            let is_escape =
                event.keystroke.key == "escape" && !event.keystroke.modifiers.modified();
            let is_toggle = self
                .shortcuts
                .matching_action(event)
                .is_some_and(|action| action == ShortcutAction::ToggleShortcutPanel);

            if is_escape || is_toggle {
                self.close_shortcut_panel(cx);
            }
            cx.stop_propagation();
            return;
        }

        if self.active_workdesk().layout_mode == LayoutMode::Grid
            && self.active_workdesk().grid_layout.expose_open
            && event.keystroke.key == "escape"
            && !event.keystroke.modifiers.modified()
        {
            self.set_grid_expose(false, cx);
            cx.stop_propagation();
            return;
        }

        if let Some(action) = self.shortcuts.matching_action(event) {
            if self.execute_shortcut_action(action, window, cx) {
                cx.stop_propagation();
                return;
            }
        }

        let Some(pane_id) = self.active_workdesk().active_pane else {
            return;
        };
        let Some(terminal) = self.active_workdesk().terminals.get(&pane_id).cloned() else {
            return;
        };

        let snapshot = terminal.snapshot();
        let Some(bytes) = terminal_input_bytes(event, &snapshot) else {
            return;
        };

        if let Err(error) = terminal.send_bytes(&bytes) {
            self.active_workdesk_mut().runtime_notice = Some(SharedString::from(format!(
                "terminal input failed for pane #{}: {}",
                pane_id.raw(),
                error
            )));
        } else {
            let _ = terminal.scroll_viewport_bottom();
            self.active_workdesk_mut().clear_selection(pane_id);
            self.cursor_blink_visible = true;
            self.last_cursor_blink_at = Instant::now();
        }

        cx.stop_propagation();
    }

    fn focus_pane(&mut self, pane_id: PaneId, cx: &mut Context<Self>) {
        self.active_workdesk_mut().focus_pane(pane_id);
        self.request_persist(cx);
    }

    fn begin_pane_drag(
        &mut self,
        pane_id: PaneId,
        position: gpui::Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let desk = self.active_workdesk_mut();
        if desk.layout_mode != LayoutMode::Free {
            desk.focus_pane(pane_id);
            self.request_persist(cx);
            cx.notify();
            return;
        }
        desk.focus_pane(pane_id);
        desk.clear_selection(pane_id);
        desk.drag_state = DragState::MovingPane {
            pane_id,
            last_mouse: position,
        };
        cx.notify();
    }

    fn begin_pane_resize(
        &mut self,
        pane_id: PaneId,
        position: gpui::Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let desk = self.active_workdesk_mut();
        if desk.layout_mode != LayoutMode::Free {
            desk.focus_pane(pane_id);
            self.request_persist(cx);
            cx.notify();
            return;
        }
        desk.focus_pane(pane_id);
        desk.clear_selection(pane_id);
        desk.drag_state = DragState::ResizingPane {
            pane_id,
            last_mouse: position,
        };
        cx.notify();
    }

    fn begin_terminal_selection(
        &mut self,
        pane_id: PaneId,
        position: gpui::Point<Pixels>,
        metrics: TerminalFrameMetrics,
        cx: &mut Context<Self>,
    ) {
        let desk = self.active_workdesk_mut();
        desk.focus_pane(pane_id);
        desk.begin_selection(pane_id, metrics.cell_at(position));
        desk.drag_state = DragState::SelectingTerminal { pane_id, metrics };
        self.cursor_blink_visible = true;
        self.last_cursor_blink_at = Instant::now();
        self.request_persist(cx);
        cx.notify();
    }

    fn on_terminal_scroll(
        &mut self,
        pane_id: PaneId,
        event: &ScrollWheelEvent,
        cx: &mut Context<Self>,
    ) {
        if event.modifiers.platform {
            return;
        }

        let Some(terminal) = self.active_workdesk().terminals.get(&pane_id) else {
            return;
        };
        let snapshot = terminal.snapshot();

        if snapshot.alternate_screen || snapshot.scrollbar.total <= snapshot.scrollbar.visible {
            cx.stop_propagation();
            return;
        }

        let delta = event.delta.pixel_delta(px(SCROLL_WHEEL_LINE_HEIGHT));
        let dominant_axis = if delta.y.abs() > delta.x.abs() {
            delta.y
        } else {
            delta.x
        };
        let mut rows = (f32::from(dominant_axis) / SCROLL_WHEEL_LINE_HEIGHT).round() as isize;
        if rows == 0 && dominant_axis != px(0.0) {
            rows = if f32::from(dominant_axis).is_sign_positive() {
                1
            } else {
                -1
            };
        }

        if rows != 0 {
            if let Err(error) = terminal.scroll_viewport_delta(rows) {
                self.active_workdesk_mut().runtime_notice = Some(SharedString::from(format!(
                    "terminal scroll failed for pane #{}: {}",
                    pane_id.raw(),
                    error
                )));
            } else {
                self.cursor_blink_visible = true;
                self.last_cursor_blink_at = Instant::now();
            }
        }

        cx.stop_propagation();
        cx.notify();
    }

    fn execute_terminal_shortcut_action(
        &mut self,
        action: ShortcutAction,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(pane_id) = self.active_workdesk().active_pane else {
            return false;
        };
        let Some(terminal) = self.active_workdesk().terminals.get(&pane_id).cloned() else {
            return false;
        };
        let snapshot = terminal.snapshot();

        match action {
            ShortcutAction::TerminalCopySelection => {
                let selected_text = self
                    .active_workdesk()
                    .terminal_views
                    .get(&pane_id)
                    .and_then(|view| view.selection)
                    .and_then(|selection| terminal_selection_text(&snapshot, selection));

                if let Some(text) = selected_text {
                    if !text.is_empty() {
                        cx.write_to_clipboard(ClipboardItem::new_string(text));
                        self.set_runtime_notice("Copied terminal selection");
                        cx.notify();
                        return true;
                    }
                }

                false
            }
            ShortcutAction::TerminalPaste => {
                let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
                    return true;
                };

                if let Err(error) = terminal.send_text(&text) {
                    self.set_runtime_notice(format!(
                        "terminal paste failed for pane #{}: {}",
                        pane_id.raw(),
                        error
                    ));
                } else {
                    let _ = terminal.scroll_viewport_bottom();
                    self.active_workdesk_mut().clear_selection(pane_id);
                    self.cursor_blink_visible = true;
                    self.last_cursor_blink_at = Instant::now();
                }

                cx.notify();
                true
            }
            ShortcutAction::TerminalSelectAll => {
                if snapshot.rows.is_empty() || snapshot.cols == 0 {
                    return true;
                }

                let last_row = snapshot.rows.len().saturating_sub(1);
                let last_col = usize::from(snapshot.cols.saturating_sub(1));
                let desk = self.active_workdesk_mut();
                desk.begin_selection(pane_id, TerminalCell { row: 0, col: 0 });
                desk.update_selection(
                    pane_id,
                    TerminalCell {
                        row: last_row,
                        col: last_col,
                    },
                );
                cx.notify();
                true
            }
            _ => false,
        }
    }

    fn select_workdesk(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.workdesks.len() || index == self.active_workdesk {
            return;
        }

        self.dismiss_workdesk_menu();
        self.active_workdesk_mut().drag_state = DragState::Idle;
        self.active_workdesk = index;
        self.active_workdesk_mut().drag_state = DragState::Idle;
        if self.active_workdesk().active_pane.is_none() {
            self.active_workdesk_mut().active_pane =
                self.active_workdesk().panes.last().map(|pane| pane.id);
        }
        self.request_persist(cx);
        cx.notify();
    }

    fn dismiss_workdesk_menu(&mut self) -> bool {
        self.workdesk_menu.take().is_some()
    }

    fn open_workdesk_menu(
        &mut self,
        index: usize,
        position: gpui::Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        if index >= self.workdesks.len() {
            return;
        }

        self.workdesk_menu = Some(WorkdeskContextMenu { index, position });
        cx.notify();
    }

    fn toggle_workdesk_menu(
        &mut self,
        index: usize,
        position: gpui::Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        if matches!(self.workdesk_menu, Some(menu) if menu.index == index) {
            self.dismiss_workdesk_menu();
            cx.notify();
            return;
        }

        self.open_workdesk_menu(index, position, cx);
    }

    fn next_workdesk_name(&self) -> String {
        let mut serial = 1;
        loop {
            let candidate = format!("Workdesk {serial}");
            if self.workdesks.iter().all(|desk| desk.name != candidate) {
                return candidate;
            }
            serial += 1;
        }
    }

    fn unique_workdesk_name(&self, base: &str) -> String {
        if self.workdesks.iter().all(|desk| desk.name != base) {
            return base.to_string();
        }

        let mut serial = 2;
        loop {
            let candidate = format!("{base} {serial}");
            if self.workdesks.iter().all(|desk| desk.name != candidate) {
                return candidate;
            }
            serial += 1;
        }
    }

    fn spawn_workdesk(&mut self, cx: &mut Context<Self>) {
        self.dismiss_workdesk_menu();
        let state = blank_workdesk(self.next_workdesk_name(), DEFAULT_WORKDESK_SUMMARY);
        self.workdesks.push(state);
        self.active_workdesk = self.workdesks.len() - 1;
        self.request_persist(cx);
        cx.notify();
    }

    fn duplicate_workdesk(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(source) = self.workdesks.get(index) else {
            return;
        };

        let mut duplicated = PersistedWorkdesk::from_state(source).into_state();
        duplicated.name = self.unique_workdesk_name(&format!("{} Copy", source.name));
        duplicated.summary = source.summary.clone();
        boot_workdesk_terminals(&mut duplicated);

        let insert_at = index + 1;
        self.workdesks.insert(insert_at, duplicated);
        self.active_workdesk = insert_at;
        self.dismiss_workdesk_menu();
        self.request_persist(cx);
        cx.notify();
    }

    fn delete_workdesk(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.workdesks.len() {
            return;
        }

        if self.workdesks.len() == 1 {
            if let Some(workdesk) = self.workdesks.get_mut(index) {
                workdesk.runtime_notice = Some(SharedString::from(
                    "Keep at least one workdesk in the session.",
                ));
            }
            self.dismiss_workdesk_menu();
            cx.notify();
            return;
        }

        let mut removed = self.workdesks.remove(index);
        shutdown_workdesk_terminals(&mut removed);

        if self.active_workdesk > index {
            self.active_workdesk -= 1;
        }
        self.active_workdesk = self
            .active_workdesk
            .min(self.workdesks.len().saturating_sub(1));

        let active_workdesk = self.active_workdesk_mut();
        active_workdesk.drag_state = DragState::Idle;
        if active_workdesk.active_pane.is_none() {
            active_workdesk.active_pane = active_workdesk.panes.last().map(|pane| pane.id);
        }

        self.dismiss_workdesk_menu();
        self.request_persist(cx);
        cx.notify();
    }

    fn pane_surface(
        &self,
        pane: &PaneRecord,
        stack_index: usize,
        frame: PaneViewportFrame,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let workdesk = self.active_workdesk();
        let pane_id = pane.id;
        let is_active = workdesk.active_pane == Some(pane.id);
        let accent = pane_accent(&pane.kind);
        let border = if is_active {
            accent
        } else {
            rgb(0x2f3a44).into()
        };
        let header_bg = if is_active {
            rgb(0x1a242c)
        } else {
            rgb(0x151d24)
        };
        let screen_x = frame.x;
        let screen_y = frame.y;
        let screen_width = frame.width;
        let screen_height = frame.height;
        let header_height = 42.0 * frame.zoom.clamp(0.75, 1.4);
        let pane_padding = 16.0 * frame.zoom.clamp(0.8, 1.35);
        let resize_handle_size = 16.0 * frame.zoom.clamp(0.85, 1.4);
        let terminal_snapshot = workdesk
            .terminals
            .get(&pane.id)
            .map(|terminal| terminal.snapshot());
        let terminal_view = workdesk
            .terminal_views
            .get(&pane.id)
            .cloned()
            .unwrap_or_default();
        let runtime_title = terminal_snapshot
            .as_ref()
            .map(|snapshot| snapshot.title.clone())
            .unwrap_or_else(|| pane.title.clone());
        let terminal_body_metrics = terminal_snapshot
            .as_ref()
            .map(|snapshot| terminal_frame_metrics(frame.zoom, screen_x, screen_y, snapshot));
        let terminal_body = terminal_body(
            &terminal_snapshot,
            &terminal_view,
            frame.zoom,
            is_active,
            self.cursor_blink_visible,
        );
        let terminal_status = terminal_footer_label(&terminal_snapshot);

        let header = {
            let header = div()
                .flex()
                .justify_between()
                .items_center()
                .gap_3()
                .h(px(header_height))
                .px(px(pane_padding))
                .bg(header_bg)
                .border_b_1()
                .border_color(border)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .text_xs()
                                .text_color(accent)
                                .child(pane_kind_label(&pane.kind)),
                        )
                        .child(div().text_sm().child(runtime_title)),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .px_2()
                                .py_1()
                                .rounded_full()
                                .bg(rgb(0x0f1419))
                                .text_xs()
                                .text_color(rgb(0x9da8b1))
                                .child(format!("#{}", stack_index + 1)),
                        )
                        .child(
                            div()
                                .px_2()
                                .py_1()
                                .rounded_full()
                                .bg(rgb(0x0f1419))
                                .text_xs()
                                .text_color(rgb(0x7cc7ff))
                                .child(terminal_status),
                        )
                        .child(
                            div()
                                .cursor_pointer()
                                .px_2()
                                .py_1()
                                .rounded_full()
                                .bg(rgb(0x2f1818))
                                .text_xs()
                                .text_color(rgb(0xffc4b5))
                                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                    cx.stop_propagation();
                                })
                                .on_mouse_up(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        this.close_pane(pane_id, cx);
                                        cx.stop_propagation();
                                    }),
                                )
                                .child("Close"),
                        ),
                );

            if frame.allow_layout_drag {
                header.cursor_move().on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                        this.begin_pane_drag(pane_id, event.position, cx);
                        window.focus(&this.focus_handle);
                        cx.stop_propagation();
                    }),
                )
            } else {
                header
            }
        };

        let body = div()
            .flex()
            .flex_1()
            .overflow_hidden()
            .p(px(pane_padding))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    if let Some(metrics) = terminal_body_metrics {
                        this.begin_terminal_selection(pane_id, event.position, metrics, cx);
                        window.focus(&this.focus_handle);
                        cx.stop_propagation();
                    }
                }),
            )
            .on_scroll_wheel(cx.listener(move |this, event: &ScrollWheelEvent, _, cx| {
                this.on_terminal_scroll(pane_id, event, cx);
            }))
            .child(terminal_body);

        let surface = div()
            .absolute()
            .left(px(screen_x))
            .top(px(screen_y))
            .w(px(screen_width))
            .h(px(screen_height))
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(rgb(0x182028))
            .border_1()
            .border_color(border)
            .rounded_lg()
            .shadow_lg()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    this.focus_pane(pane_id, cx);
                    window.focus(&this.focus_handle);
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .child(header)
            .child(body);

        if frame.allow_layout_drag {
            surface.child(
                div()
                    .absolute()
                    .right(px(8.0))
                    .bottom(px(8.0))
                    .w(px(resize_handle_size))
                    .h(px(resize_handle_size))
                    .cursor_pointer()
                    .rounded_sm()
                    .bg(rgb(0x30404d))
                    .border_1()
                    .border_color(rgb(0x526373))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                            this.begin_pane_resize(pane_id, event.position, cx);
                            window.focus(&this.focus_handle);
                            cx.stop_propagation();
                        }),
                    )
                    .child(
                        div()
                            .absolute()
                            .right(px(2.0))
                            .bottom(px(2.0))
                            .w(px(6.0))
                            .h(px(6.0))
                            .rounded_sm()
                            .bg(rgb(0xb6c4d0)),
                    ),
            )
        } else {
            surface
        }
    }
}

impl Render for CanvasShell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();
        let viewport = window.window_bounds().get_bounds();
        let viewport_width = f32::from(viewport.size.width);
        let viewport_height = f32::from(viewport.size.height);
        let open_workdesk_menu = self.workdesk_menu;
        let workdesk = self.active_workdesk();
        let layout_mode = workdesk.layout_mode;
        let grid_expose_open = layout_mode == LayoutMode::Grid && workdesk.grid_layout.expose_open;
        let grid_projection = matches!(layout_mode, LayoutMode::Grid)
            .then(|| grid_projection(&workdesk.panes))
            .unwrap_or_default();
        let split_frames = matches!(layout_mode, LayoutMode::ClassicSplit)
            .then(|| {
                split_layout_frames(
                    &workdesk.panes,
                    self.active_grid_pane_id(),
                    viewport_width,
                    viewport_height,
                )
            })
            .unwrap_or_default();
        let active_grid_pane = if layout_mode == LayoutMode::Grid {
            self.active_grid_pane_id().and_then(|pane_id| {
                workdesk
                    .panes
                    .iter()
                    .enumerate()
                    .find(|(_, pane)| pane.id == pane_id)
            })
        } else {
            None
        };

        let grid_frame_zoom = active_grid_pane.map(|(_, pane)| {
            PaneViewportFrame::for_grid(pane, viewport_width, viewport_height).zoom
        });
        let active_split_zoom = self.active_grid_pane_id().and_then(|pane_id| {
            split_frames
                .iter()
                .find(|(candidate_id, _)| *candidate_id == pane_id)
                .map(|(_, frame)| frame.zoom)
        });
        let expose_layout = grid_expose_open
            .then(|| expose_layout_frame(&workdesk.panes, viewport_width, viewport_height))
            .flatten();

        let background_elements = match layout_mode {
            LayoutMode::Free => {
                let mut grid_step = GRID_STEP_WORLD * workdesk.zoom;
                while grid_step < 72.0 {
                    grid_step *= 2.0;
                }
                while grid_step > 220.0 {
                    grid_step *= 0.5;
                }

                let mut elements = Vec::new();
                let grid_color = rgb(0x182028);
                let axis_color = rgb(0x365066);

                let mut x = workdesk.camera.x.rem_euclid(grid_step);
                while x <= viewport_width {
                    elements.push(
                        div()
                            .absolute()
                            .left(px(x))
                            .top(px(0.0))
                            .w(px(1.0))
                            .h(px(viewport_height))
                            .bg(grid_color),
                    );
                    x += grid_step;
                }

                let mut y = workdesk.camera.y.rem_euclid(grid_step);
                while y <= viewport_height {
                    elements.push(
                        div()
                            .absolute()
                            .left(px(0.0))
                            .top(px(y))
                            .w(px(viewport_width))
                            .h(px(1.0))
                            .bg(grid_color),
                    );
                    y += grid_step;
                }

                if (0.0..=viewport_width).contains(&workdesk.camera.x) {
                    elements.push(
                        div()
                            .absolute()
                            .left(px(workdesk.camera.x))
                            .top(px(0.0))
                            .w(px(1.0))
                            .h(px(viewport_height))
                            .bg(axis_color),
                    );
                }

                if (0.0..=viewport_height).contains(&workdesk.camera.y) {
                    elements.push(
                        div()
                            .absolute()
                            .left(px(0.0))
                            .top(px(workdesk.camera.y))
                            .w(px(viewport_width))
                            .h(px(1.0))
                            .bg(axis_color),
                    );
                }

                elements
            }
            LayoutMode::Grid => {
                let viewport_left = SIDEBAR_WIDTH + GRID_ACTIVE_MARGIN_X * 0.45;
                let viewport_top = GRID_ACTIVE_MARGIN_TOP * 0.5;
                let viewport_card_width =
                    (viewport_width - viewport_left - GRID_ACTIVE_MARGIN_X * 0.45).max(320.0);
                let viewport_card_height =
                    (viewport_height - viewport_top - GRID_ACTIVE_MARGIN_BOTTOM * 0.45).max(260.0);

                vec![div()
                    .absolute()
                    .left(px(viewport_left))
                    .top(px(viewport_top))
                    .w(px(viewport_card_width))
                    .h(px(viewport_card_height))
                    .rounded_xl()
                    .border_1()
                    .border_color(rgb(0x1b2730))
                    .bg(rgb(0x0f151a))]
            }
            LayoutMode::ClassicSplit => vec![div()
                .absolute()
                .left(px(SIDEBAR_WIDTH + SPLIT_MARGIN_X))
                .top(px(SPLIT_MARGIN_TOP))
                .w(px(
                    (viewport_width - SIDEBAR_WIDTH - SPLIT_MARGIN_X * 2.0).max(320.0)
                ))
                .h(px((viewport_height
                    - SPLIT_MARGIN_TOP
                    - SPLIT_MARGIN_BOTTOM)
                    .max(260.0)))
                .rounded_xl()
                .border_1()
                .border_color(rgb(0x1b2730))
                .bg(rgb(0x0f151a))],
        };

        let pane_surfaces = match layout_mode {
            LayoutMode::Free => workdesk
                .panes
                .iter()
                .enumerate()
                .map(|(index, pane)| {
                    self.pane_surface(
                        pane,
                        index,
                        PaneViewportFrame::from_free(pane, workdesk),
                        cx,
                    )
                })
                .collect::<Vec<_>>(),
            LayoutMode::ClassicSplit => split_frames
                .iter()
                .filter_map(|(pane_id, frame)| {
                    workdesk
                        .panes
                        .iter()
                        .position(|pane| pane.id == *pane_id)
                        .and_then(|index| {
                            workdesk
                                .panes
                                .get(index)
                                .map(|pane| self.pane_surface(pane, index, *frame, cx))
                        })
                })
                .collect::<Vec<_>>(),
            LayoutMode::Grid => {
                if grid_expose_open {
                    Vec::new()
                } else {
                    active_grid_pane
                        .map(|(index, pane)| {
                            vec![self.pane_surface(
                                pane,
                                index,
                                PaneViewportFrame::for_grid(pane, viewport_width, viewport_height),
                                cx,
                            )]
                        })
                        .unwrap_or_default()
                }
            }
        };

        let grid_hints = if !grid_expose_open {
            if let Some((_, active_pane)) = active_grid_pane {
                let neighbors = grid_projection
                    .neighbors
                    .get(&active_pane.id)
                    .copied()
                    .unwrap_or_default();

                [
                    GridDirection::Left,
                    GridDirection::Right,
                    GridDirection::Up,
                    GridDirection::Down,
                ]
                .into_iter()
                .filter_map(|direction| {
                    let hint = neighbors.hint(direction)?;
                    let pane = workdesk.panes.iter().find(|pane| pane.id == hint.pane_id)?;

                    Some(grid_hint_card(
                        direction,
                        hint,
                        pane,
                        viewport_width,
                        viewport_height,
                        cx.listener(move |this, _, _, cx| {
                            this.activate_grid_pane(hint.pane_id, false, cx);
                            cx.stop_propagation();
                        }),
                    ))
                })
                .collect::<Vec<_>>()
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };

        let toggle_shortcut_label = self.shortcut_label(ShortcutAction::ToggleShortcutPanel);
        let grid_expose_shortcut_label = self.shortcut_label(ShortcutAction::ToggleGridExpose);
        let expose_overlay = if let Some(layout) = expose_layout {
            let cards = workdesk
                .panes
                .iter()
                .map(|pane| {
                    let pane_id = pane.id;
                    let subtitle = format!(
                        "{} · {:.0}×{:.0}",
                        pane_kind_label(&pane.kind),
                        pane.size.width,
                        pane.size.height
                    );
                    expose_card(
                        pane,
                        layout,
                        workdesk.active_pane == Some(pane.id),
                        subtitle,
                        cx.listener(move |this, _, _, cx| {
                            this.activate_grid_pane(pane_id, true, cx);
                            cx.stop_propagation();
                        }),
                    )
                })
                .collect::<Vec<_>>();

            vec![div()
                .absolute()
                .left(px(SIDEBAR_WIDTH))
                .top(px(0.0))
                .w(px(viewport_width - SIDEBAR_WIDTH))
                .h(px(viewport_height))
                .bg(rgb(0x0b1014))
                .border_l_1()
                .border_color(rgb(0x18222a))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.set_grid_expose(false, cx);
                        cx.stop_propagation();
                    }),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(EXPOSE_MARGIN_X))
                        .top(px(18.0))
                        .px_4()
                        .py_3()
                        .bg(rgb(0x131a20))
                        .border_1()
                        .border_color(rgb(0x2a3640))
                        .rounded_lg()
                        .shadow_lg()
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(0xf0d35f))
                                .child("Grid Expose"),
                        )
                        .child(div().mt_1().text_sm().child(format!(
                            "Click a pane to zoom back into it, or press {} to close the overview.",
                            grid_expose_shortcut_label
                        ))),
                )
                .children(cards)]
        } else {
            Vec::new()
        };

        let active_pane = workdesk.active_pane_title();
        let drag_status = workdesk.drag_status();
        let zoom_label = match layout_mode {
            LayoutMode::Grid => {
                if grid_expose_open {
                    "overview".to_string()
                } else {
                    grid_frame_zoom
                        .map(|zoom| format!("{:.0}%", zoom * 100.0))
                        .unwrap_or_else(|| "auto".to_string())
                }
            }
            LayoutMode::ClassicSplit => active_split_zoom
                .map(|zoom| format!("{:.0}%", zoom * 100.0))
                .unwrap_or_else(|| "auto".to_string()),
            LayoutMode::Free => format!("{:.0}%", workdesk.zoom * 100.0),
        };
        let live_terminals = format!("{}", workdesk.terminals.len());
        let ghostty_chip = if self.ghostty_status.as_ref().contains("linked") {
            "ghostty linked".to_string()
        } else {
            "pty bridge".to_string()
        };
        let layout_status = if grid_expose_open {
            "Grid / Expose".to_string()
        } else {
            layout_mode.label().to_string()
        };
        let movement_status = match layout_mode {
            LayoutMode::Grid => {
                if grid_expose_open {
                    "Choosing pane".to_string()
                } else {
                    "Directional lens".to_string()
                }
            }
            LayoutMode::ClassicSplit => "Tiled lens".to_string(),
            LayoutMode::Free => drag_status,
        };
        let workdesk_cards = self
            .workdesks
            .iter()
            .enumerate()
            .map(|(index, desk)| {
                let is_menu_open = matches!(open_workdesk_menu, Some(menu) if menu.index == index);
                let preview = if desk.panes.is_empty() {
                    "No panes yet".to_string()
                } else {
                    desk.panes
                        .iter()
                        .take(3)
                        .map(|pane| pane.title.as_str())
                        .collect::<Vec<_>>()
                        .join(" · ")
                };
                workdesk_card(
                    index,
                    desk,
                    index == self.active_workdesk,
                    is_menu_open,
                    preview,
                    workdesk_accent(index),
                    cx.listener(move |this, _, _, cx| {
                        this.dismiss_workdesk_menu();
                        this.select_workdesk(index, cx);
                        cx.stop_propagation();
                    }),
                    cx.listener(move |this, event: &MouseUpEvent, _, cx| {
                        this.open_workdesk_menu(index, event.position, cx);
                        cx.stop_propagation();
                    }),
                    cx.listener(move |this, event: &MouseUpEvent, _, cx| {
                        this.toggle_workdesk_menu(index, event.position, cx);
                        cx.stop_propagation();
                    }),
                )
            })
            .collect::<Vec<_>>();
        let shortcut_path_label = shortcut_file_path().display().to_string();
        let recording_shortcut = self
            .shortcut_editor
            .recording
            .map(|action| action.label().to_string());
        let shortcut_sections = [
            ShortcutGroup::Workspace,
            ShortcutGroup::Layout,
            ShortcutGroup::View,
            ShortcutGroup::Terminal,
        ]
        .into_iter()
        .map(|group| {
            let accent = shortcut_group_accent(group);
            let rows = ShortcutAction::all()
                .iter()
                .copied()
                .filter(|action| action.group() == group)
                .map(|action| {
                    shortcut_row(
                        action,
                        self.shortcut_label(action),
                        self.shortcut_editor.recording == Some(action),
                        cx.listener(move |this, _, _, cx| {
                            this.begin_shortcut_recording(action, cx);
                            cx.stop_propagation();
                        }),
                        cx.listener(move |this, _, _, cx| {
                            this.reset_shortcut_binding(action, cx);
                            cx.stop_propagation();
                        }),
                    )
                })
                .collect::<Vec<_>>();

            div()
                .flex()
                .flex_col()
                .gap_3()
                .p_3()
                .bg(rgb(0x11181e))
                .border_1()
                .border_color(rgb(0x24313b))
                .rounded_lg()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(div().text_xs().text_color(accent).child(group.label()))
                        .child(div().text_sm().child(group.summary())),
                )
                .children(rows)
        })
        .collect::<Vec<_>>();
        let shortcut_overlay = if self.shortcut_editor.open {
            vec![
                div()
                    .absolute()
                    .left(px(0.0))
                    .top(px(0.0))
                    .w(px(viewport_width))
                    .h(px(viewport_height))
                    .bg(rgba(0x091016d6))
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                        this.close_shortcut_panel(cx);
                        cx.stop_propagation();
                    })),
                div()
                    .absolute()
                    .top(px(SHORTCUT_PANEL_MARGIN))
                    .right(px(SHORTCUT_PANEL_MARGIN))
                    .bottom(px(SHORTCUT_PANEL_MARGIN))
                    .w(px(
                        SHORTCUT_PANEL_WIDTH
                            .min((viewport_width - SHORTCUT_PANEL_MARGIN * 2.0).max(320.0)),
                    ))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .p_4()
                    .bg(rgb(0x0f151b))
                    .border_1()
                    .border_color(rgb(0x27333d))
                    .rounded_xl()
                    .shadow_lg()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .on_scroll_wheel(|_, _, cx| {
                        cx.stop_propagation();
                    })
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .items_start()
                            .gap_4()
                            .child(
                                div()
                                    .flex_1()
                                    .flex_col()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(rgb(0x7cc7ff))
                                            .child("Shortcut Drawer"),
                                    )
                                    .child(div().text_lg().child("Configurable hotkeys"))
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(rgb(0x90a0aa))
                                            .child(
                                                "Click any binding to record a new chord. Press Delete while recording to clear it, or Escape to cancel.",
                                            ),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(control_button(
                                        "Defaults",
                                        rgb(0xf0d35f).into(),
                                        cx.listener(|this, _, _, cx| {
                                            this.reset_all_shortcuts(cx);
                                            cx.stop_propagation();
                                        }),
                                    ))
                                    .child(control_button(
                                        "Close",
                                        rgb(0x7cc7ff).into(),
                                        cx.listener(|this, _, _, cx| {
                                            this.close_shortcut_panel(cx);
                                            cx.stop_propagation();
                                        }),
                                    )),
                            ),
                    )
                    .when_some(recording_shortcut.clone(), |panel, action_label| {
                        panel.child(
                            div()
                                .px_3()
                                .py_2()
                                .bg(rgb(0x182028))
                                .border_1()
                                .border_color(rgb(0x2e4556))
                                .rounded_lg()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(0x7cc7ff))
                                        .child(format!("Recording: {action_label}")),
                                )
                                .child(
                                    div()
                                        .mt_1()
                                        .text_xs()
                                        .text_color(rgb(0xb7c1c8))
                                        .child(
                                            "The next supported chord becomes the new binding immediately.",
                                        ),
                                ),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .px_3()
                            .py_2()
                            .bg(rgb(0x131a20))
                            .border_1()
                            .border_color(rgb(0x25303a))
                            .rounded_lg()
                            .child(status_chip("Open", toggle_shortcut_label.clone()))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(0x90a0aa))
                                    .child(shortcut_path_label.clone()),
                            ),
                    )
                    .child(
                        div()
                            .id("shortcut-scroll-list")
                            .flex_1()
                            .overflow_y_scroll()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .children(shortcut_sections),
                    ),
            ]
        } else {
            Vec::new()
        };
        let workdesk_context_menu = open_workdesk_menu.and_then(|menu| {
            let desk = self.workdesks.get(menu.index)?;
            let can_delete = self.workdesks.len() > 1;
            let menu_height = if can_delete { 174.0 } else { 128.0 };
            let max_left = (SIDEBAR_WIDTH - WORKDESK_MENU_WIDTH - 12.0).max(12.0);
            let max_top = (viewport_height - menu_height - 12.0).max(12.0);
            let left = (f32::from(menu.position.x) + 8.0).clamp(12.0, max_left);
            let top = f32::from(menu.position.y).clamp(12.0, max_top);
            let accent = workdesk_accent(menu.index);

            Some(
                div()
                    .absolute()
                    .left(px(left))
                    .top(px(top))
                    .w(px(WORKDESK_MENU_WIDTH))
                    .p_2()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .bg(rgb(0x11181e))
                    .border_1()
                    .border_color(rgb(0x2c3944))
                    .rounded_lg()
                    .shadow_lg()
                    .on_any_mouse_down(|_, _, cx| {
                        cx.stop_propagation();
                    })
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        if this.dismiss_workdesk_menu() {
                            cx.notify();
                        }
                        cx.stop_propagation();
                    }))
                    .on_scroll_wheel(|_, _, cx| {
                        cx.stop_propagation();
                    })
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(accent)
                                    .child(format!("Desk {}", menu.index + 1)),
                            )
                            .child(div().text_sm().child(desk.name.clone()))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(0x90a0aa))
                                    .child(format!("{} panes", desk.panes.len())),
                            ),
                    )
                    .child(workdesk_menu_item(
                        "Open",
                        "Switch to this desk",
                        accent,
                        cx.listener(move |this, _, _, cx| {
                            this.dismiss_workdesk_menu();
                            this.select_workdesk(menu.index, cx);
                            cx.stop_propagation();
                        }),
                    ))
                    .child(workdesk_menu_item(
                        "Duplicate",
                        "Clone the layout and panes",
                        rgb(0x7cc7ff).into(),
                        cx.listener(move |this, _, _, cx| {
                            this.duplicate_workdesk(menu.index, cx);
                            cx.stop_propagation();
                        }),
                    ))
                    .when(can_delete, |menu_div| {
                        menu_div.child(workdesk_menu_item(
                            "Delete",
                            "Remove this desk from the session",
                            rgb(0xff9b88).into(),
                            cx.listener(move |this, _, _, cx| {
                                this.delete_workdesk(menu.index, cx);
                                cx.stop_propagation();
                            }),
                        ))
                    }),
            )
        });
        let touch_entity = entity.clone();
        let magnify_entity = entity.clone();
        let swipe_entity = entity.clone();
        let smart_magnify_entity = entity.clone();

        div()
            .relative()
            .size_full()
            .overflow_hidden()
            .bg(rgb(0x11161b))
            .text_color(rgb(0xf7efe5))
            .track_focus(&self.focus_handle)
            .on_touch(move |event, window, cx| {
                touch_entity.update(cx, |this, cx| {
                    this.on_touch_event(event, window, cx);
                });
            })
            .on_magnify_gesture(move |event, _, cx| {
                magnify_entity.update(cx, |this, cx| {
                    this.on_magnify_gesture(event, cx);
                });
            })
            .on_swipe_gesture(move |event, window, cx| {
                swipe_entity.update(cx, |this, cx| {
                    this.on_swipe_gesture(event, window, cx);
                });
            })
            .on_smart_magnify_gesture(move |event, window, cx| {
                smart_magnify_entity.update(cx, |this, cx| {
                    this.on_smart_magnify_gesture(event, window, cx);
                });
            })
            .on_key_down(cx.listener(Self::handle_terminal_key_down))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_canvas_mouse_down))
            .on_mouse_move(cx.listener(Self::on_canvas_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_canvas_mouse_up))
            .on_scroll_wheel(move |event, _, cx| {
                entity.update(cx, |this, cx| {
                    this.on_trackpad_scroll(event, cx);
                });
            })
            .children(background_elements)
            .children(pane_surfaces)
            .children(grid_hints)
            .children(expose_overlay)
            .child(
                div()
                    .absolute()
                    .left(px(0.0))
                    .top(px(0.0))
                    .w(px(SIDEBAR_WIDTH))
                    .h(px(viewport_height))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .p_4()
                    .bg(rgb(0x0d1217))
                    .border_r_1()
                    .border_color(rgb(0x22303a))
                    .shadow_lg()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .on_scroll_wheel(|_, _, cx| {
                        cx.stop_propagation();
                    })
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(0x7cc7ff))
                                    .child("Workdesks"),
                            )
                            .child(div().text_lg().child("Parallel desks"))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(0x90a0aa))
                                    .child(
                                        "Separate active threads into their own spatial desks and switch context from the left rail.",
                                    ),
                            ),
                    )
                    .child(
                        control_button(
                            "+ New Desk",
                            rgb(0x77d19a).into(),
                            cx.listener(|this, _, _, cx| {
                                this.spawn_workdesk(cx);
                                cx.stop_propagation();
                            }),
                        ),
                    )
                    .children(workdesk_cards)
                    .child(
                        div()
                            .p_3()
                            .bg(rgb(0x131a20))
                            .border_1()
                            .border_color(rgb(0x2a3640))
                            .rounded_lg()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(0xb4a4ff))
                                    .child("Shortcuts"),
                            )
                            .child(
                                div()
                                    .mt_1()
                                    .text_sm()
                                    .child("Record and remap your hotkeys"),
                            )
                            .child(
                                div()
                                    .mt_1()
                                    .text_xs()
                                    .text_color(rgb(0x90a0aa))
                                    .child(
                                        "Bindings are saved to .canvas/shortcuts.json and reloaded on startup.",
                                    ),
                            )
                            .child(
                                div()
                                    .mt_3()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap_2()
                                    .child(status_chip("Open", toggle_shortcut_label.clone()))
                                    .child(toggle_button(
                                        "Keys",
                                        rgb(0xb4a4ff).into(),
                                        self.shortcut_editor.open,
                                        cx.listener(|this, _, _, cx| {
                                            this.toggle_shortcut_panel(cx);
                                            cx.stop_propagation();
                                        }),
                                    )),
                            ),
                    )
                    .child(
                        div()
                            .mt_auto()
                            .p_3()
                            .bg(rgb(0x131a20))
                            .border_1()
                            .border_color(rgb(0x2a3640))
                            .rounded_lg()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(0x7cc7ff))
                                    .child("Ghostty bridge"),
                            )
                            .child(
                                div()
                                    .mt_1()
                                    .text_sm()
                                    .text_color(rgb(0xdce2e8))
                                    .child(self.ghostty_status.clone()),
                            )
                            .child(
                                div()
                                    .mt_2()
                                    .text_xs()
                                    .text_color(rgb(0x7f8a94))
                                    .child(format!("vendor: {}", self.ghostty_vendor_dir)),
                            ),
                    ),
            )
            .child(
                div()
                    .absolute()
                    .left(px(SIDEBAR_WIDTH + 20.0))
                    .bottom(px(20.0))
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_2()
                    .bg(rgb(0x131a20))
                    .border_1()
                    .border_color(rgb(0x2a3640))
                    .rounded_xl()
                    .shadow_lg()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .on_scroll_wheel(|_, _, cx| {
                        cx.stop_propagation();
                    })
                    .child(control_button(
                        "Shell",
                        rgb(0xe59a49).into(),
                        cx.listener(|this, _, window, cx| {
                            this.spawn_pane(PaneKind::Shell, window, cx);
                            cx.stop_propagation();
                        }),
                    ))
                    .child(control_button(
                        "Agent",
                        rgb(0x7cc7ff).into(),
                        cx.listener(|this, _, window, cx| {
                            this.spawn_pane(PaneKind::Agent, window, cx);
                            cx.stop_propagation();
                        }),
                    ))
                    .child(dock_divider())
                    .child(toggle_button(
                        "Free",
                        rgb(0x77d19a).into(),
                        layout_mode == LayoutMode::Free,
                        cx.listener(|this, _, _, cx| {
                            this.set_layout_mode(LayoutMode::Free, cx);
                            cx.stop_propagation();
                        }),
                    ))
                    .child(toggle_button(
                        "Grid",
                        rgb(0xf0d35f).into(),
                        layout_mode == LayoutMode::Grid,
                        cx.listener(|this, _, _, cx| {
                            this.set_layout_mode(LayoutMode::Grid, cx);
                            cx.stop_propagation();
                        }),
                    ))
                    .child(toggle_button(
                        "Split",
                        rgb(0x7cc7ff).into(),
                        layout_mode == LayoutMode::ClassicSplit,
                        cx.listener(|this, _, _, cx| {
                            this.set_layout_mode(LayoutMode::ClassicSplit, cx);
                            cx.stop_propagation();
                        }),
                    ))
                    .child(dock_divider())
                    .child(toggle_button(
                        "Keys",
                        rgb(0xb4a4ff).into(),
                        self.shortcut_editor.open,
                        cx.listener(|this, _, _, cx| {
                            this.toggle_shortcut_panel(cx);
                            cx.stop_propagation();
                        }),
                    ))
                    .when(layout_mode == LayoutMode::Grid, |dock| {
                        dock.child(toggle_button(
                            "Expose",
                            rgb(0xb4a4ff).into(),
                            grid_expose_open,
                            cx.listener(|this, _, _, cx| {
                                this.toggle_grid_expose(cx);
                                cx.stop_propagation();
                            }),
                        ))
                    })
                    .when(layout_mode == LayoutMode::Free, |dock| {
                        dock.child(dock_divider())
                            .child(control_button(
                                "Fit",
                                rgb(0x77d19a).into(),
                                cx.listener(|this, _, window, cx| {
                                    this.fit_to_panes(window, cx);
                                    cx.stop_propagation();
                                }),
                            ))
                            .child(control_button(
                                "1:1",
                                rgb(0xf0d35f).into(),
                                cx.listener(|this, _, _, cx| {
                                    this.reset_view(cx);
                                    cx.stop_propagation();
                                }),
                            ))
                            .child(control_button(
                                "+",
                                rgb(0xb4a4ff).into(),
                                cx.listener(|this, _, window, cx| {
                                    this.zoom_about_viewport_center(1.15, window, cx);
                                    cx.stop_propagation();
                                }),
                            ))
                            .child(control_button(
                                "-",
                                rgb(0xb4a4ff).into(),
                                cx.listener(|this, _, window, cx| {
                                    this.zoom_about_viewport_center(1.0 / 1.15, window, cx);
                                    cx.stop_propagation();
                                }),
                            ))
                    })
                    .when(layout_mode == LayoutMode::Grid, |dock| {
                        dock.child(dock_divider()).child(status_chip(
                            if grid_expose_open {
                                "Overview"
                            } else {
                                "Navigate"
                            },
                            if grid_expose_open {
                                "Click pane or Esc".to_string()
                            } else {
                                "Cmd+Shift+Arrows".to_string()
                            },
                        ))
                    })
                    .when(layout_mode == LayoutMode::ClassicSplit, |dock| {
                        dock.child(dock_divider())
                            .child(status_chip("Navigate", "Cmd+Shift+Arrows".to_string()))
                    }),
            )
            .child(
                div()
                    .absolute()
                    .right(px(20.0))
                    .bottom(px(20.0))
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_2()
                    .bg(rgb(0x131a20))
                    .border_1()
                    .border_color(rgb(0x2a3640))
                    .rounded_xl()
                    .shadow_lg()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .on_scroll_wheel(|_, _, cx| {
                        cx.stop_propagation();
                    })
                    .child(status_chip("Panes", format!("{}", workdesk.panes.len())))
                    .child(status_chip("Desk", workdesk.name.clone()))
                    .child(status_chip("Live", live_terminals))
                    .child(status_chip("Focus", active_pane))
                    .child(status_chip("Layout", layout_status))
                    .child(status_chip("Mode", movement_status))
                    .child(status_chip("Zoom", zoom_label))
                    .child(status_chip("Keys", toggle_shortcut_label))
                    .child(status_chip("Bridge", ghostty_chip)),
            )
            .children(shortcut_overlay)
            .when_some(workdesk.runtime_notice.clone(), |root, notice| {
                root.child(
                    div()
                        .absolute()
                        .top(px(20.0))
                        .right(px(20.0))
                        .max_w(px(420.0))
                        .px_3()
                        .py_2()
                        .bg(rgb(0x2a1a1a))
                        .border_1()
                        .border_color(rgb(0x5b3434))
                        .rounded_lg()
                        .shadow_lg()
                        .text_xs()
                        .text_color(rgb(0xffd4c7))
                        .child(notice),
                )
            })
            .when_some(workdesk_context_menu, |root, menu| root.child(menu))
    }
}

fn main() {
    let (workdesks, active_workdesk, shortcuts, boot_notice) = load_boot_state();
    let ghostty = ghostty_build_info();
    let ghostty_vendor_dir = SharedString::from(ghostty.vendor_dir.display().to_string());
    let ghostty_status = if ghostty.linked {
        SharedString::from("libghostty-vt linked")
    } else {
        SharedString::from("temporary PTY terminal live, libghostty-vt bridge still pending")
    };

    Application::new().run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1280.0), px(820.0)), cx);
        let workdesks = workdesks.clone();
        let active_workdesk = active_workdesk;
        let shortcuts = shortcuts.clone();
        let boot_notice = boot_notice.clone();
        let ghostty_vendor_dir = ghostty_vendor_dir.clone();
        let ghostty_status = ghostty_status.clone();

        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            move |window, cx| {
                let workdesks = workdesks.clone();
                let active_workdesk = active_workdesk;
                let shortcuts = shortcuts.clone();
                let boot_notice = boot_notice.clone();
                let ghostty_vendor_dir = ghostty_vendor_dir.clone();
                let ghostty_status = ghostty_status.clone();
                let focus_handle = cx.focus_handle();
                let window_focus_handle = focus_handle.clone();
                let shell = cx.new(move |_| {
                    CanvasShell::new(
                        workdesks,
                        active_workdesk,
                        shortcuts,
                        boot_notice,
                        focus_handle.clone(),
                        ghostty_vendor_dir,
                        ghostty_status,
                    )
                });

                window.focus(&window_focus_handle);
                shell.update(cx, |this, cx| {
                    this.start_terminal_refresh_loop(cx);
                    this.request_persist(cx);
                });

                shell
            },
        )
        .unwrap();

        cx.activate(true);
    });
}

fn load_boot_state() -> (Vec<WorkdeskState>, usize, ShortcutMap, Option<SharedString>) {
    let (workdesks, active_workdesk, session_notice) = match load_persisted_session() {
        Ok(Some((workdesks, active_workdesk))) => (workdesks, active_workdesk, None),
        Ok(None) => (initial_workdesks(), 0, None),
        Err(error) => (
            initial_workdesks(),
            0,
            Some(format!(
                "session restore failed, started with a blank desk: {error}"
            )),
        ),
    };

    let (shortcuts, shortcut_notice) = match load_persisted_shortcuts() {
        Ok(payload) => payload,
        Err(error) => (
            ShortcutMap::default(),
            Some(format!(
                "shortcut config failed, using defaults instead: {error}"
            )),
        ),
    };

    let mut notices = Vec::new();
    if let Some(notice) = session_notice {
        notices.push(notice);
    }
    if let Some(notice) = shortcut_notice {
        notices.push(notice);
    }

    let boot_notice = (!notices.is_empty()).then(|| SharedString::from(notices.join(" | ")));

    (workdesks, active_workdesk, shortcuts, boot_notice)
}

fn load_persisted_session() -> Result<Option<(Vec<WorkdeskState>, usize)>, String> {
    let session_path = session_file_path();
    let payload = match fs::read(&session_path) {
        Ok(payload) => payload,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!("read {}: {error}", session_path.display()));
        }
    };

    let session = serde_json::from_slice::<PersistedSession>(&payload)
        .map_err(|error| format!("parse {}: {error}", session_path.display()))?;

    session
        .into_runtime()
        .ok_or_else(|| format!("{} did not contain any workdesks", session_path.display()))
        .map(Some)
}

fn session_file_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../.canvas")
        .join("session.json")
}

fn load_persisted_shortcuts() -> Result<(ShortcutMap, Option<String>), String> {
    let shortcut_path = shortcut_file_path();
    let payload = match fs::read(&shortcut_path) {
        Ok(payload) => payload,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok((ShortcutMap::default(), None));
        }
        Err(error) => {
            return Err(format!("read {}: {error}", shortcut_path.display()));
        }
    };

    let config = serde_json::from_slice::<PersistedShortcutConfig>(&payload)
        .map_err(|error| format!("parse {}: {error}", shortcut_path.display()))?;
    let (shortcuts, warnings) = ShortcutMap::from_persisted(config);
    let warning_notice = (!warnings.is_empty()).then(|| {
        format!(
            "shortcut config loaded with warnings: {}",
            warnings.join("; ")
        )
    });

    Ok((shortcuts, warning_notice))
}

fn shortcut_file_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../.canvas")
        .join("shortcuts.json")
}

fn initial_workdesks() -> Vec<WorkdeskState> {
    vec![blank_workdesk("Workdesk 1", DEFAULT_WORKDESK_SUMMARY)]
}

fn blank_workdesk(name: impl Into<String>, summary: impl Into<String>) -> WorkdeskState {
    WorkdeskState::new(name, summary, Vec::new())
}

fn boot_workdesk_terminals(workdesk: &mut WorkdeskState) {
    let panes_to_boot = workdesk.panes.clone();
    for pane in panes_to_boot {
        workdesk.attach_terminal_session(pane.id, &pane.kind, &pane.title, pane.size);
    }
}

fn shutdown_workdesk_terminals(workdesk: &mut WorkdeskState) {
    for terminal in workdesk.terminals.values() {
        terminal.close();
    }
    workdesk.terminals.clear();
    workdesk.terminal_revisions.clear();
    workdesk.terminal_views.clear();
}

fn workdesk_card(
    index: usize,
    desk: &WorkdeskState,
    is_active: bool,
    is_menu_open: bool,
    preview: String,
    accent: gpui::Hsla,
    listener: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
    context_listener: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
    menu_button_listener: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let border = if is_active || is_menu_open {
        accent
    } else {
        rgb(0x24313b).into()
    };
    let background = if is_active || is_menu_open {
        rgb(0x162028)
    } else {
        rgb(0x11181e)
    };
    let focus = desk.active_pane_title();

    div()
        .flex()
        .flex_col()
        .gap_2()
        .p_3()
        .bg(background)
        .border_1()
        .border_color(border)
        .rounded_lg()
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, |_, _, cx| {
            cx.stop_propagation();
        })
        .on_mouse_down(MouseButton::Right, |_, _, cx| {
            cx.stop_propagation();
        })
        .on_mouse_up(MouseButton::Left, listener)
        .on_mouse_up(MouseButton::Right, context_listener)
        .child(
            div()
                .flex()
                .justify_between()
                .items_center()
                .child(
                    div()
                        .text_xs()
                        .text_color(accent)
                        .child(format!("Desk {}", index + 1)),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .px_2()
                                .py_1()
                                .rounded_full()
                                .bg(rgb(0x0c1116))
                                .text_xs()
                                .text_color(rgb(0x9da8b1))
                                .child(format!("{} panes", desk.panes.len())),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_center()
                                .w(px(28.0))
                                .h(px(28.0))
                                .bg(if is_menu_open {
                                    rgb(0x1c2730)
                                } else {
                                    rgb(0x0c1116)
                                })
                                .border_1()
                                .border_color(if is_menu_open {
                                    accent
                                } else {
                                    rgb(0x24313b).into()
                                })
                                .rounded_md()
                                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                    cx.stop_propagation();
                                })
                                .on_mouse_up(MouseButton::Left, menu_button_listener)
                                .child(div().text_xs().text_color(rgb(0x9da8b1)).child("•••")),
                        ),
                ),
        )
        .child(div().text_sm().child(desk.name.clone()))
        .child(
            div()
                .text_xs()
                .text_color(rgb(0x90a0aa))
                .child(desk.summary.clone()),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap_2()
                .child(status_chip("Live", format!("{}", desk.terminals.len())))
                .child(status_chip("Layout", desk.layout_mode.label().to_string()))
                .child(status_chip("Focus", focus)),
        )
        .child(div().text_xs().text_color(rgb(0xb7c1c8)).child(preview))
}

fn workdesk_menu_item(
    label: &str,
    detail: &str,
    accent: gpui::Hsla,
    listener: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let label = label.to_string();
    let detail = detail.to_string();

    div()
        .flex()
        .flex_col()
        .gap_1()
        .px_3()
        .py_2()
        .cursor_pointer()
        .bg(rgb(0x171f26))
        .border_1()
        .border_color(rgb(0x293742))
        .rounded_md()
        .on_mouse_down(MouseButton::Left, |_, _, cx| {
            cx.stop_propagation();
        })
        .on_mouse_up(MouseButton::Left, listener)
        .child(div().text_sm().text_color(accent).child(label))
        .child(div().text_xs().text_color(rgb(0x90a0aa)).child(detail))
}

fn shortcut_group_accent(group: ShortcutGroup) -> gpui::Hsla {
    match group {
        ShortcutGroup::Workspace => rgb(0x77d19a).into(),
        ShortcutGroup::Layout => rgb(0xf0d35f).into(),
        ShortcutGroup::View => rgb(0xb4a4ff).into(),
        ShortcutGroup::Terminal => rgb(0x7cc7ff).into(),
    }
}

fn shortcut_binding_button(
    label: &str,
    accent: gpui::Hsla,
    active: bool,
    listener: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let label = label.to_string();
    let background = if active { rgb(0x1f2d36) } else { rgb(0x171d24) };
    let border = if active { accent } else { rgb(0x2b3641).into() };

    div()
        .flex()
        .items_center()
        .justify_center()
        .min_w(px(134.0))
        .px_3()
        .py_2()
        .cursor_pointer()
        .bg(background)
        .border_1()
        .border_color(border)
        .rounded_lg()
        .font_family(".ZedMono")
        .on_mouse_down(MouseButton::Left, |_, _, cx| {
            cx.stop_propagation();
        })
        .on_mouse_up(MouseButton::Left, listener)
        .child(div().text_xs().text_color(accent).child(label))
}

fn shortcut_row(
    action: ShortcutAction,
    binding_label: String,
    is_recording: bool,
    record_listener: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
    reset_listener: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let accent = shortcut_group_accent(action.group());
    let background = if is_recording {
        rgb(0x182028)
    } else {
        rgb(0x121920)
    };
    let border = if is_recording {
        accent
    } else {
        rgb(0x24313b).into()
    };

    div()
        .flex()
        .justify_between()
        .items_center()
        .gap_4()
        .p_3()
        .bg(background)
        .border_1()
        .border_color(border)
        .rounded_lg()
        .child(
            div()
                .flex_1()
                .flex_col()
                .gap_1()
                .child(div().text_sm().child(action.label()))
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(0x90a0aa))
                        .child(action.description()),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(shortcut_binding_button(
                    if is_recording {
                        "Press keys..."
                    } else {
                        binding_label.as_str()
                    },
                    accent,
                    is_recording,
                    record_listener,
                ))
                .child(control_button(
                    "Default",
                    rgb(0x7f8a94).into(),
                    reset_listener,
                )),
        )
}

fn control_button(
    label: &str,
    accent: gpui::Hsla,
    listener: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let label = label.to_string();

    div()
        .flex()
        .items_center()
        .justify_center()
        .min_w(px(42.0))
        .px_3()
        .py_2()
        .cursor_pointer()
        .bg(rgb(0x171d24))
        .border_1()
        .border_color(rgb(0x2b3641))
        .rounded_lg()
        .on_mouse_down(MouseButton::Left, |_, _, cx| {
            cx.stop_propagation();
        })
        .on_mouse_up(MouseButton::Left, listener)
        .child(div().text_xs().text_color(accent).child(label))
}

fn toggle_button(
    label: &str,
    accent: gpui::Hsla,
    active: bool,
    listener: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let label = label.to_string();
    let background = if active { rgb(0x1f2932) } else { rgb(0x171d24) };
    let border = if active { accent } else { rgb(0x2b3641).into() };

    div()
        .flex()
        .items_center()
        .justify_center()
        .min_w(px(52.0))
        .px_3()
        .py_2()
        .cursor_pointer()
        .bg(background)
        .border_1()
        .border_color(border)
        .rounded_lg()
        .on_mouse_down(MouseButton::Left, |_, _, cx| {
            cx.stop_propagation();
        })
        .on_mouse_up(MouseButton::Left, listener)
        .child(div().text_xs().text_color(accent).child(label))
}

fn dock_divider() -> impl IntoElement {
    div().w(px(1.0)).h(px(24.0)).bg(rgb(0x2b3641))
}

fn status_chip(label: &str, value: String) -> impl IntoElement {
    let label = label.to_string();

    div()
        .flex()
        .items_center()
        .gap_2()
        .px_2()
        .py_1()
        .rounded_full()
        .bg(rgb(0x171d24))
        .child(div().text_xs().text_color(rgb(0x7f8a94)).child(label))
        .child(div().text_xs().text_color(rgb(0xdce2e8)).child(value))
}

fn workdesk_accent(index: usize) -> gpui::Hsla {
    match index % 4 {
        0 => rgb(0xe59a49).into(),
        1 => rgb(0x7cc7ff).into(),
        2 => rgb(0x77d19a).into(),
        _ => rgb(0xb4a4ff).into(),
    }
}

fn pane_accent(kind: &PaneKind) -> gpui::Hsla {
    match kind {
        PaneKind::Shell => rgb(0xe59a49).into(),
        PaneKind::Agent => rgb(0x7cc7ff).into(),
    }
}

fn pane_kind_label(kind: &PaneKind) -> &'static str {
    match kind {
        PaneKind::Shell => "Shell pane",
        PaneKind::Agent => "Agent pane",
    }
}

fn grid_hint_card(
    direction: GridDirection,
    hint: GridDirectionHint,
    pane: &PaneRecord,
    viewport_width: f32,
    viewport_height: f32,
    listener: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let accent = pane_accent(&pane.kind);
    let (left, top) = match direction {
        GridDirection::Left => (
            SIDEBAR_WIDTH + 18.0,
            (viewport_height - GRID_HINT_HEIGHT) * 0.5,
        ),
        GridDirection::Right => (
            viewport_width - GRID_HINT_WIDTH - 18.0,
            (viewport_height - GRID_HINT_HEIGHT) * 0.5,
        ),
        GridDirection::Up => (
            SIDEBAR_WIDTH + (viewport_width - SIDEBAR_WIDTH - GRID_HINT_WIDTH) * 0.5,
            18.0,
        ),
        GridDirection::Down => (
            SIDEBAR_WIDTH + (viewport_width - SIDEBAR_WIDTH - GRID_HINT_WIDTH) * 0.5,
            viewport_height - GRID_HINT_HEIGHT - 18.0,
        ),
    };

    div()
        .absolute()
        .left(px(left))
        .top(px(top))
        .w(px(GRID_HINT_WIDTH))
        .min_h(px(GRID_HINT_HEIGHT))
        .p_3()
        .flex()
        .flex_col()
        .gap_2()
        .bg(rgb(0x151c22))
        .border_1()
        .border_color(rgb(0x2a3742))
        .rounded_lg()
        .shadow_lg()
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, |_, _, cx| {
            cx.stop_propagation();
        })
        .on_mouse_up(MouseButton::Left, listener)
        .child(
            div()
                .flex()
                .justify_between()
                .items_center()
                .child(div().text_xs().text_color(accent).child(format!(
                    "{} {}",
                    direction.glyph(),
                    direction.label()
                )))
                .child(
                    div()
                        .px_2()
                        .py_1()
                        .rounded_full()
                        .bg(rgb(0x0e1419))
                        .text_xs()
                        .text_color(rgb(0x9da8b1))
                        .child(format!("{}", hint.count)),
                ),
        )
        .child(div().text_sm().child(pane.title.clone()))
        .child(
            div()
                .text_xs()
                .text_color(rgb(0x90a0aa))
                .child(format!("{} next", pane_kind_label(&pane.kind))),
        )
}

fn expose_layout_frame(
    panes: &[PaneRecord],
    viewport_width: f32,
    viewport_height: f32,
) -> Option<ExposeLayoutFrame> {
    let first = panes.first()?;
    let mut min_x = first.position.x;
    let mut min_y = first.position.y;
    let mut max_x = first.position.x + first.size.width;
    let mut max_y = first.position.y + first.size.height;

    for pane in panes.iter().skip(1) {
        min_x = min_x.min(pane.position.x);
        min_y = min_y.min(pane.position.y);
        max_x = max_x.max(pane.position.x + pane.size.width);
        max_y = max_y.max(pane.position.y + pane.size.height);
    }

    let available_width = (viewport_width - SIDEBAR_WIDTH - EXPOSE_MARGIN_X * 2.0).max(1.0);
    let available_height = (viewport_height - EXPOSE_MARGIN_TOP - EXPOSE_MARGIN_BOTTOM).max(1.0);
    let content_width = (max_x - min_x).max(1.0);
    let content_height = (max_y - min_y).max(1.0);
    let scale = (available_width / content_width)
        .min(available_height / content_height)
        .clamp(0.16, 0.58);
    let scaled_width = content_width * scale;
    let scaled_height = content_height * scale;

    Some(ExposeLayoutFrame {
        left: EXPOSE_MARGIN_X + (available_width - scaled_width).max(0.0) * 0.5,
        top: EXPOSE_MARGIN_TOP + (available_height - scaled_height).max(0.0) * 0.5,
        scale,
        min_x,
        min_y,
    })
}

fn expose_card(
    pane: &PaneRecord,
    layout: ExposeLayoutFrame,
    is_active: bool,
    subtitle: String,
    listener: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let accent = pane_accent(&pane.kind);
    let border = if is_active {
        accent
    } else {
        rgb(0x2b3842).into()
    };
    let background = if is_active {
        rgb(0x182028)
    } else {
        rgb(0x131a20)
    };
    let width = (pane.size.width * layout.scale).max(140.0);
    let height = (pane.size.height * layout.scale).max(92.0);
    let x = layout.left + (pane.position.x - layout.min_x) * layout.scale;
    let y = layout.top + (pane.position.y - layout.min_y) * layout.scale;

    div()
        .absolute()
        .left(px(x))
        .top(px(y))
        .w(px(width))
        .h(px(height))
        .p_3()
        .flex()
        .flex_col()
        .gap_2()
        .bg(background)
        .border_1()
        .border_color(border)
        .rounded_lg()
        .shadow_md()
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, |_, _, cx| {
            cx.stop_propagation();
        })
        .on_mouse_up(MouseButton::Left, listener)
        .child(
            div()
                .flex()
                .justify_between()
                .items_center()
                .child(
                    div()
                        .text_xs()
                        .text_color(accent)
                        .child(pane_kind_label(&pane.kind)),
                )
                .when(is_active, |row| {
                    row.child(
                        div()
                            .px_2()
                            .py_1()
                            .rounded_full()
                            .bg(rgb(0x0d1217))
                            .text_xs()
                            .text_color(rgb(0xf0d35f))
                            .child("Active"),
                    )
                }),
        )
        .child(div().text_sm().child(pane.title.clone()))
        .child(div().text_xs().text_color(rgb(0x90a0aa)).child(subtitle))
}

fn pane_center(pane: &PaneRecord) -> WorkdeskPoint {
    WorkdeskPoint::new(
        pane.position.x + pane.size.width * 0.5,
        pane.position.y + pane.size.height * 0.5,
    )
}

fn grid_direction_for_delta(dx: f32, dy: f32) -> Option<(GridDirection, f32, f32)> {
    if dx.abs() < GRID_DIRECTION_EPSILON && dy.abs() < GRID_DIRECTION_EPSILON {
        return None;
    }

    if dx.abs() >= dy.abs() {
        if dx <= -GRID_DIRECTION_EPSILON {
            Some((GridDirection::Left, dx.abs(), dy.abs()))
        } else if dx >= GRID_DIRECTION_EPSILON {
            Some((GridDirection::Right, dx.abs(), dy.abs()))
        } else {
            None
        }
    } else if dy <= -GRID_DIRECTION_EPSILON {
        Some((GridDirection::Up, dy.abs(), dx.abs()))
    } else if dy >= GRID_DIRECTION_EPSILON {
        Some((GridDirection::Down, dy.abs(), dx.abs()))
    } else {
        None
    }
}

fn update_grid_neighbor(
    neighbors: &mut GridNeighbors,
    direction: GridDirection,
    pane_id: PaneId,
    primary_distance: f32,
    secondary_distance: f32,
    best_score: &mut Option<(f32, f32, PaneId)>,
) {
    match direction {
        GridDirection::Left => neighbors.left_count += 1,
        GridDirection::Right => neighbors.right_count += 1,
        GridDirection::Up => neighbors.up_count += 1,
        GridDirection::Down => neighbors.down_count += 1,
    }

    let should_replace = best_score
        .map(|(best_primary, best_secondary, _)| {
            primary_distance < best_primary
                || ((primary_distance - best_primary).abs() < f32::EPSILON
                    && secondary_distance < best_secondary)
        })
        .unwrap_or(true);

    if should_replace {
        *best_score = Some((primary_distance, secondary_distance, pane_id));
        match direction {
            GridDirection::Left => neighbors.left = Some(pane_id),
            GridDirection::Right => neighbors.right = Some(pane_id),
            GridDirection::Up => neighbors.up = Some(pane_id),
            GridDirection::Down => neighbors.down = Some(pane_id),
        }
    }
}

fn grid_projection(panes: &[PaneRecord]) -> GridProjection {
    let mut projection = GridProjection::default();
    projection.order = panes.iter().map(|pane| pane.id).collect();
    projection.order.sort_by(|left, right| {
        let left_center = panes
            .iter()
            .find(|pane| pane.id == *left)
            .map(pane_center)
            .unwrap_or(WorkdeskPoint::new(0.0, 0.0));
        let right_center = panes
            .iter()
            .find(|pane| pane.id == *right)
            .map(pane_center)
            .unwrap_or(WorkdeskPoint::new(0.0, 0.0));

        left_center
            .y
            .partial_cmp(&right_center.y)
            .unwrap_or(Ordering::Equal)
            .then_with(|| {
                left_center
                    .x
                    .partial_cmp(&right_center.x)
                    .unwrap_or(Ordering::Equal)
            })
    });

    for pane in panes {
        let source = pane_center(pane);
        let mut neighbors = GridNeighbors::default();
        let mut best_left = None;
        let mut best_right = None;
        let mut best_up = None;
        let mut best_down = None;

        for candidate in panes {
            if candidate.id == pane.id {
                continue;
            }

            let target = pane_center(candidate);
            let dx = target.x - source.x;
            let dy = target.y - source.y;
            let Some((direction, primary_distance, secondary_distance)) =
                grid_direction_for_delta(dx, dy)
            else {
                continue;
            };

            match direction {
                GridDirection::Left => update_grid_neighbor(
                    &mut neighbors,
                    direction,
                    candidate.id,
                    primary_distance,
                    secondary_distance,
                    &mut best_left,
                ),
                GridDirection::Right => update_grid_neighbor(
                    &mut neighbors,
                    direction,
                    candidate.id,
                    primary_distance,
                    secondary_distance,
                    &mut best_right,
                ),
                GridDirection::Up => update_grid_neighbor(
                    &mut neighbors,
                    direction,
                    candidate.id,
                    primary_distance,
                    secondary_distance,
                    &mut best_up,
                ),
                GridDirection::Down => update_grid_neighbor(
                    &mut neighbors,
                    direction,
                    candidate.id,
                    primary_distance,
                    secondary_distance,
                    &mut best_down,
                ),
            }
        }

        projection.neighbors.insert(pane.id, neighbors);
    }

    projection
}

fn split_layout_frames(
    panes: &[PaneRecord],
    _active_pane: Option<PaneId>,
    viewport_width: f32,
    viewport_height: f32,
) -> Vec<(PaneId, PaneViewportFrame)> {
    if panes.is_empty() {
        return Vec::new();
    }

    let mut order = panes.iter().map(|pane| pane.id).collect::<Vec<_>>();
    order.sort_by(|left, right| {
        let left_pane = panes.iter().find(|pane| pane.id == *left);
        let right_pane = panes.iter().find(|pane| pane.id == *right);

        match (left_pane, right_pane) {
            (Some(left_pane), Some(right_pane)) => left_pane
                .position
                .y
                .partial_cmp(&right_pane.position.y)
                .unwrap_or(Ordering::Equal)
                .then_with(|| {
                    left_pane
                        .position
                        .x
                        .partial_cmp(&right_pane.position.x)
                        .unwrap_or(Ordering::Equal)
                }),
            _ => Ordering::Equal,
        }
    });

    let rect = SplitRect {
        x: SIDEBAR_WIDTH + SPLIT_MARGIN_X,
        y: SPLIT_MARGIN_TOP,
        width: (viewport_width - SIDEBAR_WIDTH - SPLIT_MARGIN_X * 2.0).max(MIN_PANE_WIDTH),
        height: (viewport_height - SPLIT_MARGIN_TOP - SPLIT_MARGIN_BOTTOM).max(MIN_PANE_HEIGHT),
    };

    let panes_by_id = panes
        .iter()
        .map(|pane| (pane.id, pane))
        .collect::<HashMap<_, _>>();
    let mut frames = HashMap::new();
    assign_split_frames(&order, &panes_by_id, rect, &mut frames);

    order
        .into_iter()
        .filter_map(|pane_id| frames.remove(&pane_id).map(|frame| (pane_id, frame)))
        .collect()
}

fn assign_split_frames(
    order: &[PaneId],
    panes_by_id: &HashMap<PaneId, &PaneRecord>,
    rect: SplitRect,
    frames: &mut HashMap<PaneId, PaneViewportFrame>,
) {
    if order.is_empty() || rect.width <= 0.0 || rect.height <= 0.0 {
        return;
    }

    let pane_id = order[0];
    let Some(pane) = panes_by_id.get(&pane_id).copied() else {
        return;
    };

    if order.len() == 1 {
        frames.insert(pane_id, PaneViewportFrame::for_split(pane, rect));
        return;
    }

    let split_vertical = rect.width >= rect.height;
    let axis_len = if split_vertical {
        rect.width
    } else {
        rect.height
    };
    let min_lead = if split_vertical {
        MIN_PANE_WIDTH.min(axis_len * 0.45)
    } else {
        MIN_PANE_HEIGHT.min(axis_len * 0.45)
    };
    let lead = if axis_len <= min_lead * 2.0 + SPLIT_GAP {
        (axis_len - SPLIT_GAP).max(1.0) * 0.5
    } else {
        (axis_len * if order.len() == 2 { 0.5 } else { 0.58 })
            .clamp(min_lead, axis_len - min_lead - SPLIT_GAP)
    };

    let (lead_rect, rest_rect) = if split_vertical {
        (
            SplitRect {
                x: rect.x,
                y: rect.y,
                width: lead,
                height: rect.height,
            },
            SplitRect {
                x: rect.x + lead + SPLIT_GAP,
                y: rect.y,
                width: rect.width - lead - SPLIT_GAP,
                height: rect.height,
            },
        )
    } else {
        (
            SplitRect {
                x: rect.x,
                y: rect.y,
                width: rect.width,
                height: lead,
            },
            SplitRect {
                x: rect.x,
                y: rect.y + lead + SPLIT_GAP,
                width: rect.width,
                height: rect.height - lead - SPLIT_GAP,
            },
        )
    };

    frames.insert(pane_id, PaneViewportFrame::for_split(pane, lead_rect));
    assign_split_frames(&order[1..], panes_by_id, rest_rect, frames);
}

fn directional_projection_for_frames(frames: &[(PaneId, PaneViewportFrame)]) -> GridProjection {
    let mut projection = GridProjection {
        neighbors: HashMap::new(),
        order: frames.iter().map(|(pane_id, _)| *pane_id).collect(),
    };

    for (pane_id, frame) in frames {
        let source_x = frame.x + frame.width * 0.5;
        let source_y = frame.y + frame.height * 0.5;
        let mut neighbors = GridNeighbors::default();
        let mut best_left = None;
        let mut best_right = None;
        let mut best_up = None;
        let mut best_down = None;

        for (candidate_id, candidate_frame) in frames {
            if candidate_id == pane_id {
                continue;
            }

            let dx = candidate_frame.x + candidate_frame.width * 0.5 - source_x;
            let dy = candidate_frame.y + candidate_frame.height * 0.5 - source_y;
            let Some((direction, primary_distance, secondary_distance)) =
                grid_direction_for_delta(dx, dy)
            else {
                continue;
            };

            match direction {
                GridDirection::Left => update_grid_neighbor(
                    &mut neighbors,
                    direction,
                    *candidate_id,
                    primary_distance,
                    secondary_distance,
                    &mut best_left,
                ),
                GridDirection::Right => update_grid_neighbor(
                    &mut neighbors,
                    direction,
                    *candidate_id,
                    primary_distance,
                    secondary_distance,
                    &mut best_right,
                ),
                GridDirection::Up => update_grid_neighbor(
                    &mut neighbors,
                    direction,
                    *candidate_id,
                    primary_distance,
                    secondary_distance,
                    &mut best_up,
                ),
                GridDirection::Down => update_grid_neighbor(
                    &mut neighbors,
                    direction,
                    *candidate_id,
                    primary_distance,
                    secondary_distance,
                    &mut best_down,
                ),
            }
        }

        projection.neighbors.insert(*pane_id, neighbors);
    }

    projection
}

fn terminal_footer_label(snapshot: &Option<TerminalSnapshot>) -> String {
    let Some(snapshot) = snapshot else {
        return "offline".to_string();
    };

    let mode = if snapshot.alternate_screen {
        "alt"
    } else {
        "main"
    };
    let status = snapshot
        .status
        .clone()
        .unwrap_or_else(|| "running".to_string());

    if snapshot.scrollbar.total > snapshot.scrollbar.visible && snapshot.scrollbar.offset > 0 {
        format!(
            "{mode} {}x{} scroll {}/{} {status}",
            snapshot.cols, snapshot.rows_count, snapshot.scrollbar.offset, snapshot.scrollbar.total
        )
    } else {
        format!("{mode} {}x{} {status}", snapshot.cols, snapshot.rows_count)
    }
}

fn terminal_body(
    snapshot: &Option<TerminalSnapshot>,
    terminal_view: &TerminalViewState,
    zoom: f32,
    is_active: bool,
    cursor_blink_visible: bool,
) -> impl IntoElement {
    let font_size = 12.0 * zoom.clamp(0.8, 1.2);
    let line_height = 15.0 * zoom.clamp(0.85, 1.2);

    match snapshot {
        Some(snapshot) => {
            let rows = snapshot
                .rows
                .iter()
                .enumerate()
                .map(|(row_index, row)| {
                    div()
                        .h(px(line_height))
                        .whitespace_nowrap()
                        .child(terminal_row_display(
                            snapshot,
                            terminal_view.selection,
                            row_index,
                            row,
                            is_active,
                            cursor_blink_visible,
                        ))
                })
                .collect::<Vec<_>>();

            div()
                .flex_1()
                .overflow_hidden()
                .p_3()
                .bg(terminal_color_hsla(snapshot.theme.background))
                .border_1()
                .border_color(rgb(0x25303a))
                .rounded_md()
                .font_family(".ZedMono")
                .text_size(px(font_size))
                .line_height(px(line_height))
                .text_color(terminal_color_hsla(snapshot.theme.foreground))
                .children(rows)
        }
        None => div()
            .flex_1()
            .overflow_hidden()
            .p_3()
            .bg(rgb(0x0d1217))
            .border_1()
            .border_color(rgb(0x25303a))
            .rounded_md()
            .font_family(".ZedMono")
            .text_size(px(font_size))
            .line_height(px(line_height))
            .text_color(rgb(0xffb7a6))
            .child("terminal offline"),
    }
}

fn terminal_row_display(
    snapshot: &TerminalSnapshot,
    selection: Option<TerminalSelection>,
    row_index: usize,
    row: &TerminalRow,
    is_active: bool,
    cursor_blink_visible: bool,
) -> StyledText {
    StyledText::new(terminal_row_text(row)).with_runs(terminal_row_runs(
        snapshot,
        selection,
        row_index,
        row,
        is_active,
        cursor_blink_visible,
    ))
}

fn terminal_row_text(row: &TerminalRow) -> String {
    row.runs
        .iter()
        .map(|run| run.text.as_str())
        .collect::<String>()
}

fn terminal_row_runs(
    snapshot: &TerminalSnapshot,
    selection: Option<TerminalSelection>,
    row_index: usize,
    row: &TerminalRow,
    is_active: bool,
    cursor_blink_visible: bool,
) -> Vec<TextRun> {
    let cursor_row = usize::from(snapshot.cursor.0);
    let cursor_col = usize::from(snapshot.cursor.1);
    let show_cursor = !snapshot.closed
        && is_active
        && row_index == cursor_row
        && (cursor_blink_visible || !snapshot.cursor_blinking);

    let mut runs: Vec<TextRun> = Vec::new();
    let mut cell_index = 0usize;

    for run in &row.runs {
        for ch in run.text.chars() {
            let cell = TerminalCell {
                row: row_index,
                col: cell_index,
            };
            let mut style = run.style;

            if selection.is_some_and(|selection| selection.contains(cell)) {
                style.background = Some(terminal_color_from_hex(TERMINAL_SELECTION_BG));
                style.foreground = terminal_color_from_hex(TERMINAL_SELECTION_FG);
            }

            if show_cursor && cursor_col == cell_index {
                style.background = Some(snapshot.theme.cursor);
                style.foreground = snapshot.theme.background;
            }

            let text = ch.to_string();
            if let Some(last) = runs.last_mut() {
                if last.font == terminal_font(style)
                    && last.color == terminal_text_color_from_style(style)
                    && last.background_color == style.background.map(terminal_color_hsla)
                    && last.underline
                        == style.underline.then(|| UnderlineStyle {
                            thickness: px(1.0),
                            color: style.underline_color.map(terminal_color_hsla),
                            wavy: false,
                        })
                    && last.strikethrough
                        == style.strikethrough.then(|| StrikethroughStyle {
                            thickness: px(1.0),
                            color: Some(terminal_text_color_from_style(style)),
                        })
                {
                    last.len += text.len();
                } else {
                    runs.push(text_run_from_style(style, text.len()));
                }
            } else {
                runs.push(text_run_from_style(style, text.len()));
            }

            cell_index += 1;
        }
    }

    runs
}

fn text_run_from_style(style: canvas_terminal::TerminalTextStyle, len: usize) -> TextRun {
    TextRun {
        len,
        font: terminal_font(style),
        color: terminal_text_color_from_style(style),
        background_color: style.background.map(terminal_color_hsla),
        underline: style.underline.then(|| UnderlineStyle {
            thickness: px(1.0),
            color: style.underline_color.map(terminal_color_hsla),
            wavy: false,
        }),
        strikethrough: style.strikethrough.then(|| StrikethroughStyle {
            thickness: px(1.0),
            color: Some(terminal_text_color_from_style(style)),
        }),
    }
}

fn terminal_font(style: canvas_terminal::TerminalTextStyle) -> gpui::Font {
    let mut font = font(".ZedMono");
    if style.bold {
        font.weight = FontWeight::BOLD;
    }
    if style.italic {
        font.style = FontStyle::Italic;
    }
    font
}

fn terminal_text_color_from_style(style: canvas_terminal::TerminalTextStyle) -> gpui::Hsla {
    let mut color = terminal_color_hsla(style.foreground);
    if style.faint {
        color.a *= 0.65;
    }
    color
}

fn terminal_color_hsla(color: TerminalColor) -> gpui::Hsla {
    rgb(((color.r as u32) << 16) | ((color.g as u32) << 8) | (color.b as u32)).into()
}

fn terminal_color_from_hex(hex: u32) -> TerminalColor {
    TerminalColor {
        r: ((hex >> 16) & 0xff) as u8,
        g: ((hex >> 8) & 0xff) as u8,
        b: (hex & 0xff) as u8,
    }
}

fn terminal_frame_metrics(
    zoom: f32,
    screen_x: f32,
    screen_y: f32,
    snapshot: &TerminalSnapshot,
) -> TerminalFrameMetrics {
    let zoom_scale = zoom.clamp(0.8, 1.2);
    let line_height = 15.0 * zoom.clamp(0.85, 1.2);
    let header_height = 42.0 * zoom.clamp(0.75, 1.4);
    let pane_padding = 16.0 * zoom.clamp(0.8, 1.35);
    let body_origin_x = screen_x + pane_padding + TERMINAL_BODY_INSET;
    let body_origin_y = screen_y + header_height + pane_padding + TERMINAL_BODY_INSET;

    TerminalFrameMetrics {
        body_origin: gpui::point(px(body_origin_x), px(body_origin_y)),
        cell_width: (9.0 * zoom_scale).max(1.0),
        cell_height: line_height.max(1.0),
        cols: snapshot.cols,
        rows: snapshot.rows_count,
    }
}

fn can_bind_shortcut_keystroke(keystroke: &Keystroke) -> bool {
    if keystroke.key.is_empty() {
        return false;
    }

    if matches!(
        keystroke.key.as_str(),
        "shift" | "control" | "alt" | "platform" | "function"
    ) {
        return false;
    }

    keystroke.modifiers.modified() || is_non_text_shortcut_key(keystroke.key.as_str())
}

fn is_non_text_shortcut_key(key: &str) -> bool {
    matches!(
        key,
        "escape"
            | "tab"
            | "enter"
            | "backspace"
            | "delete"
            | "home"
            | "end"
            | "pageup"
            | "pagedown"
            | "left"
            | "right"
            | "up"
            | "down"
    ) || is_function_key(key)
}

fn is_function_key(key: &str) -> bool {
    key.strip_prefix('f')
        .is_some_and(|suffix| !suffix.is_empty() && suffix.chars().all(|ch| ch.is_ascii_digit()))
}

fn terminal_selection_text(
    snapshot: &TerminalSnapshot,
    selection: TerminalSelection,
) -> Option<String> {
    let (start, end) = selection.ordered();
    if snapshot.rows.is_empty() || start.row >= snapshot.rows.len() {
        return None;
    }

    let last_row = end.row.min(snapshot.rows.len().saturating_sub(1));
    let max_col = usize::from(snapshot.cols.saturating_sub(1));
    let mut lines = Vec::new();

    for row_index in start.row..=last_row {
        let row = snapshot.rows.get(row_index)?;
        let plain = terminal_row_plain_text(row);
        let chars = plain.chars().collect::<Vec<_>>();
        let row_start = if row_index == start.row { start.col } else { 0 };
        let row_end = if row_index == last_row {
            end.col
        } else {
            max_col
        };
        let mut line = String::new();

        for col in row_start..=row_end.min(chars.len().saturating_sub(1)) {
            line.push(chars[col]);
        }

        while line.ends_with(' ') {
            line.pop();
        }

        lines.push(line);
    }

    Some(lines.join("\n"))
}

fn terminal_row_plain_text(row: &TerminalRow) -> String {
    terminal_row_text(row).replace('\u{00A0}', " ")
}

fn terminal_input_bytes(event: &KeyDownEvent, snapshot: &TerminalSnapshot) -> Option<Vec<u8>> {
    let keystroke = &event.keystroke;

    if keystroke.modifiers.platform || keystroke.modifiers.function {
        return None;
    }

    if keystroke.modifiers.control && !keystroke.modifiers.alt {
        return control_input_bytes(keystroke.key.as_str());
    }

    let mut bytes = match keystroke.key.as_str() {
        "enter" => vec![b'\r'],
        "tab" => {
            if keystroke.modifiers.shift {
                b"\x1b[Z".to_vec()
            } else {
                vec![b'\t']
            }
        }
        "backspace" => vec![0x7f],
        "escape" => vec![0x1b],
        "up" => arrow_bytes(snapshot.application_cursor, b'A'),
        "down" => arrow_bytes(snapshot.application_cursor, b'B'),
        "right" => arrow_bytes(snapshot.application_cursor, b'C'),
        "left" => arrow_bytes(snapshot.application_cursor, b'D'),
        "home" => b"\x1b[H".to_vec(),
        "end" => b"\x1b[F".to_vec(),
        "pageup" => b"\x1b[5~".to_vec(),
        "pagedown" => b"\x1b[6~".to_vec(),
        "delete" => b"\x1b[3~".to_vec(),
        "space" => vec![b' '],
        _ => {
            let text = keystroke
                .key_char
                .clone()
                .or_else(|| (!keystroke.modifiers.modified()).then(|| keystroke.key.clone()))?;
            text.into_bytes()
        }
    };

    if keystroke.modifiers.alt {
        bytes.insert(0, 0x1b);
    }

    Some(bytes)
}

fn arrow_bytes(application_cursor: bool, suffix: u8) -> Vec<u8> {
    if application_cursor {
        vec![0x1b, b'O', suffix]
    } else {
        vec![0x1b, b'[', suffix]
    }
}

fn control_input_bytes(key: &str) -> Option<Vec<u8>> {
    match key {
        "space" => Some(vec![0x00]),
        "enter" => Some(vec![0x0a]),
        "backspace" => Some(vec![0x08]),
        "[" => Some(vec![0x1b]),
        "\\" => Some(vec![0x1c]),
        "]" => Some(vec![0x1d]),
        "^" => Some(vec![0x1e]),
        "_" => Some(vec![0x1f]),
        key if key.len() == 1 => Some(vec![key.as_bytes()[0].to_ascii_lowercase() & 0x1f]),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persisted_workdesk_round_trips_layout_state() {
        let mut state = WorkdeskState::new(
            "Desk",
            "Summary",
            vec![
                PaneRecord {
                    id: PaneId::new(7),
                    title: "Shell".to_string(),
                    kind: PaneKind::Shell,
                    position: WorkdeskPoint::new(48.0, 64.0),
                    size: WorkdeskSize::new(920.0, 560.0),
                },
                PaneRecord {
                    id: PaneId::new(8),
                    title: "Agent".to_string(),
                    kind: PaneKind::Agent,
                    position: WorkdeskPoint::new(1024.0, 120.0),
                    size: WorkdeskSize::new(720.0, 420.0),
                },
            ],
        );
        state.layout_mode = LayoutMode::ClassicSplit;
        state.camera = WorkdeskPoint::new(-180.0, 32.0);
        state.zoom = 1.24;
        state.active_pane = Some(PaneId::new(8));

        let restored = PersistedWorkdesk::from_state(&state).into_state();

        assert_eq!(restored.name, state.name);
        assert_eq!(restored.summary, state.summary);
        assert_eq!(restored.layout_mode, LayoutMode::ClassicSplit);
        assert_eq!(restored.camera, state.camera);
        assert_eq!(restored.zoom, state.zoom);
        assert_eq!(restored.active_pane, state.active_pane);
        assert_eq!(restored.panes.len(), 2);
        assert_eq!(restored.panes[0].id.raw(), 7);
        assert_eq!(restored.panes[1].kind, PaneKind::Agent);
    }

    #[test]
    fn shortcut_reassignment_clears_previous_owner() {
        let mut shortcuts = ShortcutMap::default();
        let binding = ShortcutBinding::parse("cmd-shift-n").unwrap();

        let displaced = shortcuts.set_binding(ShortcutAction::SpawnAgentPane, binding);

        assert_eq!(displaced, Some(ShortcutAction::SpawnShellPane));
        assert!(shortcuts.binding(ShortcutAction::SpawnShellPane).is_none());
        assert_eq!(
            shortcuts
                .binding(ShortcutAction::SpawnAgentPane)
                .unwrap()
                .serialized(),
            "cmd-shift-n"
        );
    }

    #[test]
    fn persisted_shortcuts_can_clear_and_override_bindings() {
        let mut bindings = BTreeMap::new();
        bindings.insert("spawn-shell-pane".to_string(), None);
        bindings.insert("zoom-in".to_string(), Some("cmd-shift-=".to_string()));

        let (shortcuts, warnings) =
            ShortcutMap::from_persisted(PersistedShortcutConfig { bindings });

        assert!(warnings.is_empty());
        assert!(shortcuts.binding(ShortcutAction::SpawnShellPane).is_none());
        assert_eq!(
            shortcuts
                .binding(ShortcutAction::ZoomIn)
                .unwrap()
                .serialized(),
            "cmd-shift-="
        );
    }

    #[test]
    fn shortcut_capture_requires_modifiers_for_plain_text() {
        assert!(!can_bind_shortcut_keystroke(
            &Keystroke::parse("a").unwrap()
        ));
        assert!(can_bind_shortcut_keystroke(
            &Keystroke::parse("cmd-a").unwrap()
        ));
        assert!(can_bind_shortcut_keystroke(
            &Keystroke::parse("escape").unwrap()
        ));
    }

    #[test]
    fn persisted_empty_workdesk_round_trips() {
        let restored =
            PersistedWorkdesk::from_state(&blank_workdesk("Desk", "Summary")).into_state();

        assert_eq!(restored.name, "Desk");
        assert_eq!(restored.summary, "Summary");
        assert!(restored.panes.is_empty());
        assert_eq!(restored.active_pane, None);
    }

    #[test]
    fn initial_workdesks_start_with_single_blank_desk() {
        let workdesks = initial_workdesks();

        assert_eq!(workdesks.len(), 1);
        assert_eq!(workdesks[0].name, "Workdesk 1");
        assert_eq!(workdesks[0].summary, DEFAULT_WORKDESK_SUMMARY);
        assert!(workdesks[0].panes.is_empty());
    }
}
