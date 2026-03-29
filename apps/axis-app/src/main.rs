use attention::{
    next_attention_pane_target, next_attention_workdesk_target, reduce_pane_attention_state,
    should_notify_attention_transition, summarize_workdesk_attention,
};
use agent_timeline::{
    build_agent_timeline_view_model, AgentTimelineEntryView, PendingApprovalView,
};
use axis_agent_runtime::WorktreeService;
use axis_core::agent::{AgentAttention, AgentLifecycle, AgentSessionRecord, AgentTransportKind};
use axis_core::paths::daemon_socket_path;
use axis_core::workdesk::{WorkdeskId, WorkdeskRecord};
use axis_core::review::DeskReviewPayload;
use axis_core::worktree::{WorktreeBinding, WorktreeId};
use axis_core::{
    automation::{
        AutomationRequest as SharedAutomationRequest,
        AutomationResponse as SharedAutomationResponse,
    },
    PaneId, PaneKind, PaneRecord, Point as WorkdeskPoint, Size as WorkdeskSize, SurfaceId,
    SurfaceKind, SurfaceRecord,
};
use axis_core::review::ReviewLineKind;
use review::{
    build_desk_review_summary_view, build_desk_review_summary_view_from_payload,
    editor_jump_line_for_review_row, merge_review_local_after_fetch,
    refreshed_desk_review_summary_view, resolve_local_desk_review_payload,
    review_changed_file_preview, review_editor_open_failed_notice,
    review_file_hunkless_notice, review_payload_worktree_rebound, review_status_label,
    review_workspace_setup_notice, reusable_review_payload_cache, DeskReviewSummaryView,
    ReviewPanelLocalState, ReviewPanelRefreshContext,
};

mod agent_provider_popup;
mod agent_sessions;
mod agent_timeline;
mod attention;
mod automation;
mod daemon_client;
mod remote_terminals;
mod review;
mod worktrees;
use automation::{AutomationEnvelope, AutomationServer};
use axis_editor::{EditorBuffer, HighlightKind};
use axis_terminal::{
    ghostty_build_info, TerminalColor, TerminalGridSize, TerminalRow, TerminalRun, TerminalSnapshot,
};
use daemon_client::DaemonClient;
use gpui::{
    div, font, img, prelude::*, px, relative, rgb, rgba, size, App, Application, AssetSource,
    Bounds, ClipboardItem, Context, Element, ElementId, ElementInputHandler, EntityInputHandler,
    FocusHandle, FontStyle, FontWeight, GlobalElementId, KeyDownEvent, KeybindingKeystroke,
    Keystroke, LayoutId, MagnifyGestureEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Pixels, Point as GpuiPoint, ScrollWheelEvent, SharedString,
    SmartMagnifyGestureEvent, Style, StyledText, SwipeGestureEvent, TextRun, Timer,
    TitlebarOptions, TouchEvent, TouchPhase, UTF16Selection, Window, WindowBounds, WindowOptions,
};
use remote_terminals::RemoteTerminalSession;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    cmp::Ordering,
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    ops::Range,
    path::{Path, PathBuf},
    process::Command,
    sync::mpsc::{self, Receiver},
    time::{Duration, Instant},
};
use worktrees::{DEFAULT_AGENT_SIZE, DEFAULT_SHELL_SIZE};

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
const SIDEBAR_WIDTH: f32 = 216.0;
const SIDEBAR_COLLAPSED_WIDTH: f32 = 72.0;
const WORKDESK_MENU_WIDTH: f32 = 208.0;
const STACK_SURFACE_MENU_WIDTH: f32 = 188.0;
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
const SESSION_INSPECTOR_WIDTH: f32 = 420.0;
const WORKSPACE_PALETTE_WIDTH: f32 = 640.0;
const WORKSPACE_PALETTE_RESULTS_LIMIT: usize = 40;
const WORKSPACE_SEARCH_RESULTS_LIMIT: usize = 40;
const WORKDESK_EDITOR_WIDTH: f32 = 436.0;
const SIDEBAR_WINDOW_CONTROLS_INSET: f32 = 34.0;
const NOTIFICATION_PANEL_WIDTH: f32 = 264.0;
const MAX_NOTIFICATION_ITEMS: usize = 24;
const PRODUCT_NAME: &str = "axis";
const APP_DATA_DIR: &str = ".axis";
const LEGACY_APP_DATA_DIR: &str = ".canvas";
const APP_DATA_DIR_ENV: &str = "AXIS_APP_DATA_DIR";
const BRAND_ICON_ASSET: &str = "assets/branding/axis-icon.svg";

#[cfg(target_os = "macos")]
const TERMINAL_FONT_FAMILY: &str = "Menlo";
#[cfg(not(target_os = "macos"))]
const TERMINAL_FONT_FAMILY: &str = ".ZedMono";

struct AxisAppAssets {
    base: PathBuf,
}

impl AxisAppAssets {
    fn new() -> Self {
        Self {
            base: workspace_root_path(),
        }
    }
}

impl AssetSource for AxisAppAssets {
    fn load(&self, path: &str) -> gpui::Result<Option<std::borrow::Cow<'static, [u8]>>> {
        fs::read(self.base.join(path))
            .map(|data| Some(std::borrow::Cow::Owned(data)))
            .map_err(Into::into)
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
        fs::read_dir(self.base.join(path))
            .map(|entries| {
                entries
                    .filter_map(|entry| {
                        entry
                            .ok()
                            .and_then(|entry| entry.file_name().into_string().ok())
                            .map(SharedString::from)
                    })
                    .collect()
            })
            .map_err(Into::into)
    }
}

#[derive(Clone)]
struct WorkdeskState {
    workdesk_id: String,
    name: String,
    summary: String,
    metadata: WorkdeskMetadata,
    /// Transient unique runtime key for bridge state; assigned on boot/create, not persisted.
    runtime_id: u64,
    /// Transient execution binding (not persisted).
    worktree_binding: Option<WorktreeBinding>,
    /// Transient review summary derived from current worktree metadata.
    review_summary: Option<DeskReviewSummaryView>,
    /// Last structured review payload from daemon/local fetch; retained when only compact summary refreshes.
    review_payload_cache: Option<DeskReviewPayload>,
    /// Desk-local selection, hunk markers, and review notices (not persisted).
    review_local_state: ReviewPanelLocalState,
    panes: Vec<PaneRecord>,
    pane_attention: HashMap<PaneId, PaneAttention>,
    terminals: HashMap<SurfaceId, RemoteTerminalSession>,
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

struct AxisShell {
    workdesks: Vec<WorkdeskState>,
    active_workdesk: usize,
    next_workdesk_id: u64,
    next_workdesk_runtime_id: u64,
    agent_runtime: agent_sessions::AgentRuntimeBridge,
    last_agent_runtime_revision: u64,
    workdesk_menu: Option<WorkdeskContextMenu>,
    stack_surface_menu: Option<StackSurfaceMenu>,
    // Popup UI and shortcuts land in follow-up tasks; state is wired here for Task 2.
    #[allow(dead_code)]
    agent_provider_popup: Option<agent_provider_popup::AgentProviderPopupState>,
    workdesk_editor: Option<WorkdeskEditorState>,
    session_inspector: Option<AgentSessionInspectorTarget>,
    agent_session_composer: Option<AgentSessionComposerState>,
    workspace_palette: Option<WorkspacePaletteState>,
    /// Desk index whose structured review payload is shown in the right-side review panel.
    review_panel: Option<usize>,
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
    notifications: NotificationCenter,
    visible_terminal_surfaces: HashSet<SurfaceId>,
    cursor_blink_visible: bool,
    last_cursor_blink_at: Instant,
    last_daemon_sync_at: Instant,
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

#[derive(Clone, Copy, Debug)]
struct StackSurfaceMenu {
    desk_index: usize,
    pane_id: PaneId,
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
    #[serde(alias = "waiting")]
    NeedsInput,
    NeedsReview,
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

#[derive(Clone, Debug, PartialEq, Eq)]
struct ProductNotification {
    id: u64,
    state: AttentionState,
    title: String,
    detail: String,
    context: String,
    unread: bool,
    workdesk_index: usize,
    pane_id: Option<PaneId>,
}

#[derive(Clone, Debug, Default)]
struct NotificationCenter {
    next_id: u64,
    items: Vec<ProductNotification>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AgentSessionInspectorTarget {
    desk_index: usize,
    surface_id: SurfaceId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct AgentSessionComposerState {
    target: AgentSessionInspectorTarget,
    active: bool,
    draft: String,
}

impl AgentSessionComposerState {
    fn new(target: AgentSessionInspectorTarget) -> Self {
        Self {
            target,
            active: false,
            draft: String::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct AgentSessionInspectorView {
    session_id: String,
    provider_profile_id: String,
    capability_note: Option<String>,
    transport: AgentTransportKind,
    lifecycle: AgentLifecycle,
    attention: AgentAttention,
    status_message: String,
    cwd: String,
    workdesk_name: String,
    pane_title: String,
    surface_id: SurfaceId,
    terminal_status: Option<String>,
    transcript_preview: Vec<String>,
    can_send_turn: bool,
    can_resume: bool,
    can_respond_approval: bool,
    timeline_entries: Vec<AgentTimelineEntryView>,
    pending_approvals: Vec<PendingApprovalView>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WorkspacePaletteMode {
    OpenFile,
    SearchWorkspace,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct WorkspaceFileCandidate {
    absolute_path: String,
    relative_path: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum WorkspacePaletteResult {
    File(WorkspaceFileCandidate),
    SearchMatch {
        absolute_path: String,
        relative_path: String,
        line_number: usize,
        preview: String,
    },
}

#[derive(Clone, Debug)]
struct WorkspacePaletteState {
    mode: WorkspacePaletteMode,
    root_path: PathBuf,
    query: String,
    all_files: Vec<WorkspaceFileCandidate>,
    results: Vec<WorkspacePaletteResult>,
    selected: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WorkdeskNavigationMode {
    Attention,
    Resume,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct WorkdeskNavigationTarget {
    pane_id: PaneId,
    state: AttentionState,
    unread: bool,
    mode: WorkdeskNavigationMode,
    label: String,
    detail: String,
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
    QuickOpen,
    SearchWorkspace,
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
    #[serde(default)]
    workdesk_id: String,
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
    fn matches_layout(
        &self,
        bounds: Bounds<Pixels>,
        line_height: f32,
        char_width: f32,
        viewport_lines: usize,
    ) -> bool {
        self.text_bounds
            .is_some_and(|current| bounds_approx_eq(current, bounds))
            && approx_eq_f32(self.line_height, line_height)
            && approx_eq_f32(self.char_width, char_width)
            && self.viewport_lines == viewport_lines.max(1)
    }

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
    shell: gpui::Entity<AxisShell>,
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
        let needs_layout_sync = {
            let shell = self.shell.read(cx);
            !shell
                .active_workdesk()
                .editor_views
                .get(&surface_id)
                .is_some_and(|view| {
                    view.matches_layout(bounds, line_height, char_width, viewport_lines)
                })
        };
        if needs_layout_sync {
            self.shell.update(cx, |shell, _cx| {
                let view = shell
                    .active_workdesk_mut()
                    .editor_views
                    .entry(surface_id)
                    .or_default();
                view.text_bounds = Some(bounds);
                view.line_height = line_height;
                view.char_width = char_width;
                view.gutter_width = 0.0;
                view.viewport_lines = viewport_lines.max(1);
            });
        }
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

impl EntityInputHandler for AxisShell {
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

    fn affects_row(self, row: usize) -> bool {
        let (start, end) = self.ordered();
        row >= start.row && row <= end.row
    }
}

impl AttentionState {
    fn label(self) -> &'static str {
        match self {
            Self::Idle => "Idle",
            Self::Working => "Working",
            Self::NeedsInput => "Needs input",
            Self::NeedsReview => "Needs review",
            Self::Error => "Error",
        }
    }

    fn is_attention(self) -> bool {
        matches!(self, Self::NeedsInput | Self::NeedsReview | Self::Error)
    }

    fn tint(self) -> gpui::Hsla {
        match self {
            Self::Idle => rgb(0x5e6c76).into(),
            Self::Working => rgb(0x7cc7ff).into(),
            Self::NeedsInput => rgb(0xf0d35f).into(),
            Self::NeedsReview => rgb(0x7cc7ff).into(),
            Self::Error => rgb(0xff9b88).into(),
        }
    }

    fn summary_priority(self) -> u8 {
        match self {
            Self::Idle => 0,
            Self::Working => 1,
            Self::NeedsReview => 2,
            Self::NeedsInput => 3,
            Self::Error => 4,
        }
    }

    fn jump_priority(self) -> u8 {
        match self {
            Self::NeedsInput => 0,
            Self::NeedsReview => 1,
            Self::Error => 2,
            Self::Working => 3,
            Self::Idle => 4,
        }
    }

    fn should_notify(self) -> bool {
        matches!(self, Self::NeedsInput | Self::NeedsReview | Self::Error)
    }

    fn notification_title(self) -> &'static str {
        match self {
            Self::NeedsInput => "Input requested",
            Self::NeedsReview => "Review requested",
            Self::Error => "Error raised",
            Self::Idle => "Attention cleared",
            Self::Working => "Work resumed",
        }
    }
}

impl WorkdeskAttentionSummary {
    fn register(&mut self, attention: PaneAttention) {
        if attention.unread && attention.state.is_attention() {
            self.unread_count += 1;
        }

        if attention.state.summary_priority() > self.highest.summary_priority() {
            self.highest = attention.state;
        }
    }
}

impl NotificationCenter {
    fn unread_count(&self) -> usize {
        self.items.iter().filter(|item| item.unread).count()
    }

    fn mark_all_read(&mut self) -> bool {
        let mut changed = false;
        for item in &mut self.items {
            if item.unread {
                item.unread = false;
                changed = true;
            }
        }
        changed
    }

    fn push_attention_event(
        &mut self,
        workdesk_index: usize,
        desk_name: &str,
        pane_id: PaneId,
        pane_title: &str,
        state: AttentionState,
        unread: bool,
    ) {
        self.next_id = self.next_id.saturating_add(1);
        self.items.push(ProductNotification {
            id: self.next_id,
            state,
            title: state.notification_title().to_string(),
            detail: format!("{pane_title} on {desk_name} is {}.", state.label()),
            context: format!("{desk_name} · {pane_title}"),
            unread,
            workdesk_index,
            pane_id: Some(pane_id),
        });
        if self.items.len() > MAX_NOTIFICATION_ITEMS {
            let overflow = self.items.len() - MAX_NOTIFICATION_ITEMS;
            self.items.drain(0..overflow);
        }
    }
}

fn terminal_snapshot_preview_lines(snapshot: &TerminalSnapshot, max_lines: usize) -> Vec<String> {
    let mut lines = snapshot
        .rows
        .iter()
        .map(|row| {
            row.runs
                .iter()
                .fold(String::new(), |mut line, run| {
                    line.push_str(&run.text);
                    line
                })
                .replace('\u{00A0}', " ")
                .trim_end()
                .to_string()
        })
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    if lines.len() > max_lines {
        lines.drain(0..lines.len() - max_lines);
    }
    lines
}

fn agent_session_inspector_view(
    bridge: &agent_sessions::AgentRuntimeBridge,
    workdesk: &WorkdeskState,
    surface_id: SurfaceId,
) -> Option<AgentSessionInspectorView> {
    let record = bridge.session_for_surface(workdesk.runtime_id, surface_id)?;
    let detail = bridge
        .session_detail(&record.id, None)
        .unwrap_or_else(|_| axis_core::agent_history::AgentSessionDetail {
            session: record.clone(),
            capabilities: Default::default(),
            started_at_ms: None,
            updated_at_ms: None,
            completed_at_ms: None,
            revision: 0,
            history_cursor: 0,
            pending_approval_id: None,
            timeline: Vec::new(),
            truncated: false,
        });
    let timeline = build_agent_timeline_view_model(&detail);
    let pane_title = workdesk
        .panes
        .iter()
        .find_map(|pane| pane.surface(surface_id).map(|surface| surface.title.clone()))
        .unwrap_or_else(|| "Agent".to_string());
    let capability_note = bridge
        .provider_profile(&record.provider_profile_id)
        .and_then(|profile| profile.capability_note);
    let terminal_status = workdesk
        .terminal_statuses
        .get(&surface_id)
        .cloned()
        .flatten()
        .filter(|status| !status.trim().is_empty());
    let transcript_preview = workdesk
        .terminals
        .get(&surface_id)
        .map(|terminal| terminal_snapshot_preview_lines(&terminal.snapshot(), 12))
        .unwrap_or_default();

    Some(AgentSessionInspectorView {
        session_id: record.id.0.clone(),
        provider_profile_id: record.provider_profile_id,
        capability_note,
        transport: record.transport,
        lifecycle: record.lifecycle,
        attention: record.attention,
        status_message: record.status_message,
        cwd: record.cwd,
        workdesk_name: workdesk.name.clone(),
        pane_title,
        surface_id,
        terminal_status,
        transcript_preview,
        can_send_turn: timeline.can_send_turn,
        can_resume: timeline.can_resume,
        can_respond_approval: timeline.can_respond_approval,
        timeline_entries: timeline.timeline_entries,
        pending_approvals: timeline.pending_approvals,
    })
}

fn agent_transport_label(transport: AgentTransportKind) -> &'static str {
    match transport {
        AgentTransportKind::CliWrapped => "CLI wrapped",
        AgentTransportKind::NativeAcp => "Native ACP",
    }
}

fn agent_lifecycle_label(lifecycle: AgentLifecycle) -> &'static str {
    match lifecycle {
        AgentLifecycle::Planned => "Planned",
        AgentLifecycle::Starting => "Starting",
        AgentLifecycle::Running => "Running",
        AgentLifecycle::Waiting => "Waiting",
        AgentLifecycle::Completed => "Completed",
        AgentLifecycle::Failed => "Failed",
        AgentLifecycle::Cancelled => "Cancelled",
    }
}

fn agent_attention_label(attention: AgentAttention) -> &'static str {
    match attention {
        AgentAttention::Quiet => "Quiet",
        AgentAttention::Working => "Working",
        AgentAttention::NeedsInput => "Needs input",
        AgentAttention::NeedsReview => "Needs review",
        AgentAttention::Error => "Error",
    }
}

impl WorkspacePaletteMode {
    fn title(self) -> &'static str {
        match self {
            Self::OpenFile => "Quick open",
            Self::SearchWorkspace => "Workspace search",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::OpenFile => "Jump to a file in the current worktree.",
            Self::SearchWorkspace => "Search across files without leaving the canvas.",
        }
    }

    fn prompt(self) -> &'static str {
        match self {
            Self::OpenFile => "Type a path fragment",
            Self::SearchWorkspace => "Type a grep query",
        }
    }

    fn empty_label(self) -> &'static str {
        match self {
            Self::OpenFile => "No files match the current query.",
            Self::SearchWorkspace => "No workspace matches yet.",
        }
    }
}

impl WorkspacePaletteState {
    fn new(mode: WorkspacePaletteMode, root_path: PathBuf) -> Self {
        let all_files = load_workspace_file_candidates(&root_path);
        let mut this = Self {
            mode,
            root_path,
            query: String::new(),
            all_files,
            results: Vec::new(),
            selected: 0,
        };
        this.refresh_results();
        this
    }

    fn refresh_results(&mut self) {
        self.results = match self.mode {
            WorkspacePaletteMode::OpenFile => quick_open_results(&self.all_files, &self.query),
            WorkspacePaletteMode::SearchWorkspace => {
                workspace_search_results(&self.root_path, &self.query)
            }
        };
        if self.results.is_empty() {
            self.selected = 0;
        } else {
            self.selected = self.selected.min(self.results.len().saturating_sub(1));
        }
    }

    fn append_query(&mut self, text: &str) {
        self.query.push_str(text);
        self.refresh_results();
    }

    fn pop_query(&mut self) {
        self.query.pop();
        self.refresh_results();
    }

    fn move_selection(&mut self, delta: isize) {
        if self.results.is_empty() {
            self.selected = 0;
            return;
        }
        let len = self.results.len() as isize;
        let next = (self.selected as isize + delta).clamp(0, len - 1);
        self.selected = next as usize;
    }

    fn selected_result(&self) -> Option<&WorkspacePaletteResult> {
        self.results.get(self.selected)
    }
}

fn load_workspace_file_candidates(root_path: &Path) -> Vec<WorkspaceFileCandidate> {
    load_workspace_file_candidates_with_rg(root_path)
        .unwrap_or_else(|| load_workspace_file_candidates_fallback(root_path))
}

fn load_workspace_file_candidates_with_rg(root_path: &Path) -> Option<Vec<WorkspaceFileCandidate>> {
    let output = Command::new("rg")
        .current_dir(root_path)
        .arg("--files")
        .arg(".")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let root = root_path.to_path_buf();
    Some(
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| {
                let absolute = root.join(line);
                relative_workspace_path(&root, &absolute).map(|relative_path| WorkspaceFileCandidate {
                    absolute_path: absolute.display().to_string(),
                    relative_path,
                })
            })
            .collect::<Vec<_>>(),
    )
}

fn load_workspace_file_candidates_fallback(root_path: &Path) -> Vec<WorkspaceFileCandidate> {
    let mut results = Vec::new();
    collect_workspace_files(root_path, root_path, &mut results);
    results.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    results
}

fn collect_workspace_files(root_path: &Path, current: &Path, results: &mut Vec<WorkspaceFileCandidate>) {
    let Ok(entries) = fs::read_dir(current) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.is_dir() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if matches!(
                name.as_ref(),
                ".git" | "target" | ".axis" | "node_modules"
            ) {
                continue;
            }
            collect_workspace_files(root_path, &path, results);
            continue;
        }
        if metadata.is_file() {
            if let Some(relative_path) = relative_workspace_path(root_path, &path) {
                results.push(WorkspaceFileCandidate {
                    absolute_path: path.display().to_string(),
                    relative_path,
                });
            }
        }
    }
}

fn relative_workspace_path(root_path: &Path, path: &Path) -> Option<String> {
    path.strip_prefix(root_path)
        .ok()
        .map(|relative| relative.display().to_string())
}

fn quick_open_results(
    all_files: &[WorkspaceFileCandidate],
    query: &str,
) -> Vec<WorkspacePaletteResult> {
    let normalized = query.trim().to_lowercase();
    let mut matches = all_files
        .iter()
        .filter_map(|candidate| {
            let relative = candidate.relative_path.to_lowercase();
            let score = if normalized.is_empty() {
                Some((0usize, candidate.relative_path.len()))
            } else {
                relative
                    .find(&normalized)
                    .map(|index| (index, candidate.relative_path.len()))
            }?;
            Some((score, WorkspacePaletteResult::File(candidate.clone())))
        })
        .collect::<Vec<_>>();
    matches.sort_by(|left, right| left.0.cmp(&right.0));
    matches
        .into_iter()
        .map(|(_, result)| result)
        .take(WORKSPACE_PALETTE_RESULTS_LIMIT)
        .collect()
}

fn workspace_search_results(root_path: &Path, query: &str) -> Vec<WorkspacePaletteResult> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }

    let Ok(output) = Command::new("rg")
        .current_dir(root_path)
        .arg("--line-number")
        .arg("--no-heading")
        .arg("--color")
        .arg("never")
        .arg("--smart-case")
        .arg(trimmed)
        .arg(".")
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() && output.status.code() != Some(1) {
        return Vec::new();
    }

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let mut parts = line.splitn(3, ':');
            let path = parts.next()?;
            let line_number = parts.next()?.parse::<usize>().ok()?;
            let preview = parts.next().unwrap_or_default().trim().to_string();
            let absolute = root_path.join(path);
            let relative_path = relative_workspace_path(root_path, &absolute)?;
            Some(WorkspacePaletteResult::SearchMatch {
                absolute_path: absolute.display().to_string(),
                relative_path,
                line_number,
                preview,
            })
        })
        .take(WORKSPACE_SEARCH_RESULTS_LIMIT)
        .collect()
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

const SHORTCUT_ACTIONS: [ShortcutAction; 31] = [
    ShortcutAction::ToggleShortcutPanel,
    ShortcutAction::ToggleInspector,
    ShortcutAction::NextAttention,
    ShortcutAction::ClearActiveAttention,
    ShortcutAction::SpawnShellPane,
    ShortcutAction::SpawnAgentPane,
    ShortcutAction::SpawnBrowserPane,
    ShortcutAction::SpawnEditorPane,
    ShortcutAction::QuickOpen,
    ShortcutAction::SearchWorkspace,
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
            Self::QuickOpen => "quick-open",
            Self::SearchWorkspace => "search-workspace",
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
            | Self::QuickOpen
            | Self::SearchWorkspace
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
            Self::QuickOpen => "Quick open file",
            Self::SearchWorkspace => "Search workspace",
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
                "Dismiss the current pane's needs-input, needs-review, or error attention state."
            }
            Self::SpawnShellPane => "Create a new shell pane near the viewport center.",
            Self::SpawnAgentPane => "Create a new agent pane near the viewport center.",
            Self::SpawnBrowserPane => "Create a new browser pane near the viewport center.",
            Self::SpawnEditorPane => "Open a file picker and create or focus an editor surface.",
            Self::QuickOpen => "Open the in-app file switcher for the current worktree.",
            Self::SearchWorkspace => "Run a workspace-wide search and jump to matching files.",
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
            Self::QuickOpen => Some("cmd-p"),
            Self::SearchWorkspace => Some("cmd-shift-f"),
            Self::NextSurface => Some("ctrl-tab"),
            Self::PreviousSurface => Some("ctrl-shift-tab"),
            Self::CloseActivePane => Some("cmd-shift-w"),
            Self::SpawnWorkdesk => Some("cmd-shift-d"),
            Self::SelectPreviousWorkdesk => Some("cmd-alt-["),
            Self::SelectNextWorkdesk => Some("cmd-alt-]"),
            Self::LayoutFree => Some("cmd-alt-1"),
            Self::LayoutGrid => Some("cmd-alt-2"),
            Self::LayoutSplit => Some("cmd-alt-3"),
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
            workdesk_id: String::new(),
            name,
            summary: summary.clone(),
            metadata: default_workdesk_metadata(summary, None, None),
            runtime_id: 0,
            worktree_binding: None,
            review_summary: None,
            review_payload_cache: None,
            review_local_state: ReviewPanelLocalState::default(),
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

    fn active_terminal_session_for_pane(&self, pane_id: PaneId) -> Option<&RemoteTerminalSession> {
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
            terminal.sync();
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
        summarize_workdesk_attention(self.pane_attention.values().copied())
    }

    fn attach_terminal_session(
        &mut self,
        surface_id: SurfaceId,
        kind: &PaneKind,
        title: &str,
        grid: TerminalGridSize,
    ) {
        match RemoteTerminalSession::attach_or_create(
            &self.workdesk_id,
            surface_id,
            kind,
            title,
            &self.metadata.cwd,
            grid,
        ) {
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
    fn from_shell(shell: &AxisShell) -> Self {
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
            workdesk_id: state.workdesk_id.clone(),
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
            workdesk_id,
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
        state.workdesk_id = workdesk_id;
        state.metadata = metadata.hydrated();
        state.worktree_binding = worktrees::refreshed_binding_from_desk_paths(
            &state.metadata.cwd,
            &state.metadata.branch,
        );
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

impl AxisShell {
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
        Self::new_with_agent_runtime(
            workdesks,
            active_workdesk,
            shortcuts,
            boot_notice,
            automation_server,
            focus_handle,
            ghostty_vendor_dir,
            ghostty_status,
            agent_sessions::AgentRuntimeBridge::new(),
        )
    }

    fn new_with_agent_runtime(
        workdesks: Vec<WorkdeskState>,
        active_workdesk: usize,
        shortcuts: ShortcutMap,
        boot_notice: Option<SharedString>,
        automation_server: AutomationServer,
        focus_handle: FocusHandle,
        ghostty_vendor_dir: SharedString,
        ghostty_status: SharedString,
        agent_runtime: agent_sessions::AgentRuntimeBridge,
    ) -> Self {
        let AutomationServer {
            receiver,
            socket_path,
        } = automation_server;
        let clamped_active_workdesk = active_workdesk.min(workdesks.len().saturating_sub(1));
        let mut shell = Self {
            workdesks,
            active_workdesk: clamped_active_workdesk,
            next_workdesk_id: 1,
            next_workdesk_runtime_id: 1,
            agent_runtime,
            last_agent_runtime_revision: 0,
            workdesk_menu: None,
            stack_surface_menu: None,
            agent_provider_popup: None,
            workdesk_editor: None,
            session_inspector: None,
            agent_session_composer: None,
            workspace_palette: None,
            review_panel: None,
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
            notifications: NotificationCenter::default(),
            visible_terminal_surfaces: HashSet::new(),
            cursor_blink_visible: true,
            last_cursor_blink_at: Instant::now(),
            last_daemon_sync_at: Instant::now(),
            persist_generation: 0,
            touchpad_pan_state: None,
            last_touchpad_pan_end: None,
        };
        shell.assign_workdesk_ids_to_workdesks();
        shell.assign_runtime_ids_to_workdesks();

        if let Some(notice) = boot_notice {
            if let Some(workdesk) = shell.workdesks.get_mut(shell.active_workdesk) {
                workdesk.runtime_notice = Some(notice);
            }
        }

        for workdesk in &mut shell.workdesks {
            boot_workdesk_terminals(workdesk);
        }

        for index in 0..shell.workdesks.len() {
            if let Err(error) = shell.sync_review_summary_for_desk(index) {
                if let Some(workdesk) = shell.workdesks.get_mut(index) {
                    workdesk.runtime_notice =
                        Some(SharedString::from(format!("review summary stale: {error}")));
                }
            }
        }

        if let Err(error) = shell.sync_daemon_runtime_state() {
            if let Some(workdesk) = shell.workdesks.get_mut(shell.active_workdesk) {
                workdesk.runtime_notice.get_or_insert_with(|| {
                    SharedString::from(format!("axisd sync failed: {error}"))
                });
            }
        }

        shell
    }

    fn active_workdesk(&self) -> &WorkdeskState {
        &self.workdesks[self.active_workdesk]
    }

    fn active_workdesk_mut(&mut self) -> &mut WorkdeskState {
        &mut self.workdesks[self.active_workdesk]
    }

    fn workdesk_agent_cwd(workdesk: &WorkdeskState) -> String {
        workdesk
            .worktree_binding
            .as_ref()
            .map(|binding| binding.root_path.clone())
            .filter(|path| !path.trim().is_empty())
            .unwrap_or_else(|| workdesk.metadata.cwd.clone())
    }

    fn assign_workdesk_ids_to_workdesks(&mut self) {
        assign_missing_workdesk_ids(&mut self.workdesks, &mut self.next_workdesk_id);
    }

    fn allocate_workdesk_id(&mut self) -> String {
        loop {
            let candidate = format!("desk-{}", self.next_workdesk_id);
            self.next_workdesk_id = self.next_workdesk_id.saturating_add(1);
            if !self
                .workdesks
                .iter()
                .any(|desk| desk.workdesk_id == candidate)
            {
                return candidate;
            }
        }
    }

    fn assign_runtime_ids_to_workdesks(&mut self) {
        let mut next_id = self.next_workdesk_runtime_id;
        for desk in &mut self.workdesks {
            if desk.runtime_id == 0 {
                desk.runtime_id = next_id;
                next_id += 1;
            } else {
                next_id = next_id.max(desk.runtime_id.saturating_add(1));
            }
        }
        self.next_workdesk_runtime_id = next_id;
    }

    fn allocate_workdesk_runtime_id(&mut self) -> u64 {
        let id = self.next_workdesk_runtime_id;
        self.next_workdesk_runtime_id = self.next_workdesk_runtime_id.saturating_add(1);
        id
    }

    fn boot_agent_sessions_for_desk(
        bridge: &agent_sessions::AgentRuntimeBridge,
        workdesk_runtime_id: u64,
        desk: &mut WorkdeskState,
    ) {
        let agent_surface_ids = desk
            .panes
            .iter()
            .flat_map(|pane| pane.surfaces.iter())
            .filter(|surface| surface.kind == PaneKind::Agent)
            .map(|surface| surface.id)
            .collect::<Vec<_>>();
        let mut errors = Vec::new();

        for surface_id in agent_surface_ids {
            if let Err(error) =
                ensure_agent_runtime_for_surface(bridge, workdesk_runtime_id, desk, surface_id)
            {
                errors.push(error);
            }
        }

        if errors.is_empty() {
            let should_clear = desk
                .runtime_notice
                .as_ref()
                .map(|notice| {
                    notice
                        .to_string()
                        .starts_with("Agent runtime did not start:")
                })
                .unwrap_or(false);
            if should_clear {
                desk.runtime_notice = None;
            }
        } else {
            desk.runtime_notice = Some(SharedString::from(format!(
                "Agent runtime did not start: {}",
                errors.join("; ")
            )));
        }
    }

    fn ensure_agent_runtime_for_pane(&mut self, desk_index: usize, pane_id: PaneId) {
        let Some((workdesk_runtime_id, surface_id)) =
            self.workdesks.get(desk_index).and_then(|desk| {
                desk.active_surface_id_for_pane(pane_id)
                    .map(|surface_id| (desk.runtime_id, surface_id))
            })
        else {
            return;
        };

        let result = {
            let bridge = &self.agent_runtime;
            let Some(desk) = self.workdesks.get_mut(desk_index) else {
                return;
            };
            ensure_agent_runtime_for_surface(bridge, workdesk_runtime_id, desk, surface_id)
        };

        if let Err(error) = result {
            if let Some(desk) = self.workdesks.get_mut(desk_index) {
                desk.runtime_notice = Some(SharedString::from(format!(
                    "Agent runtime did not start: {error}"
                )));
            }
        }
    }

    fn sync_agent_desk_paths(&self) {
        for desk in &self.workdesks {
            let cwd = Self::workdesk_agent_cwd(desk);
            self.agent_runtime.set_desk_cwd(desk.runtime_id, cwd);
        }
    }

    fn sync_daemon_runtime_state(&mut self) -> Result<(), String> {
        let daemon = DaemonClient::default();
        let workspace_root = workspace_root_path().display().to_string();
        daemon.gui_heartbeat(workspace_root.clone(), std::process::id())?;
        for desk in &self.workdesks {
            daemon.ensure_workdesk(workdesk_record_from_state(desk, &workspace_root))?;
        }
        self.last_daemon_sync_at = Instant::now();
        Ok(())
    }

    fn sync_daemon_runtime_state_if_due(&mut self) {
        if self.last_daemon_sync_at.elapsed() < Duration::from_secs(2) {
            return;
        }
        let _ = self.sync_daemon_runtime_state();
    }

    fn sync_agent_runtime_activity(&mut self, cx: &mut Context<Self>) -> bool {
        self.sync_agent_desk_paths();
        for desk in &self.workdesks {
            for pane in &desk.panes {
                for surface in &pane.surfaces {
                    if surface.kind == PaneKind::Agent && desk.terminals.contains_key(&surface.id) {
                        let _ = self.agent_runtime.poll_surface(desk.runtime_id, surface.id);
                    }
                }
            }
        }
        let rev = self.agent_runtime.revision();
        if rev != self.last_agent_runtime_revision {
            self.last_agent_runtime_revision = rev;
            let active_workdesk = self.active_workdesk;
            let mut attention_changed = false;
            for desk_index in 0..self.workdesks.len() {
                let previous_attentions = {
                    let desk = &self.workdesks[desk_index];
                    desk.panes
                        .iter()
                        .filter(|pane| pane.kind == PaneKind::Agent)
                        .map(|pane| (pane.id, desk.pane_attention(pane.id)))
                        .collect::<Vec<_>>()
                };
                let changed = {
                    let desk = &mut self.workdesks[desk_index];
                    sync_agent_runtime_attention_for_workdesk(
                        &self.agent_runtime,
                        desk_index,
                        active_workdesk,
                        desk,
                    )
                };
                attention_changed |= changed;
                if !changed {
                    continue;
                }
                let notifications = {
                    let desk = &self.workdesks[desk_index];
                    previous_attentions
                        .into_iter()
                        .filter_map(|(pane_id, previous)| {
                            let pane = desk.panes.iter().find(|pane| pane.id == pane_id)?;
                            let current = desk.pane_attention(pane_id);
                            should_notify_attention_transition(previous.state, current.state).then(
                                || {
                                    (
                                        pane_id,
                                        pane.title.clone(),
                                        desk.name.clone(),
                                        current.state,
                                        current.unread,
                                    )
                                },
                            )
                        })
                        .collect::<Vec<_>>()
                };
                for (pane_id, pane_title, desk_name, state, unread) in notifications {
                    self.push_attention_notification(
                        desk_index,
                        pane_id,
                        &pane_title,
                        &desk_name,
                        state,
                        unread,
                    );
                    self.set_runtime_notice_for_workdesk(
                        desk_index,
                        format!("{pane_title} on {desk_name} is {}", state.label()),
                    );
                }
            }
            if attention_changed {
                self.request_persist(cx);
            }
            return true;
        }
        false
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

    fn set_runtime_notice_for_workdesk(&mut self, desk_index: usize, message: impl Into<String>) {
        if let Some(workdesk) = self.workdesks.get_mut(desk_index) {
            workdesk.runtime_notice = Some(SharedString::from(message.into()));
        }
    }

    fn set_runtime_notice(&mut self, message: impl Into<String>) {
        self.set_runtime_notice_for_workdesk(self.active_workdesk, message);
    }

    fn dismiss_runtime_notice(&mut self) -> bool {
        dismiss_runtime_notice_for_workdesks(&mut self.workdesks, self.active_workdesk)
    }

    fn notification_unread_count(&self) -> usize {
        self.notifications.unread_count()
    }

    fn push_attention_notification(
        &mut self,
        desk_index: usize,
        pane_id: PaneId,
        pane_title: &str,
        desk_name: &str,
        state: AttentionState,
        unread: bool,
    ) {
        self.notifications.push_attention_event(
            desk_index,
            desk_name,
            pane_id,
            pane_title,
            state,
            unread && !self.notifications_open,
        );
    }

    fn open_notification_target(&mut self, notification_id: u64, cx: &mut Context<Self>) {
        let target = self
            .notifications
            .items
            .iter_mut()
            .find(|item| item.id == notification_id)
            .map(|item| {
                item.unread = false;
                (item.workdesk_index, item.pane_id)
            });
        let Some((desk_index, pane_id)) = target else {
            return;
        };
        self.dismiss_notifications();
        if let Some(pane_id) = pane_id {
            if !self.navigate_to_workdesk_pane(desk_index, pane_id, cx) {
                self.set_runtime_notice("Notification target is no longer available");
            }
        }
        cx.notify();
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

    fn active_surface_kind(&self) -> Option<PaneKind> {
        let pane_id = self.active_workdesk().active_pane?;
        self.active_workdesk()
            .active_surface_for_pane(pane_id)
            .map(|surface| surface.kind.clone())
    }

    fn refresh_loop_interval(&self) -> Duration {
        match self.active_surface_kind() {
            Some(PaneKind::Shell) | Some(PaneKind::Agent) => Duration::from_millis(33),
            Some(PaneKind::Editor) => {
                if self.visible_terminal_surfaces.is_empty() {
                    Duration::from_millis(220)
                } else {
                    Duration::from_millis(90)
                }
            }
            Some(PaneKind::Browser) | None => {
                if self.visible_terminal_surfaces.is_empty() {
                    Duration::from_millis(250)
                } else {
                    Duration::from_millis(110)
                }
            }
        }
    }

    fn cursor_blink_target_active(&self) -> bool {
        matches!(
            self.active_surface_kind(),
            Some(PaneKind::Shell) | Some(PaneKind::Agent) | Some(PaneKind::Editor)
        )
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
        workdesks: &[WorkdeskState],
        desk_index: usize,
        canonical_path: &str,
    ) -> Option<(PaneId, SurfaceId)> {
        let desk = workdesks.get(desk_index)?;
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
        agent_bridge: &agent_sessions::AgentRuntimeBridge,
        agent_profile_id: Option<&str>,
    ) {
        match surface.kind {
            PaneKind::Shell | PaneKind::Agent => {
                desk.attach_terminal_session(
                    surface.id,
                    &surface.kind,
                    &surface.title,
                    terminal_grid_size_for_pane(pane_size, pane_surface_count),
                );
                if surface.kind == PaneKind::Agent {
                    let cwd = desk
                        .worktree_binding
                        .as_ref()
                        .map(|b| b.root_path.as_str())
                        .unwrap_or_else(|| desk.metadata.cwd.as_str());
                    if let Some(terminal) = desk.terminals.get(&surface.id) {
                        let result = if let Some(profile_id) = agent_profile_id {
                            agent_bridge.start_agent_for_surface_with_profile(
                                desk.runtime_id,
                                &desk.workdesk_id,
                                surface.id,
                                cwd,
                                terminal,
                                profile_id,
                                vec![],
                            )
                        } else {
                            agent_bridge.start_agent_for_surface(
                                desk.runtime_id,
                                &desk.workdesk_id,
                                surface.id,
                                cwd,
                                terminal,
                            )
                        };
                        if let Err(error) = result {
                            let msg = format!("Agent runtime did not start: {error}");
                            desk.runtime_notice = Some(SharedString::from(msg));
                        }
                    }
                }
            }
            PaneKind::Editor => {
                if let Some(editor) = editor {
                    desk.editors.insert(surface.id, editor);
                }
            }
            PaneKind::Browser => {}
        }
    }

    fn spawn_surface_on_workdesk_state(
        workdesks: &mut [WorkdeskState],
        active_workdesk: &mut usize,
        desk_index: usize,
        target_pane_id: Option<PaneId>,
        kind: PaneKind,
        title: Option<String>,
        url: Option<String>,
        file_path: Option<String>,
        focus: bool,
        agent_bridge: &agent_sessions::AgentRuntimeBridge,
    ) -> Result<(PaneId, SurfaceId), String> {
        Self::spawn_surface_on_workdesk_state_with_agent_profile(
            workdesks,
            active_workdesk,
            desk_index,
            target_pane_id,
            kind,
            title,
            url,
            file_path,
            focus,
            None,
            agent_bridge,
        )
    }

    fn spawn_surface_on_workdesk_state_with_agent_profile(
        workdesks: &mut [WorkdeskState],
        active_workdesk: &mut usize,
        desk_index: usize,
        target_pane_id: Option<PaneId>,
        kind: PaneKind,
        title: Option<String>,
        url: Option<String>,
        file_path: Option<String>,
        focus: bool,
        agent_profile_id: Option<&str>,
        agent_bridge: &agent_sessions::AgentRuntimeBridge,
    ) -> Result<(PaneId, SurfaceId), String> {
        let editor_lookup_path = if kind == PaneKind::Editor {
            file_path.as_deref().map(canonical_path_string)
        } else {
            None
        };
        if let Some(canonical_path) = editor_lookup_path.as_deref() {
            if target_pane_id.is_none() {
                if let Some((pane_id, surface_id)) =
                    Self::find_editor_surface_by_path(workdesks, desk_index, canonical_path)
                {
                    if focus {
                        workdesks[desk_index].focus_surface(pane_id, surface_id);
                        *active_workdesk = desk_index;
                    }
                    return Ok((pane_id, surface_id));
                }
            }
        }

        let Some(desk) = workdesks.get_mut(desk_index) else {
            return Err(format!("workdesk {desk_index} was not found"));
        };
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
            Self::initialize_surface_runtime(
                desk,
                pane_size,
                pane_surface_count,
                &surface,
                editor,
                agent_bridge,
                agent_profile_id,
            );
            desk.resize_terminals_for_pane(pane_id);
            if focus {
                desk.focus_surface(pane_id, surface_id);
                *active_workdesk = desk_index;
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
        Self::initialize_surface_runtime(
            desk,
            size,
            1,
            &surface,
            editor,
            agent_bridge,
            agent_profile_id,
        );
        if focus {
            desk.focus_surface(pane_id, surface_id);
            *active_workdesk = desk_index;
        }
        desk.drag_state = DragState::Idle;
        Ok((pane_id, surface_id))
    }

    fn spawn_agent_surface_on_workdesk_state_with_profile(
        workdesks: &mut [WorkdeskState],
        active_workdesk: &mut usize,
        desk_index: usize,
        target_pane_id: Option<PaneId>,
        profile_id: &str,
        world_center: WorkdeskPoint,
        focus: bool,
        agent_bridge: &agent_sessions::AgentRuntimeBridge,
    ) -> Result<(PaneId, SurfaceId), String> {
        let previous_count = workdesks
            .get(desk_index)
            .ok_or_else(|| format!("workdesk {desk_index} was not found"))?
            .panes
            .len();
        let (pane_id, surface_id) = Self::spawn_surface_on_workdesk_state_with_agent_profile(
            workdesks,
            active_workdesk,
            desk_index,
            target_pane_id,
            PaneKind::Agent,
            None,
            None,
            None,
            focus,
            Some(profile_id),
            agent_bridge,
        )?;

        if target_pane_id.is_none() {
            if let Some(pane) = workdesks[desk_index]
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
        }

        Ok((pane_id, surface_id))
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
        let bridge = &self.agent_runtime;
        Self::spawn_surface_on_workdesk_state(
            &mut self.workdesks,
            &mut self.active_workdesk,
            desk_index,
            target_pane_id,
            kind,
            title,
            url,
            file_path,
            focus,
            bridge,
        )
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

        let workdesk_runtime_id = self.active_workdesk().runtime_id;
        self.agent_runtime
            .stop_surface(workdesk_runtime_id, surface_id);
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
        {
            let desk = self.active_workdesk_mut();
            desk.focus_surface(pane_id, next_surface_id);
            desk.note_pane_activity(pane_id);
        }
        let desk_index = self.active_workdesk;
        self.ensure_agent_runtime_for_pane(desk_index, pane_id);
        self.request_persist(cx);
        cx.notify();
        true
    }

    fn workdesk_name_for_template(&self, template: WorkdeskTemplate) -> String {
        self.unique_workdesk_name(template.base_name())
    }

    fn open_workdesk_creator(&mut self, cx: &mut Context<Self>) {
        self.dismiss_workdesk_menu();
        self.dismiss_stack_surface_menu();
        self.session_inspector = None;
        self.workspace_palette = None;
        self.review_panel = None;
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
        self.dismiss_stack_surface_menu();
        self.dismiss_notifications();
        self.session_inspector = None;
        self.workspace_palette = None;
        self.review_panel = None;
        self.active_workdesk = index;
        self.active_workdesk_mut().drag_state = DragState::Idle;
        if self.active_workdesk().active_pane.is_none() {
            self.active_workdesk_mut().active_pane =
                self.active_workdesk().panes.last().map(|pane| pane.id);
        }
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
                desk.workdesk_id = self.allocate_workdesk_id();
                desk.runtime_id = self.allocate_workdesk_runtime_id();
                boot_workdesk_terminals(&mut desk);
                self.workdesks.push(desk);
                self.active_workdesk = self.workdesks.len() - 1;
                let idx = self.active_workdesk;
                if let Err(error) = self.sync_review_summary_for_desk(idx) {
                    if let Some(desk) = self.workdesks.get_mut(idx) {
                        desk.runtime_notice =
                            Some(SharedString::from(format!("review summary stale: {error}")));
                    }
                }
                let bridge = &self.agent_runtime;
                let desk_ref = &mut self.workdesks[idx];
                Self::boot_agent_sessions_for_desk(bridge, desk_ref.runtime_id, desk_ref);
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
                desk.worktree_binding = worktrees::refreshed_binding_from_desk_paths(
                    &desk.metadata.cwd,
                    &desk.metadata.branch,
                );
                if let Err(error) = self.sync_review_summary_for_desk(index) {
                    if let Some(desk) = self.workdesks.get_mut(index) {
                        desk.runtime_notice =
                            Some(SharedString::from(format!("review summary stale: {error}")));
                    }
                }
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

    fn resolve_workdesk_index_by_automation_id(&self, workdesk_id: &str) -> Result<usize, String> {
        self.workdesks
            .iter()
            .position(|desk| {
                desk.workdesk_id == workdesk_id
                    || desk.runtime_id.to_string() == workdesk_id
                    || desk.name == workdesk_id
            })
            .ok_or_else(|| format!("workdesk `{workdesk_id}` was not found"))
    }

    fn resolve_workdesk_index_by_worktree_id(
        &self,
        worktree_id: &WorktreeId,
    ) -> Result<usize, String> {
        self.workdesks
            .iter()
            .position(|desk| worktree_id_from_desk(desk).as_ref() == Some(worktree_id))
            .ok_or_else(|| format!("worktree `{}` was not found", worktree_id.0))
    }

    fn worktree_state_json(&self, desk_index: usize) -> Value {
        let desk = &self.workdesks[desk_index];
        json!({
            "worktree_id": worktree_id_from_desk(desk),
            "binding": desk.worktree_binding,
            "workdesk": automation_workdesk_summary_json(
                desk_index,
                desk,
                desk_index == self.active_workdesk,
            ),
        })
    }

    fn refresh_worktree_binding_for_desk(
        &mut self,
        desk_index: usize,
        sync_review: bool,
    ) -> Result<WorktreeBinding, String> {
        let daemon = DaemonClient::default();
        let binding = {
            let desk = &self.workdesks[desk_index];
            let daemon_result = if let Some(binding) = desk.worktree_binding.clone() {
                daemon
                    .worktree_status(&WorktreeId::new(binding.root_path.clone()))
                    .map(|result| result.binding)
            } else {
                daemon
                    .worktree_create_or_attach(desk.metadata.cwd.clone(), None, None)
                    .map(|result| result.binding)
            };
            match daemon_result {
                Ok(binding) => binding,
                Err(_) => {
                    if let Some(binding) = desk.worktree_binding.clone() {
                        WorktreeService::refresh(&binding).map_err(|error| error.to_string())?
                    } else {
                        WorktreeService::attach(&desk.metadata.cwd, None)
                            .map_err(|error| error.to_string())?
                    }
                }
            }
        };

        {
            let desk = &mut self.workdesks[desk_index];
            desk.metadata.cwd = binding.root_path.clone();
            desk.metadata.branch = binding.branch.clone();
            desk.worktree_binding = Some(binding.clone());
        }
        if sync_review {
            if let Err(error) = self.sync_review_summary_for_desk(desk_index) {
                if let Some(desk) = self.workdesks.get_mut(desk_index) {
                    desk.runtime_notice =
                        Some(SharedString::from(format!("review summary stale: {error}")));
                }
            }
        }
        Ok(binding)
    }

    fn sync_review_summary_for_desk(&mut self, desk_index: usize) -> Result<(), String> {
        let previous = self
            .workdesks
            .get(desk_index)
            .and_then(|desk| desk.review_summary.clone());
        let binding = self
            .workdesks
            .get(desk_index)
            .and_then(|desk| desk.worktree_binding.clone());
        let Some(binding) = binding else {
            if let Some(desk) = self.workdesks.get_mut(desk_index) {
                desk.review_summary =
                    refreshed_desk_review_summary_view(previous.as_ref(), None, None);
                desk.review_payload_cache = None;
                desk.review_local_state = ReviewPanelLocalState::default();
            }
            return Ok(());
        };

        let worktree_id = WorktreeId::new(binding.root_path.clone());
        let refreshed_binding = WorktreeService::attach(&binding.root_path, binding.base_branch.clone())
            .map_err(|error| error.to_string())?;

        if let Ok(payload) = DaemonClient::default().desk_review_summary(&worktree_id) {
            if let Some(desk) = self.workdesks.get_mut(desk_index) {
                desk.worktree_binding = Some(refreshed_binding.clone());
                let summary =
                    build_desk_review_summary_view_from_payload(&refreshed_binding, &payload);
                Self::apply_review_payload_to_desk(
                    desk,
                    &refreshed_binding,
                    payload,
                    summary,
                    false,
                );
            }
            return Ok(());
        }

        let cached_payload = self
            .workdesks
            .get(desk_index)
            .and_then(|desk| {
                reusable_review_payload_cache(desk.review_payload_cache.as_ref(), &refreshed_binding)
            })
            .cloned();
        let resolved = resolve_local_desk_review_payload(
            &worktree_id,
            &refreshed_binding,
            cached_payload.as_ref(),
        )?;

        if let Some(desk) = self.workdesks.get_mut(desk_index) {
            desk.worktree_binding = Some(refreshed_binding.clone());
            Self::apply_review_payload_to_desk(
                desk,
                &refreshed_binding,
                resolved.payload,
                resolved.summary,
                resolved.stale,
            );
        }

        Ok(())
    }

    fn apply_review_payload_to_desk(
        desk: &mut WorkdeskState,
        refreshed_binding: &WorktreeBinding,
        payload: DeskReviewPayload,
        summary: DeskReviewSummaryView,
        stale_rich_payload: bool,
    ) {
        let workdesk_id = WorkdeskId::new(desk.workdesk_id.clone());
        let rebound = review_payload_worktree_rebound(desk.review_payload_cache.as_ref(), refreshed_binding);
        let prev_cache = desk.review_payload_cache.as_ref();
        let setup = review_workspace_setup_notice(refreshed_binding);
        let (payload, local) = merge_review_local_after_fetch(
            &workdesk_id,
            prev_cache,
            &desk.review_local_state,
            payload,
            ReviewPanelRefreshContext {
                workdesk_id: workdesk_id.clone(),
                worktree_rebound: rebound,
                stale_rich_payload,
                setup_notice: setup,
            },
        );
        desk.review_payload_cache = Some(payload.clone());
        desk.review_local_state = local;
        desk.review_summary = Some(summary);
    }

    fn ensure_worktree_backed_desk(
        &mut self,
        binding: WorktreeBinding,
        cx: &mut Context<Self>,
    ) -> usize {
        if let Some(index) = self.workdesks.iter().position(|desk| {
            desk.worktree_binding
                .as_ref()
                .map(|existing| existing.root_path == binding.root_path)
                .unwrap_or_else(|| desk.metadata.cwd == binding.root_path)
        }) {
            {
                let desk = &mut self.workdesks[index];
                desk.metadata.cwd = binding.root_path.clone();
                desk.metadata.branch = binding.branch.clone();
                desk.worktree_binding = Some(binding);
            }
            if let Err(error) = self.sync_review_summary_for_desk(index) {
                if let Some(desk) = self.workdesks.get_mut(index) {
                    desk.runtime_notice =
                        Some(SharedString::from(format!("review summary stale: {error}")));
                }
            }
            if index != self.active_workdesk {
                self.select_workdesk(index, cx);
            } else {
                self.request_persist(cx);
            }
            return index;
        }

        let name = self.unique_workdesk_name(&format!(
            "{} {}",
            WorkdeskTemplate::Implementation.base_name(),
            binding.branch
        ));
        let mut draft = WorkdeskDraft::from_template(name, WorkdeskTemplate::Implementation);
        draft.metadata.cwd = binding.root_path.clone();
        draft.metadata.branch = binding.branch.clone();
        let mut desk = workdesk_from_template(WorkdeskTemplate::Implementation, draft);
        desk.workdesk_id = self.allocate_workdesk_id();
        desk.runtime_id = self.allocate_workdesk_runtime_id();
        desk.worktree_binding = Some(binding);
        boot_workdesk_terminals(&mut desk);
        self.workdesks.push(desk);
        let index = self.workdesks.len() - 1;
        if let Err(error) = self.sync_review_summary_for_desk(index) {
            if let Some(desk) = self.workdesks.get_mut(index) {
                desk.runtime_notice =
                    Some(SharedString::from(format!("review summary stale: {error}")));
            }
        }
        let bridge = &self.agent_runtime;
        let desk_ref = &mut self.workdesks[index];
        Self::boot_agent_sessions_for_desk(bridge, desk_ref.runtime_id, desk_ref);
        self.select_workdesk(index, cx);
        index
    }

    fn first_agent_surface_id(&self, desk_index: usize) -> Option<SurfaceId> {
        self.workdesks
            .get(desk_index)?
            .panes
            .iter()
            .flat_map(|pane| pane.surfaces.iter())
            .find(|surface| surface.kind == PaneKind::Agent)
            .map(|surface| surface.id)
    }

    fn handle_automation_request(
        &mut self,
        request: SharedAutomationRequest,
        cx: &mut Context<Self>,
    ) -> SharedAutomationResponse {
        let response = (|| -> Result<Value, String> {
            match request {
                SharedAutomationRequest::WorktreeCreateOrAttach {
                    repo_root,
                    branch,
                    attach_path,
                } => {
                    let daemon = DaemonClient::default();
                    let binding = match daemon.worktree_create_or_attach(
                        repo_root.clone(),
                        branch.clone(),
                        attach_path.clone(),
                    ) {
                        Ok(result) => result.binding,
                        Err(_) => match (attach_path, branch) {
                            (Some(path), base_branch) => {
                                WorktreeService::attach(&path, base_branch)
                                    .map_err(|error| error.to_string())?
                            }
                            (None, Some(branch)) => {
                                let repo_binding = WorktreeService::attach(&repo_root, None)
                                    .map_err(|error| error.to_string())?;
                                let worktree_path = default_worktree_path(&repo_root, &branch)?;
                                if worktree_path.exists() {
                                    WorktreeService::attach(
                                        &worktree_path,
                                        Some(repo_binding.branch),
                                    )
                                    .map_err(|error| error.to_string())?
                                } else {
                                    WorktreeService::create_worktree(
                                        &repo_root,
                                        &worktree_path,
                                        &branch,
                                        &repo_binding.branch,
                                    )
                                    .map_err(|error| error.to_string())?
                                }
                            }
                            (None, None) => WorktreeService::attach(&repo_root, None)
                                .map_err(|error| error.to_string())?,
                        },
                    };
                    let desk_index = self.ensure_worktree_backed_desk(binding, cx);
                    Ok(self.worktree_state_json(desk_index))
                }
                SharedAutomationRequest::WorktreeStatus { worktree_id } => {
                    let daemon = DaemonClient::default();
                    let binding = daemon
                        .worktree_status(&worktree_id)
                        .map(|result| result.binding)
                        .or_else(|_| {
                            WorktreeService::attach(&worktree_id.0, None)
                                .map_err(|error| error.to_string())
                        })?;
                    let desk_index = self.ensure_worktree_backed_desk(binding, cx);
                    self.refresh_worktree_binding_for_desk(desk_index, true)?;
                    self.request_persist(cx);
                    Ok(self.worktree_state_json(desk_index))
                }
                SharedAutomationRequest::WorkdeskList { .. }
                | SharedAutomationRequest::WorkdeskEnsure { .. }
                | SharedAutomationRequest::TerminalEnsure { .. }
                | SharedAutomationRequest::TerminalRead { .. }
                | SharedAutomationRequest::TerminalWrite { .. }
                | SharedAutomationRequest::TerminalResize { .. }
                | SharedAutomationRequest::TerminalClose { .. }
                | SharedAutomationRequest::GuiHeartbeat { .. }
                | SharedAutomationRequest::GuiEnsureRunning { .. }
                | SharedAutomationRequest::DaemonHealth => Err(
                    "daemon-only automation requests are not supported by axis-app yet".to_string(),
                ),
                SharedAutomationRequest::AgentStart {
                    worktree_id,
                    provider_profile_id,
                    argv,
                    workdesk_id,
                    surface_id,
                } => {
                    let requested_workdesk_id = workdesk_id.clone();
                    let requested_surface_id = surface_id;
                    let desk_index = requested_workdesk_id
                        .as_ref()
                        .map(|workdesk_id| {
                            self.resolve_workdesk_index_by_automation_id(&workdesk_id.0)
                        })
                        .transpose()?
                        .or_else(|| {
                            self.resolve_workdesk_index_by_worktree_id(&worktree_id)
                                .ok()
                        });
                    if let Some(desk_index) = desk_index {
                        let desk_runtime_id = self.workdesks[desk_index].runtime_id;
                        let surface_id = requested_surface_id
                            .or_else(|| self.first_agent_surface_id(desk_index))
                            .ok_or_else(|| {
                                format!("worktree `{}` has no agent surface", worktree_id.0)
                            })?;
                        if let Some(existing) = self
                            .agent_runtime
                            .session_for_surface(desk_runtime_id, surface_id)
                        {
                            if existing.provider_profile_id != provider_profile_id {
                                return Err(format!(
                                    "agent surface already runs `{}`",
                                    existing.provider_profile_id
                                ));
                            }
                            return Ok(agent_session_json(&existing));
                        }

                        {
                            let bridge = &self.agent_runtime;
                            let desk = &mut self.workdesks[desk_index];
                            start_agent_runtime_for_surface_with_profile(
                                bridge,
                                desk_runtime_id,
                                desk,
                                surface_id,
                                Some(&provider_profile_id),
                                argv,
                            )?;
                        }
                        let record = self
                            .agent_runtime
                            .session_for_surface(desk_runtime_id, surface_id)
                            .ok_or_else(|| "agent session did not register".to_string())?;
                        Ok(agent_session_json(&record))
                    } else {
                        let record = DaemonClient::default().start_agent(
                            &worktree_id,
                            provider_profile_id,
                            argv,
                            requested_workdesk_id,
                            requested_surface_id,
                        )?;
                        Ok(agent_session_json(&record))
                    }
                }
                SharedAutomationRequest::AgentStop { agent_session_id } => {
                    self.agent_runtime.stop_session(&agent_session_id)?;
                    for desk in &self.workdesks {
                        for terminal in desk.terminals.values() {
                            if terminal
                                .agent_metadata()
                                .as_ref()
                                .is_some_and(|meta| meta.session_id == agent_session_id)
                            {
                                terminal.set_agent_metadata(None);
                            }
                        }
                    }
                    Ok(json!({
                        "agent_session_id": agent_session_id,
                        "stopped": true,
                    }))
                }
                SharedAutomationRequest::AgentList { worktree_id } => {
                    let sessions =
                        match DaemonClient::default().list_agents(worktree_id.as_ref()) {
                            Ok(records) => records,
                            Err(_) => {
                                let filter_workdesk_ids = worktree_id
                                    .as_ref()
                                    .map(|id| self.resolve_workdesk_index_by_worktree_id(id))
                                    .transpose()?
                                    .map(|index| {
                                        vec![
                                            self.workdesks[index].workdesk_id.clone(),
                                            self.workdesks[index].runtime_id.to_string(),
                                        ]
                                    });
                                self.agent_runtime
                                    .sessions_snapshot()
                                    .into_iter()
                                    .filter(|record| {
                                        filter_workdesk_ids.as_ref().map_or(true, |workdesk_ids| {
                                            record.workdesk_id.as_ref().is_some_and(|workdesk_id| {
                                                workdesk_ids
                                                    .iter()
                                                    .any(|candidate| candidate == workdesk_id)
                                            })
                                        })
                                    })
                                    .collect::<Vec<_>>()
                            }
                        }
                        .into_iter()
                        .map(|record| agent_session_json(&record))
                        .collect::<Vec<_>>();
                    Ok(Value::Array(sessions))
                }
                SharedAutomationRequest::AgentGet(request) => {
                    let detail = self
                        .agent_runtime
                        .session_detail(&request.agent_session_id, request.after_sequence)?;
                    Ok(serde_json::to_value(detail).map_err(|error| error.to_string())?)
                }
                SharedAutomationRequest::AgentSendTurn(request) => {
                    let detail = self
                        .agent_runtime
                        .send_turn(&request.agent_session_id, &request.text)?;
                    Ok(serde_json::to_value(detail).map_err(|error| error.to_string())?)
                }
                SharedAutomationRequest::AgentRespondApproval(request) => {
                    let detail = self.agent_runtime.respond_approval(
                        &request.agent_session_id,
                        &request.approval_request_id,
                        request.approved,
                        request.note,
                    )?;
                    Ok(serde_json::to_value(detail).map_err(|error| error.to_string())?)
                }
                SharedAutomationRequest::AgentResume(request) => {
                    let detail = self.agent_runtime.resume(&request.agent_session_id)?;
                    Ok(serde_json::to_value(detail).map_err(|error| error.to_string())?)
                }
                SharedAutomationRequest::DeskReviewSummary { worktree_id } => {
                    let base_for_attach = self
                        .resolve_workdesk_index_by_worktree_id(&worktree_id)
                        .ok()
                        .and_then(|index| self.workdesks.get(index))
                        .and_then(|desk| desk.worktree_binding.as_ref())
                        .and_then(|b| b.base_branch.clone());
                    let binding = WorktreeService::attach(&worktree_id.0, base_for_attach)
                        .map_err(|error| error.to_string())?;
                    let cached_payload = self
                        .resolve_workdesk_index_by_worktree_id(&worktree_id)
                        .ok()
                        .and_then(|index| self.workdesks.get(index))
                        .and_then(|desk| {
                            reusable_review_payload_cache(desk.review_payload_cache.as_ref(), &binding)
                        })
                        .cloned();

                    let (payload, summary, stale) = match DaemonClient::default()
                        .desk_review_summary(&worktree_id)
                    {
                        Ok(payload) => (
                            payload.clone(),
                            build_desk_review_summary_view_from_payload(&binding, &payload),
                            false,
                        ),
                        Err(error)
                            if !Self::daemon_review_summary_error_allows_fallback(&error) =>
                        {
                            return Err(error);
                        }
                        Err(_) => {
                            let resolved = resolve_local_desk_review_payload(
                                &worktree_id,
                                &binding,
                                cached_payload.as_ref(),
                            )?;
                            (resolved.payload, resolved.summary, resolved.stale)
                        }
                    };

                    if let Ok(desk_index) = self.resolve_workdesk_index_by_worktree_id(&worktree_id) {
                        let binding = self.refresh_worktree_binding_for_desk(desk_index, false)?;
                        if let Some(desk) = self.workdesks.get_mut(desk_index) {
                            let summary =
                                build_desk_review_summary_view(&binding, &summary.changed_files);
                            Self::apply_review_payload_to_desk(
                                desk,
                                &binding,
                                payload.clone(),
                                summary,
                                stale,
                            );
                        }
                        self.request_persist(cx);
                    }

                    let mut value = serde_json::to_value(&payload)
                        .map_err(|error| format!("serialize desk review: {error}"))?;
                    if stale {
                        if let Some(obj) = value.as_object_mut() {
                            obj.insert("review_payload_stale".to_string(), json!(true));
                        }
                    }
                    Ok(value)
                }
                SharedAutomationRequest::AttentionNext { workdesk_id } => {
                    let target = if let Some(workdesk_id) = workdesk_id {
                        let desk_index =
                            self.resolve_workdesk_index_by_automation_id(&workdesk_id)?;
                        next_attention_target_for_workdesk(&self.workdesks[desk_index])
                            .map(|pane_id| (desk_index, pane_id))
                    } else {
                        self.next_attention_target()
                    };
                    let Some((desk_index, pane_id)) = target else {
                        return Ok(Value::Null);
                    };
                    let _ = self.navigate_to_workdesk_pane(desk_index, pane_id, cx);
                    Ok(json!({
                        "workdesk": automation_workdesk_summary_json(
                            desk_index,
                            &self.workdesks[desk_index],
                            desk_index == self.active_workdesk,
                        ),
                        "pane": automation_pane_json(
                            &self.workdesks[desk_index],
                            pane_id,
                            desk_index == self.active_workdesk,
                        ),
                    }))
                }
                SharedAutomationRequest::StateCurrent { workdesk_id } => {
                    if let Some(workdesk_id) = workdesk_id {
                        let desk_index =
                            self.resolve_workdesk_index_by_automation_id(&workdesk_id)?;
                        let filter_workdesk_ids = [
                            self.workdesks[desk_index].workdesk_id.clone(),
                            self.workdesks[desk_index].runtime_id.to_string(),
                        ];
                        let sessions = self
                            .agent_runtime
                            .sessions_snapshot()
                            .into_iter()
                            .filter(|record| {
                                record
                                    .workdesk_id
                                    .as_ref()
                                    .is_some_and(|record_workdesk_id| {
                                        filter_workdesk_ids
                                            .iter()
                                            .any(|candidate| candidate == record_workdesk_id)
                                    })
                            })
                            .map(|record| agent_session_json(&record))
                            .collect::<Vec<_>>();
                        Ok(json!({
                            "active_workdesk": self.active_workdesk,
                            "socket_path": self.automation_socket_path.to_string(),
                            "workdesk": automation_workdesk_state_json(
                                desk_index,
                                &self.workdesks[desk_index],
                                desk_index == self.active_workdesk,
                            ),
                            "worktree_id": worktree_id_from_desk(&self.workdesks[desk_index]),
                            "agent_sessions": sessions,
                        }))
                    } else {
                        Ok(self.automation_state_json())
                    }
                }
            }
        })();

        match response {
            Ok(result) => SharedAutomationResponse::success_with_result(result),
            Err(error) => SharedAutomationResponse::failure(error),
        }
    }

    fn daemon_review_summary_error_allows_fallback(error: &str) -> bool {
        [
            "connect ",
            "set daemon read timeout:",
            "set daemon write timeout:",
            "serialize daemon request:",
            "write daemon request:",
            "write daemon request newline:",
            "flush daemon request:",
            "read daemon response:",
            "parse daemon response:",
            "decode daemon response:",
            "daemon automation request returned no result",
        ]
        .iter()
        .any(|prefix| error.starts_with(prefix))
    }

    fn automation_state_json(&self) -> Value {
        json!({
            "active_workdesk": self.active_workdesk,
            "socket_path": self.automation_socket_path.to_string(),
            "agent_sessions": self.agent_runtime.sessions_snapshot().iter().map(agent_session_json).collect::<Vec<_>>(),
            "workdesks": self.workdesks.iter().enumerate().map(|(index, desk)| {
                let mut state = automation_workdesk_state_json(index, desk, index == self.active_workdesk);
                if let Some(object) = state.as_object_mut() {
                    object.insert(
                        "worktree_id".to_string(),
                        serde_json::to_value(worktree_id_from_desk(desk)).unwrap_or(Value::Null),
                    );
                }
                state
            }).collect::<Vec<_>>(),
        })
    }

    fn baseline_attention_state(&self, desk_index: usize, pane_id: PaneId) -> AttentionState {
        let Some(desk) = self.workdesks.get(desk_index) else {
            return AttentionState::Idle;
        };
        agent_runtime_baseline_attention_state(&self.agent_runtime, desk, pane_id)
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
        let (changed, previous_state, desk_name, pane_title, effective_unread) = {
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
            let previous_state = desk.pane_attention(pane_id).state;
            let changed = desk.set_pane_attention_state(pane_id, state, effective_unread);
            (
                changed,
                previous_state,
                desk.name.clone(),
                pane_title,
                effective_unread,
            )
        };

        if !changed {
            return false;
        }

        if announce && should_notify_attention_transition(previous_state, state) {
            self.push_attention_notification(
                desk_index,
                pane_id,
                &pane_title,
                &desk_name,
                state,
                effective_unread,
            );
            self.set_runtime_notice_for_workdesk(
                desk_index,
                format!("{pane_title} on {desk_name} is {}", state.label()),
            );
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
            AttentionState::Idle | AttentionState::Working => AttentionState::NeedsInput,
            AttentionState::NeedsInput => AttentionState::NeedsReview,
            AttentionState::NeedsReview => AttentionState::Error,
            AttentionState::Error => self.baseline_attention_state(desk_index, pane_id),
        };
        self.set_pane_attention(desk_index, pane_id, next, next.is_attention(), true, cx)
    }

    fn next_attention_target(&self) -> Option<(usize, PaneId)> {
        next_attention_workdesk_target(self.workdesks.iter().enumerate().map(
            |(desk_index, desk)| {
                (
                    desk_index,
                    desk.panes
                        .iter()
                        .map(|pane| (pane.id, desk.pane_attention(pane.id))),
                )
            },
        ))
    }

    fn navigate_to_workdesk_pane(
        &mut self,
        desk_index: usize,
        pane_id: PaneId,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(desk) = self.workdesks.get(desk_index) else {
            return false;
        };
        if desk.pane(pane_id).is_none() {
            return false;
        }

        if desk_index != self.active_workdesk {
            self.select_workdesk(desk_index, cx);
        }
        self.focus_pane(pane_id, cx);
        true
    }

    fn navigate_next_attention(&mut self, cx: &mut Context<Self>) -> bool {
        let Some((desk_index, pane_id)) = self.next_attention_target() else {
            return false;
        };

        self.navigate_to_workdesk_pane(desk_index, pane_id, cx)
    }

    fn agent_session_inspector_view_for_target(
        &self,
        target: AgentSessionInspectorTarget,
    ) -> Option<AgentSessionInspectorView> {
        let workdesk = self.workdesks.get(target.desk_index)?;
        agent_session_inspector_view(&self.agent_runtime, workdesk, target.surface_id)
    }

    fn toggle_session_inspector(
        &mut self,
        desk_index: usize,
        surface_id: SurfaceId,
        cx: &mut Context<Self>,
    ) {
        let target = AgentSessionInspectorTarget {
            desk_index,
            surface_id,
        };
        if self.session_inspector == Some(target) {
            self.session_inspector = None;
            self.agent_session_composer = None;
        } else {
            self.dismiss_workdesk_menu();
            self.dismiss_stack_surface_menu();
            self.dismiss_notifications();
            self.review_panel = None;
            self.session_inspector = Some(target);
            self.agent_session_composer = Some(AgentSessionComposerState::new(target));
        }
        cx.notify();
    }

    fn close_session_inspector(&mut self, cx: &mut Context<Self>) {
        if self.session_inspector.take().is_some() {
            self.agent_session_composer = None;
            cx.notify();
        }
    }

    fn session_record_for_inspector_target(
        &self,
        target: AgentSessionInspectorTarget,
    ) -> Option<AgentSessionRecord> {
        let workdesk = self.workdesks.get(target.desk_index)?;
        self.agent_runtime
            .session_for_surface(workdesk.runtime_id, target.surface_id)
    }

    fn focus_agent_session_composer(
        &mut self,
        target: AgentSessionInspectorTarget,
        cx: &mut Context<Self>,
    ) {
        match self.agent_session_composer.as_mut() {
            Some(composer) if composer.target == target => composer.active = true,
            Some(composer) => {
                *composer = AgentSessionComposerState::new(target);
                composer.active = true;
            }
            None => {
                let mut composer = AgentSessionComposerState::new(target);
                composer.active = true;
                self.agent_session_composer = Some(composer);
            }
        }
        cx.notify();
    }

    fn submit_agent_session_composer(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(target) = self.session_inspector else {
            return false;
        };
        let Some(text) = self
            .agent_session_composer
            .as_ref()
            .filter(|composer| composer.target == target)
            .map(|composer| composer.draft.trim().to_string())
            .filter(|draft| !draft.is_empty())
        else {
            return false;
        };
        let Some(record) = self.session_record_for_inspector_target(target) else {
            self.set_runtime_notice_for_workdesk(
                target.desk_index,
                "Agent session is no longer available",
            );
            cx.notify();
            return true;
        };
        match self.agent_runtime.send_turn(&record.id, &text) {
            Ok(_) => {
                if let Some(composer) = self
                    .agent_session_composer
                    .as_mut()
                    .filter(|composer| composer.target == target)
                {
                    composer.draft.clear();
                    composer.active = true;
                }
                self.sync_agent_runtime_activity(cx);
                cx.notify();
            }
            Err(error) => {
                self.set_runtime_notice_for_workdesk(
                    target.desk_index,
                    format!("agent send failed: {error}"),
                );
                cx.notify();
            }
        }
        true
    }

    fn respond_to_session_inspector_approval(
        &mut self,
        target: AgentSessionInspectorTarget,
        approval_id: axis_core::agent_history::AgentApprovalRequestId,
        approved: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(record) = self.session_record_for_inspector_target(target) else {
            self.set_runtime_notice_for_workdesk(
                target.desk_index,
                "Agent session is no longer available",
            );
            cx.notify();
            return;
        };
        match self.agent_runtime.respond_approval(
            &record.id,
            &approval_id,
            approved,
            None,
        ) {
            Ok(_) => {
                self.sync_agent_runtime_activity(cx);
                cx.notify();
            }
            Err(error) => {
                self.set_runtime_notice_for_workdesk(
                    target.desk_index,
                    format!("agent approval failed: {error}"),
                );
                cx.notify();
            }
        }
    }

    fn resume_session_from_inspector(
        &mut self,
        target: AgentSessionInspectorTarget,
        cx: &mut Context<Self>,
    ) {
        let Some(record) = self.session_record_for_inspector_target(target) else {
            self.set_runtime_notice_for_workdesk(
                target.desk_index,
                "Agent session is no longer available",
            );
            cx.notify();
            return;
        };
        match self.agent_runtime.resume(&record.id) {
            Ok(_) => {
                self.sync_agent_runtime_activity(cx);
                cx.notify();
            }
            Err(error) => {
                self.set_runtime_notice_for_workdesk(
                    target.desk_index,
                    format!("agent resume failed: {error}"),
                );
                cx.notify();
            }
        }
    }

    fn handle_agent_session_composer_key_down(
        &mut self,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(target) = self.session_inspector else {
            return false;
        };
        let active = self
            .agent_session_composer
            .as_ref()
            .is_some_and(|composer| composer.target == target && composer.active);
        if !active {
            return false;
        }
        let keystroke = &event.keystroke;
        if keystroke.key == "escape" && !keystroke.modifiers.modified() {
            if let Some(composer) = self
                .agent_session_composer
                .as_mut()
                .filter(|composer| composer.target == target)
            {
                composer.active = false;
            }
            cx.notify();
            return true;
        }
        if keystroke.key == "enter" && !keystroke.modifiers.modified() {
            return self.submit_agent_session_composer(cx);
        }
        if matches!(keystroke.key.as_str(), "backspace" | "delete")
            && !keystroke.modifiers.modified()
        {
            if let Some(composer) = self
                .agent_session_composer
                .as_mut()
                .filter(|composer| composer.target == target)
            {
                composer.draft.pop();
            }
            cx.notify();
            return true;
        }
        if let Some(text) = editable_keystroke_text(keystroke) {
            if let Some(composer) = self
                .agent_session_composer
                .as_mut()
                .filter(|composer| composer.target == target)
            {
                composer.draft.push_str(&text);
            }
            cx.notify();
            return true;
        }
        true
    }

    fn handle_terminal_attention_transition(
        &mut self,
        desk_index: usize,
        pane_id: PaneId,
        status: Option<&str>,
        closed: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(desk) = self.workdesks.get(desk_index) else {
            return false;
        };
        let Some(pane) = desk.panes.iter().find(|pane| pane.id == pane_id) else {
            return false;
        };

        let next_state = infer_attention_state_from_terminal_status(&pane.kind, status, closed);
        match next_state {
            AttentionState::Idle | AttentionState::Working => {
                self.set_pane_attention(desk_index, pane_id, next_state, false, false, cx)
            }
            AttentionState::NeedsInput | AttentionState::NeedsReview | AttentionState::Error => {
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

        self.dismiss_stack_surface_menu();
        self.workspace_palette = None;
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
        {
            let desk = self.active_workdesk_mut();
            desk.focus_pane(pane_id);
            if close_expose {
                desk.grid_layout.expose_open = false;
            }
        }
        let desk_index = self.active_workdesk;
        self.ensure_agent_runtime_for_pane(desk_index, pane_id);
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
                self.open_agent_provider_popup_for_new_pane(window, cx);
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
            ShortcutAction::QuickOpen => {
                self.open_workspace_palette(WorkspacePaletteMode::OpenFile, cx);
                true
            }
            ShortcutAction::SearchWorkspace => {
                self.open_workspace_palette(WorkspacePaletteMode::SearchWorkspace, cx);
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
            let refresh_interval = this
                .update(cx, |this, _| this.refresh_loop_interval())
                .unwrap_or(Duration::from_millis(150));
            Timer::after(refresh_interval).await;

            if this
                .update(cx, |this, cx| {
                    let automation_changed = this.process_automation_commands(cx);
                    let blink_changed = this.tick_cursor_blink();
                    this.sync_daemon_runtime_state_if_due();
                    let agent_changed = this.sync_agent_runtime_activity(cx);
                    if automation_changed
                        || this.sync_terminal_revisions(cx)
                        || agent_changed
                        || blink_changed
                    {
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

            for (pane_id, surface_id) in changed_panes {
                let (status, closed) = {
                    let Some(desk) = self.workdesks.get(desk_index) else {
                        continue;
                    };
                    let Some(terminal) = desk.terminals.get(&surface_id) else {
                        continue;
                    };
                    (terminal.status(), terminal.closed())
                };
                let render_visible = desk_index == self.active_workdesk
                    && self.visible_terminal_surfaces.contains(&surface_id);

                let previous_status = self.workdesks[desk_index]
                    .terminal_statuses
                    .get(&surface_id)
                    .cloned()
                    .unwrap_or(None);
                self.workdesks[desk_index]
                    .terminal_statuses
                    .insert(surface_id, status.clone());
                if render_visible {
                    self.workdesks[desk_index].note_pane_activity(pane_id);
                    changed = true;
                }

                if previous_status != status {
                    changed = self.handle_terminal_attention_transition(
                        desk_index,
                        pane_id,
                        status.as_deref(),
                        closed,
                        cx,
                    ) || changed;
                }
            }
        }

        changed
    }

    fn tick_cursor_blink(&mut self) -> bool {
        if !self.cursor_blink_target_active() {
            let changed = !self.cursor_blink_visible;
            self.cursor_blink_visible = true;
            self.last_cursor_blink_at = Instant::now();
            return changed;
        }

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

        let mut visible_terminal_surfaces = HashSet::new();
        let grid_updates = pane_frames
            .into_iter()
            .filter_map(|(pane_id, frame)| {
                let pane = panes.iter().find(|pane| pane.id == pane_id)?;
                if pane_frame_intersects_viewport(
                    frame,
                    sidebar_width,
                    viewport_width,
                    viewport_height,
                ) {
                    if let Some(surface) = pane.active_surface() {
                        if surface.kind.is_terminal() {
                            visible_terminal_surfaces.insert(surface.id);
                        }
                    }
                }
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

        self.visible_terminal_surfaces = visible_terminal_surfaces;
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

    fn open_agent_provider_popup(
        &mut self,
        target: agent_provider_popup::AgentLaunchTarget,
        cx: &mut Context<Self>,
    ) {
        self.dismiss_workdesk_menu();
        self.dismiss_stack_surface_menu();
        self.dismiss_notifications();
        self.session_inspector = None;
        self.workspace_palette = None;
        self.review_panel = None;
        let desk_index = self.active_workdesk;
        let cwd = Self::workdesk_agent_cwd(&self.workdesks[desk_index]);
        let options = self.agent_runtime.provider_options_for_cwd(&cwd);
        self.agent_provider_popup = Some(agent_provider_popup::AgentProviderPopupState::new(
            desk_index,
            target,
            options,
        ));
        cx.notify();
    }

    fn open_agent_provider_popup_for_new_pane(&mut self, window: &Window, cx: &mut Context<Self>) {
        let world_center = self.screen_to_world(self.viewport_center(window));
        self.open_agent_provider_popup(
            agent_provider_popup::AgentLaunchTarget::NewPane { world_center },
            cx,
        );
    }

    fn open_agent_provider_popup_for_stack(&mut self, pane_id: PaneId, cx: &mut Context<Self>) {
        if self.active_workdesk().pane(pane_id).is_none() {
            return;
        }

        self.active_workdesk_mut().focus_pane(pane_id);
        self.active_workdesk_mut().note_pane_activity(pane_id);
        let desk_index = self.active_workdesk;
        self.ensure_agent_runtime_for_pane(desk_index, pane_id);
        self.open_agent_provider_popup(
            agent_provider_popup::AgentLaunchTarget::StackIntoPane(pane_id),
            cx,
        );
    }

    fn complete_agent_provider_popup_selection(
        &mut self,
        profile_id: &str,
        cx: &mut Context<Self>,
    ) -> bool {
        let (desk_index, target) = {
            let Some(popup) = self.agent_provider_popup.as_ref() else {
                return false;
            };
            if !popup.allows_selection(profile_id) {
                return false;
            }
            (popup.desk_index, popup.target)
        };

        self.agent_provider_popup = None;
        let result = match target {
            agent_provider_popup::AgentLaunchTarget::NewPane { world_center } => {
                Self::spawn_agent_surface_on_workdesk_state_with_profile(
                    &mut self.workdesks,
                    &mut self.active_workdesk,
                    desk_index,
                    None,
                    profile_id,
                    world_center,
                    true,
                    &self.agent_runtime,
                )
                .map(|_| ())
            }
            agent_provider_popup::AgentLaunchTarget::StackIntoPane(pane_id) => {
                Self::spawn_agent_surface_on_workdesk_state_with_profile(
                    &mut self.workdesks,
                    &mut self.active_workdesk,
                    desk_index,
                    Some(pane_id),
                    profile_id,
                    WorkdeskPoint::new(0.0, 0.0),
                    true,
                    &self.agent_runtime,
                )
                .map(|_| ())
            }
        };

        match result {
            Ok(()) => {
                self.request_persist(cx);
                cx.notify();
                true
            }
            Err(error) => {
                self.set_runtime_notice(error);
                cx.notify();
                false
            }
        }
    }

    fn stack_surface_in_pane(
        &mut self,
        pane_id: PaneId,
        kind: PaneKind,
        cx: &mut Context<Self>,
    ) -> bool {
        let desk_index = self.active_workdesk;
        match self.spawn_surface_on_workdesk(
            desk_index,
            Some(pane_id),
            kind,
            None,
            None,
            None,
            true,
        ) {
            Ok(_) => {
                self.request_persist(cx);
                cx.notify();
                true
            }
            Err(error) => {
                self.set_runtime_notice(error);
                cx.notify();
                false
            }
        }
    }

    fn open_editor_picker_for_target_pane(
        &mut self,
        target_pane_id: Option<PaneId>,
        cx: &mut Context<Self>,
    ) {
        self.workspace_palette = None;
        let receiver = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(format!("Open file in {PRODUCT_NAME}").into()),
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
                    target_pane_id,
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

    fn open_editor_picker(&mut self, cx: &mut Context<Self>) {
        self.open_editor_picker_for_target_pane(None, cx);
    }

    fn workspace_palette_root(&self) -> PathBuf {
        worktree_id_from_desk(self.active_workdesk())
            .map(|worktree_id| PathBuf::from(worktree_id.0))
            .filter(|path| path.exists())
            .unwrap_or_else(workspace_root_path)
    }

    fn open_workspace_palette(&mut self, mode: WorkspacePaletteMode, cx: &mut Context<Self>) {
        self.dismiss_workdesk_menu();
        self.dismiss_stack_surface_menu();
        self.dismiss_notifications();
        self.agent_provider_popup = None;
        self.workdesk_editor = None;
        self.session_inspector = None;
        self.review_panel = None;
        self.workspace_palette = Some(WorkspacePaletteState::new(mode, self.workspace_palette_root()));
        cx.notify();
    }

    fn dismiss_workspace_palette(&mut self) -> bool {
        self.workspace_palette.take().is_some()
    }

    fn move_active_editor_to_line(&mut self, surface_id: SurfaceId, line_number: usize) {
        let Some(editor) = self.active_workdesk_mut().editors.get_mut(&surface_id) else {
            return;
        };
        let line_index = line_number.saturating_sub(1);
        let target = editor.offset_for_line_col(line_index, 0);
        editor.move_to_offset(target, false);
        editor.set_scroll_top_line(line_index.saturating_sub(3));
    }

    fn open_workspace_palette_result(
        &mut self,
        result: WorkspacePaletteResult,
        cx: &mut Context<Self>,
    ) -> bool {
        let (absolute_path, line_number) = match result {
            WorkspacePaletteResult::File(file) => (file.absolute_path, None),
            WorkspacePaletteResult::SearchMatch {
                absolute_path,
                line_number,
                ..
            } => (absolute_path, Some(line_number)),
        };
        let desk_index = self.active_workdesk;
        match self.spawn_surface_on_workdesk(
            desk_index,
            None,
            PaneKind::Editor,
            None,
            None,
            Some(absolute_path),
            true,
        ) {
            Ok((pane_id, surface_id)) => {
                if let Some(line_number) = line_number {
                    self.move_active_editor_to_line(surface_id, line_number);
                    self.sync_editor_surface_metadata(pane_id, surface_id);
                }
                self.dismiss_workspace_palette();
                self.request_persist(cx);
                cx.notify();
                true
            }
            Err(error) => {
                self.set_runtime_notice(error);
                cx.notify();
                false
            }
        }
    }

    fn open_selected_workspace_palette_result(&mut self, cx: &mut Context<Self>) -> bool {
        let result = self
            .workspace_palette
            .as_ref()
            .and_then(|palette| palette.selected_result())
            .cloned();
        let Some(result) = result else {
            return false;
        };
        self.open_workspace_palette_result(result, cx)
    }

    fn open_review_panel(&mut self, desk_index: usize, cx: &mut Context<Self>) {
        let Some(desk) = self.workdesks.get(desk_index) else {
            return;
        };
        let Some(payload) = desk.review_payload_cache.as_ref() else {
            return;
        };
        if payload.files.is_empty() {
            return;
        }
        self.dismiss_workdesk_menu();
        self.dismiss_stack_surface_menu();
        self.dismiss_notifications();
        self.agent_provider_popup = None;
        self.workdesk_editor = None;
        self.session_inspector = None;
        self.workspace_palette = None;
        self.active_workdesk = desk_index;
        self.review_panel = Some(desk_index);
        if let Some(desk) = self.workdesks.get_mut(desk_index) {
            clamp_review_panel_selection(desk);
        }
        self.request_persist(cx);
        cx.notify();
    }

    fn close_review_panel(&mut self, cx: &mut Context<Self>) -> bool {
        if self.review_panel.take().is_some() {
            cx.notify();
            true
        } else {
            false
        }
    }

    fn select_review_panel_file(&mut self, desk_index: usize, file_index: usize, cx: &mut Context<Self>) {
        let Some(desk) = self.workdesks.get_mut(desk_index) else {
            return;
        };
        let Some(file_count) = desk.review_payload_cache.as_ref().map(|p| p.files.len()) else {
            return;
        };
        if file_index >= file_count {
            return;
        }
        desk.review_local_state.selected_file = file_index;
        desk.review_local_state.selected_hunk = desk
            .review_payload_cache
            .as_ref()
            .and_then(|p| p.files.get(file_index))
            .and_then(|file| {
                if file.hunks.is_empty() {
                    None
                } else {
                    Some(0)
                }
            });
        cx.notify();
    }

    fn select_review_panel_hunk(&mut self, desk_index: usize, hunk_index: usize, cx: &mut Context<Self>) {
        let Some(desk) = self.workdesks.get_mut(desk_index) else {
            return;
        };
        let Some(hunk_count) = desk
            .review_payload_cache
            .as_ref()
            .and_then(|p| p.files.get(desk.review_local_state.selected_file))
            .map(|f| f.hunks.len())
        else {
            return;
        };
        if hunk_index >= hunk_count {
            return;
        }
        desk.review_local_state.selected_hunk = Some(hunk_index);
        cx.notify();
    }

    fn review_panel_hunk_actions_enabled(&self) -> bool {
        let Some(desk_index) = self.review_panel else {
            return false;
        };
        let Some(desk) = self.workdesks.get(desk_index) else {
            return false;
        };
        let Some(payload) = desk.review_payload_cache.as_ref() else {
            return false;
        };
        let Some(file) = payload.files.get(desk.review_local_state.selected_file) else {
            return false;
        };
        review::review_local_hunk_actions_enabled(file)
    }

    fn mark_review_selected_hunk_reviewed(&mut self, cx: &mut Context<Self>) {
        self.set_selected_hunk_review_state(review::HunkReviewState::Reviewed, cx);
    }

    fn mark_review_selected_hunk_follow_up(&mut self, cx: &mut Context<Self>) {
        self.set_selected_hunk_review_state(review::HunkReviewState::FollowUp, cx);
    }

    fn mark_review_clear_selected_hunk(&mut self, cx: &mut Context<Self>) {
        let Some(desk_index) = self.review_panel else {
            return;
        };
        let Some(desk) = self.workdesks.get_mut(desk_index) else {
            return;
        };
        let Some(payload) = desk.review_payload_cache.as_ref() else {
            return;
        };
        let Some(file) = payload.files.get(desk.review_local_state.selected_file) else {
            return;
        };
        if !review::review_local_hunk_actions_enabled(file) {
            return;
        }
        let Some(hunk_index) = desk.review_local_state.selected_hunk else {
            return;
        };
        let Some(hunk) = file.hunks.get(hunk_index) else {
            return;
        };
        let key = review::ReviewHunkKey::from_hunk(
            &WorkdeskId::new(desk.workdesk_id.clone()),
            &file.path,
            hunk,
        );
        desk.review_local_state.hunk_states.remove(&key);
        cx.notify();
    }

    fn set_selected_hunk_review_state(
        &mut self,
        state: review::HunkReviewState,
        cx: &mut Context<Self>,
    ) {
        let Some(desk_index) = self.review_panel else {
            return;
        };
        let Some(desk) = self.workdesks.get_mut(desk_index) else {
            return;
        };
        let Some(payload) = desk.review_payload_cache.as_ref() else {
            return;
        };
        let Some(file) = payload.files.get(desk.review_local_state.selected_file) else {
            return;
        };
        if !review::review_local_hunk_actions_enabled(file) {
            return;
        }
        let Some(hunk_index) = desk.review_local_state.selected_hunk else {
            return;
        };
        let Some(hunk) = file.hunks.get(hunk_index) else {
            return;
        };
        let key = review::ReviewHunkKey::from_hunk(
            &WorkdeskId::new(desk.workdesk_id.clone()),
            &file.path,
            hunk,
        );
        desk.review_local_state.hunk_states.insert(key, state);
        cx.notify();
    }

    fn open_review_diff_line(
        &mut self,
        review_desk_index: usize,
        file_index: usize,
        hunk_index: usize,
        line_index: usize,
        cx: &mut Context<Self>,
    ) -> bool {
        let resolved = {
            let Some(desk) = self.workdesks.get(review_desk_index) else {
                return false;
            };
            let Some(payload) = desk.review_payload_cache.as_ref() else {
                return false;
            };
            let Some(file) = payload.files.get(file_index) else {
                return false;
            };
            let Some(hunk) = file.hunks.get(hunk_index) else {
                return false;
            };
            let Some(line) = hunk.lines.get(line_index) else {
                return false;
            };
            let Some(line_no) = editor_jump_line_for_review_row(hunk, line) else {
                return false;
            };
            let absolute_path = review_file_absolute_path(desk, &file.path);
            (absolute_path.display().to_string(), line_no)
        };
        let (absolute_string, line_no) = resolved;
        match self.spawn_surface_on_workdesk(
            review_desk_index,
            None,
            PaneKind::Editor,
            None,
            None,
            Some(absolute_string.clone()),
            true,
        ) {
            Ok((pane_id, surface_id)) => {
                self.move_active_editor_to_line(surface_id, line_no as usize);
                self.sync_editor_surface_metadata(pane_id, surface_id);
                self.request_persist(cx);
                cx.notify();
                true
            }
            Err(error) => {
                if let Some(desk) = self.workdesks.get_mut(review_desk_index) {
                    desk.runtime_notice = Some(SharedString::from(
                        review_editor_open_failed_notice(&absolute_string, &error),
                    ));
                }
                cx.notify();
                false
            }
        }
    }

    fn close_pane(&mut self, pane_id: PaneId, cx: &mut Context<Self>) {
        if matches!(
            self.stack_surface_menu,
            Some(menu) if menu.desk_index == self.active_workdesk && menu.pane_id == pane_id
        ) {
            self.dismiss_stack_surface_menu();
        }
        let removed_surfaces = self
            .active_workdesk()
            .pane(pane_id)
            .map(|pane| pane.surfaces.clone())
            .unwrap_or_default();
        let workdesk_runtime_id = self.active_workdesk().runtime_id;
        for surface in &removed_surfaces {
            if surface.kind == PaneKind::Agent {
                self.agent_runtime
                    .stop_surface(workdesk_runtime_id, surface.id);
            }
        }
        let desk = self.active_workdesk_mut();
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

        if self.handle_agent_session_composer_key_down(event, cx) {
            cx.stop_propagation();
            return;
        }

        if self.handle_workdesk_editor_key_down(event, cx) {
            cx.stop_propagation();
            return;
        }

        if self.agent_provider_popup.is_some() {
            if event.keystroke.key == "escape" && !event.keystroke.modifiers.modified() {
                if self.dismiss_agent_provider_popup() {
                    cx.notify();
                }
            }
            cx.stop_propagation();
            return;
        }

        if self.review_panel.is_some()
            && event.keystroke.key == "escape"
            && !event.keystroke.modifiers.modified()
        {
            if self.close_review_panel(cx) {
                cx.stop_propagation();
                return;
            }
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

        if self.handle_workspace_palette_key_down(event, cx) {
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
        {
            let desk = self.active_workdesk_mut();
            desk.focus_pane(pane_id);
            desk.note_pane_activity(pane_id);
        }
        let desk_index = self.active_workdesk;
        self.ensure_agent_runtime_for_pane(desk_index, pane_id);
        self.request_persist(cx);
        cx.notify();
    }

    fn begin_pane_drag(
        &mut self,
        pane_id: PaneId,
        position: gpui::Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let free_layout = self.active_workdesk().layout_mode == LayoutMode::Free;
        {
            let desk = self.active_workdesk_mut();
            desk.focus_pane(pane_id);
            if free_layout {
                desk.clear_selection(pane_id);
                desk.drag_state = DragState::MovingPane {
                    pane_id,
                    last_mouse: position,
                };
            }
        }
        let desk_index = self.active_workdesk;
        self.ensure_agent_runtime_for_pane(desk_index, pane_id);
        if !free_layout {
            self.request_persist(cx);
            cx.notify();
            return;
        }
        cx.notify();
    }

    fn begin_pane_resize(
        &mut self,
        pane_id: PaneId,
        position: gpui::Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let free_layout = self.active_workdesk().layout_mode == LayoutMode::Free;
        {
            let desk = self.active_workdesk_mut();
            desk.focus_pane(pane_id);
            if free_layout {
                desk.clear_selection(pane_id);
                desk.drag_state = DragState::ResizingPane {
                    pane_id,
                    last_mouse: position,
                };
            }
        }
        let desk_index = self.active_workdesk;
        self.ensure_agent_runtime_for_pane(desk_index, pane_id);
        if !free_layout {
            self.request_persist(cx);
            cx.notify();
            return;
        }
        cx.notify();
    }

    fn begin_terminal_selection(
        &mut self,
        pane_id: PaneId,
        position: gpui::Point<Pixels>,
        metrics: TerminalFrameMetrics,
        cx: &mut Context<Self>,
    ) {
        let surface_id = {
            let desk = self.active_workdesk_mut();
            desk.focus_pane(pane_id);
            let Some(surface_id) = desk.active_terminal_surface_id_for_pane(pane_id) else {
                return;
            };
            desk.begin_selection(surface_id, metrics.cell_at(position));
            desk.drag_state = DragState::SelectingTerminal { pane_id, metrics };
            surface_id
        };
        let desk_index = self.active_workdesk;
        self.ensure_agent_runtime_for_pane(desk_index, pane_id);
        let _ = surface_id;
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

    fn handle_workspace_palette_key_down(
        &mut self,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(palette) = self.workspace_palette.as_mut() else {
            return false;
        };
        let keystroke = &event.keystroke;
        match keystroke.key.as_str() {
            "escape" if !keystroke.modifiers.modified() => {
                self.dismiss_workspace_palette();
                cx.notify();
                return true;
            }
            "enter" if !keystroke.modifiers.modified() => {
                return self.open_selected_workspace_palette_result(cx);
            }
            "up" if !keystroke.modifiers.modified() => {
                palette.move_selection(-1);
                cx.notify();
                return true;
            }
            "down" if !keystroke.modifiers.modified() => {
                palette.move_selection(1);
                cx.notify();
                return true;
            }
            "pageup" if !keystroke.modifiers.modified() => {
                palette.move_selection(-8);
                cx.notify();
                return true;
            }
            "pagedown" if !keystroke.modifiers.modified() => {
                palette.move_selection(8);
                cx.notify();
                return true;
            }
            "backspace" | "delete" if !keystroke.modifiers.modified() => {
                palette.pop_query();
                cx.notify();
                return true;
            }
            _ => {}
        }
        if let Some(text) = editable_keystroke_text(keystroke) {
            palette.append_query(&text);
            cx.notify();
            return true;
        }
        false
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
        self.dismiss_stack_surface_menu();
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
            self.ensure_agent_runtime_for_pane(index, pane_id);
        }
        if let Err(error) = self.sync_review_summary_for_desk(index) {
            self.active_workdesk_mut().runtime_notice =
                Some(SharedString::from(format!("review summary stale: {error}")));
        }
        self.request_persist(cx);
        cx.notify();
    }

    fn dismiss_workdesk_menu(&mut self) -> bool {
        self.workdesk_menu.take().is_some()
    }

    fn dismiss_stack_surface_menu(&mut self) -> bool {
        self.stack_surface_menu.take().is_some()
    }

    fn dismiss_agent_provider_popup(&mut self) -> bool {
        self.agent_provider_popup.take().is_some()
    }

    fn dismiss_notifications(&mut self) -> bool {
        let was_open = self.notifications_open;
        self.notifications_open = false;
        was_open
    }

    fn toggle_notifications(&mut self, cx: &mut Context<Self>) {
        self.notifications_open = !self.notifications_open;
        if self.notifications_open {
            self.notifications.mark_all_read();
            self.dismiss_workdesk_menu();
            self.dismiss_stack_surface_menu();
            self.session_inspector = None;
            self.workspace_palette = None;
            self.review_panel = None;
        }
        cx.notify();
    }

    fn toggle_sidebar_collapsed(&mut self, cx: &mut Context<Self>) {
        self.sidebar_collapsed = !self.sidebar_collapsed;
        self.dismiss_workdesk_menu();
        self.dismiss_stack_surface_menu();
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

        self.dismiss_stack_surface_menu();
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

    fn open_stack_surface_menu(
        &mut self,
        pane_id: PaneId,
        position: gpui::Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        if self.active_workdesk().pane(pane_id).is_none() {
            return;
        }

        self.dismiss_workdesk_menu();
        self.dismiss_notifications();
        self.active_workdesk_mut().focus_pane(pane_id);
        self.active_workdesk_mut().note_pane_activity(pane_id);
        let desk_index = self.active_workdesk;
        self.ensure_agent_runtime_for_pane(desk_index, pane_id);
        self.stack_surface_menu = Some(StackSurfaceMenu {
            desk_index: self.active_workdesk,
            pane_id,
            position,
        });
        self.request_persist(cx);
        cx.notify();
    }

    fn toggle_stack_surface_menu(
        &mut self,
        pane_id: PaneId,
        position: gpui::Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        if matches!(
            self.stack_surface_menu,
            Some(menu) if menu.desk_index == self.active_workdesk && menu.pane_id == pane_id
        ) {
            self.dismiss_stack_surface_menu();
            cx.notify();
            return;
        }

        self.open_stack_surface_menu(pane_id, position, cx);
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
        desk.workdesk_id = self.allocate_workdesk_id();
        desk.runtime_id = self.allocate_workdesk_runtime_id();
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
        duplicated.workdesk_id = self.allocate_workdesk_id();
        duplicated.runtime_id = self.allocate_workdesk_runtime_id();

        let insert_at = index + 1;
        self.workdesks.insert(insert_at, duplicated);
        let bridge = &self.agent_runtime;
        let desk_ref = &mut self.workdesks[insert_at];
        boot_workdesk_terminals(desk_ref);
        Self::boot_agent_sessions_for_desk(bridge, desk_ref.runtime_id, desk_ref);
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
        stop_agent_runtime_for_desk(&self.agent_runtime, &mut removed);
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
            rgb(0x131a20)
        } else {
            rgb(0x10161c)
        };
        let header_divider = if pane_attention.state.is_attention() {
            attention_tint
        } else {
            rgb(0x24303a).into()
        };
        let screen_x = frame.x;
        let screen_y = frame.y;
        let screen_width = frame.width;
        let screen_height = frame.height;
        let header_height = terminal_header_height_for_surface_count(pane.surfaces.len())
            * frame.zoom.clamp(0.78, 1.3);
        let pane_padding = 12.0 * frame.zoom.clamp(0.85, 1.25);
        let stack_rail_width =
            pane_stack_rail_width(pane.surfaces.len()) * frame.zoom.clamp(0.82, 1.2);
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
        let stack_label = pane.stack_display_title().to_string();
        let stack_count_label = surface_count_label(pane.surfaces.len());
        let active_terminal_status = workdesk
            .terminal_statuses
            .get(&active_surface_id)
            .cloned()
            .unwrap_or(None)
            .filter(|status| !status.trim().is_empty());
        let active_agent_session = matches!(active_surface_kind, PaneKind::Agent)
            .then(|| {
                self.agent_runtime
                    .session_for_surface(workdesk.runtime_id, active_surface_id)
            })
            .flatten();
        let agent_provider_badge = active_agent_session
            .as_ref()
            .map(|record| {
                self.agent_runtime
                    .provider_profile(&record.provider_profile_id)
                    .and_then(|profile| {
                        profile
                            .capability_note
                            .map(|note| format!("{} · {}", record.provider_profile_id, note))
                    })
                    .unwrap_or_else(|| record.provider_profile_id.clone())
            });
        let session_inspector_active = matches!(
            self.session_inspector,
            Some(target)
                if target.desk_index == self.active_workdesk && target.surface_id == active_surface_id
        );
        let status_tint = if pane_attention.state == AttentionState::Idle {
            match &active_surface_kind {
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
        let (surface_status_label, surface_status_tint) = match &active_surface_kind {
            PaneKind::Shell | PaneKind::Agent => {
                let label = active_terminal_status.or_else(|| {
                    terminal_snapshot
                        .as_ref()
                        .filter(|snapshot| snapshot.closed)
                        .map(|_| "Closed".to_string())
                });
                let tint = terminal_snapshot
                    .as_ref()
                    .filter(|snapshot| snapshot.closed)
                    .map(|_| rgb(0xff9b88).into())
                    .unwrap_or(status_tint);
                (label, tint)
            }
            PaneKind::Editor => {
                let editor_state = workdesk.editors.get(&active_surface_id);
                if editor_state.is_some_and(EditorBuffer::external_modified) {
                    (Some("Externally changed".to_string()), rgb(0xff9b88).into())
                } else if active_surface_dirty {
                    (Some("Dirty".to_string()), rgb(0xf0d35f).into())
                } else {
                    (None, status_tint)
                }
            }
            PaneKind::Browser => (None, status_tint),
        };
        let entity = cx.entity();
        let stack_menu_open = matches!(
            self.stack_surface_menu,
            Some(menu) if menu.desk_index == self.active_workdesk && menu.pane_id == pane_id
        );

        let header = {
            let close_surface_id = active_surface_id;
            let close_surface_count = pane.surfaces.len();
            let inspector_desk_index = self.active_workdesk;
            let header = div()
                .flex()
                .items_center()
                .justify_between()
                .gap_2()
                .h(px(header_height))
                .px(px(pane_padding))
                .bg(header_bg)
                .border_b_1()
                .border_color(header_divider)
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
                                .text_color(accent)
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .child(stack_label.clone()),
                        )
                        .child(div().text_xs().text_color(rgb(0x53606a)).child("·"))
                        .child(
                            div()
                                .text_xs()
                                .text_color(status_tint)
                                .child(surface_kind_slug(&active_surface_kind)),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(0xc8d1d8))
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .child(runtime_title.clone()),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .when_some(surface_status_label.clone(), |row, status| {
                            row.child(context_pill(status, surface_status_tint))
                        })
                        .when_some(agent_provider_badge.clone(), |row, badge| {
                            row.child(context_pill(badge, rgb(0x7f8a94).into()))
                        })
                        .when(active_agent_session.is_some(), |row| {
                            row.child(compact_toggle_button(
                                "Info",
                                rgb(0x7cc7ff).into(),
                                session_inspector_active,
                                cx.listener(move |this, _, _, cx| {
                                    this.toggle_session_inspector(
                                        inspector_desk_index,
                                        active_surface_id,
                                        cx,
                                    );
                                    cx.stop_propagation();
                                }),
                            ))
                        })
                        .when(pane.surfaces.len() > 1, |row| {
                            row.child(context_pill(stack_count_label.clone(), accent))
                        })
                        .child(
                            div()
                                .cursor_pointer()
                                .w(px(20.0))
                                .h(px(20.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_sm()
                                .bg(rgb(0x171d24))
                                .border_1()
                                .border_color(rgb(0x293742))
                                .text_xs()
                                .text_color(rgb(0xaeb8bf))
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
                                .child("x"),
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

        let body_content = match active_surface_kind {
            PaneKind::Shell | PaneKind::Agent => {
                let terminal_body_metrics = terminal_snapshot.as_ref().map(|snapshot| {
                    terminal_frame_metrics(
                        terminal_metrics,
                        screen_x,
                        screen_y,
                        header_height,
                        pane_padding,
                        stack_rail_width,
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
                    let search_match_count = editor.search_match_count();
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
                        .map(|index| format!("{}/{}", index + 1, search_match_count))
                        .unwrap_or_else(|| format!("0/{}", search_match_count));
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

        let stack_rail = div()
            .flex()
            .flex_col()
            .items_center()
            .justify_between()
            .gap_2()
            .w(px(stack_rail_width))
            .py(px(pane_padding.max(8.0)))
            .bg(rgb(0x11171d))
            .border_r_1()
            .border_color(rgb(0x22303a))
            .child(
                div().flex().flex_col().items_center().gap_1().children(
                    pane.surfaces
                        .iter()
                        .enumerate()
                        .map(|(index, surface)| {
                            let surface_id = surface.id;
                            let active = surface_id == active_surface_id;
                            surface_stack_rail_button(
                                surface,
                                index,
                                active,
                                pane_accent(&surface.kind),
                                cx.listener(move |this, _, window, cx| {
                                    this.dismiss_stack_surface_menu();
                                    this.active_workdesk_mut()
                                        .focus_surface(pane_id, surface_id);
                                    this.active_workdesk_mut().note_pane_activity(pane_id);
                                    let desk_index = this.active_workdesk;
                                    this.ensure_agent_runtime_for_pane(desk_index, pane_id);
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
            .child(surface_stack_rail_action_button(
                "+",
                rgb(0x77d19a).into(),
                stack_menu_open,
                cx.listener(move |this, event: &MouseUpEvent, _, cx| {
                    this.toggle_stack_surface_menu(pane_id, event.position, cx);
                    cx.stop_propagation();
                }),
            ));

        let body = div()
            .flex()
            .flex_1()
            .overflow_hidden()
            .child(stack_rail)
            .child(body_content);

        let surface = div()
            .absolute()
            .left(px(screen_x))
            .top(px(screen_y))
            .w(px(screen_width))
            .h(px(screen_height))
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(rgb(0x161d24))
            .border_1()
            .border_color(border)
            .rounded_md()
            .shadow_md()
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

impl Render for AxisShell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();
        let viewport = window.window_bounds().get_bounds();
        let viewport_width = f32::from(viewport.size.width);
        let viewport_height = f32::from(viewport.size.height);
        self.sync_visible_terminal_grids(window, viewport_width, viewport_height);
        let sidebar_width = self.sidebar_width();
        let open_workdesk_menu = self.workdesk_menu;
        let open_stack_surface_menu = self.stack_surface_menu;
        let workdesk = self.active_workdesk();
        let layout_mode = workdesk.layout_mode;
        let grid_expose_open = layout_mode == LayoutMode::Grid && workdesk.grid_layout.expose_open;
        let active_stack = workdesk
            .active_pane
            .and_then(|pane_id| workdesk.pane(pane_id))
            .map(|pane| {
                (
                    pane.id,
                    pane.stack_display_title().to_string(),
                    surface_count_label(pane.surfaces.len()),
                )
            });
        let stack_controls_enabled = active_stack.is_some();
        let active_stack_summary = active_stack
            .as_ref()
            .map(|(_, label, count_label)| format!("{label} · {count_label}"))
            .unwrap_or_else(|| "No active pane".to_string());
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
                        desk_has_review_entries(desk),
                        cx.listener(move |this, _, _, cx| {
                            this.open_review_panel(index, cx);
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
                        workdesk_navigation_target(desk),
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
                        cx.listener(move |this, _, _, cx| {
                            let Some(target) = workdesk_navigation_target(&this.workdesks[index])
                            else {
                                return;
                            };
                            this.dismiss_workdesk_menu();
                            this.navigate_to_workdesk_pane(index, target.pane_id, cx);
                            cx.stop_propagation();
                        }),
                        cx.listener(move |this, _, _, cx| {
                            this.open_workdesk_editor_panel(index, cx);
                            cx.stop_propagation();
                        }),
                        cx.listener(move |this, event: &MouseUpEvent, _, cx| {
                            this.toggle_workdesk_menu(index, event.position, cx);
                            cx.stop_propagation();
                        }),
                        cx.listener(move |this, _, _, cx| {
                            this.open_review_panel(index, cx);
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
        let unread_notifications = self.notification_unread_count();
        let notification_entries = self
            .notifications
            .items
            .iter()
            .cloned()
            .rev()
            .collect::<Vec<_>>();
        let notification_overlay = self.notifications_open.then(|| {
            let notification_cards = if notification_entries.is_empty() {
                vec![
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .p_3()
                        .bg(rgb(0x10171d))
                        .border_1()
                        .border_color(rgb(0x24313b))
                        .rounded_lg()
                        .child(div().text_sm().text_color(rgb(0xdce2e8)).child("No attention events yet"))
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(0x8e9ba5))
                                .child("Agent and terminal attention changes will appear here."),
                        )
                        .into_any_element(),
                ]
            } else {
                notification_entries
                    .into_iter()
                    .map(|item| {
                        let notification_id = item.id;
                        notification_item(
                            &item.title,
                            &item.detail,
                            &item.context,
                            item.state.tint(),
                            item.unread,
                            cx.listener(move |this, _, _, cx| {
                                this.open_notification_target(notification_id, cx);
                                cx.stop_propagation();
                            }),
                        )
                        .into_any_element()
                    })
                    .collect::<Vec<_>>()
            };
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
                                        .child("Attention feed"),
                                )
                                .child(div().text_sm().child("Notifications"))
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(0x8e9ba5))
                                        .child(format!(
                                            "{} unread attention events across workdesks.",
                                            unread_notifications
                                        )),
                                ),
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
                .children(notification_cards)
        });
        let session_inspector_view = self
            .session_inspector
            .and_then(|target| self.agent_session_inspector_view_for_target(target));
        let session_inspector_overlay = self.session_inspector.map(|target| {
            let session_composer = self
                .agent_session_composer
                .clone()
                .filter(|composer| composer.target == target)
                .unwrap_or_else(|| AgentSessionComposerState::new(target));
            div()
                .absolute()
                .left(px(0.0))
                .top(px(0.0))
                .w(px(viewport_width))
                .h(px(viewport_height))
                .bg(rgba(0x09101660))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.close_session_inspector(cx);
                        cx.stop_propagation();
                    }),
                )
                .child(
                    div()
                        .absolute()
                        .top(px(SHORTCUT_PANEL_MARGIN))
                        .right(px(SHORTCUT_PANEL_MARGIN))
                        .bottom(px(SHORTCUT_PANEL_MARGIN))
                        .w(px(
                            SESSION_INSPECTOR_WIDTH
                                .min((viewport_width - SHORTCUT_PANEL_MARGIN * 2.0).max(320.0)),
                        ))
                        .flex()
                        .flex_col()
                        .gap_3()
                        .p_4()
                        .bg(rgb(0x0f151b))
                        .border_l_1()
                        .border_color(rgb(0x2c3944))
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
                                                .child("Agent session"),
                                        )
                                        .child(div().text_lg().child("Session inspector"))
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(rgb(0x90a0aa))
                                                .child(
                                                    "Structured timeline, approvals, and recent terminal replay.",
                                                ),
                                        ),
                                )
                                .child(compact_dock_button(
                                    "Close",
                                    rgb(0xff9b88).into(),
                                    cx.listener(|this, _, _, cx| {
                                        this.close_session_inspector(cx);
                                        cx.stop_propagation();
                                    }),
                                )),
                        )
                        .when_some(session_inspector_view.clone(), |panel, view| {
                            let composer_draft = session_composer.draft.clone();
                            let composer_active = session_composer.active;
                            let can_submit_turn =
                                view.can_send_turn && !composer_draft.trim().is_empty();
                            panel.child(
                                div()
                                    .id("agent-session-inspector-scroll")
                                    .flex_1()
                                    .flex()
                                    .flex_col()
                                    .gap_3()
                                    .overflow_y_scroll()
                                    .child(
                                        div()
                                            .flex()
                                            .flex_wrap()
                                            .gap_2()
                                            .child(context_pill(
                                                agent_lifecycle_label(view.lifecycle),
                                                rgb(0x77d19a).into(),
                                            ))
                                            .child(context_pill(
                                                agent_attention_label(view.attention),
                                                attention::agent_attention_state(view.attention)
                                                    .tint(),
                                            ))
                                            .child(context_pill(
                                                agent_transport_label(view.transport),
                                                rgb(0x90a0aa).into(),
                                            ))
                                            .when(view.can_send_turn, |row| {
                                                row.child(context_pill(
                                                    "Turn input",
                                                    rgb(0x7cc7ff).into(),
                                                ))
                                            })
                                            .when(view.can_resume, |row| {
                                                row.child(context_pill(
                                                    "Resume ready",
                                                    rgb(0xf0d35f).into(),
                                                ))
                                            }),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .gap_2()
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(rgb(0x7f8a94))
                                                    .child("Timeline"),
                                            )
                                            .children(
                                                if view.timeline_entries.is_empty() {
                                                    vec![div()
                                                        .p_3()
                                                        .bg(rgb(0x10171d))
                                                        .border_1()
                                                        .border_color(rgb(0x24313b))
                                                        .rounded_lg()
                                                        .text_xs()
                                                        .text_color(rgb(0x7f8a94))
                                                        .child("No structured events yet.")
                                                        .into_any_element()]
                                                } else {
                                                    view.timeline_entries
                                                        .clone()
                                                        .into_iter()
                                                        .map(|entry| {
                                                            agent_timeline_entry_card(entry)
                                                                .into_any_element()
                                                        })
                                                        .collect::<Vec<_>>()
                                                },
                                            ),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .gap_2()
                                            .p_3()
                                            .bg(rgb(0x10171d))
                                            .border_1()
                                            .border_color(rgb(0x24313b))
                                            .rounded_lg()
                                            .child(inspector_row("Session", view.session_id.clone()))
                                            .child(inspector_row(
                                                "Provider",
                                                view.provider_profile_id.clone(),
                                            ))
                                            .when_some(view.capability_note.clone(), |rows, note| {
                                                rows.child(inspector_row("Capability", note))
                                            })
                                            .child(inspector_row("Desk", view.workdesk_name.clone()))
                                            .child(inspector_row("Pane", view.pane_title.clone()))
                                            .child(inspector_row(
                                                "Surface",
                                                view.surface_id.raw().to_string(),
                                            ))
                                            .child(inspector_row("Cwd", view.cwd.clone()))
                                            .child(inspector_row(
                                                "Status",
                                                view.status_message.clone(),
                                            ))
                                            .when_some(view.terminal_status.clone(), |rows, status| {
                                                rows.child(inspector_row("Terminal", status))
                                            }),
                                    )
                                    .when(
                                        view.can_send_turn || view.can_resume,
                                        |content| {
                                        content.child(
                                            div()
                                                .flex()
                                                .flex_col()
                                                .gap_2()
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(rgb(0x7f8a94))
                                                        .child("Session actions"),
                                                )
                                                .when(view.can_send_turn, |section| {
                                                    section.child(workdesk_editor_field(
                                                        "Turn input",
                                                        composer_draft.clone(),
                                                        composer_active,
                                                        rgb(0x7cc7ff).into(),
                                                        cx.listener(move |this, _, _, cx| {
                                                            this.focus_agent_session_composer(
                                                                target, cx,
                                                            );
                                                            cx.stop_propagation();
                                                        }),
                                                    ))
                                                })
                                                .child(
                                                    div()
                                                        .flex()
                                                        .gap_2()
                                                        .when(view.can_send_turn, |row| {
                                                            row.child(agent_inspector_action_button(
                                                                "Send turn",
                                                                "Submit the draft prompt",
                                                                rgb(0x7cc7ff).into(),
                                                                can_submit_turn,
                                                                cx.listener(|this, _, _, cx| {
                                                                    this.submit_agent_session_composer(
                                                                        cx,
                                                                    );
                                                                    cx.stop_propagation();
                                                                }),
                                                            ))
                                                        })
                                                        .when(view.can_resume, |row| {
                                                            row.child(agent_inspector_action_button(
                                                                "Resume",
                                                                "Continue the active loop",
                                                                rgb(0xf0d35f).into(),
                                                                true,
                                                                cx.listener(move |this, _, _, cx| {
                                                                    this.resume_session_from_inspector(
                                                                        target, cx,
                                                                    );
                                                                    cx.stop_propagation();
                                                                }),
                                                            ))
                                                        }),
                                                ),
                                        )
                                        },
                                    )
                                    .when(
                                        !view.pending_approvals.is_empty(),
                                        |content| {
                                        content.child(
                                            div()
                                                .flex()
                                                .flex_col()
                                                .gap_2()
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(rgb(0x7f8a94))
                                                        .child("Pending approvals"),
                                                )
                                                .children(
                                                    view.pending_approvals
                                                        .clone()
                                                        .into_iter()
                                                        .map(|approval| {
                                                            let approve_id = approval.id.clone();
                                                            let deny_id = approval.id.clone();
                                                            agent_pending_approval_card(
                                                                approval,
                                                                view.can_respond_approval,
                                                                cx.listener(
                                                                    move |this, _, _, cx| {
                                                                        this.respond_to_session_inspector_approval(
                                                                            target,
                                                                            approve_id.clone(),
                                                                            true,
                                                                            cx,
                                                                        );
                                                                        cx.stop_propagation();
                                                                    },
                                                                ),
                                                                cx.listener(
                                                                    move |this, _, _, cx| {
                                                                        this.respond_to_session_inspector_approval(
                                                                            target,
                                                                            deny_id.clone(),
                                                                            false,
                                                                            cx,
                                                                        );
                                                                        cx.stop_propagation();
                                                                    },
                                                                ),
                                                            )
                                                            .into_any_element()
                                                        })
                                                        .collect::<Vec<_>>(),
                                                ),
                                        )
                                        },
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .gap_2()
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(rgb(0x7f8a94))
                                                    .child("Recent replay"),
                                            )
                                            .child(
                                                div()
                                                    .min_h(px(140.0))
                                                    .max_h(px((viewport_height - 460.0).max(140.0)))
                                                    .overflow_hidden()
                                                    .p_3()
                                                    .bg(rgb(0x10171d))
                                                    .border_1()
                                                    .border_color(rgb(0x24313b))
                                                    .rounded_lg()
                                                    .font_family(".ZedMono")
                                                    .text_xs()
                                                    .text_color(rgb(0xdce2e8))
                                                    .child(
                                                        div()
                                                            .flex()
                                                            .flex_col()
                                                            .gap_1()
                                                            .children(
                                                                if view.transcript_preview.is_empty() {
                                                                    vec![div()
                                                                        .text_color(rgb(0x7f8a94))
                                                                        .child("No terminal output yet.")
                                                                        .into_any_element()]
                                                                } else {
                                                                    view.transcript_preview
                                                                        .into_iter()
                                                                        .map(|line| {
                                                                            div()
                                                                                .child(line)
                                                                                .into_any_element()
                                                                        })
                                                                        .collect::<Vec<_>>()
                                                                },
                                                            ),
                                                    ),
                                            ),
                                    ),
                            )
                        })
                        .when(session_inspector_view.is_none(), |panel| {
                            panel.child(
                                div()
                                    .p_3()
                                    .bg(rgb(0x10171d))
                                    .border_1()
                                    .border_color(rgb(0x24313b))
                                    .rounded_lg()
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(rgb(0xdce2e8))
                                            .child("Session no longer available."),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(rgb(0x8e9ba5))
                                            .child(
                                                "The pane or runtime session disappeared before the inspector was rendered.",
                                            ),
                                    ),
                            )
                        }),
                )
        });
        let workspace_palette_overlay = self.workspace_palette.clone().map(|palette| {
            let query = palette.query.clone();
            let root_label = palette.root_path.display().to_string();
            let result_rows = if palette.results.is_empty() {
                vec![
                    div()
                        .p_3()
                        .bg(rgb(0x10171d))
                        .border_1()
                        .border_color(rgb(0x24313b))
                        .rounded_lg()
                        .child(
                            div()
                                .text_sm()
                                .text_color(rgb(0xdce2e8))
                                .child(palette.mode.empty_label()),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(0x8e9ba5))
                                .child(palette.mode.description()),
                        )
                        .into_any_element(),
                ]
            } else {
                palette
                    .results
                    .iter()
                    .enumerate()
                    .map(|(index, result)| {
                        let result = result.clone();
                        let listener_result = result.clone();
                        workspace_palette_result_row(
                            &result,
                            index == palette.selected,
                            cx.listener(move |this, _, _, cx| {
                                this.open_workspace_palette_result(listener_result.clone(), cx);
                                cx.stop_propagation();
                            }),
                        )
                        .into_any_element()
                    })
                    .collect::<Vec<_>>()
            };
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
                        if this.dismiss_workspace_palette() {
                            cx.notify();
                        }
                        cx.stop_propagation();
                    }),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(((viewport_width - WORKSPACE_PALETTE_WIDTH).max(32.0) * 0.5).max(16.0)))
                        .top(px(72.0))
                        .w(px(WORKSPACE_PALETTE_WIDTH.min((viewport_width - 32.0).max(320.0))))
                        .max_h(px((viewport_height - 120.0).max(220.0)))
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
                                                .child(palette.mode.title()),
                                        )
                                        .child(div().text_lg().child(palette.mode.prompt()))
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(rgb(0x8e9ba5))
                                                .child(root_label),
                                        ),
                                )
                                .child(compact_dock_button(
                                    "Close",
                                    rgb(0xff9b88).into(),
                                    cx.listener(|this, _, _, cx| {
                                        this.dismiss_workspace_palette();
                                        cx.stop_propagation();
                                    }),
                                )),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .p_3()
                                .bg(rgb(0x11181e))
                                .border_1()
                                .border_color(rgb(0x24313b))
                                .rounded_lg()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(0x7f8a94))
                                        .child("Query"),
                                )
                                .child(
                                    div()
                                        .font_family(".ZedMono")
                                        .text_sm()
                                        .text_color(if query.is_empty() {
                                            rgb(0x6f7d86)
                                        } else {
                                            rgb(0xdce2e8)
                                        })
                                        .child(if query.is_empty() {
                                            palette.mode.prompt().to_string()
                                        } else {
                                            query
                                        }),
                                ),
                        )
                        .child(
                            div()
                                .id("workspace-palette-results")
                                .flex()
                                .flex_col()
                                .gap_2()
                                .overflow_y_scroll()
                                .children(result_rows),
                        ),
                )
        });
        let review_panel_overlay = self.review_panel.and_then(|desk_index| {
            self.workdesks.get(desk_index).map(|desk| {
                let desk_name = desk.name.clone();
                let payload = desk.review_payload_cache.clone();
                let local = desk.review_local_state.clone();
                (desk_index, desk_name, payload, local)
            })
        }).map(|(desk_index, desk_name, payload, local)| {
            let panel_width = SESSION_INSPECTOR_WIDTH
                .min((viewport_width - SHORTCUT_PANEL_MARGIN * 2.0).max(320.0));
            let file_list_width = 148.0_f32;
            let selected_file = payload
                .as_ref()
                .and_then(|p| p.files.get(local.selected_file));
            let actions_enabled = selected_file
                .map(|file| review::review_local_hunk_actions_enabled(file))
                .unwrap_or(false);
            let stale_rows = local
                .stale_notice
                .clone()
                .map(|notice| {
                    vec![div()
                        .p_2()
                        .bg(rgb(0x2a2410))
                        .border_1()
                        .border_color(rgb(0x5c4d1f))
                        .rounded_lg()
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(0xf0d35f))
                                .child(notice),
                        )
                        .into_any_element()]
                })
                .unwrap_or_default();
            let setup_rows = local
                .setup_notice
                .clone()
                .map(|notice| {
                    vec![div()
                        .p_2()
                        .bg(rgb(0x1a2228))
                        .border_1()
                        .border_color(rgb(0x3a4a5a))
                        .rounded_lg()
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(0x9db4c7))
                                .child(notice),
                        )
                        .into_any_element()]
                })
                .unwrap_or_default();
            let truncated_row = payload.as_ref().filter(|p| p.truncated).map(|_| {
                div()
                    .p_2()
                    .bg(rgb(0x221a1a))
                    .border_1()
                    .border_color(rgb(0x5b3434))
                    .rounded_lg()
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(0xff9b88))
                            .child("Diff was truncated for size; some changes may be missing."),
                    )
                    .into_any_element()
            });
            let file_rows: Vec<gpui::AnyElement> = payload
                .as_ref()
                .map(|p| {
                    p.files
                        .iter()
                        .enumerate()
                        .map(|(file_index, file)| {
                            let active = file_index == local.selected_file;
                            let path_label = if file.truncated {
                                format!("{} (truncated)", file.path)
                            } else {
                                file.path.clone()
                            };
                            let row_border: gpui::Hsla = if active {
                                rgb(0x7cc7ff).into()
                            } else {
                                rgb(0x24313b).into()
                            };
                            div()
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .cursor_pointer()
                                .bg(if active {
                                    rgb(0x1c2730)
                                } else {
                                    rgb(0x12181e)
                                })
                                .border_1()
                                .border_color(row_border)
                                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                    cx.stop_propagation();
                                })
                                .on_mouse_up(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        this.select_review_panel_file(desk_index, file_index, cx);
                                        cx.stop_propagation();
                                    }),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(if active {
                                            rgb(0xdce2e8)
                                        } else {
                                            rgb(0x9aa6af)
                                        })
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .child(path_label),
                                )
                                .into_any_element()
                        })
                        .collect()
                })
                .unwrap_or_default();
            let (diff_rows, hunk_tabs): (Vec<gpui::AnyElement>, Vec<gpui::AnyElement>) =
                match payload.as_ref() {
                    None => (
                        vec![div()
                            .p_3()
                            .text_sm()
                            .text_color(rgb(0x8e9ba5))
                            .child("No structured review payload is cached for this desk.")
                            .into_any_element()],
                        vec![],
                    ),
                    Some(p) if p.files.is_empty() => (
                        vec![div()
                            .p_3()
                            .text_sm()
                            .text_color(rgb(0x8e9ba5))
                            .child("No files in this review snapshot.")
                            .into_any_element()],
                        vec![],
                    ),
                    Some(p) => {
                        if let Some(file) = p.files.get(local.selected_file) {
                            if file.hunks.is_empty() {
                                (
                                    vec![div()
                                        .flex()
                                        .flex_col()
                                        .gap_2()
                                        .p_3()
                                        .bg(rgb(0x10171d))
                                        .border_1()
                                        .border_color(rgb(0x24313b))
                                        .rounded_lg()
                                        .child(
                                            div()
                                                .text_sm()
                                                .text_color(rgb(0xdce2e8))
                                                .child(review_file_hunkless_notice(file)),
                                        )
                                        .into_any_element()],
                                    vec![],
                                )
                            } else {
                                let hunk_index =
                                    local.selected_hunk.unwrap_or(0).min(file.hunks.len() - 1);
                                let hunk = &file.hunks[hunk_index];
                                let hunk_tabs = if file.hunks.len() <= 1 {
                                    vec![]
                                } else {
                                    file.hunks
                                        .iter()
                                        .enumerate()
                                        .map(|(hi, h)| {
                                            let active = hi == hunk_index;
                                            let label = if h.header.len() > 28 {
                                                format!("{}…", &h.header[..28])
                                            } else {
                                                h.header.clone()
                                            };
                                            let tab_border: gpui::Hsla = if active {
                                                rgb(0x7cc7ff).into()
                                            } else {
                                                rgb(0x24313b).into()
                                            };
                                            div()
                                                .px_2()
                                                .py_1()
                                                .rounded_md()
                                                .cursor_pointer()
                                                .bg(if active {
                                                    rgb(0x1c2730)
                                                } else {
                                                    rgb(0x12181e)
                                                })
                                                .border_1()
                                                .border_color(tab_border)
                                                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                                    cx.stop_propagation();
                                                })
                                                .on_mouse_up(
                                                    MouseButton::Left,
                                                    cx.listener(move |this, _, _, cx| {
                                                        this.select_review_panel_hunk(
                                                            desk_index, hi, cx,
                                                        );
                                                        cx.stop_propagation();
                                                    }),
                                                )
                                                .child(
                                                    div()
                                                        .font_family(".ZedMono")
                                                        .text_xs()
                                                        .text_color(if active {
                                                            rgb(0xdce2e8)
                                                        } else {
                                                            rgb(0x8e9ba5)
                                                        })
                                                        .child(label),
                                                )
                                                .into_any_element()
                                        })
                                        .collect::<Vec<_>>()
                                };
                                let diff_rows = vec![
                                    div()
                                        .mb_2()
                                        .font_family(".ZedMono")
                                        .text_xs()
                                        .text_color(rgb(0x7f8a94))
                                        .child(hunk.header.clone())
                                        .into_any_element(),
                                ]
                                .into_iter()
                                .chain(hunk.lines.iter().enumerate().map(|(line_index, line)| {
                                    let prefix = match line.kind {
                                        ReviewLineKind::Context => " ",
                                        ReviewLineKind::Addition => "+",
                                        ReviewLineKind::Removal => "-",
                                        ReviewLineKind::Metadata => "@",
                                    };
                                    let text_color = match line.kind {
                                        ReviewLineKind::Context => rgb(0xc8d1d8),
                                        ReviewLineKind::Addition => rgb(0x77d19a),
                                        ReviewLineKind::Removal => rgb(0xff9b88),
                                        ReviewLineKind::Metadata => rgb(0x7f8a94),
                                    };
                                    let jumpable = line.jumpable
                                        && editor_jump_line_for_review_row(hunk, line).is_some();
                                    let mut row = div()
                                        .flex()
                                        .flex_row()
                                        .gap_2()
                                        .font_family(".ZedMono")
                                        .text_xs()
                                        .text_color(text_color)
                                        .child(div().w(px(14.0)).child(prefix))
                                        .child(div().flex_1().child(line.text.clone()));
                                    if jumpable {
                                        row = row
                                            .cursor_pointer()
                                            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                                cx.stop_propagation();
                                            })
                                            .on_mouse_up(
                                                MouseButton::Left,
                                                cx.listener(move |this, _, _, cx| {
                                                    let _ = this.open_review_diff_line(
                                                        desk_index,
                                                        local.selected_file,
                                                        hunk_index,
                                                        line_index,
                                                        cx,
                                                    );
                                                    cx.stop_propagation();
                                                }),
                                            );
                                    }
                                    row.into_any_element()
                                }))
                                .collect::<Vec<_>>();
                                (diff_rows, hunk_tabs)
                            }
                        } else {
                            (
                                vec![div()
                                    .p_3()
                                    .text_sm()
                                    .text_color(rgb(0x8e9ba5))
                                    .child("Selected file is no longer in the payload.")
                                    .into_any_element()],
                                vec![],
                            )
                        }
                    }
                };
            div()
                .absolute()
                .left(px(0.0))
                .top(px(0.0))
                .w(px(viewport_width))
                .h(px(viewport_height))
                .bg(rgba(0x09101660))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        let _ = this.close_review_panel(cx);
                        cx.stop_propagation();
                    }),
                )
                .child(
                    div()
                        .absolute()
                        .top(px(SHORTCUT_PANEL_MARGIN))
                        .right(px(SHORTCUT_PANEL_MARGIN))
                        .bottom(px(SHORTCUT_PANEL_MARGIN))
                        .w(px(panel_width))
                        .flex()
                        .flex_col()
                        .gap_3()
                        .p_4()
                        .bg(rgb(0x0f151b))
                        .border_l_1()
                        .border_color(rgb(0x2c3944))
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
                                                .child("Desk review"),
                                        )
                                        .child(div().text_lg().child("Review"))
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(rgb(0x90a0aa))
                                                .child(desk_name),
                                        ),
                                )
                                .child(compact_dock_button(
                                    "Close",
                                    rgb(0xff9b88).into(),
                                    cx.listener(|this, _, _, cx| {
                                        this.close_review_panel(cx);
                                        cx.stop_propagation();
                                    }),
                                )),
                        )
                        .children(stale_rows)
                        .children(setup_rows)
                        .when_some(truncated_row, |panel, row| panel.child(row))
                        .child(
                            div()
                                .flex_1()
                                .flex()
                                .flex_row()
                                .gap_3()
                                .min_h(px(200.0))
                                .child(
                                    div()
                                        .w(px(file_list_width))
                                        .flex()
                                        .flex_col()
                                        .gap_1()
                                        .overflow_hidden()
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(rgb(0x7f8a94))
                                                .child("Files"),
                                        )
                                        .children(file_rows),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .flex()
                                        .flex_col()
                                        .gap_2()
                                        .min_w(px(0.0))
                                        .when(!hunk_tabs.is_empty(), |col| {
                                            col.child(
                                                div()
                                                    .flex()
                                                    .flex_wrap()
                                                    .gap_1()
                                                    .children(hunk_tabs),
                                            )
                                        })
                                        .child(
                                            div()
                                                .flex_1()
                                                .overflow_hidden()
                                                .p_2()
                                                .bg(rgb(0x10171d))
                                                .border_1()
                                                .border_color(rgb(0x24313b))
                                                .rounded_lg()
                                                .children(diff_rows),
                                        )
                                        .child(
                                            div()
                                                .flex()
                                                .flex_wrap()
                                                .gap_2()
                                                .when(actions_enabled, |row| {
                                                    row.child(compact_dock_button_stateful(
                                                        "Mark reviewed",
                                                        rgb(0x77d19a).into(),
                                                        true,
                                                        cx.listener(|this, _, _, cx| {
                                                            this.mark_review_selected_hunk_reviewed(cx);
                                                            cx.stop_propagation();
                                                        }),
                                                    ))
                                                    .child(compact_dock_button_stateful(
                                                        "Needs follow-up",
                                                        rgb(0xf0d35f).into(),
                                                        true,
                                                        cx.listener(|this, _, _, cx| {
                                                            this.mark_review_selected_hunk_follow_up(cx);
                                                            cx.stop_propagation();
                                                        }),
                                                    ))
                                                    .child(compact_dock_button_stateful(
                                                        "Clear",
                                                        rgb(0x7f8a94).into(),
                                                        true,
                                                        cx.listener(|this, _, _, cx| {
                                                            this.mark_review_clear_selected_hunk(cx);
                                                            cx.stop_propagation();
                                                        }),
                                                    ))
                                                })
                                                .when(!actions_enabled, |row| {
                                                    row.child(
                                                        div()
                                                            .text_xs()
                                                            .text_color(rgb(0x6f7d86))
                                                            .child(
                                                                "Hunk actions are unavailable without textual hunks.",
                                                            ),
                                                    )
                                                }),
                                        ),
                                ),
                        ),
                )
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
        let stack_surface_context_menu = open_stack_surface_menu.and_then(|menu| {
            if menu.desk_index != self.active_workdesk {
                return None;
            }
            let pane = self.active_workdesk().pane(menu.pane_id)?;
            let accent = pane
                .active_surface()
                .map(|surface| pane_accent(&surface.kind))
                .unwrap_or_else(|| rgb(0x77d19a).into());
            let menu_height = 222.0;
            let min_left = sidebar_width + 10.0;
            let max_left = (viewport_width - STACK_SURFACE_MENU_WIDTH - 12.0).max(min_left);
            let max_top = (viewport_height - menu_height - 12.0).max(12.0);
            let left = (f32::from(menu.position.x) + 10.0).clamp(min_left, max_left);
            let top = (f32::from(menu.position.y) - menu_height + 18.0).clamp(12.0, max_top);
            let pane_id = menu.pane_id;

            Some(
                div()
                    .absolute()
                    .left(px(left))
                    .top(px(top))
                    .w(px(STACK_SURFACE_MENU_WIDTH))
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
                        if this.dismiss_stack_surface_menu() {
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
                            .child(div().text_xs().text_color(accent).child("Add to stack"))
                            .child(
                                div()
                                    .text_sm()
                                    .child(pane.stack_display_title().to_string()),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(0x90a0aa))
                                    .child(surface_count_label(pane.surfaces.len())),
                            ),
                    )
                    .child(workdesk_menu_item(
                        "Shell",
                        "Stack a shell in this pane",
                        rgb(0xe59a49).into(),
                        cx.listener(move |this, _, _, cx| {
                            this.dismiss_stack_surface_menu();
                            this.stack_surface_in_pane(pane_id, PaneKind::Shell, cx);
                            cx.stop_propagation();
                        }),
                    ))
                    .child(workdesk_menu_item(
                        "Agent",
                        "Stack an agent beside this flow",
                        rgb(0x7cc7ff).into(),
                        cx.listener(move |this, _, _, cx| {
                            this.open_agent_provider_popup_for_stack(pane_id, cx);
                            cx.stop_propagation();
                        }),
                    ))
                    .child(workdesk_menu_item(
                        "Browser",
                        "Stack a browser in this pane",
                        rgb(0x77d19a).into(),
                        cx.listener(move |this, _, _, cx| {
                            this.dismiss_stack_surface_menu();
                            this.stack_surface_in_pane(pane_id, PaneKind::Browser, cx);
                            cx.stop_propagation();
                        }),
                    ))
                    .child(workdesk_menu_item(
                        "Editor",
                        "Open a file into this pane",
                        rgb(0xb4a4ff).into(),
                        cx.listener(move |this, _, _, cx| {
                            this.dismiss_stack_surface_menu();
                            this.open_editor_picker_for_target_pane(Some(pane_id), cx);
                            cx.stop_propagation();
                        }),
                    )),
            )
        });
        let agent_provider_popup_overlay = self.agent_provider_popup.as_ref().map(|popup| {
            let popup_width = 360.0_f32.min(
                (viewport_width - sidebar_width - FLOATING_DOCK_MARGIN * 2.0).max(280.0),
            );
            let left = ((viewport_width - popup_width) * 0.5).max(sidebar_width + 16.0);
            let top = (viewport_height * 0.16).max(44.0);
            let option_rows = popup
                .options
                .iter()
                .cloned()
                .map(|option| {
                    let available = option.available;
                    let profile_id = option.profile_id.clone();

                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .px_3()
                        .py_2()
                        .bg(if available { rgb(0x171f26) } else { rgb(0x13191f) })
                        .border_1()
                        .border_color(if available {
                            rgb(0x2b3641)
                        } else {
                            rgb(0x22303a)
                        })
                        .rounded_md()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                            cx.stop_propagation();
                        })
                        .when(available, |row| {
                            row.cursor_pointer().on_mouse_up(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    this.complete_agent_provider_popup_selection(&profile_id, cx);
                                    cx.stop_propagation();
                                }),
                            )
                        })
                        .child(
                            div()
                                .text_sm()
                                .text_color(if available {
                                    rgb(0x7cc7ff)
                                } else {
                                    rgb(0x6f7b85)
                                })
                                .child(option.profile_id),
                        )
                        .when_some(option.capability_note, |row, note| {
                            row.child(div().text_xs().text_color(rgb(0x90a0aa)).child(note))
                        })
                        .when_some(option.unavailable_reason, |row, reason| {
                            row.child(div().text_xs().text_color(rgb(0xff9b88)).child(reason))
                        })
                })
                .collect::<Vec<_>>();

            div()
                .absolute()
                .left(px(0.0))
                .top(px(0.0))
                .w(px(viewport_width))
                .h(px(viewport_height))
                .child(
                    div()
                        .absolute()
                        .left(px(0.0))
                        .top(px(0.0))
                        .w(px(viewport_width))
                        .h(px(viewport_height))
                        .debug_selector(|| "agent-provider-popup-backdrop".to_string())
                        .bg(rgba(0x091016b8))
                        .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                            if this.dismiss_agent_provider_popup() {
                                cx.notify();
                            }
                            cx.stop_propagation();
                        }))
                        .on_scroll_wheel(|_, _, cx| {
                            cx.stop_propagation();
                        }),
                )
                .child(
                    div()
                        .absolute()
                        .top(px(top))
                        .left(px(left))
                        .w(px(popup_width))
                        .debug_selector(|| "agent-provider-popup-panel".to_string())
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
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                            if this.dismiss_agent_provider_popup() {
                                cx.notify();
                            }
                            cx.stop_propagation();
                        }))
                        .on_scroll_wheel(|_, _, cx| {
                            cx.stop_propagation();
                        })
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(div().text_xs().text_color(rgb(0x7cc7ff)).child("Start Agent"))
                                .child(div().text_lg().child("Choose a provider"))
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(0x90a0aa))
                                        .child("Pick an installed backend before opening a new agent pane."),
                                ),
                        )
                        .child(div().flex().flex_col().gap_2().children(option_rows))
                        .when_some(popup.empty_state_message(), |panel, message| {
                            panel.child(div().text_xs().text_color(rgb(0xff9b88)).child(message))
                        })
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .child(control_button(
                                    "Cancel",
                                    rgb(0x7f8a94).into(),
                                    cx.listener(|this, _, _, cx| {
                                        if this.dismiss_agent_provider_popup() {
                                            cx.notify();
                                        }
                                        cx.stop_propagation();
                                    }),
                                )),
                        ),
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
                .absolute()
                .left(px(0.0))
                .top(px(0.0))
                .w(px(viewport_width))
                .h(px(viewport_height))
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
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap_2()
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .child(brand_icon_badge(false))
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap_1()
                                                .child(chrome_button(
                                                    "Bell",
                                                    rgb(0x7cc7ff).into(),
                                                    self.notifications_open,
                                                    Some(unread_notifications),
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
                                        ),
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
                    .when(self.sidebar_collapsed, |sidebar| {
                        sidebar
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .items_center()
                                    .gap_2()
                                    .child(brand_icon_badge(true))
                                    .child(chrome_button(
                                        "N",
                                        rgb(0x7cc7ff).into(),
                                        self.notifications_open,
                                        Some(unread_notifications),
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
                    .gap_2()
                    .px_3()
                    .py_1()
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
                            .gap_2()
                            .child(
                                div()
                                    .max_w(px(260.0))
                                    .text_xs()
                                    .text_color(if stack_controls_enabled {
                                        rgb(0x97a4ad)
                                    } else {
                                        rgb(0x697680)
                                    })
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .child(active_stack_summary),
                            )
                            .child(dock_divider())
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .child(div().text_xs().text_color(rgb(0x63707a)).child("New"))
                                    .child(compact_dock_button(
                                        "+Sh",
                                        rgb(0xe59a49).into(),
                                        cx.listener(|this, _, window, cx| {
                                            this.spawn_pane(PaneKind::Shell, window, cx);
                                            cx.stop_propagation();
                                        }),
                                    ))
                                    .child(compact_dock_button(
                                        "+Ag",
                                        rgb(0x7cc7ff).into(),
                                        cx.listener(|this, _, window, cx| {
                                            this.spawn_pane(PaneKind::Agent, window, cx);
                                            cx.stop_propagation();
                                        }),
                                    ))
                                    .child(compact_dock_button(
                                        "+Web",
                                        rgb(0x77d19a).into(),
                                        cx.listener(|this, _, window, cx| {
                                            this.spawn_pane(PaneKind::Browser, window, cx);
                                            cx.stop_propagation();
                                        }),
                                    ))
                                    .child(compact_dock_button(
                                        "+Ed",
                                        rgb(0xb4a4ff).into(),
                                        cx.listener(|this, _, _, cx| {
                                            this.open_editor_picker(cx);
                                            cx.stop_propagation();
                                        }),
                                    ))
                                    .child(compact_dock_button(
                                        "Open",
                                        rgb(0xb4a4ff).into(),
                                        cx.listener(|this, _, _, cx| {
                                            this.open_workspace_palette(
                                                WorkspacePaletteMode::OpenFile,
                                                cx,
                                            );
                                            cx.stop_propagation();
                                        }),
                                    ))
                                    .child(compact_dock_button(
                                        "Grep",
                                        rgb(0x7cc7ff).into(),
                                        cx.listener(|this, _, _, cx| {
                                            this.open_workspace_palette(
                                                WorkspacePaletteMode::SearchWorkspace,
                                                cx,
                                            );
                                            cx.stop_propagation();
                                        }),
                                    )),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
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
            .when_some(session_inspector_overlay, |root, overlay| root.child(overlay))
            .when_some(workspace_palette_overlay, |root, overlay| root.child(overlay))
            .when_some(review_panel_overlay, |root, overlay| root.child(overlay))
            .children(shortcut_overlay)
            .when_some(workdesk_editor_overlay, |root, overlay| root.child(overlay))
            .when_some(notification_overlay, |root, overlay| root.child(overlay))
            .when_some(stack_surface_context_menu, |root, menu| root.child(menu))
            .when_some(agent_provider_popup_overlay, |root, overlay| root.child(overlay))
            .when_some(workdesk.runtime_notice.clone(), |root, notice| {
                root.child(
                    div()
                        .absolute()
                        .top(px(20.0))
                        .right(px(20.0))
                        .max_w(px(420.0))
                        .px_3()
                        .py_3()
                        .bg(rgb(0x2a1a1a))
                        .border_1()
                        .border_color(rgb(0x5b3434))
                        .rounded_lg()
                        .shadow_lg()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                            cx.stop_propagation();
                        })
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                            if this.dismiss_runtime_notice() {
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
                                                .text_color(rgb(0xff9b88))
                                                .child("Runtime error"),
                                        )
                                        .child(
                                            div().text_xs().text_color(rgb(0xffd4c7)).child(notice),
                                        ),
                                )
                                .child(control_button(
                                    "Dismiss",
                                    rgb(0xffb9ab).into(),
                                    cx.listener(|this, _, _, cx| {
                                        if this.dismiss_runtime_notice() {
                                            cx.notify();
                                        }
                                        cx.stop_propagation();
                                    }),
                                )),
                        ),
                )
            })
            .when_some(workdesk_context_menu, |root, menu| root.child(menu))
    }
}

fn main() {
    let (workdesks, active_workdesk, shortcuts, boot_notice) = load_boot_state();
    let (_sender, receiver) = mpsc::channel();
    let daemon_socket = daemon_socket_path();
    let automation_server = AutomationServer {
        receiver,
        socket_path: daemon_socket,
    };
    let automation_notice = DaemonClient::default()
        .daemon_health()
        .err()
        .map(|error| SharedString::from(format!("axisd unavailable: {error}")));
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

    Application::new()
        .with_assets(AxisAppAssets::new())
        .run(move |cx: &mut App| {
            install_quit_on_last_window_closed(cx);
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
                        title: Some(SharedString::from(PRODUCT_NAME)),
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
                        AxisShell::new(
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

fn install_quit_on_last_window_closed(cx: &mut App) {
    install_quit_on_last_window_closed_with(cx, |cx| cx.quit());
}

fn install_quit_on_last_window_closed_with(
    cx: &mut App,
    mut quit: impl FnMut(&mut App) + 'static,
) {
    cx.on_window_closed(move |cx| {
        if cx.windows().is_empty() {
            quit(cx);
        }
    })
    .detach();
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
    let session_path = load_session_file_path();
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
    app_data_dir().join("session.json")
}

fn load_session_file_path() -> PathBuf {
    existing_data_file_path("session.json")
}

fn load_persisted_shortcuts() -> Result<(ShortcutMap, Option<String>), String> {
    let shortcut_path = load_shortcut_file_path();
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
    app_data_dir().join("shortcuts.json")
}

fn load_shortcut_file_path() -> PathBuf {
    existing_data_file_path("shortcuts.json")
}

fn app_data_dir() -> PathBuf {
    app_data_dir_for(app_data_dir_override())
}

fn app_data_dir_for(explicit_override: Option<PathBuf>) -> PathBuf {
    if let Some(path) = explicit_override {
        return path;
    }
    workspace_root_path().join(APP_DATA_DIR)
}

fn existing_data_file_path(file_name: &str) -> PathBuf {
    let preferred = app_data_dir().join(file_name);
    if preferred.exists() {
        return preferred;
    }
    if app_data_dir_override().is_some() {
        return preferred;
    }

    let legacy = workspace_root_path()
        .join(LEGACY_APP_DATA_DIR)
        .join(file_name);
    if legacy.exists() {
        legacy
    } else {
        preferred
    }
}

fn workspace_root_path() -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    root.canonicalize().unwrap_or(root)
}

fn app_data_dir_override() -> Option<PathBuf> {
    std::env::var_os(APP_DATA_DIR_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
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
    worktrees::create_desk_from_template(draft.name, draft.summary, template, draft.metadata)
}

fn ensure_agent_runtime_for_surface(
    bridge: &agent_sessions::AgentRuntimeBridge,
    workdesk_runtime_id: u64,
    desk: &mut WorkdeskState,
    surface_id: SurfaceId,
) -> Result<bool, String> {
    start_agent_runtime_for_surface_with_profile(
        bridge,
        workdesk_runtime_id,
        desk,
        surface_id,
        None,
        vec![],
    )
}

fn start_agent_runtime_for_surface_with_profile(
    bridge: &agent_sessions::AgentRuntimeBridge,
    workdesk_runtime_id: u64,
    desk: &mut WorkdeskState,
    surface_id: SurfaceId,
    provider_profile_id: Option<&str>,
    argv_suffix: Vec<String>,
) -> Result<bool, String> {
    let Some(surface_kind) = desk
        .panes
        .iter()
        .find_map(|pane| pane.surface(surface_id).map(|surface| surface.kind.clone()))
    else {
        return Ok(false);
    };
    if surface_kind != PaneKind::Agent
        || bridge.has_session_for_surface(workdesk_runtime_id, surface_id)
    {
        return Ok(false);
    }

    let Some(terminal) = desk.terminals.get(&surface_id).cloned() else {
        return Ok(false);
    };
    let cwd = desk
        .worktree_binding
        .as_ref()
        .map(|binding| binding.root_path.as_str())
        .unwrap_or_else(|| desk.metadata.cwd.as_str());
    match provider_profile_id {
        Some(profile_id) => bridge
            .start_agent_for_surface_with_profile(
                workdesk_runtime_id,
                &desk.workdesk_id,
                surface_id,
                cwd,
                &terminal,
                profile_id,
                argv_suffix,
            )
            .map(|_| true),
        None => bridge
            .start_agent_for_surface(
                workdesk_runtime_id,
                &desk.workdesk_id,
                surface_id,
                cwd,
                &terminal,
            )
            .map(|_| true),
    }
}

fn stop_agent_runtime_for_desk(
    bridge: &agent_sessions::AgentRuntimeBridge,
    desk: &mut WorkdeskState,
) {
    let agent_surface_ids = desk
        .panes
        .iter()
        .flat_map(|pane| pane.surfaces.iter())
        .filter(|surface| surface.kind == PaneKind::Agent)
        .map(|surface| surface.id)
        .collect::<Vec<_>>();

    for surface_id in agent_surface_ids {
        bridge.stop_surface(desk.runtime_id, surface_id);
        if let Some(terminal) = desk.terminals.get(&surface_id) {
            terminal.set_agent_metadata(None);
        }
    }
}

fn agent_runtime_baseline_attention_state(
    bridge: &agent_sessions::AgentRuntimeBridge,
    desk: &WorkdeskState,
    pane_id: PaneId,
) -> AttentionState {
    let Some(pane) = desk.panes.iter().find(|pane| pane.id == pane_id) else {
        return AttentionState::Idle;
    };

    match pane.kind {
        PaneKind::Agent => {
            let baseline = desk
                .active_terminal_session_for_pane(pane_id)
                .map(|terminal| terminal.snapshot())
                .filter(|snapshot| !snapshot.closed)
                .map(|_| AttentionState::Working)
                .unwrap_or(AttentionState::Idle);
            let session_attentions = desk
                .active_terminal_surface_id_for_pane(pane_id)
                .and_then(|surface_id| bridge.attention_for_surface(desk.runtime_id, surface_id))
                .into_iter();
            reduce_pane_attention_state(baseline, session_attentions)
        }
        PaneKind::Shell | PaneKind::Browser | PaneKind::Editor => AttentionState::Idle,
    }
}

fn sync_agent_runtime_attention_for_workdesk(
    bridge: &agent_sessions::AgentRuntimeBridge,
    desk_index: usize,
    active_workdesk: usize,
    desk: &mut WorkdeskState,
) -> bool {
    let agent_pane_ids = desk
        .panes
        .iter()
        .filter(|pane| pane.kind == PaneKind::Agent)
        .map(|pane| pane.id)
        .collect::<Vec<_>>();
    let mut changed = false;

    for pane_id in agent_pane_ids {
        let next_state = agent_runtime_baseline_attention_state(bridge, desk, pane_id);
        let unread = next_state.is_attention()
            && !(active_workdesk == desk_index && desk.active_pane == Some(pane_id));
        changed |= desk.set_pane_attention_state(pane_id, next_state, unread);
    }

    changed
}

fn worktree_id_from_desk(desk: &WorkdeskState) -> Option<WorktreeId> {
    desk.worktree_binding
        .as_ref()
        .map(|binding| binding.root_path.clone())
        .or_else(|| {
            let cwd = desk.metadata.cwd.trim();
            (!cwd.is_empty()).then_some(cwd.to_string())
        })
        .map(WorktreeId::new)
}

fn review_file_absolute_path(desk: &WorkdeskState, relative_path: &str) -> PathBuf {
    let root = desk
        .worktree_binding
        .as_ref()
        .map(|binding| binding.root_path.trim())
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(desk.metadata.cwd.trim()));
    root.join(relative_path)
}

fn clamp_review_panel_selection(desk: &mut WorkdeskState) {
    let Some(payload) = desk.review_payload_cache.as_ref() else {
        return;
    };
    if payload.files.is_empty() {
        return;
    }
    let file_count = payload.files.len();
    if desk.review_local_state.selected_file >= file_count {
        desk.review_local_state.selected_file = 0;
    }
    let file_index = desk.review_local_state.selected_file;
    let Some(file) = payload.files.get(file_index) else {
        return;
    };
    if file.hunks.is_empty() {
        desk.review_local_state.selected_hunk = None;
    } else {
        let hunk_count = file.hunks.len();
        let hunk_index = desk
            .review_local_state
            .selected_hunk
            .unwrap_or(0)
            .min(hunk_count - 1);
        desk.review_local_state.selected_hunk = Some(hunk_index);
    }
}

fn desk_has_review_entries(desk: &WorkdeskState) -> bool {
    desk.review_payload_cache
        .as_ref()
        .is_some_and(|payload| !payload.files.is_empty())
}

fn workdesk_record_from_state(desk: &WorkdeskState, workspace_root: &str) -> WorkdeskRecord {
    WorkdeskRecord {
        workdesk_id: WorkdeskId::new(desk.workdesk_id.clone()),
        workspace_root: workspace_root.to_string(),
        name: desk.name.clone(),
        summary: desk.summary.clone(),
        template: None,
        worktree_binding: desk.worktree_binding.clone(),
    }
}

fn agent_session_json(record: &AgentSessionRecord) -> Value {
    json!({
        "id": record.id,
        "provider_profile_id": record.provider_profile_id,
        "transport": record.transport,
        "workdesk_id": record.workdesk_id,
        "surface_id": record.surface_id,
        "cwd": record.cwd,
        "lifecycle": record.lifecycle,
        "attention": record.attention,
        "status_message": record.status_message,
    })
}

fn dismiss_runtime_notice_for_workdesks(
    workdesks: &mut [WorkdeskState],
    active_workdesk: usize,
) -> bool {
    workdesks
        .get_mut(active_workdesk)
        .and_then(|workdesk| workdesk.runtime_notice.take())
        .is_some()
}

fn sanitize_branch_slug(branch: &str) -> String {
    let slug = branch
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>();
    let trimmed = slug.trim_matches('-');
    if trimmed.is_empty() {
        "worktree".to_string()
    } else {
        trimmed.to_string()
    }
}

fn default_worktree_path(repo_root: &str, branch: &str) -> Result<PathBuf, String> {
    let repo_root = PathBuf::from(repo_root);
    let repo_name = repo_root
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("invalid repo root `{}`", repo_root.display()))?;
    let parent = repo_root
        .parent()
        .ok_or_else(|| format!("repo root `{}` has no parent", repo_root.display()))?;
    Ok(parent.join(format!("{repo_name}-{}", sanitize_branch_slug(branch))))
}

fn assign_missing_workdesk_ids(workdesks: &mut [WorkdeskState], next_workdesk_id: &mut u64) {
    let mut seen = HashSet::new();

    for desk in workdesks {
        let existing = desk.workdesk_id.trim();
        if !existing.is_empty() && seen.insert(existing.to_string()) {
            continue;
        }

        loop {
            let candidate = format!("desk-{}", *next_workdesk_id);
            *next_workdesk_id = next_workdesk_id.saturating_add(1);
            if seen.insert(candidate.clone()) {
                desk.workdesk_id = candidate;
                break;
            }
        }
    }
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

#[cfg(test)]
mod path_override_tests {
    use super::app_data_dir_for;
    use std::path::PathBuf;

    #[test]
    fn app_data_dir_prefers_explicit_override() {
        let override_path = PathBuf::from("/tmp/axis-smoke-data");
        assert_eq!(app_data_dir_for(Some(override_path.clone())), override_path);
    }
}

#[cfg(test)]
mod asset_tests {
    use super::{AxisAppAssets, BRAND_ICON_ASSET};
    use gpui::AssetSource;

    #[test]
    fn axis_app_assets_load_brand_icon() {
        let assets = AxisAppAssets::new();
        let bytes = assets
            .load(BRAND_ICON_ASSET)
            .expect("asset load should succeed")
            .expect("brand icon asset should exist");
        let svg = std::str::from_utf8(bytes.as_ref()).expect("brand icon should be valid utf-8");

        assert!(svg.contains("<svg"));
    }
}

#[cfg(test)]
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

#[cfg(test)]
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

fn surface_kind_rail_code(kind: &SurfaceKind) -> &'static str {
    match kind {
        SurfaceKind::Shell => "S",
        SurfaceKind::Agent => "A",
        SurfaceKind::Browser => "B",
        SurfaceKind::Editor => "E",
    }
}

fn surface_count_label(surface_count: usize) -> String {
    if surface_count == 1 {
        "1 surface".to_string()
    } else {
        format!("{surface_count} surfaces")
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
        "workdesk_id": desk.workdesk_id,
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
    navigation_target: Option<WorkdeskNavigationTarget>,
    accent: gpui::Hsla,
    listener: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
    context_listener: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
    navigation_listener: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
    edit_listener: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    menu_button_listener: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
    review_listener: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
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
    let meta_line = if let Some(ref w) = desk.worktree_binding {
        format!(
            "{} · {} panes",
            worktrees::format_compact_worktree_line(w),
            desk.panes.len()
        )
    } else {
        let cwd_label = compact_cwd_label(&desk.metadata.cwd);
        let branch_label = desk
            .metadata
            .branch
            .trim()
            .is_empty()
            .then_some("no-branch".to_string())
            .unwrap_or_else(|| desk.metadata.branch.clone());
        format!("{cwd_label} · {branch_label} · {} panes", desk.panes.len())
    };
    let status_label = desk.status_label();
    let progress_label = desk.progress_label();
    let review_summary = desk.review_summary.clone();
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
    let has_navigation_target = navigation_target.is_some();
    let navigation_label = navigation_target.as_ref().map(|target| {
        if target.mode == WorkdeskNavigationMode::Attention {
            "Next up"
        } else if is_active {
            "Now"
        } else {
            "Resume"
        }
    });
    let navigation_button_label = navigation_target.as_ref().map(|target| match target.mode {
        WorkdeskNavigationMode::Attention => "Open",
        WorkdeskNavigationMode::Resume => "Resume",
    });
    let navigation_tint = navigation_target
        .as_ref()
        .map(|target| {
            if target.mode == WorkdeskNavigationMode::Attention {
                target.state.tint()
            } else {
                accent
            }
        })
        .unwrap_or(accent);

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
                                .min_w(px(40.0))
                                .h(px(22.0))
                                .px_2()
                                .cursor_pointer()
                                .bg(rgb(0x171d24))
                                .border_1()
                                .border_color(rgb(0x2b3641))
                                .rounded_md()
                                .on_mouse_down(MouseButton::Left, edit_listener)
                                .child(div().text_xs().text_color(rgb(0x9da8b1)).child("Edit")),
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
        .when_some(review_summary, |card, review| {
            let review_tint = if review.ready_for_review {
                rgb(0x77d19a).into()
            } else if review.dirty {
                rgb(0xf0d35f).into()
            } else {
                rgb(0x7f8a94).into()
            };
            let review_files = review_changed_file_preview(&review, 3);
            let file_count_label = if review.changed_files.len() == 1 {
                "1 file".to_string()
            } else {
                format!("{} files", review.changed_files.len())
            };
            card.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .child(context_pill(review_status_label(&review), review_tint))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .child(context_pill(file_count_label, rgb(0x7f8a94).into()))
                                    .when(review.ahead > 0 || review.behind > 0, |row| {
                                        row.child(context_pill(
                                            format!("↑{} ↓{}", review.ahead, review.behind),
                                            rgb(0x7f8a94).into(),
                                        ))
                                    }),
                            ),
                    )
                    .children(review_files.into_iter().map(|path| {
                        div()
                            .text_xs()
                            .text_color(rgb(0x9aa6af))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .child(path)
                    })),
            )
        })
        .when(desk_has_review_entries(desk), |card| {
                card.child(
                    div()
                        .flex()
                        .justify_end()
                        .child(control_button(
                            "Review",
                            rgb(0x7cc7ff).into(),
                            review_listener,
                        )),
                )
            },
        )
        .when_some(navigation_target.clone(), |card, target| {
            card.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .overflow_hidden()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(navigation_tint)
                                    .child(navigation_label.unwrap_or("Resume")),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .overflow_hidden()
                                    .child(attention_indicator(target.state, target.unread))
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(rgb(0xd5dde3))
                                            .overflow_hidden()
                                            .whitespace_nowrap()
                                            .child(target.label),
                                    ),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(0x7f8a94))
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .child(target.detail),
                            ),
                    )
                    .child(control_button(
                        navigation_button_label.unwrap_or("Open"),
                        navigation_tint,
                        navigation_listener,
                    )),
            )
        })
        .when(!has_navigation_target, |card| {
            card.child(
                div().flex().items_center().justify_between().gap_2().child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(attention_indicator(focus_attention, false))
                        .child(div().text_xs().text_color(rgb(0x7f8a94)).child(focus_label)),
                ),
            )
        })
}

fn workdesk_compact_chip(
    index: usize,
    desk: &WorkdeskState,
    is_active: bool,
    accent: gpui::Hsla,
    listener: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
    context_listener: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
    review_enabled: bool,
    review_listener: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let attention_summary = desk.workdesk_attention_summary();
    let border = if is_active {
        accent
    } else if attention_summary.unread_count > 0 {
        attention_summary.highest.tint()
    } else {
        rgb(0x24313b).into()
    };
    let review_button_id = SharedString::from(format!("workdesk-compact-review-{index}"));
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
        .when(review_enabled, |chip| {
            chip.child(
                div()
                    .id(review_button_id)
                    .debug_selector(|| "workdesk-compact-review".to_string())
                    .px_1()
                    .py(px(1.0))
                    .rounded_md()
                    .bg(rgb(0x171d24))
                    .border_1()
                    .border_color(accent)
                    .cursor_pointer()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .on_mouse_up(MouseButton::Left, review_listener)
                    .child(div().text_xs().text_color(accent).child("R")),
            )
        })
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
    compact_dock_button_stateful(label, accent, true, listener)
}

fn compact_dock_button_stateful(
    label: &str,
    accent: gpui::Hsla,
    enabled: bool,
    listener: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let label = label.to_string();
    let background = if enabled {
        rgb(0x161d24)
    } else {
        rgb(0x12181e)
    };
    let border: gpui::Hsla = if enabled {
        rgb(0x2b3641).into()
    } else {
        rgb(0x22303a).into()
    };
    let text_color = if enabled {
        accent
    } else {
        rgb(0x56626c).into()
    };

    div()
        .flex()
        .items_center()
        .justify_center()
        .min_w(px(36.0))
        .px_2()
        .py_1()
        .bg(background)
        .border_1()
        .border_color(border)
        .rounded_md()
        .when(enabled, |button| {
            button
                .cursor_pointer()
                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                    cx.stop_propagation();
                })
                .on_mouse_up(MouseButton::Left, listener)
        })
        .child(div().text_xs().text_color(text_color).child(label))
}

fn brand_icon_badge(compact: bool) -> impl IntoElement {
    let badge_size = if compact { 30.0 } else { 34.0 };

    div()
        .flex()
        .items_center()
        .justify_center()
        .w(px(badge_size))
        .h(px(badge_size))
        .child(img(BRAND_ICON_ASSET).size(px(badge_size)))
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
    context: &str,
    accent: gpui::Hsla,
    unread: bool,
    listener: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let title = title.to_string();
    let detail = detail.to_string();
    let context = context.to_string();

    div()
        .cursor_pointer()
        .flex()
        .flex_col()
        .gap_1()
        .p_3()
        .bg(if unread { rgb(0x131c25) } else { rgb(0x10171d) })
        .border_1()
        .border_color(if unread { accent } else { rgb(0x24313b).into() })
        .rounded_lg()
        .on_mouse_down(MouseButton::Left, |_, _, cx| {
            cx.stop_propagation();
        })
        .on_mouse_up(MouseButton::Left, listener)
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
        .child(div().text_xs().text_color(rgb(0x7f8a94)).child(context))
        .child(div().text_xs().text_color(rgb(0x95a3ad)).child(detail))
}

fn workspace_palette_result_row(
    result: &WorkspacePaletteResult,
    selected: bool,
    listener: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let (title, detail, meta) = match result {
        WorkspacePaletteResult::File(file) => (
            file.relative_path.clone(),
            file.absolute_path.clone(),
            "file".to_string(),
        ),
        WorkspacePaletteResult::SearchMatch {
            relative_path,
            line_number,
            preview,
            ..
        } => (
            format!("{relative_path}:{line_number}"),
            preview.clone(),
            "match".to_string(),
        ),
    };

    div()
        .cursor_pointer()
        .flex()
        .flex_col()
        .gap_1()
        .p_3()
        .bg(if selected { rgb(0x17212a) } else { rgb(0x10171d) })
        .border_1()
        .border_color(if selected {
            rgb(0x7cc7ff)
        } else {
            rgb(0x24313b)
        })
        .rounded_lg()
        .on_mouse_down(MouseButton::Left, |_, _, cx| {
            cx.stop_propagation();
        })
        .on_mouse_up(MouseButton::Left, listener)
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_3()
                .child(
                    div()
                        .text_sm()
                        .text_color(rgb(0xdce2e8))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .child(title),
                )
                .child(context_pill(meta, rgb(0x7f8a94).into())),
        )
        .child(
            div()
                .text_xs()
                .text_color(rgb(0x8e9ba5))
                .overflow_hidden()
                .whitespace_nowrap()
                .child(detail),
        )
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

fn agent_timeline_state_tint(state_label: &str, pending: bool) -> gpui::Hsla {
    if pending {
        return rgb(0xf0d35f).into();
    }
    match state_label {
        "completed" | "approved" => rgb(0x77d19a).into(),
        "running" | "streaming" => rgb(0x7cc7ff).into(),
        "failed" | "denied" | "cancelled" => rgb(0xff9b88).into(),
        _ => rgb(0x90a0aa).into(),
    }
}

fn agent_inspector_action_button(
    label: &str,
    detail: &str,
    accent: gpui::Hsla,
    enabled: bool,
    listener: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let label = label.to_string();
    let detail = detail.to_string();
    let background = if enabled { rgb(0x16212a) } else { rgb(0x11181e) };
    let border = if enabled { accent } else { rgb(0x293742).into() };
    let label_tint = if enabled { accent } else { rgb(0x67737d).into() };
    let detail_tint = if enabled {
        rgb(0xd5dee6)
    } else {
        rgb(0x6f7b85)
    };

    div()
        .flex()
        .flex_col()
        .gap_1()
        .px_3()
        .py_2()
        .bg(background)
        .border_1()
        .border_color(border)
        .rounded_lg()
        .on_mouse_down(MouseButton::Left, |_, _, cx| {
            cx.stop_propagation();
        })
        .when(enabled, |button| {
            button
                .cursor_pointer()
                .on_mouse_up(MouseButton::Left, listener)
        })
        .child(div().text_sm().text_color(label_tint).child(label))
        .child(div().text_xs().text_color(detail_tint).child(detail))
}

fn agent_timeline_entry_card(entry: AgentTimelineEntryView) -> impl IntoElement {
    let tint = agent_timeline_state_tint(&entry.state_label, entry.pending);
    let title = entry.title;
    let body = if entry.body.trim().is_empty() {
        "No details provided.".to_string()
    } else {
        entry.body
    };
    let state_label = entry.state_label;

    div()
        .flex()
        .flex_col()
        .gap_2()
        .p_3()
        .bg(rgb(0x10171d))
        .border_1()
        .border_color(rgb(0x24313b))
        .rounded_lg()
        .child(
            div()
                .flex()
                .justify_between()
                .items_center()
                .gap_3()
                .child(div().text_sm().text_color(rgb(0xe4ebf1)).child(title))
                .child(context_pill(state_label, tint)),
        )
        .child(div().text_xs().text_color(rgb(0xc9d3dc)).child(body))
}

fn agent_pending_approval_card(
    approval: PendingApprovalView,
    actionable: bool,
    approve_listener: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
    deny_listener: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_3()
        .p_3()
        .bg(rgb(0x15110f))
        .border_1()
        .border_color(rgb(0x5b4330))
        .rounded_lg()
        .child(div().text_sm().text_color(rgb(0xf0d35f)).child(approval.title))
        .child(div().text_xs().text_color(rgb(0xe4d4c5)).child(approval.details))
        .when(actionable, |card| {
            card.child(
                div()
                    .flex()
                    .gap_2()
                    .child(agent_inspector_action_button(
                        "Approve",
                        "Allow this request",
                        rgb(0x77d19a).into(),
                        true,
                        approve_listener,
                    ))
                    .child(agent_inspector_action_button(
                        "Deny",
                        "Reject this request",
                        rgb(0xff9b88).into(),
                        true,
                        deny_listener,
                    )),
            )
        })
        .when(!actionable, |card| {
            card.child(
                div()
                    .text_xs()
                    .text_color(rgb(0x8e9ba5))
                    .child("Approval responses are not available for this provider."),
            )
        })
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

#[cfg(test)]
fn next_attention_target_for_workdesks(workdesks: &[WorkdeskState]) -> Option<(usize, PaneId)> {
    next_attention_workdesk_target(workdesks.iter().enumerate().map(|(desk_index, desk)| {
        (
            desk_index,
            desk.panes
                .iter()
                .map(|pane| (pane.id, desk.pane_attention(pane.id))),
        )
    }))
}

fn next_attention_target_for_workdesk(desk: &WorkdeskState) -> Option<PaneId> {
    next_attention_pane_target(
        desk.panes
            .iter()
            .map(|pane| (pane.id, desk.pane_attention(pane.id))),
    )
}

fn workdesk_navigation_target(desk: &WorkdeskState) -> Option<WorkdeskNavigationTarget> {
    let (pane_id, mode) = next_attention_target_for_workdesk(desk)
        .map(|pane_id| (pane_id, WorkdeskNavigationMode::Attention))
        .or_else(|| {
            desk.active_pane
                .map(|pane_id| (pane_id, WorkdeskNavigationMode::Resume))
        })
        .or_else(|| {
            desk.panes
                .last()
                .map(|pane| (pane.id, WorkdeskNavigationMode::Resume))
        })?;
    let pane = desk.pane(pane_id)?;
    let attention = desk.pane_attention(pane_id);
    let active_surface_kind = pane
        .active_surface()
        .map(|surface| surface_kind_slug(&surface.kind))
        .unwrap_or("surface");
    let detail = match mode {
        WorkdeskNavigationMode::Attention => format!(
            "{} · {} · {}",
            attention.state.label(),
            active_surface_kind,
            surface_count_label(pane.surfaces.len())
        ),
        WorkdeskNavigationMode::Resume => {
            format!(
                "{active_surface_kind} · {}",
                surface_count_label(pane.surfaces.len())
            )
        }
    };

    Some(WorkdeskNavigationTarget {
        pane_id,
        state: attention.state,
        unread: attention.unread,
        mode,
        label: pane.stack_display_title().to_string(),
        detail,
    })
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

fn infer_attention_state_from_terminal_status(
    pane_kind: &PaneKind,
    status: Option<&str>,
    closed: bool,
) -> AttentionState {
    let status = status.unwrap_or("Running");
    let normalized = status.to_ascii_lowercase();

    if normalized.contains("error")
        || normalized.contains("failed")
        || normalized.contains("exited via")
        || (normalized.contains("exited with code") && !normalized.contains("code 0"))
    {
        return AttentionState::Error;
    }

    if normalized.contains("needs input")
        || normalized.contains("awaiting input")
        || normalized.contains("user input")
    {
        return AttentionState::NeedsInput;
    }

    if closed || normalized.contains("exited with code 0") || normalized.contains("terminated") {
        return AttentionState::NeedsReview;
    }

    match pane_kind {
        PaneKind::Agent => AttentionState::Working,
        PaneKind::Shell | PaneKind::Browser | PaneKind::Editor => AttentionState::Idle,
    }
}

fn pane_frame_intersects_viewport(
    frame: PaneViewportFrame,
    sidebar_width: f32,
    viewport_width: f32,
    viewport_height: f32,
) -> bool {
    let viewport_left = sidebar_width;
    let viewport_right = viewport_width;
    let viewport_top = 0.0;
    let viewport_bottom = viewport_height;

    frame.x < viewport_right
        && frame.x + frame.width > viewport_left
        && frame.y < viewport_bottom
        && frame.y + frame.height > viewport_top
}

fn surface_stack_rail_button(
    surface: &SurfaceRecord,
    index: usize,
    active: bool,
    accent: gpui::Hsla,
    listener: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let border = if active { accent } else { rgb(0x24303a).into() };
    let background = if active { rgb(0x19222a) } else { rgb(0x10161c) };
    let text_color = if active { accent } else { rgb(0x86929c).into() };
    let label = format!("{}{}", surface_kind_rail_code(&surface.kind), index + 1);

    div()
        .relative()
        .flex()
        .items_center()
        .justify_center()
        .w(px(28.0))
        .h(px(22.0))
        .cursor_pointer()
        .bg(background)
        .border_1()
        .border_color(border)
        .rounded_sm()
        .font_family(".ZedMono")
        .on_mouse_down(MouseButton::Left, |_, _, cx| {
            cx.stop_propagation();
        })
        .on_mouse_up(MouseButton::Left, listener)
        .child(div().text_xs().text_color(text_color).child(label))
        .when(surface.dirty, |button| {
            button.child(
                div()
                    .absolute()
                    .right(px(2.0))
                    .bottom(px(2.0))
                    .w(px(4.0))
                    .h(px(4.0))
                    .rounded_full()
                    .bg(rgb(0xf0d35f)),
            )
        })
}

fn surface_stack_rail_action_button(
    label: &str,
    accent: gpui::Hsla,
    active: bool,
    listener: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let border = if active { accent } else { rgb(0x24303a).into() };
    let background = if active { rgb(0x162029) } else { rgb(0x10161c) };

    div()
        .flex()
        .items_center()
        .justify_center()
        .w(px(28.0))
        .h(px(22.0))
        .cursor_pointer()
        .bg(background)
        .border_1()
        .border_color(border)
        .rounded_sm()
        .font_family(".ZedMono")
        .on_mouse_down(MouseButton::Left, |_, _, cx| {
            cx.stop_propagation();
        })
        .on_mouse_up(MouseButton::Left, listener)
        .child(div().text_xs().text_color(accent).child(label.to_string()))
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
        axis_editor::LanguageKind::Plaintext => "Plaintext",
        axis_editor::LanguageKind::Rust => "Rust",
        axis_editor::LanguageKind::JavaScript => "JavaScript",
        axis_editor::LanguageKind::TypeScript => "TypeScript",
        axis_editor::LanguageKind::Tsx => "TSX",
        axis_editor::LanguageKind::Jsx => "JSX",
        axis_editor::LanguageKind::Json => "JSON",
        axis_editor::LanguageKind::Toml => "TOML",
        axis_editor::LanguageKind::Yaml => "YAML",
        axis_editor::LanguageKind::Markdown => "Markdown",
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
    let highlights = editor.highlight_line(line_index);
    let selection_intersects =
        !selection.is_empty() && ranges_intersect(&selection, &line_range, true);
    let search_intersects = search_match
        .as_ref()
        .is_some_and(|range| ranges_intersect(range, &line_range, true));
    let cursor_on_line =
        show_cursor && cursor_offset >= line_range.start && cursor_offset <= line_range.end;

    if !selection_intersects && !search_intersects && !cursor_on_line {
        return editor_static_line_display(line_text, &highlights);
    }

    if display_text.is_empty() || append_cursor_cell {
        display_text.push(' ');
    }

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

        push_editor_text_run(&mut runs, ch.len_utf8(), highlight, color, background);
    }

    StyledText::new(display_text).with_runs(runs)
}

fn editor_static_line_display(
    line_text: &str,
    highlights: &[axis_editor::HighlightSpan],
) -> StyledText {
    if line_text.is_empty() {
        let mut runs = Vec::new();
        push_editor_text_run(
            &mut runs,
            1,
            HighlightKind::Plain,
            editor_text_color_for_kind(HighlightKind::Plain),
            None,
        );
        return StyledText::new(" ").with_runs(runs);
    }

    let mut cursor = 0usize;
    let mut runs = Vec::new();
    for span in highlights {
        let start = span.range.start.min(line_text.len());
        if cursor < start {
            push_editor_text_run(
                &mut runs,
                start - cursor,
                HighlightKind::Plain,
                editor_text_color_for_kind(HighlightKind::Plain),
                None,
            );
        }

        let end = span.range.end.min(line_text.len());
        if end > start {
            push_editor_text_run(
                &mut runs,
                end - start,
                span.kind,
                editor_text_color_for_kind(span.kind),
                None,
            );
            cursor = end;
        }
    }

    if cursor < line_text.len() {
        push_editor_text_run(
            &mut runs,
            line_text.len() - cursor,
            HighlightKind::Plain,
            editor_text_color_for_kind(HighlightKind::Plain),
            None,
        );
    }

    StyledText::new(line_text.to_string()).with_runs(runs)
}

fn push_editor_text_run(
    runs: &mut Vec<TextRun>,
    len: usize,
    kind: HighlightKind,
    color: gpui::Hsla,
    background: Option<gpui::Hsla>,
) {
    if len == 0 {
        return;
    }

    let font = editor_font_for_kind(kind);
    if let Some(last) = runs.last_mut() {
        if last.font == font
            && last.color == color
            && last.background_color == background
            && last.underline.is_none()
            && last.strikethrough.is_none()
        {
            last.len += len;
            return;
        }
    }

    runs.push(TextRun {
        len,
        font,
        color,
        background_color: background,
        underline: None,
        strikethrough: None,
    });
}

fn ranges_intersect(left: &Range<usize>, right: &Range<usize>, inclusive_cursor_end: bool) -> bool {
    if inclusive_cursor_end && left.start == left.end {
        left.start >= right.start && left.start <= right.end
    } else {
        left.start < right.end && left.end > right.start
    }
}

fn approx_eq_f32(left: f32, right: f32) -> bool {
    (left - right).abs() <= 0.25
}

fn bounds_approx_eq(left: Bounds<Pixels>, right: Bounds<Pixels>) -> bool {
    approx_eq_f32(f32::from(left.left()), f32::from(right.left()))
        && approx_eq_f32(f32::from(left.top()), f32::from(right.top()))
        && approx_eq_f32(f32::from(left.size.width), f32::from(right.size.width))
        && approx_eq_f32(f32::from(left.size.height), f32::from(right.size.height))
}

fn terminal_header_height_for_surface_count(surface_count: usize) -> f32 {
    if surface_count > 1 {
        32.0
    } else {
        30.0
    }
}

fn pane_stack_rail_width(surface_count: usize) -> f32 {
    if surface_count == 0 {
        0.0
    } else {
        40.0
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
    let horizontal_chrome = pane_stack_rail_width(surface_count) + TERMINAL_BODY_INSET * 2.0 + 24.0;
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
    let stack_rail_width = pane_stack_rail_width(surface_count) * frame.zoom.clamp(0.82, 1.2);
    let content_width =
        (frame.width - pane_padding * 2.0 - TERMINAL_BODY_INSET * 2.0 - stack_rail_width).max(1.0);
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
    let row_has_selection = selection.is_some_and(|selection| selection.affects_row(row_index));

    if !show_cursor && !row_has_selection {
        return terminal_row_fast_element(row, metrics);
    }

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
                    .font_weight(FontWeight::NORMAL)
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

fn terminal_row_fast_element(row: &TerminalRow, metrics: TerminalTextMetrics) -> gpui::AnyElement {
    let runs = row
        .runs
        .iter()
        .filter(|run| !run.text.is_empty())
        .map(|run| terminal_run_element(run, metrics))
        .collect::<Vec<_>>();

    div()
        .h(px(metrics.line_height))
        .flex()
        .items_start()
        .whitespace_nowrap()
        .children(runs)
        .into_any_element()
}

fn terminal_run_element(run: &TerminalRun, metrics: TerminalTextMetrics) -> gpui::AnyElement {
    let underline_color = run.style.underline_color.map(terminal_color_hsla);
    let background = run.style.background.map(terminal_color_hsla);
    let color = terminal_text_color_from_style(run.style);
    let cell_count = terminal_run_cell_count(&run.text).max(1);

    div()
        .w(px(metrics.cell_width * cell_count as f32))
        .h(px(metrics.line_height))
        .overflow_hidden()
        .font_family(TERMINAL_FONT_FAMILY)
        .font_weight(FontWeight::NORMAL)
        .text_size(px(metrics.font_size))
        .line_height(px(metrics.line_height))
        .text_color(color)
        .when_some(background, |cell, background| cell.bg(background))
        .when(run.style.underline, |cell| {
            let cell = cell.underline().text_decoration_solid();
            if let Some(color) = underline_color {
                cell.text_decoration_color(color)
            } else {
                cell
            }
        })
        .when(run.style.strikethrough, |cell| cell.line_through())
        .child(terminal_run_display_text(&run.text))
        .into_any_element()
}

fn terminal_run_cell_count(text: &str) -> usize {
    text.chars().count()
}

fn terminal_run_display_text(text: &str) -> String {
    text.chars()
        .map(|ch| if ch == ' ' { '\u{00A0}' } else { ch })
        .collect()
}

fn terminal_row_text(row: &TerminalRow) -> String {
    row.runs
        .iter()
        .map(|run| run.text.as_str())
        .collect::<String>()
}

fn terminal_text_color_from_style(style: axis_terminal::TerminalTextStyle) -> gpui::Hsla {
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
    stack_rail_width: f32,
    snapshot: &TerminalSnapshot,
) -> TerminalFrameMetrics {
    let body_origin_x = screen_x + stack_rail_width + pane_padding + TERMINAL_BODY_INSET;
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
    use axis_core::automation::AutomationResponse;
    use axis_core::paths::AXIS_SOCKET_PATH_ENV;
    use axis_agent_runtime::adapters::fake::FakeProvider;
    use axis_agent_runtime::ProviderRegistry;
    use gpui::TestAppContext;
    use std::cell::Cell;
    use std::ffi::OsString;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;
    use std::rc::Rc;
    use std::sync::{Mutex, OnceLock};
    use std::thread;

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    struct EnvVarGuard {
        key: &'static str,
        original: Option<OsString>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
            let original = std::env::var_os(key);
            std::env::set_var(key, value);
            Self { key, original }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            if let Some(value) = &self.original {
                std::env::set_var(self.key, value);
            } else {
                std::env::remove_var(self.key);
            }
        }
    }

    fn run_git(repo: &Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .status()
            .expect("spawn git");
        assert!(status.success(), "git {:?} failed", args);
    }

    fn init_repo_with_main(repo: &Path) {
        run_git(repo, &["init", "-b", "main"]);
        run_git(repo, &["config", "user.email", "axis-test@example.com"]);
        run_git(repo, &["config", "user.name", "axis test"]);
        fs::write(repo.join("README.md"), "hello\n").expect("fixture file should write");
        run_git(repo, &["add", "README.md"]);
        run_git(repo, &["commit", "-m", "init"]);
    }

    fn start_fake_daemon_failure_server(
        socket_path: PathBuf,
        error: impl Into<String>,
    ) -> thread::JoinHandle<()> {
        let error = error.into();
        if socket_path.exists() {
            let _ = fs::remove_file(&socket_path);
        }
        let listener = UnixListener::bind(&socket_path).expect("fake daemon should bind");
        listener
            .set_nonblocking(true)
            .expect("fake daemon should become nonblocking");
        thread::spawn(move || {
            let mut handled = 0usize;
            let mut last_activity = std::time::Instant::now();
            loop {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        handled += 1;
                        last_activity = std::time::Instant::now();
                        let mut line = String::new();
                        {
                            let mut reader = BufReader::new(&mut stream);
                            reader
                                .read_line(&mut line)
                                .expect("fake daemon should read request");
                        }
                        let payload = serde_json::to_vec(&AutomationResponse::failure(error.clone()))
                            .expect("fake daemon response should serialize");
                        stream
                            .write_all(&payload)
                            .expect("fake daemon response should write");
                        stream
                            .write_all(b"\n")
                            .expect("fake daemon newline should write");
                        stream.flush().expect("fake daemon response should flush");
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        if handled > 0 && last_activity.elapsed() > std::time::Duration::from_millis(250)
                        {
                            break;
                        }
                        thread::sleep(std::time::Duration::from_millis(10));
                    }
                    Err(err) => panic!("fake daemon accept failed: {err}"),
                }
            }
            let _ = fs::remove_file(&socket_path);
        })
    }

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
            workdesk_id: String::new(),
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
            "axis-editor-test-{}-{}.rs",
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
    fn persisted_workdesk_rehydrates_worktree_binding_from_metadata() {
        let path = std::env::temp_dir().join(format!(
            "axis-worktree-binding-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time should be after epoch")
                .as_nanos()
        ));
        fs::create_dir_all(&path).expect("fixture directory should exist");
        let path_string = path.display().to_string();

        let mut state = WorkdeskState::new(
            "Desk",
            "Summary",
            vec![single_surface_pane(
                13,
                "Agent",
                PaneKind::Agent,
                WorkdeskPoint::new(0.0, 0.0),
                WorkdeskSize::new(720.0, 420.0),
            )],
        );
        state.metadata = WorkdeskMetadata {
            intent: "Ship it".to_string(),
            cwd: path_string.clone(),
            branch: "topic".to_string(),
            status: None,
            progress: None,
        };

        let restored = PersistedWorkdesk::from_state(&state).into_state();

        assert_eq!(
            restored.worktree_binding,
            Some(worktrees::binding_from_desk_paths(path_string, "topic"))
        );

        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn persisted_workdesk_round_trips_stable_workdesk_id() {
        let mut state = blank_workdesk("Desk", "Summary");
        state.workdesk_id = "desk-42".to_string();

        let restored = PersistedWorkdesk::from_state(&state).into_state();

        assert_eq!(restored.workdesk_id, "desk-42");
    }

    #[test]
    fn automation_workdesk_summary_uses_stable_workdesk_id() {
        let mut desk = blank_workdesk("Desk", "Summary");
        desk.workdesk_id = "desk-7".to_string();

        let payload = automation_workdesk_summary_json(0, &desk, true);

        assert_eq!(payload["workdesk_id"], "desk-7");
    }

    #[test]
    fn assign_missing_workdesk_ids_replaces_missing_and_duplicate_ids() {
        let mut first = blank_workdesk("One", "Summary");
        first.workdesk_id = "desk-1".to_string();
        let second = blank_workdesk("Two", "Summary");
        let mut third = blank_workdesk("Three", "Summary");
        third.workdesk_id = "desk-1".to_string();

        let mut workdesks = vec![first, second, third];
        let mut next_workdesk_id = 2;
        assign_missing_workdesk_ids(&mut workdesks, &mut next_workdesk_id);

        assert_eq!(workdesks[0].workdesk_id, "desk-1");
        assert_eq!(workdesks[1].workdesk_id, "desk-2");
        assert_eq!(workdesks[2].workdesk_id, "desk-3");
        assert_eq!(next_workdesk_id, 4);
    }

    #[gpui::test]
    async fn review_summary_propagates_ambiguous_daemon_error_instead_of_falling_back(
        cx: &mut TestAppContext,
    ) {
        let _env_guard = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = std::env::temp_dir().join(format!(
            "axis-review-daemon-error-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock should be available")
                .as_nanos()
        ));
        let socket_token = format!(
            "{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock should be available")
                .as_nanos()
        );
        fs::create_dir_all(&temp).expect("temp root should exist");
        let repo = temp.join("repo");
        fs::create_dir_all(&repo).expect("repo dir should exist");
        init_repo_with_main(&repo);
        let repo_root = repo.display().to_string();

        let daemon_socket = PathBuf::from(format!("/tmp/axisd-review-{socket_token}.sock"));
        let _socket_guard = EnvVarGuard::set(AXIS_SOCKET_PATH_ENV, &daemon_socket);
        let daemon = start_fake_daemon_failure_server(
            daemon_socket,
            format!("ambiguous review base branch for worktree `{repo_root}`"),
        );

        let (shell, window) = cx.add_window_view(|_, view_cx| {
            let mut first = blank_workdesk("Desk A", "Summary");
            first.workdesk_id = "desk-a".to_string();
            first.metadata.cwd = repo_root.clone();
            first.worktree_binding = Some(WorktreeBinding {
                root_path: repo_root.clone(),
                branch: "main".to_string(),
                base_branch: Some("main".to_string()),
                ahead: 0,
                behind: 0,
                dirty: false,
            });

            let mut second = blank_workdesk("Desk B", "Summary");
            second.workdesk_id = "desk-b".to_string();
            second.metadata.cwd = repo_root.clone();
            second.worktree_binding = Some(WorktreeBinding {
                root_path: repo_root.clone(),
                branch: "main".to_string(),
                base_branch: Some("develop".to_string()),
                ahead: 0,
                behind: 0,
                dirty: false,
            });

            AxisShell::new_with_agent_runtime(
                vec![first, second],
                0,
                ShortcutMap::default(),
                None,
                automation::start_automation_server_at(PathBuf::from(format!(
                    "/tmp/axis-app-review-{socket_token}.sock"
                )))
                    .expect("automation server should start"),
                view_cx.focus_handle(),
                SharedString::from(""),
                SharedString::from(""),
                agent_sessions::AgentRuntimeBridge::new(),
            )
        });

        window.update(|_, cx| {
            shell.update(cx, |shell, view_cx| {
                for desk in &mut shell.workdesks {
                    desk.review_payload_cache = None;
                    desk.review_summary = None;
                }
                let response = shell.handle_automation_request(
                    SharedAutomationRequest::DeskReviewSummary {
                        worktree_id: WorktreeId::new(repo_root.clone()),
                    },
                    view_cx,
                );

                assert!(!response.ok, "semantic daemon errors should not fall back");
                assert!(response.result.is_none());
                let error = response.error.expect("failure should include error");
                assert!(
                    error.contains("ambiguous review base branch"),
                    "unexpected error: {error}"
                );
                assert!(
                    shell.workdesks
                        .iter()
                        .all(|desk| desk.review_payload_cache.is_none()),
                    "app path should not synthesize a fallback payload on semantic daemon errors"
                );
                assert!(
                    shell.workdesks
                        .iter()
                        .all(|desk| desk.review_summary.is_none()),
                    "app path should not silently project a fallback review summary"
                );
            });
        });

        daemon.join().expect("fake daemon thread should exit");
        let _ = fs::remove_dir_all(temp);
    }

    #[gpui::test]
    async fn attention_transition_enqueues_unread_notification(cx: &mut TestAppContext) {
        let (shell, window) = cx.add_window_view(|_, view_cx| {
            let mut desk = WorkdeskState::new(
                "Desk",
                "Summary",
                vec![
                    single_surface_pane(
                        1,
                        "Build",
                        PaneKind::Browser,
                        WorkdeskPoint::new(0.0, 0.0),
                        WorkdeskSize::new(720.0, 420.0),
                    ),
                    single_surface_pane(
                        2,
                        "Review Agent",
                        PaneKind::Browser,
                        WorkdeskPoint::new(960.0, 0.0),
                        WorkdeskSize::new(720.0, 420.0),
                    ),
                ],
            );
            desk.active_pane = Some(PaneId::new(1));
            AxisShell::new_with_agent_runtime(
                vec![desk],
                0,
                ShortcutMap::default(),
                None,
                automation::start_automation_server_at(std::env::temp_dir().join(format!(
                    "notif-attn-{}-{}.sock",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .expect("clock should be available")
                        .as_nanos()
                )))
                .expect("automation server should start"),
                view_cx.focus_handle(),
                SharedString::from(""),
                SharedString::from(""),
                agent_sessions::AgentRuntimeBridge::new(),
            )
        });

        window.update(|_, cx| {
            shell.update(cx, |shell, view_cx| {
                assert!(shell.set_pane_attention(
                    0,
                    PaneId::new(2),
                    AttentionState::NeedsReview,
                    true,
                    true,
                    view_cx
                ));
                assert_eq!(shell.notification_unread_count(), 1);
                assert_eq!(shell.notifications.items.len(), 1);
                let notification = shell
                    .notifications
                    .items
                    .last()
                    .expect("notification should be recorded");
                assert!(notification.unread);
                assert_eq!(notification.state, AttentionState::NeedsReview);
                assert_eq!(notification.workdesk_index, 0);
                assert_eq!(notification.pane_id, Some(PaneId::new(2)));
            });
        });
    }

    #[gpui::test]
    async fn opening_notifications_marks_attention_events_read(cx: &mut TestAppContext) {
        let (shell, window) = cx.add_window_view(|_, view_cx| {
            let mut desk = WorkdeskState::new(
                "Desk",
                "Summary",
                vec![
                    single_surface_pane(
                        1,
                        "Build",
                        PaneKind::Browser,
                        WorkdeskPoint::new(0.0, 0.0),
                        WorkdeskSize::new(720.0, 420.0),
                    ),
                    single_surface_pane(
                        2,
                        "Review Agent",
                        PaneKind::Browser,
                        WorkdeskPoint::new(960.0, 0.0),
                        WorkdeskSize::new(720.0, 420.0),
                    ),
                ],
            );
            desk.active_pane = Some(PaneId::new(1));
            AxisShell::new_with_agent_runtime(
                vec![desk],
                0,
                ShortcutMap::default(),
                None,
                automation::start_automation_server_at(std::env::temp_dir().join(format!(
                    "notif-read-{}-{}.sock",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .expect("clock should be available")
                        .as_nanos()
                )))
                .expect("automation server should start"),
                view_cx.focus_handle(),
                SharedString::from(""),
                SharedString::from(""),
                agent_sessions::AgentRuntimeBridge::new(),
            )
        });

        window.update(|_, cx| {
            shell.update(cx, |shell, view_cx| {
                assert!(shell.set_pane_attention(
                    0,
                    PaneId::new(2),
                    AttentionState::NeedsReview,
                    true,
                    true,
                    view_cx
                ));
                assert_eq!(shell.notification_unread_count(), 1);

                shell.toggle_notifications(view_cx);

                assert!(shell.notifications_open);
                assert_eq!(shell.notification_unread_count(), 0);
                assert!(shell.notifications.items.iter().all(|item| !item.unread));
            });
        });
    }

    #[gpui::test]
    async fn spawn_agent_shortcut_opens_popup_before_creating_pane(cx: &mut TestAppContext) {
        use std::sync::Arc;

        let window = cx.add_empty_window();
        let shell = window.update(|_, cx| {
            let mut registry = ProviderRegistry::new();
            registry.register("alpha", Arc::new(FakeProvider::with_standard_script()));
            let bridge = agent_sessions::AgentRuntimeBridge::with_registry_and_options(
                "alpha",
                registry,
                vec![agent_sessions::ProviderProfileOption {
                    profile_id: "alpha".to_string(),
                    capability_note: None,
                    available: true,
                    unavailable_reason: None,
                }],
            );

            cx.new(|view_cx| {
                AxisShell::new_with_agent_runtime(
                    vec![blank_workdesk("Desk", "Summary")],
                    0,
                    ShortcutMap::default(),
                    None,
                    automation::start_automation_server_at(std::env::temp_dir().join(format!(
                        "axis-popup-test-{}-{}.sock",
                        std::process::id(),
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .expect("clock should be available")
                            .as_nanos()
                    )))
                    .expect("automation server should start"),
                    view_cx.focus_handle(),
                    SharedString::from(""),
                    SharedString::from(""),
                    bridge,
                )
            })
        });

        window.update(|window, cx| {
            shell.update(cx, |shell, view_cx| {
                let before = shell.active_workdesk().panes.len();
                assert!(shell.execute_shortcut_action(
                    ShortcutAction::SpawnAgentPane,
                    window,
                    view_cx
                ));
                assert_eq!(shell.active_workdesk().panes.len(), before);
                assert!(shell.agent_provider_popup.is_some());
            });
        });
    }

    #[gpui::test]
    async fn popup_selection_starts_requested_provider_for_stack_target(
        cx: &mut TestAppContext,
    ) {
        use std::sync::Arc;

        let window = cx.add_empty_window();
        let shell = window.update(|_, cx| {
            let mut registry = ProviderRegistry::new();
            registry.register("alpha", Arc::new(FakeProvider::with_standard_script()));
            registry.register("beta", Arc::new(FakeProvider::with_standard_script()));
            let bridge = agent_sessions::AgentRuntimeBridge::with_registry_and_options(
                "alpha",
                registry,
                vec![
                    agent_sessions::ProviderProfileOption {
                        profile_id: "alpha".to_string(),
                        capability_note: None,
                        available: true,
                        unavailable_reason: None,
                    },
                    agent_sessions::ProviderProfileOption {
                        profile_id: "beta".to_string(),
                        capability_note: Some("basic lifecycle only".to_string()),
                        available: true,
                        unavailable_reason: None,
                    },
                ],
            );
            let mut desk = blank_workdesk("Desk", "Summary");
            desk.metadata.cwd = std::env::current_dir()
                .expect("cwd should resolve")
                .display()
                .to_string();

            cx.new(|view_cx| {
                AxisShell::new_with_agent_runtime(
                    vec![desk],
                    0,
                    ShortcutMap::default(),
                    None,
                    automation::start_automation_server_at(std::env::temp_dir().join(format!(
                        "axis-popup-stack-test-{}-{}.sock",
                        std::process::id(),
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .expect("clock should be available")
                            .as_nanos()
                    )))
                    .expect("automation server should start"),
                    view_cx.focus_handle(),
                    SharedString::from(""),
                    SharedString::from(""),
                    bridge,
                )
            })
        });

        window.update(|window, cx| {
            shell.update(cx, |shell, view_cx| {
                shell.spawn_pane(PaneKind::Shell, window, view_cx);
                let before = shell.active_workdesk().panes.len();
                let runtime_id = shell.active_workdesk().runtime_id;
                let pane_id = shell
                    .active_workdesk()
                    .panes
                    .first()
                    .map(|pane| pane.id)
                    .expect("existing pane should exist");

                shell.open_agent_provider_popup_for_stack(pane_id, view_cx);
                assert!(shell.complete_agent_provider_popup_selection("beta", view_cx));
                assert_eq!(shell.active_workdesk().panes.len(), before);
                assert!(shell.agent_provider_popup.is_none());

                let pane = shell
                    .active_workdesk()
                    .pane(pane_id)
                    .expect("stack target pane should remain");
                let record = shell
                    .agent_runtime
                    .session_for_surface(runtime_id, pane.active_surface_id)
                    .expect("runtime session should exist");
                assert_eq!(record.provider_profile_id, "beta");
                assert_eq!(pane.id, pane_id);

                shutdown_workdesk_terminals(shell.active_workdesk_mut());
            });
        });
    }

    #[test]
    fn popup_selection_starts_requested_provider_for_new_pane() {
        let mut registry = ProviderRegistry::new();
        registry.register(
            "alpha",
            std::sync::Arc::new(FakeProvider::with_standard_script()),
        );
        registry.register(
            "beta",
            std::sync::Arc::new(FakeProvider::with_standard_script()),
        );

        let bridge = agent_sessions::AgentRuntimeBridge::with_registry_and_options(
            "alpha",
            registry,
            vec![
                agent_sessions::ProviderProfileOption {
                    profile_id: "alpha".to_string(),
                    capability_note: None,
                    available: true,
                    unavailable_reason: None,
                },
                agent_sessions::ProviderProfileOption {
                    profile_id: "beta".to_string(),
                    capability_note: Some("basic lifecycle only".to_string()),
                    available: true,
                    unavailable_reason: None,
                },
            ],
        );

        let mut workdesks = vec![blank_workdesk("Desk", "Summary")];
        workdesks[0].runtime_id = 88;
        workdesks[0].metadata.cwd = std::env::current_dir()
            .expect("cwd should resolve")
            .display()
            .to_string();
        let mut active_workdesk = 0;

        let (_pane_id, surface_id) = AxisShell::spawn_agent_surface_on_workdesk_state_with_profile(
            &mut workdesks,
            &mut active_workdesk,
            0,
            None,
            "beta",
            WorkdeskPoint::new(640.0, 360.0),
            true,
            &bridge,
        )
        .expect("selected provider should launch a new agent pane");

        let record = bridge
            .session_for_surface(workdesks[0].runtime_id, surface_id)
            .expect("runtime session should exist");
        assert_eq!(record.provider_profile_id, "beta");

        shutdown_workdesk_terminals(&mut workdesks[0]);
    }

    #[gpui::test]
    async fn popup_selection_from_shortcut_creates_new_agent_pane(cx: &mut TestAppContext) {
        use std::sync::Arc;

        let window = cx.add_empty_window();
        let shell = window.update(|_, cx| {
            let mut registry = ProviderRegistry::new();
            registry.register("alpha", Arc::new(FakeProvider::with_standard_script()));
            registry.register("beta", Arc::new(FakeProvider::with_standard_script()));
            let bridge = agent_sessions::AgentRuntimeBridge::with_registry_and_options(
                "alpha",
                registry,
                vec![
                    agent_sessions::ProviderProfileOption {
                        profile_id: "alpha".to_string(),
                        capability_note: None,
                        available: true,
                        unavailable_reason: None,
                    },
                    agent_sessions::ProviderProfileOption {
                        profile_id: "beta".to_string(),
                        capability_note: Some("basic lifecycle only".to_string()),
                        available: true,
                        unavailable_reason: None,
                    },
                ],
            );
            let mut desk = blank_workdesk("Desk", "Summary");
            desk.metadata.cwd = std::env::current_dir()
                .expect("cwd should resolve")
                .display()
                .to_string();

            cx.new(|view_cx| {
                AxisShell::new_with_agent_runtime(
                    vec![desk],
                    0,
                    ShortcutMap::default(),
                    None,
                    automation::start_automation_server_at(std::env::temp_dir().join(format!(
                        "axp-{}-{}.sock",
                        std::process::id(),
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .expect("clock should be available")
                            .as_nanos()
                    )))
                    .expect("automation server should start"),
                    view_cx.focus_handle(),
                    SharedString::from(""),
                    SharedString::from(""),
                    bridge,
                )
            })
        });

        window.update(|window, cx| {
            shell.update(cx, |shell, view_cx| {
                let before = shell.active_workdesk().panes.len();
                let runtime_id = shell.active_workdesk().runtime_id;
                assert!(shell.execute_shortcut_action(
                    ShortcutAction::SpawnAgentPane,
                    window,
                    view_cx
                ));
                assert!(shell.complete_agent_provider_popup_selection("beta", view_cx));
                assert_eq!(shell.active_workdesk().panes.len(), before + 1);
                assert!(shell.agent_provider_popup.is_none());

                let surface_id = shell
                    .active_workdesk()
                    .panes
                    .last()
                    .and_then(|pane| pane.surfaces.last())
                    .map(|surface| surface.id)
                    .expect("new pane should contain an agent surface");
                let record = shell
                    .agent_runtime
                    .session_for_surface(runtime_id, surface_id)
                    .expect("runtime session should exist");
                assert_eq!(record.provider_profile_id, "beta");

                shutdown_workdesk_terminals(shell.active_workdesk_mut());
            });
        });
    }

    #[gpui::test]
    async fn dismissing_popup_keeps_pane_count_unchanged(cx: &mut TestAppContext) {
        use std::sync::Arc;

        let window = cx.add_empty_window();
        let shell = window.update(|_, cx| {
            let mut registry = ProviderRegistry::new();
            registry.register("alpha", Arc::new(FakeProvider::with_standard_script()));
            let bridge = agent_sessions::AgentRuntimeBridge::with_registry_and_options(
                "alpha",
                registry,
                vec![agent_sessions::ProviderProfileOption {
                    profile_id: "alpha".to_string(),
                    capability_note: None,
                    available: true,
                    unavailable_reason: None,
                }],
            );
            let mut desk = blank_workdesk("Desk", "Summary");
            desk.metadata.cwd = std::env::current_dir()
                .expect("cwd should resolve")
                .display()
                .to_string();

            cx.new(|view_cx| {
                AxisShell::new_with_agent_runtime(
                    vec![desk],
                    0,
                    ShortcutMap::default(),
                    None,
                    automation::start_automation_server_at(std::env::temp_dir().join(format!(
                        "axis-popup-dismiss-test-{}-{}.sock",
                        std::process::id(),
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .expect("clock should be available")
                            .as_nanos()
                    )))
                    .expect("automation server should start"),
                    view_cx.focus_handle(),
                    SharedString::from(""),
                    SharedString::from(""),
                    bridge,
                )
            })
        });

        window.update(|window, cx| {
            shell.update(cx, |shell, view_cx| {
                let before = shell.active_workdesk().panes.len();

                shell.open_agent_provider_popup_for_new_pane(window, view_cx);
                assert!(shell.agent_provider_popup.is_some());
                assert!(shell.dismiss_agent_provider_popup());
                assert_eq!(shell.active_workdesk().panes.len(), before);
                assert!(shell.agent_provider_popup.is_none());
            });
        });
    }

    #[gpui::test]
    async fn popup_dismisses_on_escape_without_side_effects(cx: &mut TestAppContext) {
        use std::sync::Arc;

        let (shell, window) = cx.add_window_view(|_, view_cx| {
            let mut registry = ProviderRegistry::new();
            registry.register("alpha", Arc::new(FakeProvider::with_standard_script()));
            let bridge = agent_sessions::AgentRuntimeBridge::with_registry_and_options(
                "alpha",
                registry,
                vec![agent_sessions::ProviderProfileOption {
                    profile_id: "alpha".to_string(),
                    capability_note: None,
                    available: true,
                    unavailable_reason: None,
                }],
            );
            let mut desk = blank_workdesk("Desk", "Summary");
            desk.metadata.cwd = std::env::current_dir()
                .expect("cwd should resolve")
                .display()
                .to_string();

            AxisShell::new_with_agent_runtime(
                vec![desk],
                0,
                ShortcutMap::default(),
                None,
                automation::start_automation_server_at(std::env::temp_dir().join(format!(
                    "apesc-{}-{}.sock",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .expect("clock should be available")
                        .as_nanos()
                )))
                .expect("automation server should start"),
                view_cx.focus_handle(),
                SharedString::from(""),
                SharedString::from(""),
                bridge,
            )
        });

        let before = window.update(|window, cx| {
            window.activate_window();
            shell.update(cx, |shell, view_cx| {
                window.focus(&shell.focus_handle);
                let before = shell.active_workdesk().panes.len();
                assert!(shell.execute_shortcut_action(
                    ShortcutAction::SpawnAgentPane,
                    window,
                    view_cx
                ));
                assert_eq!(shell.active_workdesk().panes.len(), before);
                assert!(shell.agent_provider_popup.is_some());
                assert!(shell.agent_runtime.sessions_snapshot().is_empty());
                before
            })
        });
        window.run_until_parked();
        assert!(window.debug_bounds("agent-provider-popup-panel").is_some());

        window.simulate_keystrokes("escape");

        shell.read_with(window, |shell, _| {
            assert!(shell.agent_provider_popup.is_none());
            assert_eq!(shell.active_workdesk().panes.len(), before);
            assert!(shell.agent_runtime.sessions_snapshot().is_empty());
        });
    }

    #[gpui::test]
    async fn popup_dismisses_on_outside_click_without_side_effects(cx: &mut TestAppContext) {
        use std::sync::Arc;

        let (shell, window) = cx.add_window_view(|_, view_cx| {
            let mut registry = ProviderRegistry::new();
            registry.register("alpha", Arc::new(FakeProvider::with_standard_script()));
            let bridge = agent_sessions::AgentRuntimeBridge::with_registry_and_options(
                "alpha",
                registry,
                vec![agent_sessions::ProviderProfileOption {
                    profile_id: "alpha".to_string(),
                    capability_note: None,
                    available: true,
                    unavailable_reason: None,
                }],
            );
            let mut desk = blank_workdesk("Desk", "Summary");
            desk.metadata.cwd = std::env::current_dir()
                .expect("cwd should resolve")
                .display()
                .to_string();

            AxisShell::new_with_agent_runtime(
                vec![desk],
                0,
                ShortcutMap::default(),
                None,
                automation::start_automation_server_at(std::env::temp_dir().join(format!(
                    "apout-{}-{}.sock",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .expect("clock should be available")
                        .as_nanos()
                )))
                .expect("automation server should start"),
                view_cx.focus_handle(),
                SharedString::from(""),
                SharedString::from(""),
                bridge,
            )
        });

        let before = window.update(|window, cx| {
            window.activate_window();
            shell.update(cx, |shell, view_cx| {
                window.focus(&shell.focus_handle);
                let before = shell.active_workdesk().panes.len();
                assert!(shell.execute_shortcut_action(
                    ShortcutAction::SpawnAgentPane,
                    window,
                    view_cx
                ));
                assert_eq!(shell.active_workdesk().panes.len(), before);
                assert!(shell.agent_provider_popup.is_some());
                assert!(shell.agent_runtime.sessions_snapshot().is_empty());
                before
            })
        });
        window.run_until_parked();
        let backdrop_bounds = window
            .debug_bounds("agent-provider-popup-backdrop")
            .expect("popup backdrop should render");
        let panel_bounds = window
            .debug_bounds("agent-provider-popup-panel")
            .expect("popup panel should render");
        let click_position = GpuiPoint::new(
            backdrop_bounds.origin.x + px(4.0),
            backdrop_bounds.origin.y + px(4.0),
        );
        assert!(!panel_bounds.contains(&click_position));

        window.simulate_click(click_position, gpui::Modifiers::none());

        shell.read_with(window, |shell, _| {
            assert!(shell.agent_provider_popup.is_none());
            assert_eq!(shell.active_workdesk().panes.len(), before);
            assert!(shell.agent_runtime.sessions_snapshot().is_empty());
        });
    }

    #[test]
    fn agent_pane_without_explicit_profile_uses_default_provider() {
        let mut registry = ProviderRegistry::new();
        registry.register(
            "alpha",
            std::sync::Arc::new(FakeProvider::with_standard_script()),
        );
        registry.register(
            "beta",
            std::sync::Arc::new(FakeProvider::with_standard_script()),
        );

        let bridge = agent_sessions::AgentRuntimeBridge::with_registry_and_options(
            "alpha",
            registry,
            vec![
                agent_sessions::ProviderProfileOption {
                    profile_id: "alpha".to_string(),
                    capability_note: None,
                    available: true,
                    unavailable_reason: None,
                },
                agent_sessions::ProviderProfileOption {
                    profile_id: "beta".to_string(),
                    capability_note: Some("basic lifecycle only".to_string()),
                    available: true,
                    unavailable_reason: None,
                },
            ],
        );

        let mut workdesks = vec![blank_workdesk("Desk", "Summary")];
        workdesks[0].runtime_id = 89;
        workdesks[0].metadata.cwd = std::env::current_dir()
            .expect("cwd should resolve")
            .display()
            .to_string();
        let mut active_workdesk = 0;

        let (_pane_id, surface_id) = AxisShell::spawn_surface_on_workdesk_state(
            &mut workdesks,
            &mut active_workdesk,
            0,
            None,
            PaneKind::Agent,
            None,
            None,
            None,
            true,
            &bridge,
        )
        .expect("default provider should launch a new agent pane");

        let record = bridge
            .session_for_surface(workdesks[0].runtime_id, surface_id)
            .expect("runtime session should exist");
        assert_eq!(record.provider_profile_id, "alpha");

        shutdown_workdesk_terminals(&mut workdesks[0]);
    }

    #[test]
    fn ensure_agent_runtime_for_surface_starts_lazily_and_tags_terminal() {
        let mut registry = ProviderRegistry::new();
        registry.register(
            "fake",
            std::sync::Arc::new(FakeProvider::with_standard_script()),
        );
        let bridge = agent_sessions::AgentRuntimeBridge::with_registry("fake", registry);

        let surface_id = SurfaceId::new(41);
        let mut desk = WorkdeskState::new(
            "Desk",
            "Summary",
            vec![single_surface_pane_with_ids(
                PaneId::new(1),
                surface_id,
                "Agent",
                PaneKind::Agent,
                WorkdeskPoint::new(0.0, 0.0),
                WorkdeskSize::new(720.0, 420.0),
            )],
        );
        desk.runtime_id = 7;
        desk.metadata.cwd = std::env::current_dir()
            .expect("cwd should resolve")
            .display()
            .to_string();
        desk.attach_terminal_session(
            surface_id,
            &PaneKind::Agent,
            "Agent",
            terminal_grid_size_for_pane(WorkdeskSize::new(720.0, 420.0), 1),
        );

        let terminal = desk
            .terminals
            .get(&surface_id)
            .cloned()
            .expect("agent terminal should be attached");
        assert!(terminal.agent_metadata().is_none());
        assert_eq!(
            bridge.attention_for_surface(desk.runtime_id, surface_id),
            None
        );

        let started =
            ensure_agent_runtime_for_surface(&bridge, desk.runtime_id, &mut desk, surface_id)
                .expect("restored agent surface should attach on demand");
        assert!(started);
        assert_eq!(
            bridge.attention_for_surface(desk.runtime_id, surface_id),
            Some(axis_core::agent::AgentAttention::Quiet)
        );
        assert!(terminal
            .agent_metadata()
            .expect("terminal should receive session metadata")
            .session_id
            .0
            .starts_with("fake-session-"));

        let started_again =
            ensure_agent_runtime_for_surface(&bridge, desk.runtime_id, &mut desk, surface_id)
                .expect("second attach should be a no-op");
        assert!(!started_again);

        shutdown_workdesk_terminals(&mut desk);
    }

    #[test]
    fn ensure_agent_runtime_for_surface_scopes_sessions_per_workdesk() {
        let mut registry = ProviderRegistry::new();
        registry.register(
            "fake",
            std::sync::Arc::new(FakeProvider::with_standard_script()),
        );
        let bridge = agent_sessions::AgentRuntimeBridge::with_registry("fake", registry);

        let surface_id = SurfaceId::new(41);
        let mut left = WorkdeskState::new(
            "Left",
            "Summary",
            vec![single_surface_pane_with_ids(
                PaneId::new(1),
                surface_id,
                "Agent",
                PaneKind::Agent,
                WorkdeskPoint::new(0.0, 0.0),
                WorkdeskSize::new(720.0, 420.0),
            )],
        );
        left.runtime_id = 11;
        left.metadata.cwd = std::env::current_dir()
            .expect("cwd should resolve")
            .display()
            .to_string();
        left.attach_terminal_session(
            surface_id,
            &PaneKind::Agent,
            "Agent",
            terminal_grid_size_for_pane(WorkdeskSize::new(720.0, 420.0), 1),
        );

        let mut right = WorkdeskState::new(
            "Right",
            "Summary",
            vec![single_surface_pane_with_ids(
                PaneId::new(1),
                surface_id,
                "Agent",
                PaneKind::Agent,
                WorkdeskPoint::new(960.0, 0.0),
                WorkdeskSize::new(720.0, 420.0),
            )],
        );
        right.runtime_id = 12;
        right.metadata.cwd = std::env::current_dir()
            .expect("cwd should resolve")
            .display()
            .to_string();
        right.attach_terminal_session(
            surface_id,
            &PaneKind::Agent,
            "Agent",
            terminal_grid_size_for_pane(WorkdeskSize::new(720.0, 420.0), 1),
        );

        assert!(
            ensure_agent_runtime_for_surface(&bridge, left.runtime_id, &mut left, surface_id)
                .expect("left desk should start")
        );
        assert!(ensure_agent_runtime_for_surface(
            &bridge,
            right.runtime_id,
            &mut right,
            surface_id
        )
        .expect("right desk should start independently"));

        let left_session = left
            .terminals
            .get(&surface_id)
            .and_then(|terminal| terminal.agent_metadata())
            .expect("left terminal should tag its session")
            .session_id;
        let right_session = right
            .terminals
            .get(&surface_id)
            .and_then(|terminal| terminal.agent_metadata())
            .expect("right terminal should tag its session")
            .session_id;
        assert_ne!(left_session, right_session);

        shutdown_workdesk_terminals(&mut left);
        shutdown_workdesk_terminals(&mut right);
    }

    #[test]
    fn stop_agent_runtime_for_desk_clears_bridge_sessions() {
        let mut registry = ProviderRegistry::new();
        registry.register(
            "fake",
            std::sync::Arc::new(FakeProvider::with_standard_script()),
        );
        let bridge = agent_sessions::AgentRuntimeBridge::with_registry("fake", registry);

        let surface_id = SurfaceId::new(51);
        let mut desk = WorkdeskState::new(
            "Desk",
            "Summary",
            vec![single_surface_pane_with_ids(
                PaneId::new(1),
                surface_id,
                "Agent",
                PaneKind::Agent,
                WorkdeskPoint::new(0.0, 0.0),
                WorkdeskSize::new(720.0, 420.0),
            )],
        );
        desk.runtime_id = 21;
        desk.metadata.cwd = std::env::current_dir()
            .expect("cwd should resolve")
            .display()
            .to_string();
        desk.attach_terminal_session(
            surface_id,
            &PaneKind::Agent,
            "Agent",
            terminal_grid_size_for_pane(WorkdeskSize::new(720.0, 420.0), 1),
        );

        assert!(
            ensure_agent_runtime_for_surface(&bridge, desk.runtime_id, &mut desk, surface_id)
                .expect("desk should start")
        );
        assert!(bridge.has_session_for_surface(desk.runtime_id, surface_id));

        stop_agent_runtime_for_desk(&bridge, &mut desk);

        assert!(!bridge.has_session_for_surface(desk.runtime_id, surface_id));
        shutdown_workdesk_terminals(&mut desk);
    }

    #[test]
    fn agent_runtime_attention_sync_marks_unfocused_agent_pane_unread() {
        let mut registry = ProviderRegistry::new();
        registry.register(
            "fake",
            std::sync::Arc::new(FakeProvider::with_standard_script()),
        );
        let bridge = agent_sessions::AgentRuntimeBridge::with_registry("fake", registry);

        let pane_id = PaneId::new(1);
        let surface_id = SurfaceId::new(61);
        let mut desk = WorkdeskState::new(
            "Desk",
            "Summary",
            vec![single_surface_pane_with_ids(
                pane_id,
                surface_id,
                "Agent",
                PaneKind::Agent,
                WorkdeskPoint::new(0.0, 0.0),
                WorkdeskSize::new(720.0, 420.0),
            )],
        );
        desk.runtime_id = 31;
        desk.metadata.cwd = std::env::current_dir()
            .expect("cwd should resolve")
            .display()
            .to_string();
        desk.active_pane = Some(pane_id);
        desk.attach_terminal_session(
            surface_id,
            &PaneKind::Agent,
            "Agent",
            terminal_grid_size_for_pane(WorkdeskSize::new(720.0, 420.0), 1),
        );

        assert!(
            ensure_agent_runtime_for_surface(&bridge, desk.runtime_id, &mut desk, surface_id)
                .expect("desk should start")
        );
        bridge
            .poll_surface(desk.runtime_id, surface_id)
            .expect("starting poll should succeed");
        bridge
            .poll_surface(desk.runtime_id, surface_id)
            .expect("running poll should succeed");
        bridge
            .poll_surface(desk.runtime_id, surface_id)
            .expect("attention poll should succeed");
        assert_eq!(
            bridge.attention_for_surface(desk.runtime_id, surface_id),
            Some(axis_core::agent::AgentAttention::NeedsReview)
        );

        assert!(sync_agent_runtime_attention_for_workdesk(
            &bridge, 1, 0, &mut desk
        ));
        let attention = desk.pane_attention(pane_id);
        assert_eq!(attention.state, AttentionState::NeedsReview);
        assert!(attention.unread);
        assert_eq!(next_attention_target_for_workdesk(&desk), Some(pane_id));

        shutdown_workdesk_terminals(&mut desk);
    }

    #[test]
    fn terminal_snapshot_preview_lines_flattens_runs_and_trims_padding() {
        let style = axis_terminal::TerminalTextStyle {
            foreground: TerminalColor::new(0xd5, 0xdd, 0xe4),
            background: None,
            underline_color: None,
            bold: false,
            italic: false,
            faint: false,
            underline: false,
            strikethrough: false,
        };
        let snapshot = TerminalSnapshot {
            title: "Agent".to_string(),
            rows: vec![
                TerminalRow {
                    runs: vec![
                        TerminalRun {
                            text: "cargo ".to_string(),
                            style,
                        },
                        TerminalRun {
                            text: "test".to_string(),
                            style,
                        },
                    ],
                },
                TerminalRow {
                    runs: vec![TerminalRun {
                        text: "\u{00A0}\u{00A0}".to_string(),
                        style,
                    }],
                },
                TerminalRow {
                    runs: vec![TerminalRun {
                        text: "done  ".to_string(),
                        style,
                    }],
                },
            ],
            theme: axis_terminal::TerminalTheme::default(),
            cursor: (0, 0),
            cursor_blinking: false,
            cols: 80,
            rows_count: 3,
            scrollbar: axis_terminal::TerminalScrollbar::default(),
            alternate_screen: false,
            application_cursor: false,
            closed: false,
            status: None,
        };

        assert_eq!(
            terminal_snapshot_preview_lines(&snapshot, 8),
            vec!["cargo test".to_string(), "done".to_string()]
        );
    }

    #[test]
    fn agent_session_inspector_view_returns_runtime_session_details() {
        let mut registry = ProviderRegistry::new();
        registry.register(
            "fake",
            std::sync::Arc::new(FakeProvider::with_standard_script()),
        );
        let bridge = agent_sessions::AgentRuntimeBridge::with_registry("fake", registry);

        let pane_id = PaneId::new(1);
        let surface_id = SurfaceId::new(61);
        let mut desk = WorkdeskState::new(
            "Desk",
            "Summary",
            vec![single_surface_pane_with_ids(
                pane_id,
                surface_id,
                "Agent",
                PaneKind::Agent,
                WorkdeskPoint::new(0.0, 0.0),
                WorkdeskSize::new(720.0, 420.0),
            )],
        );
        desk.runtime_id = 31;
        desk.metadata.cwd = std::env::current_dir()
            .expect("cwd should resolve")
            .display()
            .to_string();
        desk.attach_terminal_session(
            surface_id,
            &PaneKind::Agent,
            "Agent",
            terminal_grid_size_for_pane(WorkdeskSize::new(720.0, 420.0), 1),
        );

        assert!(
            ensure_agent_runtime_for_surface(&bridge, desk.runtime_id, &mut desk, surface_id)
                .expect("desk should start")
        );
        let record = bridge
            .session_for_surface(desk.runtime_id, surface_id)
            .expect("session should exist");
        bridge
            .send_turn(&record.id, "Continue with the test.")
            .expect("send_turn should succeed");

        let view = agent_session_inspector_view(&bridge, &desk, surface_id)
            .expect("agent session inspector view should exist");
        assert_eq!(view.provider_profile_id, "fake");
        assert_eq!(view.workdesk_name, "Desk");
        assert_eq!(view.pane_title, "Agent");
        assert_eq!(view.surface_id, surface_id);
        assert!(!view.session_id.is_empty());
        assert!(view.can_send_turn);
        assert!(view.can_resume);
        assert_eq!(view.timeline_entries.len(), 1);
        assert_eq!(view.timeline_entries[0].title, "User turn");
        assert_eq!(view.timeline_entries[0].body, "Continue with the test.");

        shutdown_workdesk_terminals(&mut desk);
    }

    #[test]
    fn quick_open_results_orders_matches_by_position_then_path_length() {
        let files = vec![
            WorkspaceFileCandidate {
                absolute_path: "/tmp/project/src/main.rs".to_string(),
                relative_path: "src/main.rs".to_string(),
            },
            WorkspaceFileCandidate {
                absolute_path: "/tmp/project/docs/main-guide.md".to_string(),
                relative_path: "docs/main-guide.md".to_string(),
            },
            WorkspaceFileCandidate {
                absolute_path: "/tmp/project/src/feature_main.rs".to_string(),
                relative_path: "src/feature_main.rs".to_string(),
            },
        ];

        let results = quick_open_results(&files, "main");
        let relative_paths = results
            .into_iter()
            .map(|result| match result {
                WorkspacePaletteResult::File(file) => file.relative_path,
                WorkspacePaletteResult::SearchMatch { .. } => {
                    panic!("quick open should only return file matches")
                }
            })
            .collect::<Vec<_>>();

        assert_eq!(
            relative_paths,
            vec![
                "src/main.rs".to_string(),
                "docs/main-guide.md".to_string(),
                "src/feature_main.rs".to_string(),
            ]
        );
    }

    #[test]
    fn workspace_search_results_collects_relative_paths_and_line_numbers() {
        let root = std::env::temp_dir().join(format!(
            "axis-workspace-search-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time should be after epoch")
                .as_nanos()
        ));
        fs::create_dir_all(root.join("src")).expect("fixture directory should exist");
        fs::write(root.join("src/lib.rs"), "alpha\nneedle here\nomega\n")
            .expect("fixture file should write");
        fs::write(root.join("README.md"), "needle in docs\n").expect("fixture file should write");

        let results = workspace_search_results(&root, "needle");

        assert!(results.iter().any(|result| matches!(
            result,
            WorkspacePaletteResult::SearchMatch {
                relative_path,
                line_number,
                preview,
                ..
            } if relative_path == "src/lib.rs"
                && *line_number == 2
                && preview == "needle here"
        )));
        assert!(results.iter().any(|result| matches!(
            result,
            WorkspacePaletteResult::SearchMatch {
                relative_path,
                line_number,
                preview,
                ..
            } if relative_path == "README.md"
                && *line_number == 1
                && preview == "needle in docs"
        )));

        let _ = fs::remove_dir_all(root);
    }

    fn review_test_summary() -> axis_core::worktree::ReviewSummary {
        axis_core::worktree::ReviewSummary {
            files_changed: 1,
            uncommitted_files: 0,
            ready_for_review: true,
            last_inspected_at_ms: Some(1),
        }
    }

    fn review_payload_single_text_file(root: &str) -> DeskReviewPayload {
        use axis_core::review::{
            ReviewFileChangeKind, ReviewFileDiff, ReviewHunk, ReviewLine,
        };
        DeskReviewPayload {
            worktree_id: WorktreeId::new(root.to_string()),
            summary: review_test_summary(),
            files: vec![ReviewFileDiff {
                path: "src/demo.rs".to_string(),
                old_path: None,
                change_kind: ReviewFileChangeKind::Modified,
                added_lines: 1,
                removed_lines: 1,
                truncated: false,
                hunks: vec![ReviewHunk {
                    header: "@@ -1,3 +1,3 @@".to_string(),
                    old_start: 1,
                    old_lines: 3,
                    new_start: 1,
                    new_lines: 3,
                    anchor_new_line: Some(2),
                    truncated: false,
                    lines: vec![
                        ReviewLine::context(Some(1), Some(1), true, "first line"),
                        ReviewLine::removed(Some(2), None, true, "removed line"),
                        ReviewLine::added(None, Some(2), true, "second line"),
                    ],
                }],
            }],
            truncated: false,
        }
    }

    fn review_payload_hunkless_file(root: &str) -> DeskReviewPayload {
        use axis_core::review::{ReviewFileChangeKind, ReviewFileDiff};
        DeskReviewPayload {
            worktree_id: WorktreeId::new(root.to_string()),
            summary: axis_core::worktree::ReviewSummary {
                files_changed: 1,
                uncommitted_files: 0,
                ready_for_review: false,
                last_inspected_at_ms: Some(1),
            },
            files: vec![ReviewFileDiff {
                path: "data.bin".to_string(),
                old_path: None,
                change_kind: ReviewFileChangeKind::Modified,
                added_lines: 0,
                removed_lines: 0,
                truncated: false,
                hunks: vec![],
            }],
            truncated: false,
        }
    }

    fn review_payload_removal_anchor(root: &str) -> DeskReviewPayload {
        use axis_core::review::{
            ReviewFileChangeKind, ReviewFileDiff, ReviewHunk, ReviewLine,
        };
        DeskReviewPayload {
            worktree_id: WorktreeId::new(root.to_string()),
            summary: review_test_summary(),
            files: vec![ReviewFileDiff {
                path: "src/demo.rs".to_string(),
                old_path: None,
                change_kind: ReviewFileChangeKind::Modified,
                added_lines: 0,
                removed_lines: 1,
                truncated: false,
                hunks: vec![ReviewHunk {
                    header: "@@ -2,1 +2,0 @@".to_string(),
                    old_start: 2,
                    old_lines: 1,
                    new_start: 2,
                    new_lines: 0,
                    anchor_new_line: Some(3),
                    truncated: false,
                    lines: vec![ReviewLine::removed(Some(2), None, true, "gone")],
                }],
            }],
            truncated: false,
        }
    }

    #[test]
    fn stale_review_fallback_updates_desk_summary_but_retains_cached_payload() {
        let root = std::env::temp_dir().join(format!(
            "axis-review-stale-summary-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock should be available")
                .as_nanos()
        ));
        fs::create_dir_all(&root).expect("stale review test root should exist");
        init_repo_with_main(&root);
        let root_string = root.display().to_string();
        let binding = WorktreeBinding {
            root_path: root_string.clone(),
            branch: "feature/stale".to_string(),
            base_branch: Some("main".to_string()),
            ahead: 2,
            behind: 1,
            dirty: false,
        };
        let stale_payload = review_payload_single_text_file(&root_string);
        let fresh_summary = build_desk_review_summary_view(&binding, &[]);

        let mut desk = blank_workdesk("Desk", "Summary");
        desk.workdesk_id = "desk-review".to_string();
        desk.metadata.cwd = root_string;
        desk.worktree_binding = Some(binding.clone());
        desk.review_payload_cache = Some(stale_payload.clone());

        AxisShell::apply_review_payload_to_desk(
            &mut desk,
            &binding,
            stale_payload.clone(),
            fresh_summary.clone(),
            true,
        );

        assert_eq!(desk.review_payload_cache, Some(stale_payload));
        assert_eq!(desk.review_summary, Some(fresh_summary));
        assert!(
            desk.review_local_state.stale_notice.is_some(),
            "stale payload fallback should keep the non-blocking stale notice"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[gpui::test]
    async fn review_panel_opens_from_reviewable_desk_selects_first_file(
        cx: &mut TestAppContext,
    ) {
        let root = std::env::temp_dir().join(format!(
            "axis-review-open-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time should be after epoch")
                .as_nanos()
        ));
        let root_string = root.display().to_string();
        let payload = review_payload_single_text_file(&root_string);

        let (shell, window) = cx.add_window_view(|_, view_cx| {
            let mut desk = blank_workdesk("Desk", "Summary");
            desk.metadata.cwd = root_string.clone();
            AxisShell::new_with_agent_runtime(
                vec![desk],
                0,
                ShortcutMap::default(),
                None,
                automation::start_automation_server_at(std::env::temp_dir().join(format!(
                    "review-open-{}-{}.sock",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .expect("clock should be available")
                        .as_nanos()
                )))
                .expect("automation server should start"),
                view_cx.focus_handle(),
                SharedString::from(""),
                SharedString::from(""),
                agent_sessions::AgentRuntimeBridge::new(),
            )
        });

        window.update(|_, cx| {
            shell.update(cx, |shell, view_cx| {
                shell.workdesks[0].review_payload_cache = Some(payload);
                shell.open_review_panel(0, view_cx);
                assert_eq!(shell.review_panel, Some(0));
                assert_eq!(shell.active_workdesk, 0);
                let desk = &shell.workdesks[0];
                assert_eq!(desk.review_local_state.selected_file, 0);
                assert_eq!(
                    desk.review_local_state
                        .selected_file_path(desk.review_payload_cache.as_ref().unwrap()),
                    Some("src/demo.rs")
                );
                assert_eq!(desk.review_local_state.selected_hunk, Some(0));
            });
        });
    }

    #[gpui::test]
    async fn review_panel_collapsed_sidebar_review_affordance_opens_review_panel(
        cx: &mut TestAppContext,
    ) {
        let root_string = std::env::temp_dir()
            .join(format!(
                "axis-review-collapsed-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .expect("clock should be available")
                    .as_nanos()
            ))
            .display()
            .to_string();
        let payload = review_payload_single_text_file(&root_string);

        let (shell, window) = cx.add_window_view(|_, view_cx| {
            let mut desk = blank_workdesk("Desk", "Summary");
            desk.metadata.cwd = root_string.clone();
            AxisShell::new_with_agent_runtime(
                vec![desk],
                0,
                ShortcutMap::default(),
                None,
                automation::start_automation_server_at(std::env::temp_dir().join(format!(
                    "review-collapsed-{}-{}.sock",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .expect("clock should be available")
                        .as_nanos()
                )))
                .expect("automation server should start"),
                view_cx.focus_handle(),
                SharedString::from(""),
                SharedString::from(""),
                agent_sessions::AgentRuntimeBridge::new(),
            )
        });

        window.update(|window, cx| {
            window.activate_window();
            shell.update(cx, |shell, view_cx| {
                shell.workdesks[0].review_payload_cache = Some(payload);
                shell.sidebar_collapsed = true;
                view_cx.notify();
            });
        });

        window.run_until_parked();
        let button_bounds = window
            .debug_bounds("workdesk-compact-review")
            .expect("collapsed review affordance should render");
        let click_position = GpuiPoint::new(
            button_bounds.origin.x + px(2.0),
            button_bounds.origin.y + px(2.0),
        );
        window.simulate_click(click_position, gpui::Modifiers::none());
        window.run_until_parked();

        shell.read_with(window, |shell, _| {
            assert_eq!(shell.review_panel, Some(0));
            assert_eq!(shell.active_workdesk, 0);
        });
    }

    #[gpui::test]
    async fn review_panel_jump_opens_editor_at_new_side_line(cx: &mut TestAppContext) {
        let root = std::env::temp_dir().join(format!(
            "axis-review-jump-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time should be after epoch")
                .as_nanos()
        ));
        fs::create_dir_all(root.join("src")).expect("fixture directory should exist");
        let file_path = root.join("src/demo.rs");
        fs::write(&file_path, "first line\nsecond line\nthird line\n")
            .expect("fixture file should write");
        let root_string = root.display().to_string();
        let file_path_string = file_path.display().to_string();
        let payload = review_payload_single_text_file(&root_string);

        let (shell, window) = cx.add_window_view(|_, view_cx| {
            let mut desk = blank_workdesk("Desk", "Summary");
            desk.metadata.cwd = root_string.clone();
            AxisShell::new_with_agent_runtime(
                vec![desk],
                0,
                ShortcutMap::default(),
                None,
                automation::start_automation_server_at(std::env::temp_dir().join(format!(
                    "review-jump-{}-{}.sock",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .expect("clock should be available")
                        .as_nanos()
                )))
                .expect("automation server should start"),
                view_cx.focus_handle(),
                SharedString::from(""),
                SharedString::from(""),
                agent_sessions::AgentRuntimeBridge::new(),
            )
        });

        window.update(|_, cx| {
            shell.update(cx, |shell, view_cx| {
                shell.workdesks[0].review_payload_cache = Some(payload);
                shell.open_review_panel(0, view_cx);
                assert!(shell.open_review_diff_line(0, 0, 0, 2, view_cx));
                assert_eq!(shell.review_panel, Some(0));

                let editor = shell.active_editor().expect("editor should open");
                assert_eq!(
                    canonical_path_string(&editor.path_string()),
                    canonical_path_string(&file_path_string)
                );
                let (line, column) = editor.line_col_for_offset(editor.cursor_offset());
                assert_eq!((line, column), (1, 0));
            });
        });

        let _ = fs::remove_dir_all(root);
    }

    #[gpui::test]
    async fn review_panel_mark_reviewed_sets_hunk_state(cx: &mut TestAppContext) {
        use crate::review::{HunkReviewState, ReviewHunkKey};

        let root_string = std::env::temp_dir()
            .join(format!(
                "axis-review-mark-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .expect("clock should be available")
                    .as_nanos()
            ))
            .display()
            .to_string();
        let payload = review_payload_single_text_file(&root_string);

        let (shell, window) = cx.add_window_view(|_, view_cx| {
            let mut desk = blank_workdesk("Desk", "Summary");
            desk.metadata.cwd = root_string.clone();
            AxisShell::new_with_agent_runtime(
                vec![desk],
                0,
                ShortcutMap::default(),
                None,
                automation::start_automation_server_at(std::env::temp_dir().join(format!(
                    "review-mark-{}-{}.sock",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .expect("clock should be available")
                        .as_nanos()
                )))
                .expect("automation server should start"),
                view_cx.focus_handle(),
                SharedString::from(""),
                SharedString::from(""),
                agent_sessions::AgentRuntimeBridge::new(),
            )
        });

        window.update(|_, cx| {
            shell.update(cx, |shell, view_cx| {
                shell.workdesks[0].review_payload_cache = Some(payload.clone());
                shell.open_review_panel(0, view_cx);
                shell.mark_review_selected_hunk_reviewed(view_cx);
                let desk = &shell.workdesks[0];
                let file = &desk.review_payload_cache.as_ref().unwrap().files[0];
                let hunk = &file.hunks[0];
                let key = ReviewHunkKey::from_hunk(
                    &WorkdeskId::new(desk.workdesk_id.clone()),
                    &file.path,
                    hunk,
                );
                assert_eq!(
                    desk.review_local_state.hunk_states.get(&key),
                    Some(&HunkReviewState::Reviewed)
                );
            });
        });
    }

    #[gpui::test]
    async fn review_panel_hunkless_file_disables_hunk_actions(cx: &mut TestAppContext) {
        let root_string = std::env::temp_dir()
            .join(format!(
                "axis-review-hunkless-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .expect("clock should be available")
                    .as_nanos()
            ))
            .display()
            .to_string();
        let payload = review_payload_hunkless_file(&root_string);

        let (shell, window) = cx.add_window_view(|_, view_cx| {
            let mut desk = blank_workdesk("Desk", "Summary");
            desk.metadata.cwd = root_string.clone();
            AxisShell::new_with_agent_runtime(
                vec![desk],
                0,
                ShortcutMap::default(),
                None,
                automation::start_automation_server_at(std::env::temp_dir().join(format!(
                    "review-hunkless-{}-{}.sock",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .expect("clock should be available")
                        .as_nanos()
                )))
                .expect("automation server should start"),
                view_cx.focus_handle(),
                SharedString::from(""),
                SharedString::from(""),
                agent_sessions::AgentRuntimeBridge::new(),
            )
        });

        window.update(|_, cx| {
            shell.update(cx, |shell, view_cx| {
                shell.workdesks[0].review_payload_cache = Some(payload);
                shell.open_review_panel(0, view_cx);
                assert!(
                    !shell.review_panel_hunk_actions_enabled(),
                    "hunk-level actions should be disabled without textual hunks"
                );
                shell.mark_review_selected_hunk_reviewed(view_cx);
                let desk = &shell.workdesks[0];
                assert!(
                    desk.review_local_state.hunk_states.is_empty(),
                    "mark reviewed should not fabricate state for hunkless entries"
                );
            });
        });
    }

    #[gpui::test]
    async fn review_panel_removal_jump_uses_anchor_new_line(cx: &mut TestAppContext) {
        let root = std::env::temp_dir().join(format!(
            "axis-review-removal-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time should be after epoch")
                .as_nanos()
        ));
        fs::create_dir_all(root.join("src")).expect("fixture directory should exist");
        let file_path = root.join("src/demo.rs");
        fs::write(&file_path, "one\ntwo\nthree\n")
            .expect("fixture file should write");
        let root_string = root.display().to_string();
        let file_path_string = file_path.display().to_string();
        let payload = review_payload_removal_anchor(&root_string);

        let (shell, window) = cx.add_window_view(|_, view_cx| {
            let mut desk = blank_workdesk("Desk", "Summary");
            desk.metadata.cwd = root_string.clone();
            AxisShell::new_with_agent_runtime(
                vec![desk],
                0,
                ShortcutMap::default(),
                None,
                automation::start_automation_server_at(std::env::temp_dir().join(format!(
                    "review-removal-{}-{}.sock",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .expect("clock should be available")
                        .as_nanos()
                )))
                .expect("automation server should start"),
                view_cx.focus_handle(),
                SharedString::from(""),
                SharedString::from(""),
                agent_sessions::AgentRuntimeBridge::new(),
            )
        });

        window.update(|_, cx| {
            shell.update(cx, |shell, view_cx| {
                shell.workdesks[0].review_payload_cache = Some(payload);
                shell.open_review_panel(0, view_cx);
                assert!(shell.open_review_diff_line(0, 0, 0, 0, view_cx));
                assert_eq!(shell.review_panel, Some(0));

                let editor = shell.active_editor().expect("editor should open");
                assert_eq!(
                    canonical_path_string(&editor.path_string()),
                    canonical_path_string(&file_path_string)
                );
                let (line, column) = editor.line_col_for_offset(editor.cursor_offset());
                assert_eq!((line, column), (2, 0));
            });
        });

        let _ = fs::remove_dir_all(root);
    }

    #[gpui::test]
    async fn opening_selected_workspace_search_result_focuses_requested_line(
        cx: &mut TestAppContext,
    ) {
        let root = std::env::temp_dir().join(format!(
            "axis-workspace-open-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time should be after epoch")
                .as_nanos()
        ));
        let file_path = root.join("src/demo.rs");
        fs::create_dir_all(file_path.parent().expect("fixture parent should exist"))
            .expect("fixture directory should exist");
        fs::write(&file_path, "first line\nsecond line\nthird line\n")
            .expect("fixture file should write");
        let root_string = root.display().to_string();
        let file_path_string = file_path.display().to_string();

        let (shell, window) = cx.add_window_view(|_, view_cx| {
            let mut desk = blank_workdesk("Desk", "Summary");
            desk.metadata.cwd = root_string.clone();
            desk.worktree_binding =
                Some(worktrees::binding_from_desk_paths(root_string.clone(), "main"));
            AxisShell::new_with_agent_runtime(
                vec![desk],
                0,
                ShortcutMap::default(),
                None,
                automation::start_automation_server_at(std::env::temp_dir().join(format!(
                    "workspace-open-{}-{}.sock",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .expect("clock should be available")
                        .as_nanos()
                )))
                .expect("automation server should start"),
                view_cx.focus_handle(),
                SharedString::from(""),
                SharedString::from(""),
                agent_sessions::AgentRuntimeBridge::new(),
            )
        });

        window.update(|_, cx| {
            shell.update(cx, |shell, view_cx| {
                shell.workspace_palette = Some(WorkspacePaletteState {
                    mode: WorkspacePaletteMode::SearchWorkspace,
                    root_path: root.clone(),
                    query: "third".to_string(),
                    all_files: Vec::new(),
                    results: vec![WorkspacePaletteResult::SearchMatch {
                        absolute_path: file_path_string.clone(),
                        relative_path: "src/demo.rs".to_string(),
                        line_number: 3,
                        preview: "third line".to_string(),
                    }],
                    selected: 0,
                });

                assert!(shell.open_selected_workspace_palette_result(view_cx));
                assert!(shell.workspace_palette.is_none());

                let editor = shell.active_editor().expect("editor should open");
                assert_eq!(
                    canonical_path_string(&editor.path_string()),
                    canonical_path_string(&file_path_string)
                );
                let (line, column) = editor.line_col_for_offset(editor.cursor_offset());
                assert_eq!((line, column), (2, 0));
            });
        });

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn stacking_surface_on_existing_pane_preserves_stack_identity_and_persistence() {
        let mut workdesks = vec![WorkdeskState::new(
            "Desk",
            "Summary",
            vec![PaneRecord::new(
                PaneId::new(1),
                WorkdeskPoint::new(0.0, 0.0),
                WorkdeskSize::new(980.0, 640.0),
                SurfaceRecord::browser(SurfaceId::new(11), "Research", "https://example.com"),
                None,
            )],
        )];
        let mut active_workdesk = 0;

        let bridge = agent_sessions::AgentRuntimeBridge::new();
        let (pane_id, surface_id) = AxisShell::spawn_surface_on_workdesk_state(
            &mut workdesks,
            &mut active_workdesk,
            0,
            Some(PaneId::new(1)),
            PaneKind::Browser,
            Some("Preview".to_string()),
            Some("https://example.com/preview".to_string()),
            None,
            true,
            &bridge,
        )
        .expect("stacking into the active pane should succeed");

        assert_eq!(pane_id, PaneId::new(1));
        assert_eq!(active_workdesk, 0);

        let pane = workdesks[0]
            .pane(pane_id)
            .expect("pane should remain present");
        assert_eq!(pane.surfaces.len(), 2);
        assert_eq!(pane.active_surface_id, surface_id);
        assert_eq!(pane.stack_title.as_deref(), Some("Research"));
        assert_eq!(pane.stack_display_title(), "Research");
        assert_eq!(pane.title, "Preview");

        let restored = PersistedWorkdesk::from_state(&workdesks[0]).into_state();
        let restored_pane = restored
            .pane(pane_id)
            .expect("restored pane should remain present");
        assert_eq!(restored_pane.surfaces.len(), 2);
        assert_eq!(restored_pane.active_surface_id, surface_id);
        assert_eq!(restored_pane.stack_title.as_deref(), Some("Research"));
        assert_eq!(restored_pane.stack_display_title(), "Research");
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
                state: AttentionState::NeedsInput,
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
    fn next_attention_target_for_single_workdesk_picks_oldest_unread_attention() {
        let mut desk = WorkdeskState::new(
            "Desk",
            "Summary",
            vec![
                single_surface_pane(
                    1,
                    "Build Shell",
                    PaneKind::Shell,
                    WorkdeskPoint::new(0.0, 0.0),
                    WorkdeskSize::new(920.0, 560.0),
                ),
                single_surface_pane(
                    2,
                    "Review Agent",
                    PaneKind::Agent,
                    WorkdeskPoint::new(960.0, 0.0),
                    WorkdeskSize::new(720.0, 420.0),
                ),
            ],
        );
        desk.pane_attention.insert(
            PaneId::new(1),
            PaneAttention {
                state: AttentionState::Error,
                unread: true,
                last_attention_sequence: 9,
                last_activity_sequence: 4,
            },
        );
        desk.pane_attention.insert(
            PaneId::new(2),
            PaneAttention {
                state: AttentionState::NeedsReview,
                unread: true,
                last_attention_sequence: 6,
                last_activity_sequence: 5,
            },
        );

        assert_eq!(
            next_attention_target_for_workdesk(&desk),
            Some(PaneId::new(2))
        );
    }

    #[test]
    fn workdesk_navigation_target_falls_back_to_active_pane_when_attention_is_clear() {
        let mut desk = WorkdeskState::new(
            "Desk",
            "Summary",
            vec![single_surface_pane(
                3,
                "Build Shell",
                PaneKind::Shell,
                WorkdeskPoint::new(0.0, 0.0),
                WorkdeskSize::new(920.0, 560.0),
            )],
        );
        desk.active_pane = Some(PaneId::new(3));
        desk.pane_attention.insert(
            PaneId::new(3),
            PaneAttention {
                state: AttentionState::Idle,
                unread: false,
                last_attention_sequence: 0,
                last_activity_sequence: 2,
            },
        );

        let target = workdesk_navigation_target(&desk).expect("active pane should be resumable");

        assert_eq!(target.pane_id, PaneId::new(3));
        assert_eq!(target.mode, WorkdeskNavigationMode::Resume);
        assert_eq!(target.label, "Build Shell");
        assert!(target.detail.contains("shell"));
        assert!(target.detail.contains("1 surface"));
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
                state: AttentionState::NeedsInput,
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
            Value::String("needs-input".to_string())
        );
    }

    #[test]
    fn dismiss_runtime_notice_only_clears_active_workdesk() {
        let mut workdesks = vec![
            WorkdeskState::new("Left", "Summary", Vec::new()),
            WorkdeskState::new("Right", "Summary", Vec::new()),
        ];
        workdesks[0].runtime_notice = Some(SharedString::from("left failure"));
        workdesks[1].runtime_notice = Some(SharedString::from("right failure"));

        assert!(dismiss_runtime_notice_for_workdesks(&mut workdesks, 0));
        assert!(workdesks[0].runtime_notice.is_none());
        assert_eq!(
            workdesks[1]
                .runtime_notice
                .as_ref()
                .map(ToString::to_string),
            Some("right failure".to_string())
        );
        assert!(!dismiss_runtime_notice_for_workdesks(&mut workdesks, 0));
    }

    #[gpui::test]
    async fn closing_last_window_quits_the_app(cx: &mut TestAppContext) {
        let did_quit = Rc::new(Cell::new(false));
        cx.update(|cx| {
            install_quit_on_last_window_closed_with(cx, |cx| cx.shutdown());
            let did_quit = did_quit.clone();
            cx.on_app_quit(move |_| {
                let did_quit = did_quit.clone();
                async move {
                    did_quit.set(true);
                }
            })
            .detach();
        });

        let cx = cx.add_empty_window();
        cx.update(|window, _| {
            window.remove_window();
        });

        assert!(did_quit.get(), "closing the last window should quit the app");
    }
}
