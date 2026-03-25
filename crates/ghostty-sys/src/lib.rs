use std::ffi::c_void;
use std::mem;
use std::path::PathBuf;

pub type GhosttyTerminal = *mut c_void;
pub type GhosttyRenderState = *mut c_void;
pub type GhosttyRenderStateRowIterator = *mut c_void;
pub type GhosttyRenderStateRowCells = *mut c_void;

pub type GhosttyMode = u16;
pub type GhosttyResult = i32;

pub const GHOSTTY_SUCCESS: GhosttyResult = 0;
pub const GHOSTTY_OUT_OF_MEMORY: GhosttyResult = -1;
pub const GHOSTTY_INVALID_VALUE: GhosttyResult = -2;
pub const GHOSTTY_OUT_OF_SPACE: GhosttyResult = -3;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GhosttyBuildInfo {
    pub vendor_dir: PathBuf,
    pub lib_dir: PathBuf,
    pub linked: bool,
}

impl GhosttyBuildInfo {
    pub fn current() -> Self {
        Self {
            vendor_dir: PathBuf::from(env!("GHOSTTY_VENDOR_DIR")),
            lib_dir: PathBuf::from(env!("GHOSTTY_VT_LIB_DIR")),
            linked: true,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GhosttyTerminalOptions {
    pub cols: u16,
    pub rows: u16,
    pub max_scrollback: usize,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GhosttyColorRgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

pub type GhosttyColorPaletteIndex = u8;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GhosttyStyleColorTag {
    None = 0,
    Palette = 1,
    Rgb = 2,
}

impl Default for GhosttyStyleColorTag {
    fn default() -> Self {
        Self::None
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub union GhosttyStyleColorValue {
    pub palette: GhosttyColorPaletteIndex,
    pub rgb: GhosttyColorRgb,
    _padding: u64,
}

impl Default for GhosttyStyleColorValue {
    fn default() -> Self {
        Self { _padding: 0 }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct GhosttyStyleColor {
    pub tag: GhosttyStyleColorTag,
    pub value: GhosttyStyleColorValue,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyStyle {
    pub size: usize,
    pub fg_color: GhosttyStyleColor,
    pub bg_color: GhosttyStyleColor,
    pub underline_color: GhosttyStyleColor,
    pub bold: bool,
    pub italic: bool,
    pub faint: bool,
    pub blink: bool,
    pub inverse: bool,
    pub invisible: bool,
    pub strikethrough: bool,
    pub overline: bool,
    pub underline: i32,
}

impl Default for GhosttyStyle {
    fn default() -> Self {
        Self {
            size: mem::size_of::<Self>(),
            fg_color: GhosttyStyleColor::default(),
            bg_color: GhosttyStyleColor::default(),
            underline_color: GhosttyStyleColor::default(),
            bold: false,
            italic: false,
            faint: false,
            blink: false,
            inverse: false,
            invisible: false,
            strikethrough: false,
            overline: false,
            underline: 0,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct GhosttyRenderStateColors {
    pub size: usize,
    pub background: GhosttyColorRgb,
    pub foreground: GhosttyColorRgb,
    pub cursor: GhosttyColorRgb,
    pub cursor_has_value: bool,
    pub palette: [GhosttyColorRgb; 256],
}

impl Default for GhosttyRenderStateColors {
    fn default() -> Self {
        Self {
            size: mem::size_of::<Self>(),
            background: GhosttyColorRgb::default(),
            foreground: GhosttyColorRgb::default(),
            cursor: GhosttyColorRgb::default(),
            cursor_has_value: false,
            palette: [GhosttyColorRgb::default(); 256],
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GhosttyTerminalScrollbar {
    pub total: u64,
    pub offset: u64,
    pub len: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GhosttyTerminalScrollViewportTag {
    Top = 0,
    Bottom = 1,
    Delta = 2,
}

impl Default for GhosttyTerminalScrollViewportTag {
    fn default() -> Self {
        Self::Bottom
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub union GhosttyTerminalScrollViewportValue {
    pub delta: isize,
    _padding: [u64; 2],
}

impl Default for GhosttyTerminalScrollViewportValue {
    fn default() -> Self {
        Self { _padding: [0, 0] }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct GhosttyTerminalScrollViewport {
    pub tag: GhosttyTerminalScrollViewportTag,
    pub value: GhosttyTerminalScrollViewportValue,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GhosttyTerminalScreen {
    Primary = 0,
    Alternate = 1,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GhosttyTerminalData {
    Invalid = 0,
    Cols = 1,
    Rows = 2,
    CursorX = 3,
    CursorY = 4,
    CursorPendingWrap = 5,
    ActiveScreen = 6,
    CursorVisible = 7,
    KittyKeyboardFlags = 8,
    Scrollbar = 9,
    CursorStyle = 10,
    MouseTracking = 11,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GhosttyRenderStateDirty {
    False = 0,
    Partial = 1,
    Full = 2,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GhosttyRenderStateCursorVisualStyle {
    Bar = 0,
    Block = 1,
    Underline = 2,
    BlockHollow = 3,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GhosttyRenderStateData {
    Invalid = 0,
    Cols = 1,
    Rows = 2,
    Dirty = 3,
    RowIterator = 4,
    ColorBackground = 5,
    ColorForeground = 6,
    ColorCursor = 7,
    ColorCursorHasValue = 8,
    ColorPalette = 9,
    CursorVisualStyle = 10,
    CursorVisible = 11,
    CursorBlinking = 12,
    CursorPasswordInput = 13,
    CursorViewportHasValue = 14,
    CursorViewportX = 15,
    CursorViewportY = 16,
    CursorViewportWideTail = 17,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GhosttyRenderStateOption {
    Dirty = 0,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GhosttyRenderStateRowData {
    Invalid = 0,
    Dirty = 1,
    Raw = 2,
    Cells = 3,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GhosttyRenderStateRowOption {
    Dirty = 0,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GhosttyRenderStateRowCellsData {
    Invalid = 0,
    Raw = 1,
    Style = 2,
    GraphemesLen = 3,
    GraphemesBuf = 4,
    BgColor = 5,
    FgColor = 6,
}

pub const fn ghostty_mode_new(value: u16, ansi: bool) -> GhosttyMode {
    (value & 0x7fff) | ((ansi as u16) << 15)
}

pub const GHOSTTY_MODE_DECCKM: GhosttyMode = ghostty_mode_new(1, false);

pub fn ghostty_build_info() -> GhosttyBuildInfo {
    GhosttyBuildInfo::current()
}

#[link(name = "ghostty-vt")]
unsafe extern "C" {
    pub fn ghostty_terminal_new(
        allocator: *const c_void,
        terminal: *mut GhosttyTerminal,
        options: GhosttyTerminalOptions,
    ) -> GhosttyResult;
    pub fn ghostty_terminal_free(terminal: GhosttyTerminal);
    pub fn ghostty_terminal_reset(terminal: GhosttyTerminal);
    pub fn ghostty_terminal_resize(
        terminal: GhosttyTerminal,
        cols: u16,
        rows: u16,
    ) -> GhosttyResult;
    pub fn ghostty_terminal_vt_write(terminal: GhosttyTerminal, data: *const u8, len: usize);
    pub fn ghostty_terminal_scroll_viewport(
        terminal: GhosttyTerminal,
        behavior: GhosttyTerminalScrollViewport,
    );
    pub fn ghostty_terminal_mode_get(
        terminal: GhosttyTerminal,
        mode: GhosttyMode,
        out_value: *mut bool,
    ) -> GhosttyResult;
    pub fn ghostty_terminal_get(
        terminal: GhosttyTerminal,
        data: GhosttyTerminalData,
        out: *mut c_void,
    ) -> GhosttyResult;

    pub fn ghostty_render_state_new(
        allocator: *const c_void,
        state: *mut GhosttyRenderState,
    ) -> GhosttyResult;
    pub fn ghostty_render_state_free(state: GhosttyRenderState);
    pub fn ghostty_render_state_update(
        state: GhosttyRenderState,
        terminal: GhosttyTerminal,
    ) -> GhosttyResult;
    pub fn ghostty_render_state_get(
        state: GhosttyRenderState,
        data: GhosttyRenderStateData,
        out: *mut c_void,
    ) -> GhosttyResult;
    pub fn ghostty_render_state_set(
        state: GhosttyRenderState,
        option: GhosttyRenderStateOption,
        value: *const c_void,
    ) -> GhosttyResult;
    pub fn ghostty_render_state_colors_get(
        state: GhosttyRenderState,
        out_colors: *mut GhosttyRenderStateColors,
    ) -> GhosttyResult;

    pub fn ghostty_render_state_row_iterator_new(
        allocator: *const c_void,
        out_iterator: *mut GhosttyRenderStateRowIterator,
    ) -> GhosttyResult;
    pub fn ghostty_render_state_row_iterator_free(iterator: GhosttyRenderStateRowIterator);
    pub fn ghostty_render_state_row_iterator_next(iterator: GhosttyRenderStateRowIterator) -> bool;
    pub fn ghostty_render_state_row_get(
        iterator: GhosttyRenderStateRowIterator,
        data: GhosttyRenderStateRowData,
        out: *mut c_void,
    ) -> GhosttyResult;
    pub fn ghostty_render_state_row_set(
        iterator: GhosttyRenderStateRowIterator,
        option: GhosttyRenderStateRowOption,
        value: *const c_void,
    ) -> GhosttyResult;

    pub fn ghostty_render_state_row_cells_new(
        allocator: *const c_void,
        out_cells: *mut GhosttyRenderStateRowCells,
    ) -> GhosttyResult;
    pub fn ghostty_render_state_row_cells_free(cells: GhosttyRenderStateRowCells);
    pub fn ghostty_render_state_row_cells_next(cells: GhosttyRenderStateRowCells) -> bool;
    pub fn ghostty_render_state_row_cells_select(
        cells: GhosttyRenderStateRowCells,
        x: u16,
    ) -> GhosttyResult;
    pub fn ghostty_render_state_row_cells_get(
        cells: GhosttyRenderStateRowCells,
        data: GhosttyRenderStateRowCellsData,
        out: *mut c_void,
    ) -> GhosttyResult;
}
