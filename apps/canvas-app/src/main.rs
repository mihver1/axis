use canvas_core::{
    PaneId, PaneKind, PaneRecord, Point as WorkdeskPoint, Size as WorkdeskSize, SurfaceId,
    SurfaceKind, SurfaceRecord,
};
use canvas_editor::{EditorBuffer, HighlightKind};
use canvas_terminal::{
    ghostty_build_info, spawn_terminal_session_with_grid, TerminalColor, TerminalGridSize,
    TerminalRow, TerminalSession, TerminalSnapshot,
};
use gpui::{
    div, font, prelude::*, px, relative, rgb, rgba, size, App, Application, Bounds, ClipboardItem,
    Context, Element, ElementId, ElementInputHandler, EntityInputHandler, FocusHandle, FontStyle,
    FontWeight, GlobalElementId, KeyDownEvent, KeybindingKeystroke, Keystroke, LayoutId,
    MagnifyGestureEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels,
    Point as GpuiPoint, ScrollWheelEvent, SharedString, SmartMagnifyGestureEvent, Style,
    StyledText, SwipeGestureEvent, TextRun, Timer, TitlebarOptions, TouchEvent, TouchPhase,
    UTF16Selection, Window, WindowBounds, WindowOptions,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    cmp::Ordering,
    collections::{BTreeMap, HashMap},
    fs,
    io::{BufRead, BufReader, Write},
    ops::Range,
    os::unix::{fs::PermissionsExt, net::UnixListener},
    path::PathBuf,
    process::Command,
    sync::mpsc::{self, Receiver, Sender},
    thread,
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
const SIDEBAR_WIDTH: f32 = 216.0;
const SIDEBAR_COLLAPSED_WIDTH: f32 = 72.0;
const WORKDESK_MENU_WIDTH: f32 = 208.0;
const CURSOR_BLINK_INTERVAL: Duration = Duration::from_millis(530);
const TERMINAL_SELECTION_BG: u32 = 0x2d5b88;
const TERMINAL_SELECTION_FG: u32 = 0xf4f8fb;
const TERMINAL_BODY_INSET: f32 = 12.0;
const TERMINAL_FONT_SIZE: f32 = 13.0;
const TERMINAL_CELL_WIDTH: f32 = 7.8;
const TERMINAL_CELL_HEIGHT: f32 = 18.0;
const SESSION_SAVE_DEBOUNCE: Duration = Duration::from_millis(240);
const GRID_ACTIVE_MARGIN_X: f32 = 56.0;
const GRID_ACTIVE_MARGIN_TOP: f32 = 34.0;
const GRID_ACTIVE_MARGIN_BOTTOM: f32 = 62.0;
const GRID_HINT_WIDTH: f32 = 160.0;
const GRID_HINT_HEIGHT: f32 = 80.0;
const GRID_DIRECTION_EPSILON: f32 = 24.0;
const EXPOSE_MARGIN_X: f32 = 42.0;
const EXPOSE_MARGIN_TOP: f32 = 48.0;
const EXPOSE_MARGIN_BOTTOM: f32 = 88.0;
const SPLIT_MARGIN_X: f32 = 18.0;
const SPLIT_MARGIN_TOP: f32 = 18.0;
const SPLIT_MARGIN_BOTTOM: f32 = 62.0;
const SPLIT_GAP: f32 = 14.0;
const SHORTCUT_PANEL_WIDTH: f32 = 540.0;
const SHORTCUT_PANEL_MARGIN: f32 = 20.0;
const FLOATING_DOCK_MARGIN: f32 = 16.0;
const INSPECTOR_PANEL_WIDTH: f32 = 360.0;
const WORKDESK_EDITOR_WIDTH: f32 = 436.0;
const SIDEBAR_WINDOW_CONTROLS_INSET: f32 = 34.0;
const NOTIFICATION_PANEL_WIDTH: f32 = 264.0;

#[cfg(target_os = "macos")]
const TERMINAL_FONT_FAMILY: &str = "Menlo";
#[cfg(not(target_os = "macos"))]
const TERMINAL_FONT_FAMILY: &str = ".ZedMono";

#[derive(Clone)]
struct WorkdeskState {
    name: String,
    summary: String,
    metadata: WorkdeskMetadata,
    panes: Vec<PaneRecord>,
    pane_attention: HashMap<PaneId, PaneAttention>,
    terminals: HashMap<SurfaceId, TerminalSession>,
    terminal_revisions: HashMap<SurfaceId, u64>,
    terminal_statuses: HashMap<SurfaceId, Option<String>>,
    terminal_views: HashMap<SurfaceId, TerminalViewState>,
    terminal_grids: HashMap<SurfaceId, TerminalGridSize>,
    editors: HashMap<SurfaceId, EditorBuffer>,
    editor_views: HashMap<SurfaceId, EditorViewState>,
    next_pane_serial: u64,
    next_surface_serial: u64,
    attention_sequence: u64,
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
    workdesk_editor: Option<WorkdeskEditorState>,
    automation_rx: Receiver<AutomationEnvelope>,
    focus_handle: FocusHandle,
    automation_socket_path: SharedString,
    ghostty_vendor_dir: SharedString,
    ghostty_status: SharedString,
    shortcuts: ShortcutMap,
    shortcut_editor: ShortcutEditorState,
    inspector_open: bool,
    sidebar_collapsed: bool,
    notifications_open: bool,
    mock_notifications_unread: usize,
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

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct WorkdeskMetadata {
    #[serde(default)]
    intent: String,
    #[serde(default)]
    cwd: String,
    #[serde(default)]
    branch: String,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    progress: Option<WorkdeskProgress>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct WorkdeskProgress {
    label: String,
    value: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WorkdeskTemplate {
    ShellDesk,
    AgentReview,
    Debug,
    Implementation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WorkdeskEditorField {
    Name,
    Intent,
    Summary,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WorkdeskEditorMode {
    Create,
    Edit(usize),
}

#[derive(Clone, Debug)]
struct WorkdeskDraft {
    name: String,
    summary: String,
    metadata: WorkdeskMetadata,
}

#[derive(Clone, Debug)]
struct WorkdeskEditorState {
    mode: WorkdeskEditorMode,
    template: WorkdeskTemplate,
    active_field: WorkdeskEditorField,
    draft: WorkdeskDraft,
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
    SelectingEditor {
        pane_id: PaneId,
        surface_id: SurfaceId,
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum AttentionState {
    #[default]
    Idle,
    Working,
    Waiting,
    Error,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
struct PaneAttention {
    #[serde(default)]
    state: AttentionState,
    #[serde(default)]
    unread: bool,
    #[serde(default)]
    last_attention_sequence: u64,
    #[serde(default)]
    last_activity_sequence: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct WorkdeskAttentionSummary {
    highest: AttentionState,
    unread_count: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum ShortcutAction {
    ToggleShortcutPanel,
    ToggleInspector,
    NextAttention,
    ClearActiveAttention,
    SpawnShellPane,
    SpawnAgentPane,
    SpawnBrowserPane,
    SpawnEditorPane,
    NextSurface,
    PreviousSurface,
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
struct TerminalTextMetrics {
    font_size: f32,
    line_height: f32,
    cell_width: f32,
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
    #[serde(default)]
    metadata: WorkdeskMetadata,
    panes: Vec<PersistedPane>,
    #[serde(default)]
    attention_sequence: u64,
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
    #[serde(default)]
    active_surface_id: Option<u64>,
    #[serde(default)]
    surfaces: Vec<PersistedSurface>,
    #[serde(default)]
    stack_title: Option<String>,
    #[serde(default)]
    attention: PaneAttention,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PersistedSurface {
    id: u64,
    title: String,
    kind: PersistedPaneKind,
    #[serde(default)]
    browser_url: Option<String>,
    #[serde(default)]
    editor_file_path: Option<String>,
    #[serde(default)]
    dirty: bool,
    #[serde(default)]
    editor_buffer_text: Option<String>,
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
    Browser,
    Editor,
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

struct AutomationServer {
    receiver: Receiver<AutomationEnvelope>,
    socket_path: PathBuf,
}

struct AutomationEnvelope {
    request: AutomationRequest,
    response_tx: Sender<AutomationResponse>,
}

#[derive(Clone, Debug, Deserialize)]
struct AutomationRequest {
    #[serde(default)]
    id: Option<Value>,
    method: String,
    #[serde(default = "empty_json_value")]
    params: Value,
}

#[derive(Clone, Debug, Serialize)]
struct AutomationResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<Value>,
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

fn empty_json_value() -> Value {
    Value::Object(Default::default())
}

#[derive(Clone, Default)]
struct TerminalViewState {
    selection: Option<TerminalSelection>,
}

#[derive(Clone, Default)]
struct EditorViewState {
    text_bounds: Option<Bounds<Pixels>>,
    line_height: f32,
    char_width: f32,
    gutter_width: f32,
    viewport_lines: usize,
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

impl EditorViewState {
    fn offset_for_point(
        &self,
        editor: &EditorBuffer,
        position: GpuiPoint<Pixels>,
    ) -> Option<usize> {
        let bounds = self.text_bounds?;
        let local = bounds.localize(&position)?;
        let line = editor
            .scroll_top_line()
            .saturating_add(
                (f32::from(local.y) / self.line_height.max(1.0))
                    .floor()
                    .max(0.0) as usize,
            )
            .min(editor.line_count().saturating_sub(1));
        let column = ((f32::from(local.x) / self.char_width.max(1.0))
            .round()
            .max(0.0)) as usize;
        Some(editor.offset_for_line_col(line, column))
    }

    fn bounds_for_range(
        &self,
        editor: &EditorBuffer,
        range: Range<usize>,
    ) -> Option<Bounds<Pixels>> {
        let bounds = self.text_bounds?;
        let start = range.start.min(editor.text().len());
        let (line, column) = editor.line_col_for_offset(start);
        let visible_line = line.saturating_sub(editor.scroll_top_line());
        let origin = gpui::point(
            bounds.left() + px(column as f32 * self.char_width),
            bounds.top() + px(visible_line as f32 * self.line_height),
        );
        Some(Bounds::new(
            origin,
            gpui::size(px(self.char_width.max(2.0)), px(self.line_height.max(2.0))),
        ))
    }
}

struct EditorInputOverlay {
    shell: gpui::Entity<CanvasShell>,
    active: bool,
    surface_id: SurfaceId,
    line_height: f32,
    char_width: f32,
    viewport_lines: usize,
}

impl IntoElement for EditorInputOverlay {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for EditorInputOverlay {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Self::PrepaintState {
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let surface_id = self.surface_id;
        let line_height = self.line_height;
        let char_width = self.char_width;
        let viewport_lines = self.viewport_lines;
        self.shell.update(cx, |shell, _cx| {
            shell
                .active_workdesk_mut()
                .editor_views
                .entry(surface_id)
                .or_default()
                .text_bounds = Some(bounds);
            if let Some(view) = shell
                .active_workdesk_mut()
                .editor_views
                .get_mut(&surface_id)
            {
                view.line_height = line_height;
                view.char_width = char_width;
                view.gutter_width = 0.0;
                view.viewport_lines = viewport_lines.max(1);
            }
        });
        if !self.active {
            return;
        }
        let focus_handle = self.shell.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.shell.clone()),
            cx,
        );
    }
}

impl EntityInputHandler for CanvasShell {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let editor = self.active_editor()?;
        let range = editor.range_from_utf16(&range_utf16);
        adjusted_range.replace(editor.range_to_utf16(&range));
        Some(editor.text().get(range)?.to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let editor = self.active_editor()?;
        Some(UTF16Selection {
            range: editor.range_to_utf16(&editor.selection().range),
            reversed: editor.selection().reversed,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        let editor = self.active_editor()?;
        editor
            .marked_range()
            .map(|range| editor.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some((pane_id, surface_id)) = self.active_editor_ids() else {
            return;
        };
        {
            let Some(editor) = self.active_editor_mut() else {
                return;
            };
            let Some(marked_range) = editor.marked_range().cloned() else {
                return;
            };
            let marked_text = editor
                .text()
                .get(marked_range.clone())
                .unwrap_or_default()
                .to_string();
            let _ = editor.replace_text_in_range_utf16(
                Some(editor.range_to_utf16(&marked_range)),
                &marked_text,
            );
        }
        self.sync_editor_surface_metadata(pane_id, surface_id);
        self.request_persist(cx);
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((pane_id, surface_id)) = self.active_editor_ids() else {
            return;
        };
        let changed = self
            .active_editor_mut()
            .is_some_and(|editor| editor.replace_text_in_range_utf16(range_utf16, text));
        if changed {
            self.sync_editor_surface_metadata(pane_id, surface_id);
            self.active_workdesk_mut().note_pane_activity(pane_id);
            self.cursor_blink_visible = true;
            self.last_cursor_blink_at = Instant::now();
            self.request_persist(cx);
            cx.notify();
        }
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((pane_id, surface_id)) = self.active_editor_ids() else {
            return;
        };
        let changed = self.active_editor_mut().is_some_and(|editor| {
            editor.replace_and_mark_text_in_range_utf16(
                range_utf16,
                new_text,
                new_selected_range_utf16,
            )
        });
        if changed {
            self.sync_editor_surface_metadata(pane_id, surface_id);
            self.active_workdesk_mut().note_pane_activity(pane_id);
            self.cursor_blink_visible = true;
            self.last_cursor_blink_at = Instant::now();
            self.request_persist(cx);
            cx.notify();
        }
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        _element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let editor = self.active_editor()?;
        let view = self.active_editor_view()?;
        let range = editor.range_from_utf16(&range_utf16);
        view.bounds_for_range(editor, range)
    }

    fn character_index_for_point(
        &mut self,
        point: gpui::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let editor = self.active_editor()?;
        let view = self.active_editor_view()?;
        view.offset_for_point(editor, point)
            .map(|offset| editor.offset_to_utf16(offset))
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

impl AttentionState {
    fn from_api_name(value: &str) -> Option<Self> {
        match value {
            "idle" => Some(Self::Idle),
            "working" => Some(Self::Working),
            "waiting" => Some(Self::Waiting),
            "error" => Some(Self::Error),
            _ => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Idle => "Idle",
            Self::Working => "Working",
            Self::Waiting => "Waiting",
            Self::Error => "Error",
        }
    }

    fn is_attention(self) -> bool {
        matches!(self, Self::Waiting | Self::Error)
    }

    fn tint(self) -> gpui::Hsla {
        match self {
            Self::Idle => rgb(0x5e6c76).into(),
            Self::Working => rgb(0x7cc7ff).into(),
            Self::Waiting => rgb(0xf0d35f).into(),
            Self::Error => rgb(0xff9b88).into(),
        }
    }

    fn priority(self) -> u8 {
        match self {
            Self::Idle => 0,
            Self::Working => 1,
            Self::Waiting => 2,
            Self::Error => 3,
        }
    }
}

impl AutomationResponse {
    fn success(id: Option<Value>, result: Value) -> Self {
        Self {
            id,
            ok: true,
            result: Some(result),
            error: None,
        }
    }

    fn error(id: Option<Value>, message: impl Into<String>) -> Self {
        Self {
            id,
            ok: false,
            result: None,
            error: Some(message.into()),
        }
    }
}

impl WorkdeskAttentionSummary {
    fn register(&mut self, attention: PaneAttention) {
        if attention.unread && attention.state.is_attention() {
            self.unread_count += 1;
        }

        if attention.state.priority() > self.highest.priority() {
            self.highest = attention.state;
        }
    }
}

impl WorkdeskMetadata {
    fn hydrated(mut self) -> Self {
        let (cwd, branch) = workspace_defaults();
        if self.cwd.trim().is_empty() {
            self.cwd = cwd;
        }
        if self.branch.trim().is_empty() {
            self.branch = branch;
        }
        self
    }

    fn intent_label(&self, summary: &str) -> String {
        let intent = self.intent.trim();
        if intent.is_empty() {
            summary.to_string()
        } else {
            intent.to_string()
        }
    }

    fn status_label(&self) -> Option<String> {
        self.status
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    }

    fn progress_label(&self) -> Option<String> {
        self.progress
            .as_ref()
            .map(|progress| format!("{} {}%", progress.label, progress.value))
    }
}

impl WorkdeskProgress {
    fn new(label: impl Into<String>, value: u8) -> Self {
        Self {
            label: label.into(),
            value: value.min(100),
        }
    }
}

impl WorkdeskTemplate {
    fn all() -> [Self; 4] {
        [
            Self::ShellDesk,
            Self::AgentReview,
            Self::Debug,
            Self::Implementation,
        ]
    }

    fn from_api_name(value: &str) -> Option<Self> {
        match value {
            "shell-desk" | "shell" => Some(Self::ShellDesk),
            "agent-review" | "review" => Some(Self::AgentReview),
            "debug" => Some(Self::Debug),
            "implementation" | "implement" => Some(Self::Implementation),
            _ => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::ShellDesk => "Shell Desk",
            Self::AgentReview => "Agent Review",
            Self::Debug => "Debug",
            Self::Implementation => "Implementation",
        }
    }

    fn base_name(self) -> &'static str {
        self.label()
    }

    fn summary(self) -> &'static str {
        match self {
            Self::ShellDesk => "One shell kept hot for command-first work.",
            Self::AgentReview => "Agent plus shell loop for review and verification.",
            Self::Debug => "Repro shell and debug agent kept in one workspace.",
            Self::Implementation => "Execution shell with an implementation agent beside it.",
        }
    }

    fn intent(self) -> &'static str {
        match self {
            Self::ShellDesk => "Run commands, inspect state, and keep one terminal ready.",
            Self::AgentReview => "Review agent output, verify claims, and keep evidence nearby.",
            Self::Debug => "Pin the repro, keep logs visible, and iterate on fixes.",
            Self::Implementation => {
                "Ship a scoped change, verify locally, and keep the build green."
            }
        }
    }

    fn status(self) -> Option<String> {
        match self {
            Self::ShellDesk => Some("Ready".to_string()),
            Self::AgentReview => Some("Reviewing".to_string()),
            Self::Debug => Some("Debugging".to_string()),
            Self::Implementation => Some("Building".to_string()),
        }
    }

    fn progress(self) -> Option<WorkdeskProgress> {
        match self {
            Self::ShellDesk => None,
            Self::AgentReview => Some(WorkdeskProgress::new("Review", 20)),
            Self::Debug => Some(WorkdeskProgress::new("Debug", 15)),
            Self::Implementation => Some(WorkdeskProgress::new("Build", 25)),
        }
    }

    fn accent(self) -> gpui::Hsla {
        match self {
            Self::ShellDesk => rgb(0xe59a49).into(),
            Self::AgentReview => rgb(0x7cc7ff).into(),
            Self::Debug => rgb(0xff9b88).into(),
            Self::Implementation => rgb(0x77d19a).into(),
        }
    }
}

impl WorkdeskEditorField {
    fn label(self) -> &'static str {
        match self {
            Self::Name => "Name",
            Self::Intent => "Intent",
            Self::Summary => "Summary",
        }
    }

    fn cycle(self, reverse: bool) -> Self {
        match (self, reverse) {
            (Self::Name, false) => Self::Intent,
            (Self::Intent, false) => Self::Summary,
            (Self::Summary, false) => Self::Name,
            (Self::Name, true) => Self::Summary,
            (Self::Intent, true) => Self::Name,
            (Self::Summary, true) => Self::Intent,
        }
    }
}

impl WorkdeskDraft {
    fn from_template(name: String, template: WorkdeskTemplate) -> Self {
        Self {
            name,
            summary: template.summary().to_string(),
            metadata: default_workdesk_metadata(
                template.intent().to_string(),
                template.status(),
                template.progress(),
            ),
        }
    }

    fn from_workdesk(desk: &WorkdeskState) -> Self {
        Self {
            name: desk.name.clone(),
            summary: desk.summary.clone(),
            metadata: desk.metadata.clone(),
        }
    }

    fn field_value(&self, field: WorkdeskEditorField) -> &str {
        match field {
            WorkdeskEditorField::Name => &self.name,
            WorkdeskEditorField::Intent => &self.metadata.intent,
            WorkdeskEditorField::Summary => &self.summary,
        }
    }

    fn field_value_mut(&mut self, field: WorkdeskEditorField) -> &mut String {
        match field {
            WorkdeskEditorField::Name => &mut self.name,
            WorkdeskEditorField::Intent => &mut self.metadata.intent,
            WorkdeskEditorField::Summary => &mut self.summary,
        }
    }
}

impl WorkdeskEditorState {
    fn new_create(template: WorkdeskTemplate, name: String) -> Self {
        Self {
            mode: WorkdeskEditorMode::Create,
            template,
            active_field: WorkdeskEditorField::Name,
            draft: WorkdeskDraft::from_template(name, template),
        }
    }

    fn new_edit(index: usize, desk: &WorkdeskState) -> Self {
        Self {
            mode: WorkdeskEditorMode::Edit(index),
            template: WorkdeskTemplate::ShellDesk,
            active_field: WorkdeskEditorField::Name,
            draft: WorkdeskDraft::from_workdesk(desk),
        }
    }

    fn title(&self) -> &'static str {
        match self.mode {
            WorkdeskEditorMode::Create => "New workdesk",
            WorkdeskEditorMode::Edit(_) => "Edit workdesk",
        }
    }

    fn submit_label(&self) -> &'static str {
        match self.mode {
            WorkdeskEditorMode::Create => "Create",
            WorkdeskEditorMode::Edit(_) => "Save",
        }
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
            PaneKind::Browser => Self::Browser,
            PaneKind::Editor => Self::Editor,
        }
    }
}

impl From<PersistedPaneKind> for PaneKind {
    fn from(value: PersistedPaneKind) -> Self {
        match value {
            PersistedPaneKind::Shell => Self::Shell,
            PersistedPaneKind::Agent => Self::Agent,
            PersistedPaneKind::Browser => Self::Browser,
            PersistedPaneKind::Editor => Self::Editor,
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

const SHORTCUT_ACTIONS: [ShortcutAction; 29] = [
    ShortcutAction::ToggleShortcutPanel,
    ShortcutAction::ToggleInspector,
    ShortcutAction::NextAttention,
    ShortcutAction::ClearActiveAttention,
    ShortcutAction::SpawnShellPane,
    ShortcutAction::SpawnAgentPane,
    ShortcutAction::SpawnBrowserPane,
    ShortcutAction::SpawnEditorPane,
    ShortcutAction::NextSurface,
    ShortcutAction::PreviousSurface,
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
            Self::Workspace => {
                "Create panes, move between desks, inspect attention, and open utility overlays."
            }
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
            Self::ToggleInspector => "toggle-inspector",
            Self::NextAttention => "next-attention",
            Self::ClearActiveAttention => "clear-active-attention",
            Self::SpawnShellPane => "spawn-shell-pane",
            Self::SpawnAgentPane => "spawn-agent-pane",
            Self::SpawnBrowserPane => "spawn-browser-pane",
            Self::SpawnEditorPane => "spawn-editor-pane",
            Self::NextSurface => "next-surface",
            Self::PreviousSurface => "previous-surface",
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
            | Self::ToggleInspector
            | Self::NextAttention
            | Self::ClearActiveAttention
            | Self::SpawnShellPane
            | Self::SpawnAgentPane
            | Self::SpawnBrowserPane
            | Self::SpawnEditorPane
            | Self::NextSurface
            | Self::PreviousSurface
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
            Self::ToggleInspector => "Toggle developer inspector",
            Self::NextAttention => "Jump to next attention",
            Self::ClearActiveAttention => "Clear active attention",
            Self::SpawnShellPane => "New shell pane",
            Self::SpawnAgentPane => "New agent pane",
            Self::SpawnBrowserPane => "New browser pane",
            Self::SpawnEditorPane => "Open file in editor",
            Self::NextSurface => "Next surface in pane",
            Self::PreviousSurface => "Previous surface in pane",
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
            Self::ToggleInspector => {
                "Open the debug-only inspector with bridge and layout diagnostics."
            }
            Self::NextAttention => {
                "Jump to the next pane across all desks that still has unread attention."
            }
            Self::ClearActiveAttention => {
                "Dismiss the current pane's waiting or error attention state."
            }
            Self::SpawnShellPane => "Create a new shell pane near the viewport center.",
            Self::SpawnAgentPane => "Create a new agent pane near the viewport center.",
            Self::SpawnBrowserPane => "Create a new browser pane near the viewport center.",
            Self::SpawnEditorPane => "Open a file picker and create or focus an editor surface.",
            Self::NextSurface => "Cycle to the next surface stacked inside the active pane.",
            Self::PreviousSurface => {
                "Cycle to the previous surface stacked inside the active pane."
            }
            Self::CloseActivePane => {
                "Close the focused surface, or the pane if it is the last one."
            }
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
            Self::ToggleInspector => Some("cmd-alt-i"),
            Self::NextAttention => Some("cmd-alt-j"),
            Self::ClearActiveAttention => Some("cmd-alt-k"),
            Self::SpawnShellPane => Some("cmd-shift-n"),
            Self::SpawnAgentPane => Some("cmd-alt-n"),
            Self::SpawnBrowserPane => Some("cmd-shift-b"),
            Self::SpawnEditorPane => Some("cmd-shift-e"),
            Self::NextSurface => Some("ctrl-tab"),
            Self::PreviousSurface => Some("ctrl-shift-tab"),
            Self::CloseActivePane => Some("cmd-shift-w"),
            Self::SpawnWorkdesk => Some("cmd-shift-d"),
            Self::SelectPreviousWorkdesk => Some("cmd-alt-["),
            Self::SelectNextWorkdesk => Some("cmd-alt-]"),
            Self::LayoutFree => Some("cmd-shift-f"),
            Self::LayoutGrid => Some("cmd-shift-g"),
            Self::LayoutSplit => Some("cmd-shift-s"),
            Self::ToggleGridExpose => Some("cmd-shift-o"),
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

    fn for_grid(
        pane: &PaneRecord,
        viewport_width: f32,
        viewport_height: f32,
        sidebar_width: f32,
    ) -> Self {
        let available_width =
            (viewport_width - sidebar_width - GRID_ACTIVE_MARGIN_X * 2.0).max(MIN_PANE_WIDTH);
        let available_height =
            (viewport_height - GRID_ACTIVE_MARGIN_TOP - GRID_ACTIVE_MARGIN_BOTTOM)
                .max(MIN_PANE_HEIGHT);
        let zoom = (available_width / pane.size.width)
            .min(available_height / pane.size.height)
            .clamp(0.72, 1.35);
        let width = pane.size.width * zoom;
        let height = pane.size.height * zoom;
        let x = sidebar_width + GRID_ACTIVE_MARGIN_X + (available_width - width).max(0.0) * 0.5;
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
        let name = name.into();
        let summary = summary.into();
        let next_pane_serial = panes.iter().map(|pane| pane.id.raw()).max().unwrap_or(0) + 1;
        let next_surface_serial = panes
            .iter()
            .flat_map(|pane| pane.surfaces.iter().map(|surface| surface.id.raw()))
            .max()
            .unwrap_or(0)
            + 1;
        let active_pane = panes.last().map(|pane| pane.id);
        let pane_attention = panes
            .iter()
            .map(|pane| {
                let state = baseline_attention_state_for_kind(&pane.kind);
                (
                    pane.id,
                    PaneAttention {
                        state,
                        unread: false,
                        last_attention_sequence: 0,
                        last_activity_sequence: 0,
                    },
                )
            })
            .collect();

        Self {
            name,
            summary: summary.clone(),
            metadata: default_workdesk_metadata(summary, None, None),
            panes,
            pane_attention,
            terminals: HashMap::new(),
            terminal_revisions: HashMap::new(),
            terminal_statuses: HashMap::new(),
            terminal_views: HashMap::new(),
            terminal_grids: HashMap::new(),
            editors: HashMap::new(),
            editor_views: HashMap::new(),
            next_pane_serial,
            next_surface_serial,
            attention_sequence: 0,
            layout_mode: LayoutMode::Free,
            grid_layout: GridLayoutState::default(),
            camera: WorkdeskPoint::new(0.0, 0.0),
            zoom: 1.0,
            active_pane,
            drag_state: DragState::Idle,
            runtime_notice: None,
        }
    }

    fn pane(&self, pane_id: PaneId) -> Option<&PaneRecord> {
        self.panes.iter().find(|pane| pane.id == pane_id)
    }

    fn pane_mut(&mut self, pane_id: PaneId) -> Option<&mut PaneRecord> {
        self.panes.iter_mut().find(|pane| pane.id == pane_id)
    }

    fn surface_owner(&self, surface_id: SurfaceId) -> Option<PaneId> {
        self.panes
            .iter()
            .find(|pane| pane.surfaces.iter().any(|surface| surface.id == surface_id))
            .map(|pane| pane.id)
    }

    fn surface_mut(&mut self, surface_id: SurfaceId) -> Option<&mut SurfaceRecord> {
        self.panes
            .iter_mut()
            .find_map(|pane| pane.surface_mut(surface_id))
    }

    fn active_surface_id_for_pane(&self, pane_id: PaneId) -> Option<SurfaceId> {
        self.pane(pane_id).map(|pane| pane.active_surface_id)
    }

    fn active_surface_for_pane(&self, pane_id: PaneId) -> Option<&SurfaceRecord> {
        self.pane(pane_id).and_then(PaneRecord::active_surface)
    }

    fn active_terminal_surface_id_for_pane(&self, pane_id: PaneId) -> Option<SurfaceId> {
        self.active_surface_for_pane(pane_id)
            .filter(|surface| surface.kind.is_terminal())
            .map(|surface| surface.id)
    }

    fn active_terminal_session_for_pane(&self, pane_id: PaneId) -> Option<&TerminalSession> {
        let surface_id = self.active_terminal_surface_id_for_pane(pane_id)?;
        self.terminals.get(&surface_id)
    }

    fn active_terminal_view_for_pane(&self, pane_id: PaneId) -> Option<&TerminalViewState> {
        let surface_id = self.active_terminal_surface_id_for_pane(pane_id)?;
        self.terminal_views.get(&surface_id)
    }

    fn focus_surface(&mut self, pane_id: PaneId, surface_id: SurfaceId) {
        if let Some(pane) = self.pane_mut(pane_id) {
            pane.focus_surface(surface_id);
        }
        self.focus_pane(pane_id);
    }

    fn next_surface_id(&self, pane_id: PaneId, backwards: bool) -> Option<SurfaceId> {
        self.pane(pane_id)
            .and_then(|pane| pane.next_surface_id(backwards))
    }

    fn sync_terminal_revisions(&mut self) -> Vec<(PaneId, SurfaceId)> {
        let mut changed = Vec::new();

        for (surface_id, terminal) in &self.terminals {
            let revision = terminal.revision();
            if self.terminal_revisions.get(surface_id).copied() != Some(revision) {
                self.terminal_revisions.insert(*surface_id, revision);
                if let Some(pane_id) = self.surface_owner(*surface_id) {
                    changed.push((pane_id, *surface_id));
                }
            }
        }

        self.terminal_revisions
            .retain(|surface_id, _| self.terminals.contains_key(surface_id));
        self.terminal_statuses
            .retain(|surface_id, _| self.terminals.contains_key(surface_id));
        self.terminal_views
            .retain(|surface_id, _| self.terminals.contains_key(surface_id));
        self.terminal_grids
            .retain(|surface_id, _| self.terminals.contains_key(surface_id));
        let live_surface_ids = self
            .panes
            .iter()
            .flat_map(|pane| pane.surfaces.iter().map(|surface| surface.id))
            .collect::<Vec<_>>();
        self.editors
            .retain(|surface_id, _| live_surface_ids.contains(surface_id));
        self.editor_views
            .retain(|surface_id, _| live_surface_ids.contains(surface_id));
        self.pane_attention
            .retain(|pane_id, _| self.panes.iter().any(|pane| pane.id == *pane_id));

        changed
    }

    fn next_attention_sequence(&mut self) -> u64 {
        self.attention_sequence += 1;
        self.attention_sequence
    }

    fn pane_attention(&self, pane_id: PaneId) -> PaneAttention {
        self.pane_attention
            .get(&pane_id)
            .copied()
            .unwrap_or_default()
    }

    fn pane_attention_mut(&mut self, pane_id: PaneId) -> &mut PaneAttention {
        self.pane_attention.entry(pane_id).or_default()
    }

    fn set_pane_attention_state(
        &mut self,
        pane_id: PaneId,
        state: AttentionState,
        unread: bool,
    ) -> bool {
        let next_sequence = if state.is_attention() {
            Some(self.next_attention_sequence())
        } else {
            None
        };
        let attention = self.pane_attention_mut(pane_id);
        let changed = attention.state != state || attention.unread != unread;
        if !changed {
            return false;
        }

        attention.state = state;
        attention.unread = unread && state.is_attention();
        if let Some(sequence) = next_sequence {
            attention.last_attention_sequence = sequence;
        }
        changed
    }

    fn mark_pane_attention_seen(&mut self, pane_id: PaneId) -> bool {
        let attention = self.pane_attention_mut(pane_id);
        if !attention.unread {
            return false;
        }
        attention.unread = false;
        true
    }

    fn note_pane_activity(&mut self, pane_id: PaneId) {
        let sequence = self.next_attention_sequence();
        self.pane_attention_mut(pane_id).last_activity_sequence = sequence;
    }

    fn workdesk_attention_summary(&self) -> WorkdeskAttentionSummary {
        let mut summary = WorkdeskAttentionSummary::default();
        for attention in self.pane_attention.values().copied() {
            summary.register(attention);
        }
        summary
    }

    fn attach_terminal_session(
        &mut self,
        surface_id: SurfaceId,
        kind: &PaneKind,
        title: &str,
        grid: TerminalGridSize,
    ) {
        match spawn_terminal_session_with_grid(kind, title, grid) {
            Ok(session) => {
                self.terminal_revisions
                    .insert(surface_id, session.revision());
                self.terminal_statuses.insert(surface_id, None);
                self.terminal_grids.insert(surface_id, grid);
                self.terminals.insert(surface_id, session);
                self.terminal_views.entry(surface_id).or_default();
                let Some(pane_id) = self.surface_owner(surface_id) else {
                    return;
                };
                self.pane_attention.entry(pane_id).or_insert(PaneAttention {
                    state: match kind {
                        PaneKind::Shell => AttentionState::Idle,
                        PaneKind::Agent => AttentionState::Working,
                        PaneKind::Browser | PaneKind::Editor => AttentionState::Idle,
                    },
                    unread: false,
                    last_attention_sequence: 0,
                    last_activity_sequence: 0,
                });
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
        }
        self.resize_terminals_for_pane(pane_id);
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
        self.mark_pane_attention_seen(pane_id);
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
            DragState::SelectingEditor { pane_id, .. } => {
                format!("Editing pane #{}", pane_id.raw())
            }
        }
    }

    fn active_pane_title(&self) -> String {
        self.active_pane
            .and_then(|pane_id| self.panes.iter().find(|pane| pane.id == pane_id))
            .map(|pane| pane.title.clone())
            .unwrap_or_else(|| "None".to_string())
    }

    fn resize_terminals_for_pane(&mut self, pane_id: PaneId) {
        let Some(pane) = self.pane(pane_id) else {
            return;
        };
        let grid = terminal_grid_size_for_pane(pane.size, pane.surfaces.len());
        self.resize_terminals_for_pane_to_grid(pane_id, grid);
    }

    fn resize_terminals_for_pane_to_grid(&mut self, pane_id: PaneId, grid: TerminalGridSize) {
        let Some(pane) = self.pane(pane_id) else {
            return;
        };
        let pane_title = pane.title.clone();
        let terminal_surface_ids = pane
            .surfaces
            .iter()
            .filter(|surface| surface.kind.is_terminal())
            .map(|surface| surface.id)
            .collect::<Vec<_>>();

        for surface_id in terminal_surface_ids {
            if self.terminal_grids.get(&surface_id).copied() == Some(grid) {
                continue;
            }
            if let Some(terminal) = self.terminals.get(&surface_id) {
                if let Err(error) = terminal.resize(grid) {
                    self.runtime_notice = Some(SharedString::from(format!(
                        "terminal resize failed for {}: {}",
                        pane_title, error
                    )));
                } else {
                    self.terminal_grids.insert(surface_id, grid);
                }
            }
        }
    }

    fn intent_label(&self) -> String {
        self.metadata.intent_label(&self.summary)
    }

    fn status_label(&self) -> Option<String> {
        self.metadata.status_label()
    }

    fn progress_label(&self) -> Option<String> {
        self.metadata.progress_label()
    }

    fn clear_selection(&mut self, pane_id: PaneId) {
        let Some(surface_id) = self.active_surface_id_for_pane(pane_id) else {
            return;
        };
        if let Some(view) = self.terminal_views.get_mut(&surface_id) {
            view.selection = None;
        }
    }

    fn clear_all_selections(&mut self) {
        for view in self.terminal_views.values_mut() {
            view.selection = None;
        }
    }

    fn begin_selection(&mut self, surface_id: SurfaceId, cell: TerminalCell) {
        self.terminal_views.entry(surface_id).or_default().selection = Some(TerminalSelection {
            anchor: cell,
            focus: cell,
        });
    }

    fn update_selection(&mut self, surface_id: SurfaceId, cell: TerminalCell) {
        if let Some(selection) = self
            .terminal_views
            .entry(surface_id)
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
            metadata: state.metadata.clone(),
            panes: state
                .panes
                .iter()
                .map(|pane| PersistedPane {
                    id: pane.id.raw(),
                    title: pane.title.clone(),
                    kind: PersistedPaneKind::from(&pane.kind),
                    position: PersistedPoint::from(pane.position),
                    size: PersistedSize::from(pane.size),
                    active_surface_id: Some(pane.active_surface_id.raw()),
                    surfaces: pane
                        .surfaces
                        .iter()
                        .map(|surface| PersistedSurface {
                            id: surface.id.raw(),
                            title: surface.title.clone(),
                            kind: PersistedPaneKind::from(&surface.kind),
                            browser_url: surface.browser_url.clone(),
                            editor_file_path: surface.editor_file_path.clone(),
                            dirty: surface.dirty,
                            editor_buffer_text: state
                                .editors
                                .get(&surface.id)
                                .and_then(|editor| editor.persisted_buffer_text())
                                .map(ToOwned::to_owned),
                        })
                        .collect(),
                    stack_title: pane.stack_title.clone(),
                    attention: state.pane_attention(pane.id),
                })
                .collect(),
            attention_sequence: state.attention_sequence,
            layout_mode: state.layout_mode.into(),
            camera: state.camera.into(),
            zoom: state.zoom,
            active_pane: state.active_pane.map(PaneId::raw),
        }
    }

    fn into_state(self) -> WorkdeskState {
        let PersistedWorkdesk {
            name,
            summary,
            metadata,
            panes,
            attention_sequence,
            layout_mode,
            camera,
            zoom,
            active_pane,
        } = self;

        let pane_attention = panes
            .iter()
            .map(|pane| (PaneId::new(pane.id), pane.attention))
            .collect::<HashMap<_, _>>();
        let mut editor_restores = Vec::new();
        let panes = panes
            .into_iter()
            .map(|pane| {
                let persisted_surfaces = if pane.surfaces.is_empty() {
                    vec![PersistedSurface {
                        id: pane.id,
                        title: pane.title.clone(),
                        kind: pane.kind,
                        browser_url: None,
                        editor_file_path: None,
                        dirty: false,
                        editor_buffer_text: None,
                    }]
                } else {
                    pane.surfaces
                };
                let mut runtime_surfaces = persisted_surfaces
                    .iter()
                    .map(|surface| {
                        let kind: PaneKind = surface.kind.into();
                        let mut runtime = match kind {
                            PaneKind::Browser => SurfaceRecord::browser(
                                SurfaceId::new(surface.id),
                                surface.title.clone(),
                                surface
                                    .browser_url
                                    .clone()
                                    .unwrap_or_else(|| "https://example.com".to_string()),
                            ),
                            PaneKind::Editor => SurfaceRecord::editor(
                                SurfaceId::new(surface.id),
                                surface.title.clone(),
                                surface
                                    .editor_file_path
                                    .clone()
                                    .unwrap_or_else(|| surface.title.clone()),
                                surface.dirty,
                            ),
                            PaneKind::Shell | PaneKind::Agent => SurfaceRecord::new(
                                SurfaceId::new(surface.id),
                                surface.title.clone(),
                                kind,
                            ),
                        };
                        runtime.browser_url = surface.browser_url.clone();
                        runtime.editor_file_path = surface.editor_file_path.clone();
                        runtime.dirty = surface.dirty;
                        runtime
                    })
                    .collect::<Vec<_>>();

                for surface in &persisted_surfaces {
                    if matches!(surface.kind, PersistedPaneKind::Editor) {
                        if let Some(path) = surface.editor_file_path.clone() {
                            editor_restores.push((
                                SurfaceId::new(surface.id),
                                path,
                                surface.dirty,
                                surface.editor_buffer_text.clone(),
                            ));
                        }
                    }
                }

                let first_surface = runtime_surfaces
                    .drain(..1)
                    .next()
                    .expect("pane should contain at least one surface");
                let mut runtime_pane = PaneRecord::new(
                    PaneId::new(pane.id),
                    pane.position.into(),
                    pane.size.into(),
                    first_surface,
                    pane.stack_title,
                );
                for surface in runtime_surfaces {
                    runtime_pane.push_surface(surface, false);
                }
                if let Some(active_surface_id) = pane.active_surface_id.map(SurfaceId::new) {
                    runtime_pane.focus_surface(active_surface_id);
                }
                runtime_pane
            })
            .collect::<Vec<_>>();

        let mut state = WorkdeskState::new(name, summary, panes);
        state.metadata = metadata.hydrated();
        state.pane_attention = pane_attention;
        state.attention_sequence = attention_sequence;
        state.layout_mode = layout_mode.into();
        state.camera = camera.into();
        state.zoom = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        state.active_pane = active_pane
            .map(PaneId::new)
            .filter(|pane_id| state.panes.iter().any(|pane| pane.id == *pane_id));
        state.grid_layout = GridLayoutState::default();
        state.drag_state = DragState::Idle;
        state.runtime_notice = None;
        for (surface_id, path, dirty, buffer_text) in editor_restores {
            let editor = match buffer_text {
                Some(text) => EditorBuffer::restore(path, text, dirty),
                None => match EditorBuffer::load(&path) {
                    Ok(editor) => editor,
                    Err(_) => EditorBuffer::restore(path, "", dirty),
                },
            };
            state.editors.insert(surface_id, editor);
        }
        state
    }
}

impl CanvasShell {
    fn new(
        workdesks: Vec<WorkdeskState>,
        active_workdesk: usize,
        shortcuts: ShortcutMap,
        boot_notice: Option<SharedString>,
        automation_server: AutomationServer,
        focus_handle: FocusHandle,
        ghostty_vendor_dir: SharedString,
        ghostty_status: SharedString,
    ) -> Self {
        let AutomationServer {
            receiver,
            socket_path,
        } = automation_server;
        let clamped_active_workdesk = active_workdesk.min(workdesks.len().saturating_sub(1));
        let mut shell = Self {
            workdesks,
            active_workdesk: clamped_active_workdesk,
            workdesk_menu: None,
            workdesk_editor: None,
            automation_rx: receiver,
            focus_handle,
            automation_socket_path: SharedString::from(socket_path.display().to_string()),
            ghostty_vendor_dir,
            ghostty_status,
            shortcuts,
            shortcut_editor: ShortcutEditorState::default(),
            inspector_open: false,
            sidebar_collapsed: false,
            notifications_open: false,
            mock_notifications_unread: 3,
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

    fn sidebar_width(&self) -> f32 {
        if self.sidebar_collapsed {
            SIDEBAR_COLLAPSED_WIDTH
        } else {
            SIDEBAR_WIDTH
        }
    }

    fn shortcut_label(&self, action: ShortcutAction) -> String {
        self.shortcuts.display_label(action)
    }

    fn set_runtime_notice(&mut self, message: impl Into<String>) {
        if let Some(workdesk) = self.workdesks.get_mut(self.active_workdesk) {
            workdesk.runtime_notice = Some(SharedString::from(message.into()));
        }
    }

    fn active_editor_ids(&self) -> Option<(PaneId, SurfaceId)> {
        let pane_id = self.active_workdesk().active_pane?;
        let surface = self.active_workdesk().active_surface_for_pane(pane_id)?;
        (surface.kind == PaneKind::Editor).then_some((pane_id, surface.id))
    }

    fn active_editor(&self) -> Option<&EditorBuffer> {
        let (_, surface_id) = self.active_editor_ids()?;
        self.active_workdesk().editors.get(&surface_id)
    }

    fn active_editor_mut(&mut self) -> Option<&mut EditorBuffer> {
        let (_, surface_id) = self.active_editor_ids()?;
        self.active_workdesk_mut().editors.get_mut(&surface_id)
    }

    fn active_editor_view(&self) -> Option<&EditorViewState> {
        let (_, surface_id) = self.active_editor_ids()?;
        self.active_workdesk().editor_views.get(&surface_id)
    }

    fn sync_editor_surface_metadata(&mut self, pane_id: PaneId, surface_id: SurfaceId) {
        let Some(editor) = self
            .workdesks
            .get(self.active_workdesk)
            .and_then(|desk| desk.editors.get(&surface_id))
        else {
            return;
        };
        let title = editor.title();
        let file_path = editor.path_string();
        let dirty = editor.dirty();
        if let Some(surface) = self.workdesks[self.active_workdesk].surface_mut(surface_id) {
            surface.title = title;
            surface.editor_file_path = Some(file_path);
            surface.dirty = dirty;
        }
        if let Some(pane) = self.workdesks[self.active_workdesk].pane_mut(pane_id) {
            pane.sync_from_active_surface();
        }
    }

    fn find_editor_surface_by_path(
        &self,
        desk_index: usize,
        canonical_path: &str,
    ) -> Option<(PaneId, SurfaceId)> {
        let desk = self.workdesks.get(desk_index)?;
        desk.panes.iter().find_map(|pane| {
            pane.surfaces.iter().find_map(|surface| {
                (surface.kind == PaneKind::Editor
                    && surface
                        .editor_file_path
                        .as_deref()
                        .is_some_and(|path| canonical_path_string(path) == canonical_path))
                .then_some((pane.id, surface.id))
            })
        })
    }

    fn build_surface_record(
        surface_id: SurfaceId,
        kind: PaneKind,
        title: Option<String>,
        url: Option<String>,
        file_path: Option<String>,
    ) -> Result<(SurfaceRecord, Option<EditorBuffer>), String> {
        match kind {
            PaneKind::Shell | PaneKind::Agent => {
                let title = title.unwrap_or_else(|| {
                    format!("{} {}", base_label_for_kind(&kind), surface_id.raw())
                });
                Ok((SurfaceRecord::new(surface_id, title, kind), None))
            }
            PaneKind::Browser => {
                let url = url.unwrap_or_else(|| "https://example.com".to_string());
                let title = title.unwrap_or_else(|| browser_title(&url));
                Ok((SurfaceRecord::browser(surface_id, title, url), None))
            }
            PaneKind::Editor => {
                let file_path =
                    file_path.ok_or_else(|| "editor surfaces require `file_path`".to_string())?;
                let canonical_path = canonical_path_string(&file_path);
                let editor = match EditorBuffer::load(&canonical_path) {
                    Ok(editor) => editor,
                    Err(_) => EditorBuffer::restore(&canonical_path, "", false),
                };
                let title = title.unwrap_or_else(|| editor.title());
                Ok((
                    SurfaceRecord::editor(surface_id, title, editor.path_string(), editor.dirty()),
                    Some(editor),
                ))
            }
        }
    }

    fn initialize_surface_runtime(
        desk: &mut WorkdeskState,
        pane_size: WorkdeskSize,
        pane_surface_count: usize,
        surface: &SurfaceRecord,
        editor: Option<EditorBuffer>,
    ) {
        match surface.kind {
            PaneKind::Shell | PaneKind::Agent => {
                desk.attach_terminal_session(
                    surface.id,
                    &surface.kind,
                    &surface.title,
                    terminal_grid_size_for_pane(pane_size, pane_surface_count),
                );
            }
            PaneKind::Editor => {
                if let Some(editor) = editor {
                    desk.editors.insert(surface.id, editor);
                }
            }
            PaneKind::Browser => {}
        }
    }

    fn spawn_surface_on_workdesk(
        &mut self,
        desk_index: usize,
        target_pane_id: Option<PaneId>,
        kind: PaneKind,
        title: Option<String>,
        url: Option<String>,
        file_path: Option<String>,
        focus: bool,
    ) -> Result<(PaneId, SurfaceId), String> {
        let editor_lookup_path = if kind == PaneKind::Editor {
            file_path.as_deref().map(canonical_path_string)
        } else {
            None
        };
        if let Some(canonical_path) = editor_lookup_path.as_deref() {
            if target_pane_id.is_none() {
                if let Some((pane_id, surface_id)) =
                    self.find_editor_surface_by_path(desk_index, canonical_path)
                {
                    if focus {
                        self.workdesks[desk_index].focus_surface(pane_id, surface_id);
                        self.active_workdesk = desk_index;
                    }
                    return Ok((pane_id, surface_id));
                }
            }
        }

        let desk = &mut self.workdesks[desk_index];
        let surface_id = SurfaceId::new(desk.next_surface_serial);
        desk.next_surface_serial += 1;
        let requested_file_path = file_path.or(editor_lookup_path);
        let (surface, editor) =
            Self::build_surface_record(surface_id, kind.clone(), title, url, requested_file_path)?;

        if let Some(pane_id) = target_pane_id {
            let pane = desk
                .pane_mut(pane_id)
                .ok_or_else(|| format!("pane {} was not found", pane_id.raw()))?;
            let pane_size = pane.size;
            pane.push_surface(surface.clone(), focus);
            let pane_surface_count = pane.surfaces.len();
            Self::initialize_surface_runtime(desk, pane_size, pane_surface_count, &surface, editor);
            desk.resize_terminals_for_pane(pane_id);
            if focus {
                desk.focus_surface(pane_id, surface_id);
                self.active_workdesk = desk_index;
            }
            return Ok((pane_id, surface_id));
        }

        let size = default_size_for_kind(&kind);
        let pane_id = PaneId::new(desk.next_pane_serial);
        desk.next_pane_serial += 1;
        let cascade = 36.0 * (desk.panes.len() % 6) as f32;
        let position = desk
            .active_pane
            .and_then(|active_pane_id| {
                desk.panes
                    .iter()
                    .find(|pane| pane.id == active_pane_id)
                    .map(|pane| {
                        let horizontal_offset = if desk.layout_mode == LayoutMode::Free {
                            42.0
                        } else {
                            pane.size.width + 96.0
                        };
                        WorkdeskPoint::new(
                            pane.position.x + horizontal_offset,
                            pane.position.y + 42.0 * ((desk.panes.len() % 3) as f32),
                        )
                    })
            })
            .unwrap_or_else(|| WorkdeskPoint::new(80.0 + cascade, 96.0 + cascade));
        let pane = PaneRecord::new(pane_id, position, size, surface.clone(), None);
        desk.panes.push(pane);
        Self::initialize_surface_runtime(desk, size, 1, &surface, editor);
        if focus {
            desk.focus_surface(pane_id, surface_id);
            self.active_workdesk = desk_index;
        }
        desk.drag_state = DragState::Idle;
        Ok((pane_id, surface_id))
    }

    fn close_surface(&mut self, pane_id: PaneId, surface_id: SurfaceId, cx: &mut Context<Self>) {
        let remaining_surfaces = self
            .active_workdesk()
            .pane(pane_id)
            .map(|pane| pane.surfaces.len())
            .unwrap_or(0);
        if remaining_surfaces <= 1 {
            self.close_pane(pane_id, cx);
            return;
        }

        let desk = self.active_workdesk_mut();
        let Some(pane) = desk.pane_mut(pane_id) else {
            return;
        };
        pane.remove_surface(surface_id);
        if let Some(terminal) = desk.terminals.remove(&surface_id) {
            terminal.close();
        }
        desk.terminal_revisions.remove(&surface_id);
        desk.terminal_statuses.remove(&surface_id);
        desk.terminal_views.remove(&surface_id);
        desk.terminal_grids.remove(&surface_id);
        desk.editors.remove(&surface_id);
        desk.editor_views.remove(&surface_id);
        desk.resize_terminals_for_pane(pane_id);
        desk.focus_pane(pane_id);
        self.request_persist(cx);
        cx.notify();
    }

    fn cycle_active_pane_surface(&mut self, backwards: bool, cx: &mut Context<Self>) -> bool {
        let Some(pane_id) = self.active_workdesk().active_pane else {
            return false;
        };
        let Some(next_surface_id) = self.active_workdesk().next_surface_id(pane_id, backwards)
        else {
            return false;
        };
        let desk = self.active_workdesk_mut();
        desk.focus_surface(pane_id, next_surface_id);
        desk.note_pane_activity(pane_id);
        self.request_persist(cx);
        cx.notify();
        true
    }

    fn workdesk_name_for_template(&self, template: WorkdeskTemplate) -> String {
        self.unique_workdesk_name(template.base_name())
    }

    fn open_workdesk_creator(&mut self, cx: &mut Context<Self>) {
        self.dismiss_workdesk_menu();
        self.workdesk_editor = Some(WorkdeskEditorState::new_create(
            WorkdeskTemplate::ShellDesk,
            self.workdesk_name_for_template(WorkdeskTemplate::ShellDesk),
        ));
        cx.notify();
    }

    fn open_workdesk_editor_panel(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(desk) = self.workdesks.get(index).cloned() else {
            return;
        };
        self.dismiss_workdesk_menu();
        self.workdesk_editor = Some(WorkdeskEditorState::new_edit(index, &desk));
        cx.notify();
    }

    fn close_workdesk_editor(&mut self, cx: &mut Context<Self>) {
        if self.workdesk_editor.take().is_some() {
            cx.notify();
        }
    }

    fn select_workdesk_template(&mut self, template: WorkdeskTemplate, cx: &mut Context<Self>) {
        let name = self.workdesk_name_for_template(template);
        let Some(editor) = self.workdesk_editor.as_mut() else {
            return;
        };
        if editor.mode != WorkdeskEditorMode::Create || editor.template == template {
            return;
        }

        editor.template = template;
        editor.active_field = WorkdeskEditorField::Name;
        editor.draft = WorkdeskDraft::from_template(name, template);
        cx.notify();
    }

    fn commit_workdesk_editor(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(editor) = self.workdesk_editor.clone() else {
            return false;
        };

        match editor.mode {
            WorkdeskEditorMode::Create => {
                let name = self.workdesk_name_for_template(editor.template);
                let draft =
                    normalize_workdesk_draft(editor.draft, &name, editor.template.summary());
                let mut desk = workdesk_from_template(editor.template, draft);
                boot_workdesk_terminals(&mut desk);
                self.workdesks.push(desk);
                self.active_workdesk = self.workdesks.len() - 1;
            }
            WorkdeskEditorMode::Edit(index) => {
                if index >= self.workdesks.len() {
                    self.workdesk_editor = None;
                    cx.notify();
                    return false;
                }

                let current_name = self.workdesks[index].name.clone();
                let current_summary = self.workdesks[index].summary.clone();
                let fallback_name = self.unique_workdesk_name_except(index, &current_name);
                let draft =
                    normalize_workdesk_draft(editor.draft, &fallback_name, &current_summary);
                let desk = &mut self.workdesks[index];
                desk.name = draft.name;
                desk.summary = draft.summary;
                desk.metadata = draft.metadata.hydrated();
            }
        }

        self.workdesk_editor = None;
        self.request_persist(cx);
        cx.notify();
        true
    }

    fn handle_workdesk_editor_key_down(
        &mut self,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(editor) = self.workdesk_editor.as_mut() else {
            return false;
        };
        let keystroke = &event.keystroke;

        if keystroke.key == "escape" && !keystroke.modifiers.modified() {
            self.close_workdesk_editor(cx);
            return true;
        }

        if keystroke.key == "enter" && !keystroke.modifiers.modified() {
            self.commit_workdesk_editor(cx);
            return true;
        }

        if keystroke.key == "tab"
            && !keystroke.modifiers.control
            && !keystroke.modifiers.alt
            && !keystroke.modifiers.platform
            && !keystroke.modifiers.function
        {
            editor.active_field = editor.active_field.cycle(keystroke.modifiers.shift);
            cx.notify();
            return true;
        }

        if matches!(keystroke.key.as_str(), "backspace" | "delete")
            && !keystroke.modifiers.modified()
        {
            editor.draft.field_value_mut(editor.active_field).pop();
            cx.notify();
            return true;
        }

        if let Some(text) = editable_keystroke_text(keystroke) {
            editor
                .draft
                .field_value_mut(editor.active_field)
                .push_str(&text);
            cx.notify();
            return true;
        }

        true
    }

    fn process_automation_commands(&mut self, cx: &mut Context<Self>) -> bool {
        let mut processed = false;

        while let Ok(envelope) = self.automation_rx.try_recv() {
            processed = true;
            let response = self.handle_automation_request(envelope.request, cx);
            let _ = envelope.response_tx.send(response);
        }

        processed
    }

    fn handle_automation_request(
        &mut self,
        request: AutomationRequest,
        cx: &mut Context<Self>,
    ) -> AutomationResponse {
        let id = request.id.clone();
        let response: Result<Value, String> = (|| match request.method.as_str() {
            "workdesk.list" => Ok(Value::Array(
                self.workdesks
                    .iter()
                    .enumerate()
                    .map(|(index, desk)| {
                        automation_workdesk_summary_json(index, desk, index == self.active_workdesk)
                    })
                    .collect(),
            )),
            "workdesk.create" => {
                let template = json_string_at(&request.params, &["template"])
                    .map(|value| {
                        WorkdeskTemplate::from_api_name(&value)
                            .ok_or_else(|| format!("unknown template `{value}`"))
                    })
                    .transpose()?
                    .unwrap_or(WorkdeskTemplate::ShellDesk);
                let default_name = self.workdesk_name_for_template(template);
                let mut draft = WorkdeskDraft::from_template(default_name.clone(), template);
                if let Some(name) = json_string_at(&request.params, &["name"]) {
                    draft.name = self.unique_workdesk_name(&name);
                }
                if let Some(summary) = json_string_at(&request.params, &["summary"]) {
                    draft.summary = summary;
                }
                if let Some(intent) = json_string_at(&request.params, &["intent"]) {
                    draft.metadata.intent = intent;
                }
                if let Some(cwd) = json_string_at(&request.params, &["cwd"]) {
                    draft.metadata.cwd = cwd;
                }
                if let Some(branch) = json_string_at(&request.params, &["branch"]) {
                    draft.metadata.branch = branch;
                }
                if json_has_any(&request.params, &["status"]) {
                    draft.metadata.status = json_optional_string_at(&request.params, &["status"])?;
                }
                if let Some(progress) = json_value_at(&request.params, &["progress"]) {
                    draft.metadata.progress = parse_progress_value(progress)?;
                }
                let select = json_bool_at(&request.params, &["select"]).unwrap_or(true);
                let mut desk = workdesk_from_template(template, draft);
                boot_workdesk_terminals(&mut desk);
                self.workdesks.push(desk);
                let index = self.workdesks.len() - 1;
                if select {
                    self.select_workdesk(index, cx);
                } else {
                    self.request_persist(cx);
                }
                Ok(automation_workdesk_summary_json(
                    index,
                    &self.workdesks[index],
                    index == self.active_workdesk,
                ))
            }
            "workdesk.select" => {
                let index = if let Some(name) =
                    json_string_at(&request.params, &["workdesk_name", "name"])
                {
                    self.workdesks
                        .iter()
                        .position(|desk| desk.name == name)
                        .ok_or_else(|| format!("workdesk `{name}` was not found"))?
                } else {
                    self.resolve_automation_workdesk_index(&request.params)?
                };
                self.select_workdesk(index, cx);
                Ok(automation_workdesk_summary_json(
                    index,
                    &self.workdesks[index],
                    true,
                ))
            }
            "workdesk.rename" => {
                let index = self.resolve_automation_workdesk_index(&request.params)?;
                let name = require_json_string_at(&request.params, &["name"], "name")?;
                let name = self.unique_workdesk_name_except(index, &name);
                self.workdesks[index].name = name;
                self.request_persist(cx);
                Ok(automation_workdesk_summary_json(
                    index,
                    &self.workdesks[index],
                    index == self.active_workdesk,
                ))
            }
            "pane.create" => {
                let desk_index = self.resolve_automation_workdesk_index(&request.params)?;
                let kind = json_string_at(&request.params, &["kind"])
                    .and_then(|value| parse_pane_kind(&value))
                    .ok_or_else(|| {
                        "pane.create requires `kind` = `shell`, `agent`, `browser`, or `editor`"
                            .to_string()
                    })?;
                let title = json_string_at(&request.params, &["title"]);
                let url = json_string_at(&request.params, &["url"]);
                let file_path = json_string_at(&request.params, &["file_path"]);
                let focus = json_bool_at(&request.params, &["focus"]).unwrap_or(true);
                let (pane_id, _) = self.spawn_surface_on_workdesk(
                    desk_index, None, kind, title, url, file_path, focus,
                )?;
                self.request_persist(cx);
                Ok(automation_pane_json(
                    &self.workdesks[desk_index],
                    pane_id,
                    desk_index == self.active_workdesk,
                ))
            }
            "surface.list" => {
                let desk_index = self.resolve_automation_workdesk_index(&request.params)?;
                let pane_id = self.resolve_automation_pane_id(desk_index, &request.params)?;
                let desk = &self.workdesks[desk_index];
                let pane = desk
                    .pane(pane_id)
                    .ok_or_else(|| format!("pane {} was not found", pane_id.raw()))?;
                Ok(Value::Array(
                    pane.surfaces
                        .iter()
                        .map(|surface| automation_surface_json(desk, pane_id, surface))
                        .collect(),
                ))
            }
            "surface.create" => {
                let desk_index = self.resolve_automation_workdesk_index(&request.params)?;
                let target_pane_id = json_u64_at(&request.params, &["pane_id"]).map(PaneId::new);
                let kind = json_string_at(&request.params, &["kind"])
                    .and_then(|value| parse_pane_kind(&value))
                    .ok_or_else(|| {
                        "surface.create requires `kind` = `shell`, `agent`, `browser`, or `editor`"
                            .to_string()
                    })?;
                let title = json_string_at(&request.params, &["title"]);
                let url = json_string_at(&request.params, &["url"]);
                let file_path = json_string_at(&request.params, &["file_path"]);
                let focus = json_bool_at(&request.params, &["focus"]).unwrap_or(true);
                let (pane_id, surface_id) = self.spawn_surface_on_workdesk(
                    desk_index,
                    target_pane_id,
                    kind,
                    title,
                    url,
                    file_path,
                    focus,
                )?;
                self.request_persist(cx);
                let desk = &self.workdesks[desk_index];
                let pane = desk
                    .pane(pane_id)
                    .ok_or_else(|| format!("pane {} was not found", pane_id.raw()))?;
                let surface = pane
                    .surface(surface_id)
                    .ok_or_else(|| format!("surface {} was not found", surface_id.raw()))?;
                Ok(json!({
                    "pane": automation_pane_json(desk, pane_id, desk_index == self.active_workdesk),
                    "surface": automation_surface_json(desk, pane_id, surface),
                }))
            }
            "surface.focus" => {
                let desk_index = self.resolve_automation_workdesk_index(&request.params)?;
                let pane_id = self.resolve_automation_pane_id(desk_index, &request.params)?;
                let surface_id =
                    self.resolve_automation_surface_id(desk_index, pane_id, &request.params)?;
                self.active_workdesk = desk_index;
                self.workdesks[desk_index].focus_surface(pane_id, surface_id);
                self.request_persist(cx);
                Ok(automation_pane_json(
                    &self.workdesks[desk_index],
                    pane_id,
                    true,
                ))
            }
            "surface.close" => {
                let desk_index = self.resolve_automation_workdesk_index(&request.params)?;
                let pane_id = self.resolve_automation_pane_id(desk_index, &request.params)?;
                let surface_id =
                    self.resolve_automation_surface_id(desk_index, pane_id, &request.params)?;
                let pane_had_multiple = self.workdesks[desk_index]
                    .pane(pane_id)
                    .map(|pane| pane.surfaces.len() > 1)
                    .unwrap_or(false);
                self.active_workdesk = desk_index;
                self.close_surface(pane_id, surface_id, cx);
                self.request_persist(cx);
                if pane_had_multiple {
                    Ok(automation_pane_json(
                        &self.workdesks[desk_index],
                        pane_id,
                        true,
                    ))
                } else {
                    Ok(json!({
                        "pane_closed": true,
                        "pane_id": pane_id.raw(),
                        "surface_id": surface_id.raw(),
                    }))
                }
            }
            "pane.focus" => {
                let desk_index = self.resolve_automation_workdesk_index(&request.params)?;
                let pane_id = self.resolve_automation_pane_id(desk_index, &request.params)?;
                if desk_index != self.active_workdesk {
                    self.select_workdesk(desk_index, cx);
                }
                self.focus_pane(pane_id, cx);
                Ok(automation_pane_json(
                    &self.workdesks[desk_index],
                    pane_id,
                    true,
                ))
            }
            "attention.set" => {
                let desk_index = self.resolve_automation_workdesk_index(&request.params)?;
                let pane_id = self.resolve_automation_pane_id(desk_index, &request.params)?;
                let state = json_string_at(&request.params, &["state"])
                    .and_then(|value| AttentionState::from_api_name(&value))
                    .ok_or_else(|| "attention.set requires `state`".to_string())?;
                let unread =
                    json_bool_at(&request.params, &["unread"]).unwrap_or(state.is_attention());
                self.set_pane_attention(desk_index, pane_id, state, unread, true, cx);
                Ok(automation_pane_json(
                    &self.workdesks[desk_index],
                    pane_id,
                    desk_index == self.active_workdesk,
                ))
            }
            "attention.clear" => {
                let desk_index = self.resolve_automation_workdesk_index(&request.params)?;
                let pane_id = self.resolve_automation_pane_id(desk_index, &request.params)?;
                let baseline = self.baseline_attention_state(desk_index, pane_id);
                self.set_pane_attention(desk_index, pane_id, baseline, false, false, cx);
                Ok(automation_pane_json(
                    &self.workdesks[desk_index],
                    pane_id,
                    desk_index == self.active_workdesk,
                ))
            }
            "status.set" => {
                let desk_index = self.resolve_automation_workdesk_index(&request.params)?;
                self.workdesks[desk_index].metadata.status =
                    json_optional_string_at(&request.params, &["value", "status"])?;
                self.request_persist(cx);
                Ok(automation_workdesk_summary_json(
                    desk_index,
                    &self.workdesks[desk_index],
                    desk_index == self.active_workdesk,
                ))
            }
            "progress.set" => {
                let desk_index = self.resolve_automation_workdesk_index(&request.params)?;
                let progress = if let Some(progress_value) =
                    json_value_at(&request.params, &["progress", "value"])
                {
                    if progress_value.is_object() {
                        parse_progress_value(progress_value)?
                    } else {
                        let label = require_json_string_at(&request.params, &["label"], "label")?;
                        let value = progress_value.as_u64().ok_or_else(|| {
                            "`value` must be an integer from 0 to 100".to_string()
                        })?;
                        Some(WorkdeskProgress::new(label, value as u8))
                    }
                } else {
                    None
                };
                self.workdesks[desk_index].metadata.progress = progress;
                self.request_persist(cx);
                Ok(automation_workdesk_summary_json(
                    desk_index,
                    &self.workdesks[desk_index],
                    desk_index == self.active_workdesk,
                ))
            }
            "notification.create" => {
                let desk_index = self.resolve_automation_workdesk_index(&request.params)?;
                let body = require_json_string_at(&request.params, &["body", "message"], "body")?;
                let title = json_string_at(&request.params, &["title"]);
                let message = title
                    .as_deref()
                    .map(|title| format!("{title} · {body}"))
                    .unwrap_or_else(|| body.clone());
                self.workdesks[desk_index].runtime_notice =
                    Some(SharedString::from(message.clone()));
                if json_bool_at(&request.params, &["desktop"]).unwrap_or(false) {
                    post_attention_notification(
                        title.unwrap_or_else(|| "Canvas".to_string()),
                        body.clone(),
                    );
                }
                Ok(json!({
                    "desk_index": desk_index,
                    "message": message,
                }))
            }
            "state.current" => Ok(self.automation_state_json()),
            other => Err(format!("unknown automation method `{other}`")),
        })();

        match response {
            Ok(result) => AutomationResponse::success(id, result),
            Err(error) => AutomationResponse::error(id, error),
        }
    }

    fn resolve_automation_workdesk_index(&self, params: &Value) -> Result<usize, String> {
        if let Some(index) = json_u64_at(params, &["workdesk_index", "index"]) {
            let index = index as usize;
            if index < self.workdesks.len() {
                return Ok(index);
            }
            return Err(format!("workdesk index {index} is out of range"));
        }

        if let Some(name) = json_string_at(params, &["workdesk_name"]) {
            return self
                .workdesks
                .iter()
                .position(|desk| desk.name == name)
                .ok_or_else(|| format!("workdesk `{name}` was not found"));
        }

        Ok(self.active_workdesk)
    }

    fn resolve_automation_pane_id(
        &self,
        desk_index: usize,
        params: &Value,
    ) -> Result<PaneId, String> {
        let desk = self
            .workdesks
            .get(desk_index)
            .ok_or_else(|| format!("workdesk index {desk_index} is out of range"))?;

        if let Some(raw_id) = json_u64_at(params, &["pane_id", "id"]) {
            let pane_id = PaneId::new(raw_id);
            if desk.panes.iter().any(|pane| pane.id == pane_id) {
                return Ok(pane_id);
            }
            return Err(format!(
                "pane {} was not found on workdesk `{}`",
                raw_id, desk.name
            ));
        }

        desk.active_pane
            .ok_or_else(|| format!("workdesk `{}` does not have an active pane", desk.name))
    }

    fn resolve_automation_surface_id(
        &self,
        desk_index: usize,
        pane_id: PaneId,
        params: &Value,
    ) -> Result<SurfaceId, String> {
        let desk = self
            .workdesks
            .get(desk_index)
            .ok_or_else(|| format!("workdesk index {desk_index} is out of range"))?;
        let pane = desk
            .pane(pane_id)
            .ok_or_else(|| format!("pane {} was not found", pane_id.raw()))?;

        if let Some(raw_id) = json_u64_at(params, &["surface_id", "id"]) {
            let surface_id = SurfaceId::new(raw_id);
            if pane.surface(surface_id).is_some() {
                return Ok(surface_id);
            }
            return Err(format!(
                "surface {} was not found on pane {}",
                raw_id,
                pane_id.raw()
            ));
        }

        Ok(pane.active_surface_id)
    }

    fn automation_state_json(&self) -> Value {
        json!({
            "active_workdesk": self.active_workdesk,
            "socket_path": self.automation_socket_path.to_string(),
            "workdesks": self.workdesks.iter().enumerate().map(|(index, desk)| {
                automation_workdesk_state_json(index, desk, index == self.active_workdesk)
            }).collect::<Vec<_>>(),
        })
    }

    fn baseline_attention_state(&self, desk_index: usize, pane_id: PaneId) -> AttentionState {
        let Some(desk) = self.workdesks.get(desk_index) else {
            return AttentionState::Idle;
        };
        let Some(pane) = desk.panes.iter().find(|pane| pane.id == pane_id) else {
            return AttentionState::Idle;
        };

        match pane.kind {
            PaneKind::Agent => desk
                .active_terminal_session_for_pane(pane_id)
                .map(|terminal| terminal.snapshot())
                .filter(|snapshot| !snapshot.closed)
                .map(|_| AttentionState::Working)
                .unwrap_or(AttentionState::Idle),
            PaneKind::Shell | PaneKind::Browser | PaneKind::Editor => AttentionState::Idle,
        }
    }

    fn set_pane_attention(
        &mut self,
        desk_index: usize,
        pane_id: PaneId,
        state: AttentionState,
        unread: bool,
        announce: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        let active_workdesk = self.active_workdesk;
        let (changed, desk_name, pane_title, is_visible) = {
            let Some(desk) = self.workdesks.get_mut(desk_index) else {
                return false;
            };
            let Some(pane_title) = desk
                .panes
                .iter()
                .find(|pane| pane.id == pane_id)
                .map(|pane| pane.title.clone())
            else {
                return false;
            };
            let effective_unread =
                unread && !(active_workdesk == desk_index && desk.active_pane == Some(pane_id));
            let changed = desk.set_pane_attention_state(pane_id, state, effective_unread);
            (
                changed,
                desk.name.clone(),
                pane_title,
                active_workdesk == desk_index && desk.active_pane == Some(pane_id),
            )
        };

        if !changed {
            return false;
        }

        if announce && state.is_attention() {
            self.set_runtime_notice(format!("{pane_title} on {desk_name} is {}", state.label()));
            if !is_visible {
                post_attention_notification(
                    format!("Canvas · {desk_name}"),
                    format!("{pane_title} is {}", state.label().to_lowercase()),
                );
            }
        }

        self.request_persist(cx);
        cx.notify();
        true
    }

    fn clear_active_pane_attention(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(pane_id) = self.active_workdesk().active_pane else {
            return false;
        };
        let state = self.baseline_attention_state(self.active_workdesk, pane_id);
        self.set_pane_attention(self.active_workdesk, pane_id, state, false, false, cx)
    }

    fn cycle_manual_pane_attention(&mut self, pane_id: PaneId, cx: &mut Context<Self>) -> bool {
        let desk_index = self.active_workdesk;
        let current = self.active_workdesk().pane_attention(pane_id).state;
        let next = match current {
            AttentionState::Idle | AttentionState::Working => AttentionState::Waiting,
            AttentionState::Waiting => AttentionState::Error,
            AttentionState::Error => self.baseline_attention_state(desk_index, pane_id),
        };
        self.set_pane_attention(desk_index, pane_id, next, next.is_attention(), true, cx)
    }

    fn next_attention_target(&self) -> Option<(usize, PaneId)> {
        next_attention_target_for_workdesks(&self.workdesks)
    }

    fn navigate_next_attention(&mut self, cx: &mut Context<Self>) -> bool {
        let Some((desk_index, pane_id)) = self.next_attention_target() else {
            return false;
        };

        if desk_index != self.active_workdesk {
            self.select_workdesk(desk_index, cx);
        }
        self.focus_pane(pane_id, cx);
        true
    }

    fn handle_terminal_attention_transition(
        &mut self,
        desk_index: usize,
        pane_id: PaneId,
        snapshot: &TerminalSnapshot,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(desk) = self.workdesks.get(desk_index) else {
            return false;
        };
        let Some(pane) = desk.panes.iter().find(|pane| pane.id == pane_id) else {
            return false;
        };

        let next_state = infer_attention_state_from_snapshot(&pane.kind, snapshot);
        match next_state {
            AttentionState::Idle | AttentionState::Working => {
                self.set_pane_attention(desk_index, pane_id, next_state, false, false, cx)
            }
            AttentionState::Waiting | AttentionState::Error => {
                self.set_pane_attention(desk_index, pane_id, next_state, true, true, cx)
            }
        }
    }

    fn close_inspector(&mut self, cx: &mut Context<Self>) {
        if !self.inspector_open {
            return;
        }

        self.inspector_open = false;
        cx.notify();
    }

    fn toggle_inspector(&mut self, cx: &mut Context<Self>) {
        if !cfg!(debug_assertions) {
            return;
        }

        self.inspector_open = !self.inspector_open;
        cx.notify();
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
                    self.sidebar_width(),
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
            ShortcutAction::ToggleInspector => {
                self.toggle_inspector(cx);
                cfg!(debug_assertions)
            }
            ShortcutAction::NextAttention => self.navigate_next_attention(cx),
            ShortcutAction::ClearActiveAttention => self.clear_active_pane_attention(cx),
            ShortcutAction::SpawnShellPane => {
                self.spawn_pane(PaneKind::Shell, window, cx);
                true
            }
            ShortcutAction::SpawnAgentPane => {
                self.spawn_pane(PaneKind::Agent, window, cx);
                true
            }
            ShortcutAction::SpawnBrowserPane => {
                self.spawn_pane(PaneKind::Browser, window, cx);
                true
            }
            ShortcutAction::SpawnEditorPane => {
                self.open_editor_picker(cx);
                true
            }
            ShortcutAction::NextSurface => self.cycle_active_pane_surface(false, cx),
            ShortcutAction::PreviousSurface => self.cycle_active_pane_surface(true, cx),
            ShortcutAction::CloseActivePane => {
                let Some(pane_id) = self.active_workdesk().active_pane else {
                    return false;
                };
                let surface_id = self
                    .active_workdesk()
                    .pane(pane_id)
                    .map(|pane| pane.active_surface_id);
                if let Some(surface_id) = surface_id {
                    self.close_surface(pane_id, surface_id, cx);
                } else {
                    self.close_pane(pane_id, cx);
                }
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
                    let automation_changed = this.process_automation_commands(cx);
                    let blink_changed = this.tick_cursor_blink();
                    if automation_changed || this.sync_terminal_revisions(cx) || blink_changed {
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

    fn sync_terminal_revisions(&mut self, cx: &mut Context<Self>) -> bool {
        let mut changed = false;

        for desk_index in 0..self.workdesks.len() {
            let changed_panes = {
                let desk = &mut self.workdesks[desk_index];
                desk.sync_terminal_revisions()
            };
            if !changed_panes.is_empty() {
                changed = true;
            }

            for (pane_id, surface_id) in changed_panes {
                let snapshot = {
                    let Some(desk) = self.workdesks.get(desk_index) else {
                        continue;
                    };
                    let Some(terminal) = desk.terminals.get(&surface_id) else {
                        continue;
                    };
                    terminal.snapshot()
                };

                let previous_status = self.workdesks[desk_index]
                    .terminal_statuses
                    .get(&surface_id)
                    .cloned()
                    .unwrap_or(None);
                self.workdesks[desk_index]
                    .terminal_statuses
                    .insert(surface_id, snapshot.status.clone());
                self.workdesks[desk_index].note_pane_activity(pane_id);

                if previous_status != snapshot.status {
                    changed = self
                        .handle_terminal_attention_transition(desk_index, pane_id, &snapshot, cx)
                        || changed;
                }
            }
        }

        changed
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
        let sidebar_width = self.sidebar_width();
        let visible_width = (f32::from(viewport.size.width) - sidebar_width).max(1.0);
        gpui::point(px(sidebar_width + visible_width * 0.5), viewport.center().y)
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
        let available_width = (viewport_width - self.sidebar_width() - margin * 2.0).max(1.0);
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

    fn sync_visible_terminal_grids(
        &mut self,
        window: &Window,
        viewport_width: f32,
        viewport_height: f32,
    ) {
        let sidebar_width = self.sidebar_width();
        let layout_mode = self.active_workdesk().layout_mode;
        let grid_expose_open =
            layout_mode == LayoutMode::Grid && self.active_workdesk().grid_layout.expose_open;
        let active_grid_pane_id = self.active_grid_pane_id();
        let camera = self.active_workdesk().camera;
        let zoom = self.active_workdesk().zoom;
        let panes = self.active_workdesk().panes.clone();

        let pane_frames = match layout_mode {
            LayoutMode::Free => panes
                .iter()
                .map(|pane| {
                    (
                        pane.id,
                        PaneViewportFrame {
                            x: camera.x + pane.position.x * zoom,
                            y: camera.y + pane.position.y * zoom,
                            width: pane.size.width * zoom,
                            height: pane.size.height * zoom,
                            zoom,
                            allow_layout_drag: true,
                        },
                    )
                })
                .collect::<Vec<_>>(),
            LayoutMode::ClassicSplit => split_layout_frames(
                &panes,
                active_grid_pane_id,
                viewport_width,
                viewport_height,
                sidebar_width,
            ),
            LayoutMode::Grid => {
                if grid_expose_open {
                    Vec::new()
                } else {
                    active_grid_pane_id
                        .and_then(|pane_id| {
                            panes.iter().find(|pane| pane.id == pane_id).map(|pane| {
                                (
                                    pane.id,
                                    PaneViewportFrame::for_grid(
                                        pane,
                                        viewport_width,
                                        viewport_height,
                                        sidebar_width,
                                    ),
                                )
                            })
                        })
                        .into_iter()
                        .collect::<Vec<_>>()
                }
            }
        };

        let grid_updates = pane_frames
            .into_iter()
            .filter_map(|(pane_id, frame)| {
                let pane = panes.iter().find(|pane| pane.id == pane_id)?;
                pane.surfaces
                    .iter()
                    .any(|surface| surface.kind.is_terminal())
                    .then_some((
                        pane_id,
                        terminal_grid_size_for_frame(
                            frame,
                            pane.surfaces.len(),
                            terminal_text_metrics(window, frame.zoom),
                        ),
                    ))
            })
            .collect::<Vec<_>>();

        let desk = self.active_workdesk_mut();
        for (pane_id, grid) in grid_updates {
            desk.resize_terminals_for_pane_to_grid(pane_id, grid);
        }
    }

    fn spawn_pane_on_workdesk(
        &mut self,
        desk_index: usize,
        kind: PaneKind,
        title: Option<String>,
        focus: bool,
    ) -> PaneId {
        match self.spawn_surface_on_workdesk(desk_index, None, kind, title, None, None, focus) {
            Ok((pane_id, _)) => pane_id,
            Err(error) => {
                self.set_runtime_notice(error);
                self.active_workdesk()
                    .active_pane
                    .unwrap_or_else(|| PaneId::new(0))
            }
        }
    }

    fn spawn_pane(&mut self, kind: PaneKind, window: &Window, cx: &mut Context<Self>) {
        let world_center = self.screen_to_world(self.viewport_center(window));
        let previous_count = self.active_workdesk().panes.len();
        let pane_id = self.spawn_pane_on_workdesk(self.active_workdesk, kind, None, true);
        if let Some(pane) = self
            .active_workdesk_mut()
            .panes
            .iter_mut()
            .find(|pane| pane.id == pane_id)
        {
            let cascade = 36.0 * (previous_count % 6) as f32;
            pane.position = WorkdeskPoint::new(
                world_center.x - pane.size.width * 0.5 + cascade,
                world_center.y - pane.size.height * 0.5 + cascade,
            );
        }
        self.request_persist(cx);
        cx.notify();
    }

    fn open_editor_picker(&mut self, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Open file in Canvas".into()),
        });
        let desk_index = self.active_workdesk;
        cx.spawn(async move |this, cx| {
            let Ok(result) = receiver.await else {
                return;
            };
            let Ok(Some(paths)) = result else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let path_string = path.display().to_string();
            let _ = this.update(cx, |this, cx| {
                match this.spawn_surface_on_workdesk(
                    desk_index,
                    None,
                    PaneKind::Editor,
                    None,
                    None,
                    Some(path_string),
                    true,
                ) {
                    Ok(_) => {
                        this.request_persist(cx);
                        cx.notify();
                    }
                    Err(error) => this.set_runtime_notice(error),
                }
            });
        })
        .detach();
    }

    fn close_pane(&mut self, pane_id: PaneId, cx: &mut Context<Self>) {
        let desk = self.active_workdesk_mut();
        let removed_surfaces = desk
            .pane(pane_id)
            .map(|pane| pane.surfaces.clone())
            .unwrap_or_default();
        desk.panes.retain(|pane| pane.id != pane_id);

        for surface in removed_surfaces {
            if let Some(terminal) = desk.terminals.remove(&surface.id) {
                terminal.close();
            }
            desk.terminal_revisions.remove(&surface.id);
            desk.terminal_statuses.remove(&surface.id);
            desk.terminal_views.remove(&surface.id);
            desk.terminal_grids.remove(&surface.id);
            desk.editors.remove(&surface.id);
            desk.editor_views.remove(&surface.id);
        }
        desk.pane_attention.remove(&pane_id);

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
        let usable_width = (f32::from(viewport.size.width) - self.sidebar_width()).max(220.0);
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
        f32::from(position.x) > self.sidebar_width()
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
        let step_x = (f32::from(viewport.size.width) - self.sidebar_width()).max(220.0)
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
        self.dismiss_workdesk_menu();
        self.dismiss_notifications();
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
                if let Some(surface_id) = desk.active_terminal_surface_id_for_pane(pane_id) {
                    desk.update_selection(surface_id, cell);
                }
                cx.notify();
            }
            DragState::SelectingEditor {
                pane_id,
                surface_id,
            } => {
                let offset = {
                    let Some(editor) = self.active_workdesk().editors.get(&surface_id) else {
                        return;
                    };
                    let Some(view) = self.active_workdesk().editor_views.get(&surface_id) else {
                        return;
                    };
                    view.offset_for_point(editor, event.position)
                };
                let Some(offset) = offset else {
                    return;
                };
                let desk = self.active_workdesk_mut();
                desk.focus_surface(pane_id, surface_id);
                if let Some(editor) = desk.editors.get_mut(&surface_id) {
                    editor.move_to_offset(offset, true);
                }
                self.cursor_blink_visible = true;
                self.last_cursor_blink_at = Instant::now();
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

        if self.handle_workdesk_editor_key_down(event, cx) {
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

        if self.inspector_open {
            let is_escape =
                event.keystroke.key == "escape" && !event.keystroke.modifiers.modified();
            let is_toggle = self
                .shortcuts
                .matching_action(event)
                .is_some_and(|action| action == ShortcutAction::ToggleInspector);

            if is_escape || is_toggle {
                self.close_inspector(cx);
                cx.stop_propagation();
                return;
            }
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

        if self.handle_editor_key_down(event, cx) {
            cx.stop_propagation();
            return;
        }

        let Some(pane_id) = self.active_workdesk().active_pane else {
            return;
        };
        let Some(terminal) = self
            .active_workdesk()
            .active_terminal_session_for_pane(pane_id)
            .cloned()
        else {
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
            let desk = self.active_workdesk_mut();
            desk.clear_selection(pane_id);
            desk.note_pane_activity(pane_id);
            self.cursor_blink_visible = true;
            self.last_cursor_blink_at = Instant::now();
        }

        cx.stop_propagation();
    }

    fn focus_pane(&mut self, pane_id: PaneId, cx: &mut Context<Self>) {
        let desk = self.active_workdesk_mut();
        desk.focus_pane(pane_id);
        desk.note_pane_activity(pane_id);
        self.request_persist(cx);
        cx.notify();
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
        let Some(surface_id) = desk.active_terminal_surface_id_for_pane(pane_id) else {
            return;
        };
        desk.begin_selection(surface_id, metrics.cell_at(position));
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

        let Some(terminal) = self
            .active_workdesk()
            .active_terminal_session_for_pane(pane_id)
        else {
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

    fn begin_editor_selection(
        &mut self,
        pane_id: PaneId,
        surface_id: SurfaceId,
        position: gpui::Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let offset = {
            let Some(editor) = self.active_workdesk().editors.get(&surface_id) else {
                return;
            };
            let Some(view) = self.active_workdesk().editor_views.get(&surface_id) else {
                return;
            };
            view.offset_for_point(editor, position)
        };
        let Some(offset) = offset else {
            return;
        };
        let desk = self.active_workdesk_mut();
        desk.focus_surface(pane_id, surface_id);
        if let Some(editor) = desk.editors.get_mut(&surface_id) {
            editor.move_to_offset(offset, false);
        }
        desk.drag_state = DragState::SelectingEditor {
            pane_id,
            surface_id,
        };
        cx.notify();
    }

    fn on_editor_scroll(
        &mut self,
        surface_id: SurfaceId,
        event: &ScrollWheelEvent,
        cx: &mut Context<Self>,
    ) {
        if event.modifiers.platform {
            return;
        }

        let delta = event.delta.pixel_delta(px(SCROLL_WHEEL_LINE_HEIGHT));
        let dominant_axis = if delta.y.abs() > delta.x.abs() {
            delta.y
        } else {
            delta.x
        };
        let mut lines = (f32::from(dominant_axis) / SCROLL_WHEEL_LINE_HEIGHT).round() as isize;
        if lines == 0 && dominant_axis != px(0.0) {
            lines = if f32::from(dominant_axis).is_sign_positive() {
                1
            } else {
                -1
            };
        }

        let viewport_lines = self
            .active_workdesk()
            .editor_views
            .get(&surface_id)
            .map(|view| view.viewport_lines)
            .unwrap_or(20);
        if let Some(editor) = self.active_workdesk_mut().editors.get_mut(&surface_id) {
            editor.scroll_by_lines(lines, viewport_lines);
            cx.stop_propagation();
            cx.notify();
        }
    }

    fn handle_editor_key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        let Some((pane_id, surface_id)) = self.active_editor_ids() else {
            return false;
        };
        let viewport_lines = self
            .active_workdesk()
            .editor_views
            .get(&surface_id)
            .map(|view| view.viewport_lines)
            .unwrap_or(24);
        let keystroke = &event.keystroke;
        let mut changed = false;
        let mut changed_search_only = false;

        if self
            .active_editor()
            .is_some_and(|editor| editor.search_state().open)
        {
            if keystroke.key == "escape" && !keystroke.modifiers.modified() {
                if let Some(editor) = self.active_editor_mut() {
                    editor.close_search();
                }
                cx.notify();
                return true;
            }
            if keystroke.key == "enter" && !keystroke.modifiers.modified() {
                if let Some(editor) = self.active_editor_mut() {
                    editor.next_search_match();
                }
                cx.notify();
                return true;
            }
            if matches!(keystroke.key.as_str(), "backspace" | "delete")
                && !keystroke.modifiers.modified()
            {
                if let Some(editor) = self.active_editor_mut() {
                    editor.pop_search_text();
                }
                cx.notify();
                return true;
            }
            if let Some(text) = editable_keystroke_text(keystroke) {
                if let Some(editor) = self.active_editor_mut() {
                    editor.append_search_text(&text);
                }
                cx.notify();
                return true;
            }
        }

        if keystroke.modifiers.platform {
            match keystroke.key.as_str() {
                "s" => {
                    match self
                        .active_editor_mut()
                        .and_then(|editor| editor.save().ok())
                    {
                        Some(()) => {
                            self.sync_editor_surface_metadata(pane_id, surface_id);
                            self.request_persist(cx);
                            self.set_runtime_notice("Saved editor buffer");
                            cx.notify();
                        }
                        None => self.set_runtime_notice("Editor save failed"),
                    }
                    return true;
                }
                "f" => {
                    if let Some(editor) = self.active_editor_mut() {
                        editor.open_search();
                    }
                    cx.notify();
                    return true;
                }
                "g" => {
                    if let Some(editor) = self.active_editor_mut() {
                        if keystroke.modifiers.shift {
                            editor.previous_search_match();
                        } else {
                            editor.next_search_match();
                        }
                    }
                    cx.notify();
                    return true;
                }
                "z" => {
                    if let Some(editor) = self.active_editor_mut() {
                        changed = if keystroke.modifiers.shift {
                            editor.redo()
                        } else {
                            editor.undo()
                        };
                    }
                }
                "c" => {
                    if let Some(text) = self
                        .active_editor()
                        .and_then(|editor| editor.selected_text())
                    {
                        cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()));
                    }
                    return true;
                }
                "x" => {
                    if let Some(text) = self
                        .active_editor()
                        .and_then(|editor| editor.selected_text())
                    {
                        cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()));
                    }
                    if let Some(editor) = self.active_editor_mut() {
                        changed = editor.replace_selection("");
                    }
                }
                "v" => {
                    if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                        if let Some(editor) = self.active_editor_mut() {
                            changed = editor.replace_selection(&text);
                        }
                    }
                }
                "a" => {
                    if let Some(editor) = self.active_editor_mut() {
                        editor.select_all();
                    }
                    cx.notify();
                    return true;
                }
                _ => {}
            }
        }

        match keystroke.key.as_str() {
            "left" => {
                if let Some(editor) = self.active_editor_mut() {
                    editor.move_left(keystroke.modifiers.shift);
                }
                changed_search_only = true;
            }
            "right" => {
                if let Some(editor) = self.active_editor_mut() {
                    editor.move_right(keystroke.modifiers.shift);
                }
                changed_search_only = true;
            }
            "up" => {
                if let Some(editor) = self.active_editor_mut() {
                    editor.move_up(keystroke.modifiers.shift);
                }
                changed_search_only = true;
            }
            "down" => {
                if let Some(editor) = self.active_editor_mut() {
                    editor.move_down(keystroke.modifiers.shift);
                }
                changed_search_only = true;
            }
            "home" => {
                if let Some(editor) = self.active_editor_mut() {
                    editor.move_home(keystroke.modifiers.shift);
                }
                changed_search_only = true;
            }
            "end" => {
                if let Some(editor) = self.active_editor_mut() {
                    editor.move_end(keystroke.modifiers.shift);
                }
                changed_search_only = true;
            }
            "pageup" => {
                if let Some(editor) = self.active_editor_mut() {
                    editor.page_up(keystroke.modifiers.shift, viewport_lines);
                }
                changed_search_only = true;
            }
            "pagedown" => {
                if let Some(editor) = self.active_editor_mut() {
                    editor.page_down(keystroke.modifiers.shift, viewport_lines);
                }
                changed_search_only = true;
            }
            "backspace" => {
                if let Some(editor) = self.active_editor_mut() {
                    changed = editor.backspace();
                }
            }
            "delete" => {
                if let Some(editor) = self.active_editor_mut() {
                    changed = editor.delete_forward();
                }
            }
            "enter" => {
                if let Some(editor) = self.active_editor_mut() {
                    changed = editor.insert_newline();
                }
            }
            "tab" if !keystroke.modifiers.modified() || keystroke.modifiers.shift => {
                if let Some(editor) = self.active_editor_mut() {
                    changed = editor.insert_tab();
                }
            }
            _ => {
                if editable_keystroke_text(keystroke).is_none() {
                    return false;
                }
                return false;
            }
        }

        if changed {
            self.sync_editor_surface_metadata(pane_id, surface_id);
            self.request_persist(cx);
        }
        if changed || changed_search_only {
            self.active_workdesk_mut().note_pane_activity(pane_id);
            cx.notify();
            return true;
        }

        false
    }

    fn execute_terminal_shortcut_action(
        &mut self,
        action: ShortcutAction,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(pane_id) = self.active_workdesk().active_pane else {
            return false;
        };
        let Some(terminal) = self
            .active_workdesk()
            .active_terminal_session_for_pane(pane_id)
            .cloned()
        else {
            return false;
        };
        let snapshot = terminal.snapshot();

        match action {
            ShortcutAction::TerminalCopySelection => {
                let selected_text = self
                    .active_workdesk()
                    .active_terminal_view_for_pane(pane_id)
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
                    let desk = self.active_workdesk_mut();
                    desk.clear_selection(pane_id);
                    desk.note_pane_activity(pane_id);
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
                if let Some(surface_id) = desk.active_terminal_surface_id_for_pane(pane_id) {
                    desk.begin_selection(surface_id, TerminalCell { row: 0, col: 0 });
                    desk.update_selection(
                        surface_id,
                        TerminalCell {
                            row: last_row,
                            col: last_col,
                        },
                    );
                }
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
        self.dismiss_notifications();
        self.active_workdesk_mut().drag_state = DragState::Idle;
        self.active_workdesk = index;
        self.active_workdesk_mut().drag_state = DragState::Idle;
        if self.active_workdesk().active_pane.is_none() {
            self.active_workdesk_mut().active_pane =
                self.active_workdesk().panes.last().map(|pane| pane.id);
        }
        if let Some(pane_id) = self.active_workdesk().active_pane {
            self.active_workdesk_mut().mark_pane_attention_seen(pane_id);
        }
        self.request_persist(cx);
        cx.notify();
    }

    fn dismiss_workdesk_menu(&mut self) -> bool {
        self.workdesk_menu.take().is_some()
    }

    fn dismiss_notifications(&mut self) -> bool {
        let was_open = self.notifications_open;
        self.notifications_open = false;
        was_open
    }

    fn toggle_notifications(&mut self, cx: &mut Context<Self>) {
        self.notifications_open = !self.notifications_open;
        if self.notifications_open {
            self.mock_notifications_unread = 0;
            self.dismiss_workdesk_menu();
        }
        cx.notify();
    }

    fn toggle_sidebar_collapsed(&mut self, cx: &mut Context<Self>) {
        self.sidebar_collapsed = !self.sidebar_collapsed;
        self.dismiss_workdesk_menu();
        self.dismiss_notifications();
        cx.notify();
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

    fn unique_workdesk_name_except(&self, index: usize, base: &str) -> String {
        if self
            .workdesks
            .iter()
            .enumerate()
            .all(|(desk_index, desk)| desk_index == index || desk.name != base)
        {
            return base.to_string();
        }

        let mut serial = 2;
        loop {
            let candidate = format!("{base} {serial}");
            if self
                .workdesks
                .iter()
                .enumerate()
                .all(|(desk_index, desk)| desk_index == index || desk.name != candidate)
            {
                return candidate;
            }
            serial += 1;
        }
    }

    fn spawn_workdesk(&mut self, cx: &mut Context<Self>) {
        self.dismiss_workdesk_menu();
        let mut desk = workdesk_from_template(
            WorkdeskTemplate::ShellDesk,
            WorkdeskDraft::from_template(
                self.workdesk_name_for_template(WorkdeskTemplate::ShellDesk),
                WorkdeskTemplate::ShellDesk,
            ),
        );
        boot_workdesk_terminals(&mut desk);
        self.workdesks.push(desk);
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
        _stack_index: usize,
        frame: PaneViewportFrame,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let workdesk = self.active_workdesk();
        let pane_id = pane.id;
        let active_surface = pane
            .active_surface()
            .expect("pane should always contain an active surface");
        let active_surface_id = active_surface.id;
        let active_surface_kind = active_surface.kind.clone();
        let active_surface_title = active_surface.title.clone();
        let active_surface_dirty = active_surface.dirty;
        let active_browser_url = active_surface.browser_url.clone();
        let is_active = workdesk.active_pane == Some(pane.id);
        let accent = pane_accent(&active_surface_kind);
        let pane_attention = workdesk.pane_attention(pane.id);
        let attention_tint = pane_attention.state.tint();
        let border = if pane_attention.state.is_attention() {
            attention_tint
        } else if is_active {
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
        let header_height = terminal_header_height_for_surface_count(pane.surfaces.len())
            * frame.zoom.clamp(0.78, 1.3);
        let pane_padding = 12.0 * frame.zoom.clamp(0.85, 1.25);
        let resize_handle_size = 14.0 * frame.zoom.clamp(0.85, 1.3);
        let terminal_metrics = terminal_text_metrics(window, frame.zoom);
        let terminal_snapshot = active_surface_kind
            .is_terminal()
            .then(|| {
                workdesk
                    .terminals
                    .get(&active_surface_id)
                    .map(|terminal| terminal.snapshot())
            })
            .flatten();
        let terminal_view = workdesk
            .terminal_views
            .get(&active_surface_id)
            .cloned()
            .unwrap_or_default();
        let runtime_title = terminal_snapshot
            .as_ref()
            .map(|snapshot| snapshot.title.clone())
            .unwrap_or_else(|| active_surface_title.clone());
        let status_tint = if pane_attention.state == AttentionState::Idle {
            match active_surface_kind {
                PaneKind::Shell | PaneKind::Agent => terminal_snapshot
                    .as_ref()
                    .map(|snapshot| {
                        if snapshot.closed {
                            rgb(0xff9b88).into()
                        } else if is_active {
                            accent
                        } else {
                            rgb(0x55616b).into()
                        }
                    })
                    .unwrap_or_else(|| rgb(0x55616b).into()),
                PaneKind::Browser => {
                    if is_active {
                        accent
                    } else {
                        rgb(0x55616b).into()
                    }
                }
                PaneKind::Editor => {
                    if active_surface_dirty {
                        rgb(0xf0d35f).into()
                    } else if is_active {
                        accent
                    } else {
                        rgb(0x55616b).into()
                    }
                }
            }
        } else {
            attention_tint
        };
        let entity = cx.entity();

        let header = {
            let close_surface_id = active_surface_id;
            let close_surface_count = pane.surfaces.len();
            let header = div()
                .flex()
                .flex_col()
                .justify_center()
                .gap_1()
                .h(px(header_height))
                .px(px(pane_padding))
                .py(px(6.0 * frame.zoom.clamp(0.9, 1.2)))
                .bg(header_bg)
                .border_b_1()
                .border_color(border)
                .child(
                    div()
                        .flex()
                        .justify_between()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .flex_1()
                                .flex()
                                .items_center()
                                .gap_2()
                                .overflow_hidden()
                                .child(
                                    div()
                                        .cursor_pointer()
                                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                            cx.stop_propagation();
                                        })
                                        .on_mouse_up(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                this.cycle_manual_pane_attention(pane_id, cx);
                                                cx.stop_propagation();
                                            }),
                                        )
                                        .child(attention_indicator(
                                            pane_attention.state,
                                            pane_attention.unread,
                                        )),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(status_tint)
                                        .child(surface_kind_slug(&active_surface_kind)),
                                )
                                .child(div().text_sm().child(runtime_title.clone())),
                        )
                        .child(
                            div().flex().items_center().gap_1().child(
                                div()
                                    .cursor_pointer()
                                    .w(px(24.0))
                                    .h(px(24.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded_md()
                                    .bg(rgb(0x23171a))
                                    .text_xs()
                                    .text_color(rgb(0xffc4b5))
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                        cx.stop_propagation();
                                    })
                                    .on_mouse_up(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            if close_surface_count > 1 {
                                                this.close_surface(pane_id, close_surface_id, cx);
                                            } else {
                                                this.close_pane(pane_id, cx);
                                            }
                                            cx.stop_propagation();
                                        }),
                                    )
                                    .child("X"),
                            ),
                        ),
                );

            let header = if pane.surfaces.len() > 1 {
                header.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .overflow_hidden()
                        .children(
                            pane.surfaces
                                .iter()
                                .map(|surface| {
                                    let surface_id = surface.id;
                                    let active = surface_id == active_surface_id;
                                    surface_stack_chip(
                                        surface,
                                        active,
                                        pane_accent(&surface.kind),
                                        cx.listener(move |this, _, window, cx| {
                                            this.active_workdesk_mut()
                                                .focus_surface(pane_id, surface_id);
                                            this.active_workdesk_mut().note_pane_activity(pane_id);
                                            this.cursor_blink_visible = true;
                                            this.last_cursor_blink_at = Instant::now();
                                            this.request_persist(cx);
                                            window.focus(&this.focus_handle);
                                            cx.stop_propagation();
                                            cx.notify();
                                        }),
                                    )
                                })
                                .collect::<Vec<_>>(),
                        ),
                )
            } else {
                header
            };

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

        let body = match active_surface_kind {
            PaneKind::Shell | PaneKind::Agent => {
                let terminal_body_metrics = terminal_snapshot.as_ref().map(|snapshot| {
                    terminal_frame_metrics(
                        terminal_metrics,
                        screen_x,
                        screen_y,
                        header_height,
                        pane_padding,
                        snapshot,
                    )
                });
                let terminal_body = terminal_body(
                    &terminal_snapshot,
                    &terminal_view,
                    terminal_metrics,
                    is_active,
                    self.cursor_blink_visible,
                );
                div()
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
                    .child(terminal_body)
                    .into_any_element()
            }
            PaneKind::Editor => {
                let font_size = 12.5 * frame.zoom.clamp(0.82, 1.18);
                let line_height = 16.0 * frame.zoom.clamp(0.85, 1.18);
                let char_width = 7.4 * frame.zoom.clamp(0.85, 1.18);
                if let Some(editor) = workdesk.editors.get(&active_surface_id) {
                    let search_matches = editor.search_matches();
                    let search_height = if editor.search_state().open {
                        34.0
                    } else {
                        0.0
                    };
                    let footer_height = 26.0;
                    let viewport_lines = ((screen_height
                        - header_height
                        - pane_padding * 2.0
                        - search_height
                        - footer_height
                        - 24.0)
                        / line_height)
                        .floor()
                        .max(1.0) as usize;
                    let gutter_width =
                        ((editor.line_number_width() + 1) as f32 * char_width + 18.0).max(44.0);
                    let line_number_width = editor.line_number_width();
                    let visible_lines = editor
                        .visible_line_range(viewport_lines)
                        .map(|line_index| {
                            let line_label =
                                format!("{:>width$}", line_index + 1, width = line_number_width);
                            div()
                                .h(px(line_height))
                                .flex()
                                .items_start()
                                .child(
                                    div()
                                        .w(px(gutter_width))
                                        .pr_3()
                                        .text_right()
                                        .text_color(rgb(0x63717b))
                                        .child(line_label),
                                )
                                .child(div().flex_1().whitespace_nowrap().child(
                                    editor_line_display(
                                        editor,
                                        line_index,
                                        is_active,
                                        self.cursor_blink_visible,
                                    ),
                                ))
                        })
                        .collect::<Vec<_>>();
                    let search_label = editor
                        .search_state()
                        .active_match
                        .map(|index| format!("{}/{}", index + 1, search_matches.len()))
                        .unwrap_or_else(|| format!("0/{}", search_matches.len()));
                    let path_label = editor.path_string();
                    let language_label = editor_language_label(editor);
                    let dirty = editor.dirty();
                    let external_modified = editor.external_modified();
                    let surface_id = active_surface_id;

                    div()
                        .flex()
                        .flex_1()
                        .overflow_hidden()
                        .p(px(pane_padding))
                        .child(
                            div()
                                .flex_1()
                                .flex()
                                .flex_col()
                                .gap_2()
                                .when(editor.search_state().open, |column| {
                                    column.child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .justify_between()
                                            .gap_3()
                                            .px_3()
                                            .py_2()
                                            .bg(rgb(0x131a20))
                                            .border_1()
                                            .border_color(rgb(0x24303a))
                                            .rounded_md()
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(rgb(0xf0d35f))
                                                    .child("Find"),
                                            )
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .text_xs()
                                                    .text_color(rgb(0xdce2e8))
                                                    .child(editor.search_state().query.clone()),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(rgb(0x7f8a94))
                                                    .child(search_label),
                                            ),
                                    )
                                })
                                .child(
                                    div()
                                        .relative()
                                        .flex_1()
                                        .overflow_hidden()
                                        .p_2()
                                        .bg(rgb(0x0f151a))
                                        .border_1()
                                        .border_color(rgb(0x24303a))
                                        .rounded_md()
                                        .font_family(".ZedMono")
                                        .text_size(px(font_size))
                                        .line_height(px(line_height))
                                        .text_color(rgb(0xdce2e8))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(
                                                move |this, event: &MouseDownEvent, window, cx| {
                                                    this.begin_editor_selection(
                                                        pane_id,
                                                        surface_id,
                                                        event.position,
                                                        cx,
                                                    );
                                                    window.focus(&this.focus_handle);
                                                    cx.stop_propagation();
                                                },
                                            ),
                                        )
                                        .on_scroll_wheel(cx.listener(
                                            move |this, event: &ScrollWheelEvent, _, cx| {
                                                this.on_editor_scroll(surface_id, event, cx);
                                            },
                                        ))
                                        .child(
                                            div().flex().flex_col().gap_0().children(visible_lines),
                                        )
                                        .child(
                                            div()
                                                .absolute()
                                                .left(px(gutter_width + 8.0))
                                                .top(px(8.0))
                                                .right(px(8.0))
                                                .bottom(px(8.0))
                                                .child(EditorInputOverlay {
                                                    shell: entity.clone(),
                                                    active: is_active,
                                                    surface_id,
                                                    line_height,
                                                    char_width,
                                                    viewport_lines,
                                                }),
                                        ),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .justify_between()
                                        .items_center()
                                        .gap_3()
                                        .child(
                                            div()
                                                .flex_1()
                                                .text_xs()
                                                .text_color(rgb(0x7f8a94))
                                                .overflow_hidden()
                                                .whitespace_nowrap()
                                                .child(path_label),
                                        )
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap_2()
                                                .child(context_pill(language_label, accent))
                                                .when(dirty, |row| {
                                                    row.child(context_pill(
                                                        "Dirty",
                                                        rgb(0xf0d35f).into(),
                                                    ))
                                                })
                                                .when(external_modified, |row| {
                                                    row.child(context_pill(
                                                        "Externally changed",
                                                        rgb(0xff9b88).into(),
                                                    ))
                                                }),
                                        ),
                                ),
                        )
                        .into_any_element()
                } else {
                    div()
                        .flex()
                        .flex_1()
                        .overflow_hidden()
                        .p(px(pane_padding))
                        .child(
                            div()
                                .flex_1()
                                .flex()
                                .items_center()
                                .justify_center()
                                .bg(rgb(0x0f151a))
                                .border_1()
                                .border_color(rgb(0x24303a))
                                .rounded_md()
                                .text_color(rgb(0xffb7a6))
                                .child("editor buffer offline"),
                        )
                        .into_any_element()
                }
            }
            PaneKind::Browser => {
                let url = active_browser_url.unwrap_or_else(|| "https://example.com".to_string());
                let open_url = url.clone();
                div()
                    .flex()
                    .flex_1()
                    .overflow_hidden()
                    .p(px(pane_padding))
                    .child(browser_preview_card(
                        &url,
                        accent,
                        cx.listener(move |_, _, _, cx| {
                            cx.open_url(&open_url);
                            cx.stop_propagation();
                        }),
                    ))
                    .into_any_element()
            }
        };

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
        self.sync_visible_terminal_grids(window, viewport_width, viewport_height);
        let sidebar_width = self.sidebar_width();
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
                    sidebar_width,
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
            PaneViewportFrame::for_grid(pane, viewport_width, viewport_height, sidebar_width).zoom
        });
        let active_split_zoom = self.active_grid_pane_id().and_then(|pane_id| {
            split_frames
                .iter()
                .find(|(candidate_id, _)| *candidate_id == pane_id)
                .map(|(_, frame)| frame.zoom)
        });
        let expose_layout = grid_expose_open
            .then(|| {
                expose_layout_frame(
                    &workdesk.panes,
                    viewport_width,
                    viewport_height,
                    sidebar_width,
                )
            })
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
                let viewport_left = sidebar_width + GRID_ACTIVE_MARGIN_X * 0.45;
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
                .left(px(sidebar_width + SPLIT_MARGIN_X))
                .top(px(SPLIT_MARGIN_TOP))
                .w(px(
                    (viewport_width - sidebar_width - SPLIT_MARGIN_X * 2.0).max(320.0)
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
                        window,
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
                                .map(|pane| self.pane_surface(pane, index, *frame, window, cx))
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
                                PaneViewportFrame::for_grid(
                                    pane,
                                    viewport_width,
                                    viewport_height,
                                    sidebar_width,
                                ),
                                window,
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
                        sidebar_width,
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
                .left(px(sidebar_width))
                .top(px(0.0))
                .w(px(viewport_width - sidebar_width))
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
        let inspector_zoom_label = match layout_mode {
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
        let show_inspector = cfg!(debug_assertions) && self.inspector_open;
        let inspector_toggle_label = self.shortcut_label(ShortcutAction::ToggleInspector);
        let sidebar_header_inset = if cfg!(target_os = "macos") {
            SIDEBAR_WINDOW_CONTROLS_INSET
        } else {
            0.0
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
                        .rev()
                        .take(2)
                        .map(|pane| pane.title.as_str())
                        .collect::<Vec<_>>()
                        .join(" · ")
                };
                let accent = workdesk_accent(index);
                if self.sidebar_collapsed {
                    workdesk_compact_chip(
                        index,
                        desk,
                        index == self.active_workdesk,
                        accent,
                        cx.listener(move |this, _, _, cx| {
                            this.dismiss_workdesk_menu();
                            this.select_workdesk(index, cx);
                            cx.stop_propagation();
                        }),
                        cx.listener(move |this, event: &MouseUpEvent, _, cx| {
                            this.open_workdesk_menu(index, event.position, cx);
                            cx.stop_propagation();
                        }),
                    )
                    .into_any_element()
                } else {
                    workdesk_card(
                        index,
                        desk,
                        index == self.active_workdesk,
                        is_menu_open,
                        preview,
                        accent,
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
                    .into_any_element()
                }
            })
            .collect::<Vec<_>>();
        let notification_panel_left = if self.sidebar_collapsed {
            (sidebar_width + 12.0).max(84.0)
        } else {
            sidebar_width + 12.0
        }
        .min((viewport_width - NOTIFICATION_PANEL_WIDTH - 16.0).max(12.0));
        let notification_overlay = self.notifications_open.then(|| {
            div()
                .absolute()
                .left(px(notification_panel_left))
                .top(px(22.0 + sidebar_header_inset))
                .w(px(
                    NOTIFICATION_PANEL_WIDTH.min((viewport_width - 24.0).max(220.0))
                ))
                .max_h(px(
                    (viewport_height - 32.0 - sidebar_header_inset).max(180.0)
                ))
                .p_3()
                .flex()
                .flex_col()
                .gap_2()
                .bg(rgb(0x0f151b))
                .border_1()
                .border_color(rgb(0x2b3641))
                .rounded_xl()
                .shadow_lg()
                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                    cx.stop_propagation();
                })
                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    if this.dismiss_notifications() {
                        cx.notify();
                    }
                    cx.stop_propagation();
                }))
                .child(
                    div()
                        .flex()
                        .items_start()
                        .justify_between()
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
                                        .child("Mock inbox"),
                                )
                                .child(div().text_sm().child("Notifications"))
                                .child(div().text_xs().text_color(rgb(0x8e9ba5)).child(
                                    "Temporary center until real notification plumbing lands.",
                                )),
                        )
                        .child(control_button(
                            "Close",
                            rgb(0x7f8a94).into(),
                            cx.listener(|this, _, _, cx| {
                                if this.dismiss_notifications() {
                                    cx.notify();
                                }
                                cx.stop_propagation();
                            }),
                        )),
                )
                .child(notification_item(
                    "Agent ready",
                    "Implement Agent finished a response and left the pane waiting.",
                    rgb(0x7cc7ff).into(),
                    true,
                ))
                .child(notification_item(
                    "Build changed",
                    "Implementation desk build status moved to 25% for the current branch.",
                    rgb(0x77d19a).into(),
                    false,
                ))
                .child(notification_item(
                    "Review pending",
                    "Shell Desk has a waiting pane that has not been revisited yet.",
                    rgb(0xe59a49).into(),
                    false,
                ))
        });
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
            let menu_height = if can_delete { 220.0 } else { 174.0 };
            let min_left = if self.sidebar_collapsed {
                sidebar_width + 8.0
            } else {
                12.0
            };
            let max_left = (viewport_width - WORKDESK_MENU_WIDTH - 12.0).max(min_left);
            let max_top = (viewport_height - menu_height - 12.0).max(12.0);
            let left = (f32::from(menu.position.x) + 8.0).clamp(min_left, max_left);
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
                        "Edit",
                        "Rename and update intent",
                        rgb(0x77d19a).into(),
                        cx.listener(move |this, _, _, cx| {
                            this.open_workdesk_editor_panel(menu.index, cx);
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
        let workdesk_editor_overlay = self.workdesk_editor.as_ref().map(|editor| {
            let accent = match editor.mode {
                WorkdeskEditorMode::Create => editor.template.accent(),
                WorkdeskEditorMode::Edit(index) => workdesk_accent(index),
            };
            let template_cards = WorkdeskTemplate::all()
                .into_iter()
                .map(|template| {
                    workdesk_template_card(
                        template,
                        editor.template == template,
                        cx.listener(move |this, _, _, cx| {
                            this.select_workdesk_template(template, cx);
                            cx.stop_propagation();
                        }),
                    )
                })
                .collect::<Vec<_>>();
            let editor_fields = [
                WorkdeskEditorField::Name,
                WorkdeskEditorField::Intent,
                WorkdeskEditorField::Summary,
            ]
            .into_iter()
            .map(|field| {
                workdesk_editor_field(
                    field.label(),
                    editor.draft.field_value(field).to_string(),
                    editor.active_field == field,
                    accent,
                    cx.listener(move |this, _, _, cx| {
                        if let Some(editor) = this.workdesk_editor.as_mut() {
                            editor.active_field = field;
                            cx.notify();
                        }
                        cx.stop_propagation();
                    }),
                )
            })
            .collect::<Vec<_>>();
            let metadata_cwd = compact_cwd_label(&editor.draft.metadata.cwd);
            let metadata_branch = if editor.draft.metadata.branch.trim().is_empty() {
                "no-branch".to_string()
            } else {
                editor.draft.metadata.branch.clone()
            };

            div()
                .child(
                    div()
                        .absolute()
                        .left(px(0.0))
                        .top(px(0.0))
                        .w(px(viewport_width))
                        .h(px(viewport_height))
                        .bg(rgba(0x091016a6))
                        .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                            this.close_workdesk_editor(cx);
                            cx.stop_propagation();
                        })),
                )
                .child(
                    div()
                        .absolute()
                        .top(px((viewport_height * 0.14).max(44.0)))
                        .left(px(
                            ((viewport_width - WORKDESK_EDITOR_WIDTH) * 0.5)
                                .max(sidebar_width + 16.0),
                        ))
                        .w(px(
                            WORKDESK_EDITOR_WIDTH.min(
                                (viewport_width - sidebar_width - FLOATING_DOCK_MARGIN * 2.0)
                                    .max(320.0),
                            ),
                        ))
                        .flex()
                        .flex_col()
                        .gap_3()
                        .p_4()
                        .bg(rgb(0x0f151b))
                        .border_1()
                        .border_color(rgb(0x2c3944))
                        .rounded_xl()
                        .shadow_lg()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
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
                                                .text_color(accent)
                                                .child(editor.title()),
                                        )
                                        .child(div().text_lg().child(editor.draft.name.clone()))
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(rgb(0x90a0aa))
                                                .child(
                                                    "Tab switches fields. Enter saves. Escape cancels.",
                                                ),
                                        ),
                                )
                                .child(compact_dock_button(
                                    "Close",
                                    rgb(0xff9b88).into(),
                                    cx.listener(|this, _, _, cx| {
                                        this.close_workdesk_editor(cx);
                                        cx.stop_propagation();
                                    }),
                                )),
                        )
                        .when(matches!(editor.mode, WorkdeskEditorMode::Create), |panel| {
                            panel.child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(rgb(0x90a0aa))
                                            .child("Templates"),
                                    )
                                    .children(template_cards),
                            )
                        })
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .flex_wrap()
                                .child(context_pill(metadata_cwd, rgb(0x7f8a94).into()))
                                .child(context_pill(metadata_branch, rgb(0x7cc7ff).into()))
                                .when_some(editor.draft.metadata.status_label(), |row, status| {
                                    row.child(context_pill(status, accent))
                                })
                                .when_some(editor.draft.metadata.progress_label(), |row, progress| {
                                    row.child(context_pill(progress, rgb(0x77d19a).into()))
                                }),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_2()
                                .children(editor_fields),
                        )
                        .child(
                            div()
                                .flex()
                                .justify_between()
                                .items_center()
                                .gap_3()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(0x7f8a94))
                                        .child(match editor.mode {
                                            WorkdeskEditorMode::Create => {
                                                "Shortcut quick-create still makes a Shell Desk."
                                            }
                                            WorkdeskEditorMode::Edit(_) => {
                                                "Status, branch, cwd, and progress stay on the desk metadata."
                                            }
                                        }),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .child(control_button(
                                            "Cancel",
                                            rgb(0x7f8a94).into(),
                                            cx.listener(|this, _, _, cx| {
                                                this.close_workdesk_editor(cx);
                                                cx.stop_propagation();
                                            }),
                                        ))
                                        .child(control_button(
                                            editor.submit_label(),
                                            accent,
                                            cx.listener(|this, _, _, cx| {
                                                this.commit_workdesk_editor(cx);
                                                cx.stop_propagation();
                                            }),
                                        )),
                                ),
                        ),
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
                    .w(px(sidebar_width))
                    .h(px(viewport_height))
                    .relative()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .px(if self.sidebar_collapsed {
                        px(2.0)
                    } else {
                        px(12.0)
                    })
                    .pb_3()
                    .pt(px(14.0 + sidebar_header_inset))
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
                    .when(!self.sidebar_collapsed, |sidebar| {
                        sidebar.child(
                            div()
                                .absolute()
                                .left(px(10.0))
                                .top(px(10.0))
                                .w(px((sidebar_width - 20.0)
                                    .max(178.0)
                                    .min((viewport_width - 20.0).max(120.0))))
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap_2()
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_1()
                                        .when(cfg!(target_os = "macos"), |row| {
                                            row.child(div().w(px(56.0)))
                                        })
                                        .child(chrome_button(
                                            "Bell",
                                            rgb(0x7cc7ff).into(),
                                            self.notifications_open,
                                            Some(self.mock_notifications_unread),
                                            cx.listener(|this, _, _, cx| {
                                                this.toggle_notifications(cx);
                                                cx.stop_propagation();
                                            }),
                                        ))
                                        .child(chrome_button(
                                            "Hide",
                                            rgb(0x90a0aa).into(),
                                            false,
                                            None,
                                            cx.listener(|this, _, _, cx| {
                                                this.toggle_sidebar_collapsed(cx);
                                                cx.stop_propagation();
                                            }),
                                        )),
                                )
                                .child(compact_dock_button(
                                    "+",
                                    rgb(0x77d19a).into(),
                                    cx.listener(|this, _, _, cx| {
                                        this.open_workdesk_creator(cx);
                                        cx.stop_propagation();
                                    }),
                                )),
                        )
                    })
                    .when(!self.sidebar_collapsed, |sidebar| {
                        sidebar.child(
                            div().flex().items_center().justify_between().gap_2().child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(2.0))
                                    .child(
                                        div().text_xs().text_color(rgb(0x7f8a94)).child("Canvas"),
                                    )
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(rgb(0xdce2e8))
                                            .child("Workdesks"),
                                    ),
                            ),
                        )
                    })
                    .when(self.sidebar_collapsed, |sidebar| {
                        sidebar
                            .child(
                                div()
                                    .absolute()
                                    .left(px(10.0))
                                    .top(px(14.0 + sidebar_header_inset))
                                    .flex()
                                    .flex_col()
                                    .gap_2()
                                    .child(chrome_button(
                                        "N",
                                        rgb(0x7cc7ff).into(),
                                        self.notifications_open,
                                        Some(self.mock_notifications_unread),
                                        cx.listener(|this, _, _, cx| {
                                            this.toggle_notifications(cx);
                                            cx.stop_propagation();
                                        }),
                                    ))
                                    .child(chrome_button(
                                        ">",
                                        rgb(0x90a0aa).into(),
                                        false,
                                        None,
                                        cx.listener(|this, _, _, cx| {
                                            this.toggle_sidebar_collapsed(cx);
                                            cx.stop_propagation();
                                        }),
                                    )),
                            )
                            .child(div().flex().justify_center().child(compact_dock_button(
                                "+",
                                rgb(0x77d19a).into(),
                                cx.listener(|this, _, _, cx| {
                                    this.open_workdesk_creator(cx);
                                    cx.stop_propagation();
                                }),
                            )))
                    })
                    .child(
                        div()
                            .id("workdesk-rail-scroll")
                            .flex_1()
                            .overflow_y_scroll()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .when(self.sidebar_collapsed, |rail| rail.items_center())
                            .children(workdesk_cards),
                    ),
            )
            .child(
                div()
                    .absolute()
                    .left(px(sidebar_width))
                    .bottom(px(0.0))
                    .w(px((viewport_width - sidebar_width).max(240.0)))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_4()
                    .px_4()
                    .py_2()
                    .bg(rgba(0x0d1217f0))
                    .border_t_1()
                    .border_color(rgb(0x25303a))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .on_scroll_wheel(|_, _, cx| {
                        cx.stop_propagation();
                    })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(div().text_xs().text_color(rgb(0x7f8a94)).child("Create"))
                            .child(compact_dock_button(
                                "Shell",
                                rgb(0xe59a49).into(),
                                cx.listener(|this, _, window, cx| {
                                    this.spawn_pane(PaneKind::Shell, window, cx);
                                    cx.stop_propagation();
                                }),
                            ))
                            .child(compact_dock_button(
                                "Agent",
                                rgb(0x7cc7ff).into(),
                                cx.listener(|this, _, window, cx| {
                                    this.spawn_pane(PaneKind::Agent, window, cx);
                                    cx.stop_propagation();
                                }),
                            ))
                            .child(compact_dock_button(
                                "Browser",
                                rgb(0x77d19a).into(),
                                cx.listener(|this, _, window, cx| {
                                    this.spawn_pane(PaneKind::Browser, window, cx);
                                    cx.stop_propagation();
                                }),
                            ))
                            .child(compact_dock_button(
                                "Editor",
                                rgb(0xb4a4ff).into(),
                                cx.listener(|this, _, _, cx| {
                                    this.open_editor_picker(cx);
                                    cx.stop_propagation();
                                }),
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(div().text_xs().text_color(rgb(0x7f8a94)).child("Layout"))
                            .child(compact_toggle_button(
                                "Free",
                                rgb(0x77d19a).into(),
                                layout_mode == LayoutMode::Free,
                                cx.listener(|this, _, _, cx| {
                                    this.set_layout_mode(LayoutMode::Free, cx);
                                    cx.stop_propagation();
                                }),
                            ))
                            .child(compact_toggle_button(
                                "Grid",
                                rgb(0xf0d35f).into(),
                                layout_mode == LayoutMode::Grid,
                                cx.listener(|this, _, _, cx| {
                                    this.set_layout_mode(LayoutMode::Grid, cx);
                                    cx.stop_propagation();
                                }),
                            ))
                            .child(compact_toggle_button(
                                "Split",
                                rgb(0x7cc7ff).into(),
                                layout_mode == LayoutMode::ClassicSplit,
                                cx.listener(|this, _, _, cx| {
                                    this.set_layout_mode(LayoutMode::ClassicSplit, cx);
                                    cx.stop_propagation();
                                }),
                            ))
                            .child(dock_divider())
                            .child(compact_toggle_button(
                                "Keys",
                                rgb(0xb4a4ff).into(),
                                self.shortcut_editor.open,
                                cx.listener(|this, _, _, cx| {
                                    this.toggle_shortcut_panel(cx);
                                    cx.stop_propagation();
                                }),
                            ))
                            .when(layout_mode == LayoutMode::Grid, |dock| {
                                dock.child(compact_toggle_button(
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
                                dock.child(dock_divider()).child(compact_dock_button(
                                    "Fit",
                                    rgb(0x77d19a).into(),
                                    cx.listener(|this, _, window, cx| {
                                        this.fit_to_panes(window, cx);
                                        cx.stop_propagation();
                                    }),
                                ))
                            }),
                    ),
            )
            .when(show_inspector, |root| {
                root.child(
                    div()
                        .absolute()
                        .left(px(0.0))
                        .top(px(0.0))
                        .w(px(viewport_width))
                        .h(px(viewport_height))
                        .bg(rgba(0x09101680))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.close_inspector(cx);
                                cx.stop_propagation();
                            }),
                        ),
                )
                .child(
                    div()
                        .absolute()
                        .top(px(72.0))
                        .right(px(FLOATING_DOCK_MARGIN))
                        .w(px(
                            INSPECTOR_PANEL_WIDTH.min((viewport_width - 32.0).max(280.0))
                        ))
                        .flex()
                        .flex_col()
                        .gap_3()
                        .p_4()
                        .bg(rgb(0x0f151b))
                        .border_1()
                        .border_color(rgb(0x2c3944))
                        .rounded_xl()
                        .shadow_lg()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
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
                                                .child("Developer Inspector"),
                                        )
                                        .child(div().text_lg().child("Debug surface"))
                                        .child(div().text_xs().text_color(rgb(0x90a0aa)).child(
                                            format!("Toggle with {}", inspector_toggle_label),
                                        )),
                                )
                                .child(compact_dock_button(
                                    "Close",
                                    rgb(0xff9b88).into(),
                                    cx.listener(|this, _, _, cx| {
                                        this.close_inspector(cx);
                                        cx.stop_propagation();
                                    }),
                                )),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_2()
                                .child(inspector_row("Socket", self.automation_socket_path.clone()))
                                .child(inspector_row("Bridge", self.ghostty_status.clone()))
                                .child(inspector_row("Vendor", self.ghostty_vendor_dir.clone()))
                                .child(inspector_row("Desk", workdesk.name.clone()))
                                .child(inspector_row("Pane", active_pane))
                                .child(inspector_row("Layout", layout_mode.label()))
                                .child(inspector_row("Zoom", inspector_zoom_label))
                                .child(inspector_row("Drag", workdesk.drag_status()))
                                .child(inspector_row("Keys", toggle_shortcut_label.clone())),
                        ),
                )
            })
            .children(shortcut_overlay)
            .when_some(workdesk_editor_overlay, |root, overlay| root.child(overlay))
            .when_some(notification_overlay, |root, overlay| root.child(overlay))
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
    let (automation_server, automation_notice) = match start_automation_server() {
        Ok(server) => (server, None),
        Err(error) => {
            let (_sender, receiver) = mpsc::channel();
            (
                AutomationServer {
                    receiver,
                    socket_path: automation_socket_path(),
                },
                Some(SharedString::from(format!(
                    "automation socket disabled: {error}"
                ))),
            )
        }
    };
    let boot_notice = match (boot_notice, automation_notice) {
        (Some(left), Some(right)) => Some(SharedString::from(format!("{left} | {right}"))),
        (Some(left), None) => Some(left),
        (None, Some(right)) => Some(right),
        (None, None) => None,
    };
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
        let automation_socket_path = automation_server.socket_path.clone();
        let ghostty_vendor_dir = ghostty_vendor_dir.clone();
        let ghostty_status = ghostty_status.clone();

        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some(SharedString::from("Canvas")),
                    appears_transparent: true,
                    traffic_light_position: cfg!(target_os = "macos")
                        .then(|| gpui::point(px(14.0), px(14.0))),
                }),
                ..Default::default()
            },
            move |window, cx| {
                let workdesks = workdesks.clone();
                let active_workdesk = active_workdesk;
                let shortcuts = shortcuts.clone();
                let boot_notice = boot_notice.clone();
                let automation_server = AutomationServer {
                    receiver: automation_server.receiver,
                    socket_path: automation_socket_path,
                };
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
                        automation_server,
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

fn automation_socket_path() -> PathBuf {
    workspace_root_path().join(".canvas").join("canvas.sock")
}

fn start_automation_server() -> Result<AutomationServer, String> {
    start_automation_server_at(automation_socket_path())
}

fn start_automation_server_at(socket_path: PathBuf) -> Result<AutomationServer, String> {
    let Some(socket_dir) = socket_path.parent() else {
        return Err("invalid automation socket path".to_string());
    };
    fs::create_dir_all(socket_dir)
        .map_err(|error| format!("create {}: {error}", socket_dir.display()))?;
    if socket_path.exists() {
        fs::remove_file(&socket_path)
            .map_err(|error| format!("remove stale {}: {error}", socket_path.display()))?;
    }

    let listener = UnixListener::bind(&socket_path)
        .map_err(|error| format!("bind {}: {error}", socket_path.display()))?;
    fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("chmod {}: {error}", socket_path.display()))?;
    let (sender, receiver) = mpsc::channel();

    thread::spawn(move || automation_listener_loop(listener, sender));

    Ok(AutomationServer {
        receiver,
        socket_path,
    })
}

fn automation_listener_loop(listener: UnixListener, sender: Sender<AutomationEnvelope>) {
    for stream in listener.incoming() {
        let Ok(stream) = stream else {
            break;
        };
        let sender = sender.clone();
        thread::spawn(move || {
            let reader = match stream.try_clone() {
                Ok(reader) => reader,
                Err(_) => return,
            };
            let mut writer = stream;
            for line in BufReader::new(reader).lines() {
                let Ok(line) = line else {
                    break;
                };
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }

                let response = match serde_json::from_str::<AutomationRequest>(trimmed) {
                    Ok(request) => {
                        let (response_tx, response_rx) = mpsc::channel();
                        let id = request.id.clone();
                        if sender
                            .send(AutomationEnvelope {
                                request,
                                response_tx,
                            })
                            .is_err()
                        {
                            AutomationResponse::error(id, "automation command queue is closed")
                        } else {
                            response_rx.recv().unwrap_or_else(|_| {
                                AutomationResponse::error(
                                    id,
                                    "automation command dropped before completion",
                                )
                            })
                        }
                    }
                    Err(error) => {
                        AutomationResponse::error(None, format!("invalid request: {error}"))
                    }
                };

                let Ok(payload) = serde_json::to_vec(&response) else {
                    break;
                };
                if writer.write_all(&payload).is_err()
                    || writer.write_all(b"\n").is_err()
                    || writer.flush().is_err()
                {
                    break;
                }
            }
        });
    }
}

fn workspace_root_path() -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    root.canonicalize().unwrap_or(root)
}

fn workspace_defaults() -> (String, String) {
    let root = workspace_root_path();
    let branch = Command::new("git")
        .arg("-C")
        .arg(&root)
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|stdout| stdout.trim().to_string())
        .unwrap_or_default();

    (root.display().to_string(), branch)
}

fn default_workdesk_metadata(
    intent: impl Into<String>,
    status: Option<String>,
    progress: Option<WorkdeskProgress>,
) -> WorkdeskMetadata {
    let (cwd, branch) = workspace_defaults();
    WorkdeskMetadata {
        intent: intent.into(),
        cwd,
        branch,
        status,
        progress,
    }
}

fn normalize_workdesk_draft(
    mut draft: WorkdeskDraft,
    fallback_name: &str,
    fallback_summary: &str,
) -> WorkdeskDraft {
    let name = draft.name.trim();
    draft.name = if name.is_empty() {
        fallback_name.to_string()
    } else {
        name.to_string()
    };

    let summary = draft.summary.trim();
    draft.summary = if summary.is_empty() {
        fallback_summary.to_string()
    } else {
        summary.to_string()
    };

    let intent = draft.metadata.intent.trim();
    draft.metadata.intent = if intent.is_empty() {
        draft.summary.clone()
    } else {
        intent.to_string()
    };
    draft.metadata = draft.metadata.hydrated();
    draft
}

fn workdesk_from_template(template: WorkdeskTemplate, draft: WorkdeskDraft) -> WorkdeskState {
    let draft = normalize_workdesk_draft(draft, template.base_name(), template.summary());
    let panes = match template {
        WorkdeskTemplate::ShellDesk => vec![single_surface_pane(
            1,
            "Shell 1",
            PaneKind::Shell,
            WorkdeskPoint::new(120.0, 96.0),
            DEFAULT_SHELL_SIZE,
        )],
        WorkdeskTemplate::AgentReview => vec![
            single_surface_pane(
                1,
                "Review Shell",
                PaneKind::Shell,
                WorkdeskPoint::new(80.0, 96.0),
                DEFAULT_SHELL_SIZE,
            ),
            single_surface_pane(
                2,
                "Review Agent",
                PaneKind::Agent,
                WorkdeskPoint::new(1048.0, 132.0),
                DEFAULT_AGENT_SIZE,
            ),
        ],
        WorkdeskTemplate::Debug => vec![
            single_surface_pane(
                1,
                "Repro Shell",
                PaneKind::Shell,
                WorkdeskPoint::new(80.0, 96.0),
                DEFAULT_SHELL_SIZE,
            ),
            single_surface_pane(
                2,
                "Debug Agent",
                PaneKind::Agent,
                WorkdeskPoint::new(1048.0, 120.0),
                DEFAULT_AGENT_SIZE,
            ),
        ],
        WorkdeskTemplate::Implementation => vec![
            single_surface_pane(
                1,
                "Build Shell",
                PaneKind::Shell,
                WorkdeskPoint::new(80.0, 96.0),
                DEFAULT_SHELL_SIZE,
            ),
            single_surface_pane(
                2,
                "Implement Agent",
                PaneKind::Agent,
                WorkdeskPoint::new(1048.0, 132.0),
                DEFAULT_AGENT_SIZE,
            ),
        ],
    };

    let mut desk = WorkdeskState::new(draft.name, draft.summary, panes);
    desk.metadata = draft.metadata;
    desk
}

fn compact_cwd_label(path: &str) -> String {
    let parts = path
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts.is_empty() {
        return "~".to_string();
    }
    if parts.len() == 1 {
        return parts[0].to_string();
    }
    format!("{}/{}", parts[parts.len() - 2], parts[parts.len() - 1])
}

fn single_surface_pane(
    raw_id: u64,
    title: impl Into<String>,
    kind: PaneKind,
    position: WorkdeskPoint,
    size: WorkdeskSize,
) -> PaneRecord {
    single_surface_pane_with_ids(
        PaneId::new(raw_id),
        SurfaceId::new(raw_id),
        title,
        kind,
        position,
        size,
    )
}

fn single_surface_pane_with_ids(
    pane_id: PaneId,
    surface_id: SurfaceId,
    title: impl Into<String>,
    kind: PaneKind,
    position: WorkdeskPoint,
    size: WorkdeskSize,
) -> PaneRecord {
    let title = title.into();
    PaneRecord::new(
        pane_id,
        position,
        size,
        SurfaceRecord::new(surface_id, title, kind),
        None,
    )
}

fn baseline_attention_state_for_kind(kind: &SurfaceKind) -> AttentionState {
    match kind {
        SurfaceKind::Agent => AttentionState::Working,
        SurfaceKind::Shell | SurfaceKind::Browser | SurfaceKind::Editor => AttentionState::Idle,
    }
}

fn surface_kind_slug(kind: &SurfaceKind) -> &'static str {
    match kind {
        SurfaceKind::Shell => "shell",
        SurfaceKind::Agent => "agent",
        SurfaceKind::Browser => "browser",
        SurfaceKind::Editor => "editor",
    }
}

fn default_size_for_kind(kind: &PaneKind) -> WorkdeskSize {
    match kind {
        PaneKind::Shell => DEFAULT_SHELL_SIZE,
        PaneKind::Agent => DEFAULT_AGENT_SIZE,
        PaneKind::Browser => WorkdeskSize::new(980.0, 640.0),
        PaneKind::Editor => WorkdeskSize::new(920.0, 620.0),
    }
}

fn base_label_for_kind(kind: &PaneKind) -> &'static str {
    match kind {
        PaneKind::Shell => "Shell",
        PaneKind::Agent => "Agent",
        PaneKind::Browser => "Browser",
        PaneKind::Editor => "Editor",
    }
}

fn browser_title(url: &str) -> String {
    url.split("://")
        .nth(1)
        .unwrap_or(url)
        .split('/')
        .next()
        .filter(|host| !host.is_empty())
        .unwrap_or("Browser")
        .to_string()
}

fn canonical_path_string(path: &str) -> String {
    PathBuf::from(path)
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from(path))
        .display()
        .to_string()
}

fn editable_keystroke_text(keystroke: &Keystroke) -> Option<String> {
    if keystroke.modifiers.control
        || keystroke.modifiers.alt
        || keystroke.modifiers.platform
        || keystroke.modifiers.function
    {
        return None;
    }

    match keystroke.key.as_str() {
        "space" => Some(" ".to_string()),
        "tab" | "enter" | "backspace" | "delete" | "escape" => None,
        _ => keystroke
            .key_char
            .clone()
            .or_else(|| (!keystroke.modifiers.modified()).then(|| keystroke.key.clone())),
    }
}

fn json_value_at<'a>(params: &'a Value, keys: &[&str]) -> Option<&'a Value> {
    keys.iter().find_map(|key| params.get(*key))
}

fn json_has_any(params: &Value, keys: &[&str]) -> bool {
    json_value_at(params, keys).is_some()
}

fn json_string_at(params: &Value, keys: &[&str]) -> Option<String> {
    json_value_at(params, keys)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn json_optional_string_at(params: &Value, keys: &[&str]) -> Result<Option<String>, String> {
    match json_value_at(params, keys) {
        None => Ok(None),
        Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => {
            Ok((!value.trim().is_empty()).then(|| value.trim().to_string()))
        }
        Some(_) => Err(format!("expected string or null for `{}`", keys[0])),
    }
}

fn json_u64_at(params: &Value, keys: &[&str]) -> Option<u64> {
    json_value_at(params, keys).and_then(Value::as_u64)
}

fn json_bool_at(params: &Value, keys: &[&str]) -> Option<bool> {
    json_value_at(params, keys).and_then(Value::as_bool)
}

fn require_json_string_at(params: &Value, keys: &[&str], label: &str) -> Result<String, String> {
    json_string_at(params, keys).ok_or_else(|| format!("missing required `{label}`"))
}

fn parse_pane_kind(value: &str) -> Option<PaneKind> {
    match value {
        "shell" => Some(PaneKind::Shell),
        "agent" => Some(PaneKind::Agent),
        "browser" => Some(PaneKind::Browser),
        "editor" => Some(PaneKind::Editor),
        _ => None,
    }
}

fn parse_progress_value(value: &Value) -> Result<Option<WorkdeskProgress>, String> {
    if value.is_null() {
        return Ok(None);
    }

    let Some(object) = value.as_object() else {
        return Err("progress must be an object with `label` and `value`".to_string());
    };
    let label = object
        .get("label")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|label| !label.is_empty())
        .ok_or_else(|| "progress.label is required".to_string())?;
    let value = object
        .get("value")
        .and_then(Value::as_u64)
        .ok_or_else(|| "progress.value is required".to_string())?;

    Ok(Some(WorkdeskProgress::new(label, value as u8)))
}

fn automation_pane_json(desk: &WorkdeskState, pane_id: PaneId, desk_active: bool) -> Value {
    let pane = desk
        .panes
        .iter()
        .find(|pane| pane.id == pane_id)
        .expect("automation pane target should exist");
    let attention = desk.pane_attention(pane_id);
    let status = desk
        .active_terminal_surface_id_for_pane(pane_id)
        .and_then(|surface_id| desk.terminal_statuses.get(&surface_id))
        .cloned()
        .unwrap_or(None);

    json!({
        "id": pane.id.raw(),
        "title": pane.title,
        "kind": surface_kind_slug(&pane.kind),
        "focused": desk.active_pane == Some(pane_id),
        "desk_active": desk_active,
        "active_surface_id": pane.active_surface_id.raw(),
        "surface_count": pane.surfaces.len(),
        "attention": {
            "state": attention.state,
            "unread": attention.unread,
            "last_attention_sequence": attention.last_attention_sequence,
            "last_activity_sequence": attention.last_activity_sequence,
        },
        "status": status,
        "surfaces": pane
            .surfaces
            .iter()
            .map(|surface| automation_surface_json(desk, pane_id, surface))
            .collect::<Vec<_>>(),
    })
}

fn automation_surface_json(
    desk: &WorkdeskState,
    pane_id: PaneId,
    surface: &SurfaceRecord,
) -> Value {
    let status = surface
        .kind
        .is_terminal()
        .then(|| {
            desk.terminal_statuses
                .get(&surface.id)
                .cloned()
                .unwrap_or(None)
        })
        .flatten();

    json!({
        "id": surface.id.raw(),
        "title": surface.title,
        "kind": surface_kind_slug(&surface.kind),
        "active": desk
            .pane(pane_id)
            .is_some_and(|pane| pane.active_surface_id == surface.id),
        "status": status,
        "dirty": surface.dirty,
        "url": surface.browser_url,
        "file_path": surface.editor_file_path,
    })
}

fn automation_workdesk_summary_json(index: usize, desk: &WorkdeskState, active: bool) -> Value {
    let attention = desk.workdesk_attention_summary();

    json!({
        "index": index,
        "active": active,
        "name": desk.name,
        "summary": desk.summary,
        "intent": desk.metadata.intent,
        "cwd": desk.metadata.cwd,
        "branch": desk.metadata.branch,
        "status": desk.metadata.status,
        "progress": desk.metadata.progress.as_ref().map(|progress| json!({
            "label": progress.label,
            "value": progress.value,
        })),
        "pane_count": desk.panes.len(),
        "live_count": desk.terminals.len(),
        "active_pane_id": desk.active_pane.map(PaneId::raw),
        "active_pane_title": desk.active_pane.map(|_| desk.active_pane_title()),
        "attention": {
            "highest": attention.highest,
            "unread_count": attention.unread_count,
        },
    })
}

fn automation_workdesk_state_json(index: usize, desk: &WorkdeskState, active: bool) -> Value {
    json!({
        "workdesk": automation_workdesk_summary_json(index, desk, active),
        "panes": desk.panes.iter().map(|pane| automation_pane_json(desk, pane.id, active)).collect::<Vec<_>>(),
    })
}

fn initial_workdesks() -> Vec<WorkdeskState> {
    vec![workdesk_from_template(
        WorkdeskTemplate::ShellDesk,
        WorkdeskDraft::from_template("Shell Desk".to_string(), WorkdeskTemplate::ShellDesk),
    )]
}

#[cfg(test)]
fn blank_workdesk(name: impl Into<String>, summary: impl Into<String>) -> WorkdeskState {
    WorkdeskState::new(name, summary, Vec::new())
}

fn boot_workdesk_terminals(workdesk: &mut WorkdeskState) {
    let panes_to_boot = workdesk.panes.clone();
    for pane in panes_to_boot {
        let grid = terminal_grid_size_for_pane(pane.size, pane.surfaces.len());
        for surface in pane.surfaces {
            if surface.kind.is_terminal() {
                workdesk.attach_terminal_session(surface.id, &surface.kind, &surface.title, grid);
            }
        }
    }
}

fn shutdown_workdesk_terminals(workdesk: &mut WorkdeskState) {
    for terminal in workdesk.terminals.values() {
        terminal.close();
    }
    workdesk.terminals.clear();
    workdesk.terminal_revisions.clear();
    workdesk.terminal_statuses.clear();
    workdesk.terminal_views.clear();
    workdesk.terminal_grids.clear();
    workdesk.editors.clear();
    workdesk.editor_views.clear();
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
    let attention_summary = desk.workdesk_attention_summary();
    let attention_tint = if attention_summary.highest == AttentionState::Idle {
        accent
    } else {
        attention_summary.highest.tint()
    };
    let border = if is_active || is_menu_open {
        accent
    } else if attention_summary.unread_count > 0 {
        attention_tint
    } else {
        rgb(0x24313b).into()
    };
    let background = if is_active || is_menu_open {
        rgb(0x131b22)
    } else {
        rgb(0x0f151b)
    };
    let focus_label = if desk.active_pane.is_some() {
        desk.active_pane_title()
    } else {
        "No focus".to_string()
    };
    let focus_attention = desk
        .active_pane
        .map(|pane_id| desk.pane_attention(pane_id).state)
        .unwrap_or(AttentionState::Idle);
    let live_label = if desk.terminals.is_empty() {
        "idle".to_string()
    } else {
        format!("live {}", desk.terminals.len())
    };
    let intent_label = desk.intent_label();
    let cwd_label = compact_cwd_label(&desk.metadata.cwd);
    let branch_label = desk
        .metadata
        .branch
        .trim()
        .is_empty()
        .then_some("no-branch".to_string())
        .unwrap_or_else(|| desk.metadata.branch.clone());
    let status_label = desk.status_label();
    let progress_label = desk.progress_label();
    let deck_label = if is_active {
        "ACTIVE".to_string()
    } else {
        format!("{:02}", index + 1)
    };
    let preview_label = if preview.trim().is_empty() {
        focus_label.clone()
    } else {
        preview
    };
    let meta_line = format!("{cwd_label} · {branch_label} · {} panes", desk.panes.len());

    div()
        .flex()
        .flex_col()
        .gap_1()
        .p_2()
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
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(attention_indicator(
                            attention_summary.highest,
                            attention_summary.unread_count > 0,
                        ))
                        .child(div().text_sm().child(desk.name.clone())),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(
                            div()
                                .px_2()
                                .py(px(2.0))
                                .rounded_full()
                                .bg(rgb(0x0c1116))
                                .text_xs()
                                .text_color(accent)
                                .child(deck_label),
                        )
                        .when(attention_summary.unread_count > 0, |row| {
                            row.child(
                                div()
                                    .px_2()
                                    .py(px(2.0))
                                    .rounded_full()
                                    .bg(rgb(0x0c1116))
                                    .border_1()
                                    .border_color(attention_tint)
                                    .text_xs()
                                    .text_color(attention_tint)
                                    .child(format!("{} unread", attention_summary.unread_count)),
                            )
                        })
                        .child(
                            div()
                                .px_2()
                                .py(px(2.0))
                                .rounded_full()
                                .bg(rgb(0x0c1116))
                                .text_xs()
                                .text_color(rgb(0x9da8b1))
                                .child(live_label),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_center()
                                .w(px(22.0))
                                .h(px(22.0))
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
        .child(div().text_xs().text_color(accent).child(intent_label))
        .child(
            div()
                .text_xs()
                .text_color(rgb(0x9aa6af))
                .overflow_hidden()
                .whitespace_nowrap()
                .child(preview_label),
        )
        .child(
            div()
                .text_xs()
                .text_color(rgb(0x6f7c86))
                .overflow_hidden()
                .whitespace_nowrap()
                .child(meta_line),
        )
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_2()
                .when_some(progress_label, |row, progress| {
                    row.child(context_pill(progress, rgb(0x77d19a).into()))
                })
                .when_some(status_label, |row, status| {
                    row.child(context_pill(status, attention_tint))
                }),
        )
        .child(
            div().flex().items_center().justify_between().gap_2().child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(attention_indicator(focus_attention, false))
                    .child(div().text_xs().text_color(rgb(0x7f8a94)).child(focus_label)),
            ),
        )
}

fn workdesk_compact_chip(
    index: usize,
    desk: &WorkdeskState,
    is_active: bool,
    accent: gpui::Hsla,
    listener: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
    context_listener: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let attention_summary = desk.workdesk_attention_summary();
    let border = if is_active {
        accent
    } else if attention_summary.unread_count > 0 {
        attention_summary.highest.tint()
    } else {
        rgb(0x24313b).into()
    };

    div()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_1()
        .w(px(52.0))
        .min_h(px(56.0))
        .p_2()
        .bg(if is_active {
            rgb(0x141c23)
        } else {
            rgb(0x0f151b)
        })
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
        .child(attention_indicator(
            attention_summary.highest,
            attention_summary.unread_count > 0,
        ))
        .child(
            div()
                .text_xs()
                .text_color(if is_active {
                    accent
                } else {
                    rgb(0x9da8b1).into()
                })
                .child(format!("{:02}", index + 1)),
        )
}

fn workdesk_template_card(
    template: WorkdeskTemplate,
    selected: bool,
    listener: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let accent = template.accent();
    let border = if selected {
        accent
    } else {
        rgb(0x293742).into()
    };
    let background = if selected {
        rgb(0x17212a)
    } else {
        rgb(0x121920)
    };

    div()
        .flex()
        .flex_col()
        .gap_1()
        .p_3()
        .bg(background)
        .border_1()
        .border_color(border)
        .rounded_lg()
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, |_, _, cx| {
            cx.stop_propagation();
        })
        .on_mouse_up(MouseButton::Left, listener)
        .child(div().text_sm().text_color(accent).child(template.label()))
        .child(
            div()
                .text_xs()
                .text_color(rgb(0x90a0aa))
                .child(template.summary()),
        )
}

fn workdesk_editor_field(
    label: &str,
    value: String,
    active: bool,
    accent: gpui::Hsla,
    listener: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let label = label.to_string();
    let border = if active { accent } else { rgb(0x293742).into() };
    let background = if active { rgb(0x16212a) } else { rgb(0x121920) };
    let text_tint: gpui::Hsla = if value.is_empty() {
        rgb(0x67737d).into()
    } else {
        rgb(0xe4ebf1).into()
    };
    let placeholder = format!("{label}...");

    div()
        .flex()
        .flex_col()
        .gap_1()
        .p_3()
        .bg(background)
        .border_1()
        .border_color(border)
        .rounded_lg()
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, |_, _, cx| {
            cx.stop_propagation();
        })
        .on_mouse_up(MouseButton::Left, listener)
        .child(div().text_xs().text_color(accent).child(label))
        .child(
            div()
                .text_sm()
                .text_color(text_tint)
                .child(if value.is_empty() {
                    placeholder
                } else if active {
                    format!("{value}|")
                } else {
                    value
                }),
        )
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
        .items_start()
        .justify_center()
        .min_w(px(104.0))
        .px_2()
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
        .flex_col()
        .items_start()
        .gap_3()
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
                .justify_between()
                .flex_wrap()
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

fn compact_dock_button(
    label: &str,
    accent: gpui::Hsla,
    listener: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let label = label.to_string();

    div()
        .flex()
        .items_center()
        .justify_center()
        .min_w(px(36.0))
        .px_2()
        .py_1()
        .cursor_pointer()
        .bg(rgb(0x161d24))
        .border_1()
        .border_color(rgb(0x2b3641))
        .rounded_md()
        .on_mouse_down(MouseButton::Left, |_, _, cx| {
            cx.stop_propagation();
        })
        .on_mouse_up(MouseButton::Left, listener)
        .child(div().text_xs().text_color(accent).child(label))
}

fn chrome_button(
    label: &str,
    accent: gpui::Hsla,
    active: bool,
    badge: Option<usize>,
    listener: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let label = label.to_string();
    let background = if active { rgb(0x1b2730) } else { rgb(0x161d24) };
    let border = if active { accent } else { rgb(0x2b3641).into() };
    let badge_label = badge.and_then(|value| {
        if value == 0 {
            None
        } else if value > 9 {
            Some("9+".to_string())
        } else {
            Some(value.to_string())
        }
    });

    div()
        .relative()
        .flex()
        .items_center()
        .justify_center()
        .min_w(px(28.0))
        .h(px(24.0))
        .px_2()
        .cursor_pointer()
        .bg(background)
        .border_1()
        .border_color(border)
        .rounded_md()
        .font_family(".ZedMono")
        .on_mouse_down(MouseButton::Left, |_, _, cx| {
            cx.stop_propagation();
        })
        .on_mouse_up(MouseButton::Left, listener)
        .child(div().text_xs().text_color(accent).child(label))
        .when_some(badge_label, |button, badge| {
            button.child(
                div()
                    .absolute()
                    .right(px(-6.0))
                    .top(px(-6.0))
                    .min_w(px(16.0))
                    .h(px(16.0))
                    .px_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .bg(rgb(0x7cc7ff))
                    .border_1()
                    .border_color(rgb(0x0d1217))
                    .child(div().text_xs().text_color(rgb(0x071015)).child(badge)),
            )
        })
}

fn compact_toggle_button(
    label: &str,
    accent: gpui::Hsla,
    active: bool,
    listener: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let label = label.to_string();
    let background = if active { rgb(0x1d2730) } else { rgb(0x161d24) };
    let border = if active { accent } else { rgb(0x2b3641).into() };

    div()
        .flex()
        .items_center()
        .justify_center()
        .min_w(px(40.0))
        .px_2()
        .py_1()
        .cursor_pointer()
        .bg(background)
        .border_1()
        .border_color(border)
        .rounded_md()
        .on_mouse_down(MouseButton::Left, |_, _, cx| {
            cx.stop_propagation();
        })
        .on_mouse_up(MouseButton::Left, listener)
        .child(div().text_xs().text_color(accent).child(label))
}

fn dock_divider() -> impl IntoElement {
    div().w(px(1.0)).h(px(24.0)).bg(rgb(0x2b3641))
}

fn notification_item(
    title: &str,
    detail: &str,
    accent: gpui::Hsla,
    unread: bool,
) -> impl IntoElement {
    let title = title.to_string();
    let detail = detail.to_string();

    div()
        .flex()
        .flex_col()
        .gap_1()
        .p_3()
        .bg(if unread { rgb(0x131c25) } else { rgb(0x10171d) })
        .border_1()
        .border_color(if unread { accent } else { rgb(0x24313b).into() })
        .rounded_lg()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_3()
                .child(div().text_sm().text_color(accent).child(title))
                .when(unread, |row| {
                    row.child(div().w(px(8.0)).h(px(8.0)).rounded_full().bg(accent))
                }),
        )
        .child(div().text_xs().text_color(rgb(0x95a3ad)).child(detail))
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

fn context_pill(value: impl Into<String>, tint: gpui::Hsla) -> impl IntoElement {
    let value = value.into();

    div()
        .flex()
        .items_center()
        .gap_2()
        .px_2()
        .py_1()
        .rounded_full()
        .bg(rgb(0x151c23))
        .border_1()
        .border_color(rgb(0x2b3641))
        .child(div().text_xs().text_color(tint).child(value))
}

fn inspector_row(label: &str, value: impl Into<String>) -> impl IntoElement {
    let label = label.to_string();
    let value = value.into();

    div()
        .flex()
        .justify_between()
        .items_start()
        .gap_4()
        .child(div().text_xs().text_color(rgb(0x7f8a94)).child(label))
        .child(
            div()
                .text_xs()
                .text_color(rgb(0xdce2e8))
                .max_w(px(220.0))
                .text_right()
                .child(value),
        )
}

fn signal_dot(color: gpui::Hsla, filled: bool) -> impl IntoElement {
    div()
        .w(px(8.0))
        .h(px(8.0))
        .rounded_full()
        .bg(if filled {
            color
        } else {
            rgba(0x00000000).into()
        })
        .border_1()
        .border_color(color)
}

fn attention_indicator(state: AttentionState, unread: bool) -> impl IntoElement {
    let tint = state.tint();
    let filled = state != AttentionState::Idle;

    div()
        .relative()
        .w(px(10.0))
        .h(px(10.0))
        .child(signal_dot(tint, filled))
        .when(unread, |root| {
            root.child(
                div()
                    .absolute()
                    .left(px(-2.0))
                    .top(px(-2.0))
                    .w(px(14.0))
                    .h(px(14.0))
                    .rounded_full()
                    .border_1()
                    .border_color(tint),
            )
        })
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
        PaneKind::Browser => rgb(0x77d19a).into(),
        PaneKind::Editor => rgb(0xb4a4ff).into(),
    }
}

fn pane_kind_label(kind: &PaneKind) -> &'static str {
    match kind {
        PaneKind::Shell => "Shell pane",
        PaneKind::Agent => "Agent pane",
        PaneKind::Browser => "Browser pane",
        PaneKind::Editor => "Editor pane",
    }
}

fn grid_hint_card(
    direction: GridDirection,
    hint: GridDirectionHint,
    pane: &PaneRecord,
    viewport_width: f32,
    viewport_height: f32,
    sidebar_width: f32,
    listener: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let accent = pane_accent(&pane.kind);
    let (left, top) = match direction {
        GridDirection::Left => (
            sidebar_width + 18.0,
            (viewport_height - GRID_HINT_HEIGHT) * 0.5,
        ),
        GridDirection::Right => (
            viewport_width - GRID_HINT_WIDTH - 18.0,
            (viewport_height - GRID_HINT_HEIGHT) * 0.5,
        ),
        GridDirection::Up => (
            sidebar_width + (viewport_width - sidebar_width - GRID_HINT_WIDTH) * 0.5,
            18.0,
        ),
        GridDirection::Down => (
            sidebar_width + (viewport_width - sidebar_width - GRID_HINT_WIDTH) * 0.5,
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
    sidebar_width: f32,
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

    let available_width = (viewport_width - sidebar_width - EXPOSE_MARGIN_X * 2.0).max(1.0);
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

fn next_attention_target_for_workdesks(workdesks: &[WorkdeskState]) -> Option<(usize, PaneId)> {
    let mut best: Option<((u64, usize, u64), PaneId)> = None;

    for (desk_index, desk) in workdesks.iter().enumerate() {
        for pane in &desk.panes {
            let attention = desk.pane_attention(pane.id);
            if !attention.unread || !attention.state.is_attention() {
                continue;
            }

            let candidate_key = (
                attention.last_attention_sequence.max(1),
                desk_index,
                pane.id.raw(),
            );

            if best
                .as_ref()
                .map_or(true, |(best_key, _)| candidate_key < *best_key)
            {
                best = Some((candidate_key, pane.id));
            }
        }
    }

    best.map(|((_, desk_index, _), pane_id)| (desk_index, pane_id))
}

fn split_layout_frames(
    panes: &[PaneRecord],
    _active_pane: Option<PaneId>,
    viewport_width: f32,
    viewport_height: f32,
    sidebar_width: f32,
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
        x: sidebar_width + SPLIT_MARGIN_X,
        y: SPLIT_MARGIN_TOP,
        width: (viewport_width - sidebar_width - SPLIT_MARGIN_X * 2.0).max(MIN_PANE_WIDTH),
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

fn infer_attention_state_from_snapshot(
    pane_kind: &PaneKind,
    snapshot: &TerminalSnapshot,
) -> AttentionState {
    let status = snapshot.status.as_deref().unwrap_or("Running");
    let normalized = status.to_ascii_lowercase();

    if normalized.contains("error")
        || normalized.contains("failed")
        || normalized.contains("exited via")
        || (normalized.contains("exited with code") && !normalized.contains("code 0"))
    {
        return AttentionState::Error;
    }

    if snapshot.closed
        || normalized.contains("exited with code 0")
        || normalized.contains("terminated")
    {
        return AttentionState::Waiting;
    }

    match pane_kind {
        PaneKind::Agent => AttentionState::Working,
        PaneKind::Shell | PaneKind::Browser | PaneKind::Editor => AttentionState::Idle,
    }
}

fn post_attention_notification(title: String, body: String) {
    #[cfg(target_os = "macos")]
    {
        let title = escape_applescript_string(&title);
        let body = escape_applescript_string(&body);
        let _ = Command::new("osascript")
            .arg("-e")
            .arg(format!(
                "display notification \"{body}\" with title \"{title}\""
            ))
            .spawn();
    }
}

fn escape_applescript_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn surface_stack_chip(
    surface: &SurfaceRecord,
    active: bool,
    accent: gpui::Hsla,
    listener: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let border = if active { accent } else { rgb(0x2b3641).into() };
    let background = if active { rgb(0x1c2630) } else { rgb(0x151c23) };
    let label = if surface.dirty {
        format!("{}*", surface.title)
    } else {
        surface.title.clone()
    };

    div()
        .flex()
        .items_center()
        .gap_2()
        .max_w(px(196.0))
        .px_2()
        .py_1()
        .cursor_pointer()
        .bg(background)
        .border_1()
        .border_color(border)
        .rounded_md()
        .overflow_hidden()
        .on_mouse_down(MouseButton::Left, |_, _, cx| {
            cx.stop_propagation();
        })
        .on_mouse_up(MouseButton::Left, listener)
        .child(
            div()
                .text_xs()
                .text_color(accent)
                .child(surface_kind_slug(&surface.kind)),
        )
        .child(
            div()
                .text_xs()
                .text_color(rgb(0xdce2e8))
                .overflow_hidden()
                .whitespace_nowrap()
                .child(label),
        )
}

fn browser_preview_card(
    url: &str,
    accent: gpui::Hsla,
    listener: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let host = browser_title(url);

    div()
        .flex_1()
        .flex()
        .flex_col()
        .justify_between()
        .gap_4()
        .p_4()
        .bg(rgb(0x0f151a))
        .border_1()
        .border_color(rgb(0x24303a))
        .rounded_md()
        .child(
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(div().text_xs().text_color(accent).child("Browser preview"))
                .child(div().text_lg().child(host))
                .child(
                    div()
                        .text_sm()
                        .text_color(rgb(0xa8b5be))
                        .child(url.to_string()),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_3()
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(0x7f8a94))
                        .child("URL is persisted on the surface and can be reopened externally."),
                )
                .child(control_button("Open external", accent, listener)),
        )
}

fn editor_language_label(editor: &EditorBuffer) -> &'static str {
    match editor.language() {
        canvas_editor::LanguageKind::Plaintext => "Plaintext",
        canvas_editor::LanguageKind::Rust => "Rust",
        canvas_editor::LanguageKind::JavaScript => "JavaScript",
        canvas_editor::LanguageKind::TypeScript => "TypeScript",
        canvas_editor::LanguageKind::Tsx => "TSX",
        canvas_editor::LanguageKind::Jsx => "JSX",
        canvas_editor::LanguageKind::Json => "JSON",
        canvas_editor::LanguageKind::Toml => "TOML",
        canvas_editor::LanguageKind::Yaml => "YAML",
        canvas_editor::LanguageKind::Markdown => "Markdown",
    }
}

fn editor_font_for_kind(kind: HighlightKind) -> gpui::Font {
    let mut font = font(".ZedMono");
    match kind {
        HighlightKind::Keyword | HighlightKind::Type => font.weight = FontWeight::BOLD,
        HighlightKind::Comment => font.style = FontStyle::Italic,
        HighlightKind::Plain | HighlightKind::String | HighlightKind::Number => {}
    }
    font
}

fn editor_text_color_for_kind(kind: HighlightKind) -> gpui::Hsla {
    match kind {
        HighlightKind::Plain => rgb(0xdce2e8).into(),
        HighlightKind::Comment => rgb(0x72808a).into(),
        HighlightKind::String => rgb(0xb8d98f).into(),
        HighlightKind::Keyword => rgb(0xf5c07a).into(),
        HighlightKind::Number => rgb(0x8fd0ff).into(),
        HighlightKind::Type => rgb(0xcaa6ff).into(),
    }
}

fn editor_line_display(
    editor: &EditorBuffer,
    line_index: usize,
    is_active: bool,
    cursor_blink_visible: bool,
) -> StyledText {
    let line_range = editor.line_range(line_index);
    let line_text = editor.line_text(line_index);
    let mut display_text = line_text.to_string();
    let selection = editor.selection().range.clone();
    let search_match = editor.current_search_match();
    let cursor_offset = editor.cursor_offset();
    let show_cursor = is_active && cursor_blink_visible && selection.is_empty();
    let append_cursor_cell = show_cursor && cursor_offset == line_range.end;

    if display_text.is_empty() || append_cursor_cell {
        display_text.push(' ');
    }

    let highlights = editor.highlight_line(line_index);
    let original_len = line_text.len();
    let mut runs: Vec<TextRun> = Vec::new();

    for (start, ch) in display_text.char_indices() {
        let synthetic = start >= original_len;
        let absolute_start = if synthetic {
            line_range.end
        } else {
            line_range.start + start
        };
        let absolute_end = if synthetic {
            line_range.end
        } else {
            (absolute_start + ch.len_utf8()).min(line_range.end)
        };
        let highlight = if synthetic {
            HighlightKind::Plain
        } else {
            highlights
                .iter()
                .find(|span| span.range.start <= start && start < span.range.end)
                .map(|span| span.kind)
                .unwrap_or(HighlightKind::Plain)
        };
        let mut background = None;
        let mut color = editor_text_color_for_kind(highlight);

        if !selection.is_empty()
            && absolute_start < selection.end
            && absolute_end.max(absolute_start + 1) > selection.start
        {
            background = Some(rgb(0x2d5b88).into());
            color = rgb(0xf4f8fb).into();
        } else if search_match.as_ref().is_some_and(|range| {
            absolute_start < range.end && absolute_end.max(absolute_start + 1) > range.start
        }) {
            background = Some(rgb(0x3b3530).into());
        }

        if show_cursor && absolute_start == cursor_offset {
            background = Some(rgb(0xdce2e8).into());
            color = rgb(0x10161b).into();
        }

        let font = editor_font_for_kind(highlight);
        if let Some(last) = runs.last_mut() {
            if last.font == font
                && last.color == color
                && last.background_color == background
                && last.underline.is_none()
                && last.strikethrough.is_none()
            {
                last.len += ch.len_utf8();
                continue;
            }
        }

        runs.push(TextRun {
            len: ch.len_utf8(),
            font,
            color,
            background_color: background,
            underline: None,
            strikethrough: None,
        });
    }

    StyledText::new(display_text).with_runs(runs)
}

fn terminal_header_height_for_surface_count(surface_count: usize) -> f32 {
    if surface_count > 1 {
        58.0
    } else {
        38.0
    }
}

fn terminal_font_size_for_zoom(zoom: f32) -> f32 {
    TERMINAL_FONT_SIZE * zoom.clamp(0.84, 1.18)
}

fn terminal_line_height_for_zoom(zoom: f32) -> f32 {
    TERMINAL_CELL_HEIGHT * zoom.clamp(0.88, 1.18)
}

fn terminal_cell_width_for_zoom(zoom: f32) -> f32 {
    TERMINAL_CELL_WIDTH * zoom.clamp(0.84, 1.18)
}

fn terminal_text_metrics(window: &Window, zoom: f32) -> TerminalTextMetrics {
    let font_size = terminal_font_size_for_zoom(zoom);
    let fallback_line_height = terminal_line_height_for_zoom(zoom);
    let fallback_cell_width = terminal_cell_width_for_zoom(zoom);
    let font_id = window
        .text_system()
        .resolve_font(&font(TERMINAL_FONT_FAMILY));
    let cell_width = window
        .text_system()
        .ch_advance(font_id, px(font_size))
        .map(f32::from)
        .unwrap_or(fallback_cell_width)
        .max(1.0);
    let text_height = f32::from(window.text_system().ascent(font_id, px(font_size)))
        + f32::from(window.text_system().descent(font_id, px(font_size)));
    let line_height = fallback_line_height.max(text_height.ceil() + 1.0);

    TerminalTextMetrics {
        font_size,
        line_height,
        cell_width,
    }
}

fn terminal_grid_size_for_pane(size: WorkdeskSize, surface_count: usize) -> TerminalGridSize {
    let horizontal_chrome = TERMINAL_BODY_INSET * 2.0 + 24.0;
    let vertical_chrome =
        terminal_header_height_for_surface_count(surface_count) + TERMINAL_BODY_INSET * 2.0 + 24.0;
    let cols = ((size.width - horizontal_chrome) / terminal_cell_width_for_zoom(1.0))
        .floor()
        .max(40.0) as u16;
    let rows = ((size.height - vertical_chrome) / terminal_line_height_for_zoom(1.0))
        .floor()
        .max(10.0) as u16;

    TerminalGridSize::new(cols, rows)
}

fn terminal_grid_size_for_frame(
    frame: PaneViewportFrame,
    surface_count: usize,
    metrics: TerminalTextMetrics,
) -> TerminalGridSize {
    let header_height =
        terminal_header_height_for_surface_count(surface_count) * frame.zoom.clamp(0.78, 1.3);
    let pane_padding = 12.0 * frame.zoom.clamp(0.85, 1.25);
    let content_width = (frame.width - pane_padding * 2.0 - TERMINAL_BODY_INSET * 2.0).max(1.0);
    let content_height =
        (frame.height - header_height - pane_padding * 2.0 - TERMINAL_BODY_INSET * 2.0).max(1.0);
    let cols = (content_width / metrics.cell_width).floor().max(40.0) as u16;
    let rows = (content_height / metrics.line_height).floor().max(10.0) as u16;

    TerminalGridSize::new(cols, rows)
}

fn terminal_body(
    snapshot: &Option<TerminalSnapshot>,
    terminal_view: &TerminalViewState,
    metrics: TerminalTextMetrics,
    is_active: bool,
    cursor_blink_visible: bool,
) -> impl IntoElement {
    match snapshot {
        Some(snapshot) => {
            let rows = snapshot
                .rows
                .iter()
                .enumerate()
                .map(|(row_index, row)| {
                    terminal_row_element(
                        snapshot,
                        terminal_view.selection,
                        row_index,
                        row,
                        metrics,
                        is_active,
                        cursor_blink_visible,
                    )
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
                .font_family(TERMINAL_FONT_FAMILY)
                .text_size(px(metrics.font_size))
                .line_height(px(metrics.line_height))
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
            .font_family(TERMINAL_FONT_FAMILY)
            .text_size(px(metrics.font_size))
            .line_height(px(metrics.line_height))
            .text_color(rgb(0xffb7a6))
            .child("terminal offline"),
    }
}

fn terminal_row_element(
    snapshot: &TerminalSnapshot,
    selection: Option<TerminalSelection>,
    row_index: usize,
    row: &TerminalRow,
    metrics: TerminalTextMetrics,
    is_active: bool,
    cursor_blink_visible: bool,
) -> gpui::AnyElement {
    let cursor_row = usize::from(snapshot.cursor.0);
    let cursor_col = usize::from(snapshot.cursor.1);
    let show_cursor = !snapshot.closed
        && is_active
        && row_index == cursor_row
        && (cursor_blink_visible || !snapshot.cursor_blinking);
    let mut cell_index = 0usize;
    let mut cells = Vec::new();

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

            let display = match ch {
                ' ' => "\u{00A0}".to_string(),
                _ => ch.to_string(),
            };
            let underline_color = style.underline_color.map(terminal_color_hsla);
            let background = style.background.map(terminal_color_hsla);
            let color = terminal_text_color_from_style(style);

            cells.push(
                div()
                    .w(px(metrics.cell_width))
                    .h(px(metrics.line_height))
                    .overflow_hidden()
                    .font_family(TERMINAL_FONT_FAMILY)
                    .font_weight(if style.bold {
                        FontWeight::MEDIUM
                    } else {
                        FontWeight::NORMAL
                    })
                    .when(style.italic, |cell| cell.italic())
                    .text_size(px(metrics.font_size))
                    .line_height(px(metrics.line_height))
                    .text_color(color)
                    .when_some(background, |cell, background| cell.bg(background))
                    .when(style.underline, |cell| {
                        let cell = cell.underline().text_decoration_solid();
                        if let Some(color) = underline_color {
                            cell.text_decoration_color(color)
                        } else {
                            cell
                        }
                    })
                    .when(style.strikethrough, |cell| cell.line_through())
                    .child(display)
                    .into_any_element(),
            );

            cell_index += 1;
        }
    }

    div()
        .h(px(metrics.line_height))
        .flex()
        .items_start()
        .whitespace_nowrap()
        .children(cells)
        .into_any_element()
}

fn terminal_row_text(row: &TerminalRow) -> String {
    row.runs
        .iter()
        .map(|run| run.text.as_str())
        .collect::<String>()
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
    metrics: TerminalTextMetrics,
    screen_x: f32,
    screen_y: f32,
    header_height: f32,
    pane_padding: f32,
    snapshot: &TerminalSnapshot,
) -> TerminalFrameMetrics {
    let body_origin_x = screen_x + pane_padding + TERMINAL_BODY_INSET;
    let body_origin_y = screen_y + header_height + pane_padding + TERMINAL_BODY_INSET;

    TerminalFrameMetrics {
        body_origin: gpui::point(px(body_origin_x), px(body_origin_y)),
        cell_width: metrics.cell_width.max(1.0),
        cell_height: metrics.line_height.max(1.0),
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
                single_surface_pane(
                    7,
                    "Shell",
                    PaneKind::Shell,
                    WorkdeskPoint::new(48.0, 64.0),
                    WorkdeskSize::new(920.0, 560.0),
                ),
                single_surface_pane(
                    8,
                    "Agent",
                    PaneKind::Agent,
                    WorkdeskPoint::new(1024.0, 120.0),
                    WorkdeskSize::new(720.0, 420.0),
                ),
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
    fn persisted_workdesk_round_trips_attention_state() {
        let mut state = WorkdeskState::new(
            "Desk",
            "Summary",
            vec![single_surface_pane(
                9,
                "Agent",
                PaneKind::Agent,
                WorkdeskPoint::new(0.0, 0.0),
                WorkdeskSize::new(720.0, 420.0),
            )],
        );
        state.attention_sequence = 14;
        state.pane_attention.insert(
            PaneId::new(9),
            PaneAttention {
                state: AttentionState::Error,
                unread: true,
                last_attention_sequence: 12,
                last_activity_sequence: 8,
            },
        );

        let restored = PersistedWorkdesk::from_state(&state).into_state();
        let attention = restored.pane_attention(PaneId::new(9));

        assert_eq!(restored.attention_sequence, 14);
        assert_eq!(attention.state, AttentionState::Error);
        assert!(attention.unread);
        assert_eq!(attention.last_attention_sequence, 12);
        assert_eq!(attention.last_activity_sequence, 8);
    }

    #[test]
    fn legacy_persisted_pane_migrates_to_single_surface_stack() {
        let restored = PersistedWorkdesk {
            name: "Desk".to_string(),
            summary: "Summary".to_string(),
            metadata: WorkdeskMetadata::default(),
            panes: vec![PersistedPane {
                id: 11,
                title: "Shell".to_string(),
                kind: PersistedPaneKind::Shell,
                position: PersistedPoint { x: 24.0, y: 48.0 },
                size: PersistedSize {
                    width: 920.0,
                    height: 560.0,
                },
                active_surface_id: None,
                surfaces: Vec::new(),
                stack_title: None,
                attention: PaneAttention::default(),
            }],
            attention_sequence: 0,
            layout_mode: PersistedLayoutMode::Free,
            camera: PersistedPoint { x: 0.0, y: 0.0 },
            zoom: 1.0,
            active_pane: Some(11),
        }
        .into_state();

        assert_eq!(restored.panes.len(), 1);
        assert_eq!(restored.panes[0].surfaces.len(), 1);
        assert_eq!(restored.panes[0].active_surface_id, SurfaceId::new(11));
        assert_eq!(restored.panes[0].surfaces[0].kind, PaneKind::Shell);
        assert_eq!(restored.panes[0].title, restored.panes[0].surfaces[0].title);
    }

    #[test]
    fn persisted_editor_surface_restores_dirty_buffer_text() {
        let path = std::env::temp_dir().join(format!(
            "canvas-editor-test-{}-{}.rs",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time should be after epoch")
                .as_nanos()
        ));
        fs::write(&path, "fn main() {}\n").expect("fixture should write");

        let pane = PaneRecord::new(
            PaneId::new(12),
            WorkdeskPoint::new(0.0, 0.0),
            WorkdeskSize::new(920.0, 620.0),
            SurfaceRecord::editor(
                SurfaceId::new(21),
                "editor.rs",
                path.display().to_string(),
                true,
            ),
            None,
        );
        let mut state = WorkdeskState::new("Desk", "Summary", vec![pane]);
        state.editors.insert(
            SurfaceId::new(21),
            EditorBuffer::restore(&path, "let updated = true;\n", true),
        );

        let restored = PersistedWorkdesk::from_state(&state).into_state();
        let editor = restored
            .editors
            .get(&SurfaceId::new(21))
            .expect("editor should restore");

        assert_eq!(editor.text(), "let updated = true;\n");
        assert!(editor.dirty());
        assert_eq!(editor.path_string(), path.display().to_string());

        let _ = fs::remove_file(path);
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
    fn next_attention_target_picks_oldest_unread_attention() {
        let mut left = WorkdeskState::new(
            "Left",
            "Summary",
            vec![single_surface_pane(
                1,
                "Shell",
                PaneKind::Shell,
                WorkdeskPoint::new(0.0, 0.0),
                WorkdeskSize::new(920.0, 560.0),
            )],
        );
        left.pane_attention.insert(
            PaneId::new(1),
            PaneAttention {
                state: AttentionState::Waiting,
                unread: true,
                last_attention_sequence: 7,
                last_activity_sequence: 7,
            },
        );

        let mut right = WorkdeskState::new(
            "Right",
            "Summary",
            vec![single_surface_pane(
                2,
                "Agent",
                PaneKind::Agent,
                WorkdeskPoint::new(0.0, 0.0),
                WorkdeskSize::new(720.0, 420.0),
            )],
        );
        right.pane_attention.insert(
            PaneId::new(2),
            PaneAttention {
                state: AttentionState::Error,
                unread: true,
                last_attention_sequence: 11,
                last_activity_sequence: 11,
            },
        );

        assert_eq!(
            next_attention_target_for_workdesks(&[left, right]),
            Some((0, PaneId::new(1)))
        );
    }

    #[test]
    fn focusing_pane_clears_unread_but_keeps_error_state() {
        let mut state = WorkdeskState::new(
            "Desk",
            "Summary",
            vec![single_surface_pane(
                3,
                "Agent",
                PaneKind::Agent,
                WorkdeskPoint::new(0.0, 0.0),
                WorkdeskSize::new(720.0, 420.0),
            )],
        );
        state.pane_attention.insert(
            PaneId::new(3),
            PaneAttention {
                state: AttentionState::Error,
                unread: true,
                last_attention_sequence: 5,
                last_activity_sequence: 4,
            },
        );

        state.focus_pane(PaneId::new(3));
        let attention = state.pane_attention(PaneId::new(3));

        assert_eq!(attention.state, AttentionState::Error);
        assert!(!attention.unread);
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
    fn persisted_workdesk_round_trips_metadata() {
        let mut state = WorkdeskState::new("Desk", "Summary", Vec::new());
        state.metadata = WorkdeskMetadata {
            intent: "Ship the fix".to_string(),
            cwd: "/tmp/project".to_string(),
            branch: "codex/phase-3".to_string(),
            status: Some("Building".to_string()),
            progress: Some(WorkdeskProgress::new("Build", 42)),
        };

        let restored = PersistedWorkdesk::from_state(&state).into_state();

        assert_eq!(restored.metadata.intent, "Ship the fix");
        assert_eq!(restored.metadata.cwd, "/tmp/project");
        assert_eq!(restored.metadata.branch, "codex/phase-3");
        assert_eq!(restored.metadata.status.as_deref(), Some("Building"));
        assert_eq!(
            restored
                .metadata
                .progress
                .as_ref()
                .map(|progress| progress.value),
            Some(42)
        );
    }

    #[test]
    fn initial_workdesks_start_with_shell_template() {
        let workdesks = initial_workdesks();

        assert_eq!(workdesks.len(), 1);
        assert_eq!(workdesks[0].name, "Shell Desk");
        assert_eq!(workdesks[0].summary, WorkdeskTemplate::ShellDesk.summary());
        assert_eq!(workdesks[0].panes.len(), 1);
        assert_eq!(workdesks[0].panes[0].kind, PaneKind::Shell);
        assert_eq!(
            workdesks[0].metadata.intent,
            WorkdeskTemplate::ShellDesk.intent()
        );
    }

    #[test]
    fn automation_server_round_trips_request_lines() {
        let socket_path = std::env::temp_dir().join(format!(
            "canvas-automation-test-{}-{}.sock",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time should be after epoch")
                .as_nanos()
        ));
        let server = start_automation_server_at(socket_path.clone())
            .expect("automation server should start");
        let mut stream = std::os::unix::net::UnixStream::connect(&socket_path)
            .expect("socket should accept clients");

        stream
            .write_all(br#"{"id":1,"method":"state.current","params":{}}"#)
            .expect("request should write");
        stream.write_all(b"\n").expect("newline should write");

        let envelope = server
            .receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("automation envelope should be received");
        assert_eq!(envelope.request.method, "state.current");
        envelope
            .response_tx
            .send(AutomationResponse::success(
                envelope.request.id.clone(),
                json!({ "ok": true }),
            ))
            .expect("response should send");

        let mut response_line = String::new();
        BufReader::new(stream)
            .read_line(&mut response_line)
            .expect("response line should read");
        let response: Value =
            serde_json::from_str(response_line.trim()).expect("response should be valid json");

        assert_eq!(response["ok"], Value::Bool(true));
        assert_eq!(response["result"]["ok"], Value::Bool(true));

        let _ = fs::remove_file(socket_path);
    }

    #[test]
    fn automation_state_json_includes_metadata_and_panes() {
        let mut desk = WorkdeskState::new(
            "Desk",
            "Summary",
            vec![single_surface_pane(
                7,
                "Agent",
                PaneKind::Agent,
                WorkdeskPoint::new(0.0, 0.0),
                WorkdeskSize::new(720.0, 420.0),
            )],
        );
        desk.metadata = WorkdeskMetadata {
            intent: "Ship".to_string(),
            cwd: "/tmp/project".to_string(),
            branch: "codex/test".to_string(),
            status: Some("Working".to_string()),
            progress: Some(WorkdeskProgress::new("Build", 64)),
        };
        desk.pane_attention.insert(
            PaneId::new(7),
            PaneAttention {
                state: AttentionState::Waiting,
                unread: true,
                last_attention_sequence: 9,
                last_activity_sequence: 6,
            },
        );
        desk.terminal_statuses
            .insert(SurfaceId::new(7), Some("Running".to_string()));

        let payload = automation_workdesk_state_json(0, &desk, true);

        assert_eq!(
            payload["workdesk"]["intent"],
            Value::String("Ship".to_string())
        );
        assert_eq!(
            payload["workdesk"]["progress"]["value"],
            Value::Number(64.into())
        );
        assert_eq!(payload["panes"][0]["id"], Value::Number(7.into()));
        assert_eq!(
            payload["panes"][0]["kind"],
            Value::String("agent".to_string())
        );
        assert_eq!(
            payload["panes"][0]["surface_count"],
            Value::Number(1.into())
        );
        assert_eq!(
            payload["panes"][0]["surfaces"][0]["kind"],
            Value::String("agent".to_string())
        );
        assert_eq!(
            payload["panes"][0]["surfaces"][0]["status"],
            Value::String("Running".to_string())
        );
        assert_eq!(
            payload["panes"][0]["attention"]["state"],
            Value::String("waiting".to_string())
        );
    }
}
