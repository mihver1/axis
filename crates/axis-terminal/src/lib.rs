use anyhow::{anyhow, Context, Result};
use axis_core::{PaneKind, Point, Size, Workdesk};
use ghostty_sys::{
    ghostty_build_info as ghostty_vt_build_info, ghostty_render_state_colors_get,
    ghostty_render_state_free, ghostty_render_state_get, ghostty_render_state_new,
    ghostty_render_state_row_cells_free, ghostty_render_state_row_cells_get,
    ghostty_render_state_row_cells_new, ghostty_render_state_row_cells_next,
    ghostty_render_state_row_get, ghostty_render_state_row_iterator_free,
    ghostty_render_state_row_iterator_new, ghostty_render_state_row_iterator_next,
    ghostty_render_state_update, ghostty_terminal_free, ghostty_terminal_get,
    ghostty_terminal_mode_get, ghostty_terminal_new, ghostty_terminal_resize,
    ghostty_terminal_scroll_viewport, ghostty_terminal_vt_write, GhosttyBuildInfo, GhosttyColorRgb,
    GhosttyRenderState, GhosttyRenderStateColors, GhosttyRenderStateData,
    GhosttyRenderStateRowCells, GhosttyRenderStateRowCellsData, GhosttyRenderStateRowData,
    GhosttyRenderStateRowIterator, GhosttyStyle, GhosttyStyleColor, GhosttyStyleColorTag,
    GhosttyTerminal, GhosttyTerminalData, GhosttyTerminalOptions, GhosttyTerminalScreen,
    GhosttyTerminalScrollViewport, GhosttyTerminalScrollViewportTag, GhosttyTerminalScrollbar,
    GHOSTTY_MODE_DECCKM, GHOSTTY_SUCCESS,
};
pub use process_manager::TerminalGridSize;
use process_manager::{spawn_process, ProcessSpec, RunningProcess};
use std::ffi::c_void;
use std::io::Read;
use std::ptr;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex,
};

const DEFAULT_SCROLLBACK: usize = 10_000;
const TERMINAL_CELL_WIDTH: f32 = 7.8;
const TERMINAL_CELL_HEIGHT: f32 = 18.0;
const TERMINAL_HORIZONTAL_PADDING: f32 = 32.0;
const TERMINAL_VERTICAL_CHROME: f32 = 86.0;
const MIN_TERMINAL_COLS: u16 = 40;
const MIN_TERMINAL_ROWS: u16 = 10;

pub struct TerminalPaneSpec {
    pub title: String,
    pub kind: PaneKind,
    pub command: ProcessSpec,
    pub position: Point,
    pub size: Size,
}

/// Optional link from a terminal pane to an `axis-agent-runtime` session (UI may ignore for now).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TerminalAgentMetadata {
    pub session_id: axis_core::agent::AgentSessionId,
}

impl TerminalPaneSpec {
    pub fn shell(title: impl Into<String>, position: Point, size: Size) -> Self {
        Self {
            title: title.into(),
            kind: PaneKind::Shell,
            command: ProcessSpec::login_shell(),
            position,
            size,
        }
    }

    pub fn agent(
        title: impl Into<String>,
        command: impl Into<Vec<String>>,
        position: Point,
        size: Size,
    ) -> Self {
        Self {
            title: title.into(),
            kind: PaneKind::Agent,
            command: ProcessSpec::new(command),
            position,
            size,
        }
    }
}

#[derive(Clone)]
pub struct TerminalSession {
    inner: Arc<TerminalSessionInner>,
}

struct TerminalSessionInner {
    fallback_title: String,
    engine: Arc<Mutex<GhosttyEngine>>,
    process: RunningProcess,
    revision: Arc<AtomicU64>,
    closed: Arc<AtomicBool>,
    status: Arc<Mutex<Option<String>>>,
    agent_metadata: Arc<Mutex<Option<TerminalAgentMetadata>>>,
}

#[derive(Clone, Debug)]
pub struct TerminalSnapshot {
    pub title: String,
    pub rows: Vec<TerminalRow>,
    pub theme: TerminalTheme,
    pub cursor: (u16, u16),
    pub cursor_blinking: bool,
    pub cols: u16,
    pub rows_count: u16,
    pub scrollbar: TerminalScrollbar,
    pub alternate_screen: bool,
    pub application_cursor: bool,
    pub closed: bool,
    pub status: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TerminalScrollbar {
    pub total: u64,
    pub offset: u64,
    pub visible: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TerminalColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl TerminalColor {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalTheme {
    pub background: TerminalColor,
    pub foreground: TerminalColor,
    pub cursor: TerminalColor,
}

impl Default for TerminalTheme {
    fn default() -> Self {
        Self {
            background: TerminalColor::new(0x0d, 0x12, 0x17),
            foreground: TerminalColor::new(0xd5, 0xdd, 0xe4),
            cursor: TerminalColor::new(0xe5, 0x9a, 0x49),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalTextStyle {
    pub foreground: TerminalColor,
    pub background: Option<TerminalColor>,
    pub underline_color: Option<TerminalColor>,
    pub bold: bool,
    pub italic: bool,
    pub faint: bool,
    pub underline: bool,
    pub strikethrough: bool,
}

impl TerminalTextStyle {
    fn plain(theme: &TerminalTheme) -> Self {
        Self {
            foreground: theme.foreground,
            background: None,
            underline_color: None,
            bold: false,
            italic: false,
            faint: false,
            underline: false,
            strikethrough: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalRun {
    pub text: String,
    pub style: TerminalTextStyle,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalRow {
    pub runs: Vec<TerminalRun>,
}

impl TerminalRow {
    fn blank(cols: u16, theme: &TerminalTheme) -> Self {
        Self {
            runs: vec![TerminalRun {
                text: "\u{00A0}".repeat(cols as usize),
                style: TerminalTextStyle::plain(theme),
            }],
        }
    }

    fn plain(text: impl Into<String>, theme: &TerminalTheme) -> Self {
        Self {
            runs: vec![TerminalRun {
                text: normalize_terminal_text(text.into()),
                style: TerminalTextStyle::plain(theme),
            }],
        }
    }
}

struct GhosttyEngine {
    terminal: GhosttyTerminal,
    render_state: GhosttyRenderState,
    row_iter: GhosttyRenderStateRowIterator,
    row_cells: GhosttyRenderStateRowCells,
}

unsafe impl Send for GhosttyEngine {}

impl GhosttyEngine {
    fn new(grid: TerminalGridSize) -> Result<Self> {
        let mut terminal = ptr::null_mut();
        ghostty_check("ghostty_terminal_new", unsafe {
            ghostty_terminal_new(
                ptr::null(),
                &mut terminal,
                GhosttyTerminalOptions {
                    cols: grid.cols,
                    rows: grid.rows,
                    max_scrollback: DEFAULT_SCROLLBACK,
                },
            )
        })?;

        let mut render_state = ptr::null_mut();
        ghostty_check("ghostty_render_state_new", unsafe {
            ghostty_render_state_new(ptr::null(), &mut render_state)
        })?;

        let mut row_iter = ptr::null_mut();
        ghostty_check("ghostty_render_state_row_iterator_new", unsafe {
            ghostty_render_state_row_iterator_new(ptr::null(), &mut row_iter)
        })?;

        let mut row_cells = ptr::null_mut();
        ghostty_check("ghostty_render_state_row_cells_new", unsafe {
            ghostty_render_state_row_cells_new(ptr::null(), &mut row_cells)
        })?;

        let mut engine = Self {
            terminal,
            render_state,
            row_iter,
            row_cells,
        };
        engine.refresh()?;
        Ok(engine)
    }

    fn write(&mut self, bytes: &[u8]) -> Result<()> {
        unsafe {
            ghostty_terminal_vt_write(self.terminal, bytes.as_ptr(), bytes.len());
        }
        self.refresh()
    }

    fn resize(&mut self, grid: TerminalGridSize) -> Result<()> {
        ghostty_check("ghostty_terminal_resize", unsafe {
            ghostty_terminal_resize(self.terminal, grid.cols, grid.rows)
        })?;
        self.refresh()
    }

    fn scroll_viewport_delta(&mut self, delta: isize) -> Result<()> {
        unsafe {
            ghostty_terminal_scroll_viewport(
                self.terminal,
                GhosttyTerminalScrollViewport {
                    tag: GhosttyTerminalScrollViewportTag::Delta,
                    value: ghostty_sys::GhosttyTerminalScrollViewportValue { delta },
                },
            );
        }
        self.refresh()
    }

    fn scroll_viewport_bottom(&mut self) -> Result<()> {
        unsafe {
            ghostty_terminal_scroll_viewport(
                self.terminal,
                GhosttyTerminalScrollViewport {
                    tag: GhosttyTerminalScrollViewportTag::Bottom,
                    value: Default::default(),
                },
            );
        }
        self.refresh()
    }

    fn snapshot(&mut self, fallback_title: &str) -> Result<TerminalSnapshot> {
        let mut cols = 0u16;
        ghostty_check("ghostty_render_state_get(cols)", unsafe {
            ghostty_render_state_get(
                self.render_state,
                GhosttyRenderStateData::Cols,
                (&mut cols as *mut u16).cast::<c_void>(),
            )
        })?;

        let mut rows_count = 0u16;
        ghostty_check("ghostty_render_state_get(rows)", unsafe {
            ghostty_render_state_get(
                self.render_state,
                GhosttyRenderStateData::Rows,
                (&mut rows_count as *mut u16).cast::<c_void>(),
            )
        })?;

        let mut colors = GhosttyRenderStateColors::default();
        let _ = unsafe { ghostty_render_state_colors_get(self.render_state, &mut colors) };
        let theme = TerminalTheme {
            background: color_from_rgb(colors.background),
            foreground: color_from_rgb(colors.foreground),
            cursor: if colors.cursor_has_value {
                color_from_rgb(colors.cursor)
            } else {
                color_from_rgb(colors.foreground)
            },
        };

        let mut cursor_visible = false;
        let _ = ghostty_check("ghostty_render_state_get(cursor_visible)", unsafe {
            ghostty_render_state_get(
                self.render_state,
                GhosttyRenderStateData::CursorVisible,
                (&mut cursor_visible as *mut bool).cast::<c_void>(),
            )
        });

        let mut cursor_has_value = false;
        let _ = ghostty_check(
            "ghostty_render_state_get(cursor_viewport_has_value)",
            unsafe {
                ghostty_render_state_get(
                    self.render_state,
                    GhosttyRenderStateData::CursorViewportHasValue,
                    (&mut cursor_has_value as *mut bool).cast::<c_void>(),
                )
            },
        );

        let cursor = if cursor_visible && cursor_has_value {
            let mut x = 0u16;
            let mut y = 0u16;
            let _ = ghostty_check("ghostty_render_state_get(cursor_x)", unsafe {
                ghostty_render_state_get(
                    self.render_state,
                    GhosttyRenderStateData::CursorViewportX,
                    (&mut x as *mut u16).cast::<c_void>(),
                )
            });
            let _ = ghostty_check("ghostty_render_state_get(cursor_y)", unsafe {
                ghostty_render_state_get(
                    self.render_state,
                    GhosttyRenderStateData::CursorViewportY,
                    (&mut y as *mut u16).cast::<c_void>(),
                )
            });
            (y, x)
        } else {
            (0, 0)
        };

        let mut cursor_blinking = false;
        let _ = ghostty_check("ghostty_render_state_get(cursor_blinking)", unsafe {
            ghostty_render_state_get(
                self.render_state,
                GhosttyRenderStateData::CursorBlinking,
                (&mut cursor_blinking as *mut bool).cast::<c_void>(),
            )
        });

        ghostty_check("ghostty_render_state_get(row_iterator)", unsafe {
            ghostty_render_state_get(
                self.render_state,
                GhosttyRenderStateData::RowIterator,
                (&mut self.row_iter as *mut GhosttyRenderStateRowIterator).cast::<c_void>(),
            )
        })?;

        let mut rows = Vec::with_capacity(rows_count as usize);
        while rows.len() < rows_count as usize {
            if !unsafe { ghostty_render_state_row_iterator_next(self.row_iter) } {
                rows.push(TerminalRow::blank(cols, &theme));
                continue;
            }

            ghostty_check("ghostty_render_state_row_get(cells)", unsafe {
                ghostty_render_state_row_get(
                    self.row_iter,
                    GhosttyRenderStateRowData::Cells,
                    (&mut self.row_cells as *mut GhosttyRenderStateRowCells).cast::<c_void>(),
                )
            })?;

            let mut row_runs = Vec::new();
            let mut active_text = String::new();
            let mut active_style: Option<TerminalTextStyle> = None;
            let mut visited_cells = 0u16;
            while visited_cells < cols {
                if !unsafe { ghostty_render_state_row_cells_next(self.row_cells) } {
                    break;
                }

                visited_cells += 1;

                let mut style = GhosttyStyle::default();
                ghostty_check("ghostty_render_state_row_cells_get(style)", unsafe {
                    ghostty_render_state_row_cells_get(
                        self.row_cells,
                        GhosttyRenderStateRowCellsData::Style,
                        (&mut style as *mut GhosttyStyle).cast::<c_void>(),
                    )
                })?;

                let mut grapheme_len = 0u32;
                ghostty_check(
                    "ghostty_render_state_row_cells_get(graphemes_len)",
                    unsafe {
                        ghostty_render_state_row_cells_get(
                            self.row_cells,
                            GhosttyRenderStateRowCellsData::GraphemesLen,
                            (&mut grapheme_len as *mut u32).cast::<c_void>(),
                        )
                    },
                )?;

                let text = if grapheme_len == 0 {
                    "\u{00A0}".to_string()
                } else {
                    let mut graphemes = vec![0u32; grapheme_len as usize];
                    ghostty_check(
                        "ghostty_render_state_row_cells_get(graphemes_buf)",
                        unsafe {
                            ghostty_render_state_row_cells_get(
                                self.row_cells,
                                GhosttyRenderStateRowCellsData::GraphemesBuf,
                                graphemes.as_mut_ptr().cast::<c_void>(),
                            )
                        },
                    )?;

                    let text = graphemes
                        .into_iter()
                        .map(|codepoint| {
                            let ch = char::from_u32(codepoint).unwrap_or('\u{fffd}');
                            if ch.is_control() && ch != '\t' {
                                ' '
                            } else {
                                ch
                            }
                        })
                        .collect::<String>();
                    normalize_terminal_text(text)
                };

                let cell_style = resolve_cell_style(style, &colors, &theme);

                if active_style != Some(cell_style) {
                    if !active_text.is_empty() {
                        row_runs.push(TerminalRun {
                            text: std::mem::take(&mut active_text),
                            style: active_style
                                .take()
                                .expect("terminal run should have style before flushing"),
                        });
                    }
                    active_style = Some(cell_style);
                }

                active_text.push_str(&text);
            }

            if visited_cells < cols {
                let padding_style = TerminalTextStyle::plain(&theme);
                if active_style != Some(padding_style) {
                    if !active_text.is_empty() {
                        row_runs.push(TerminalRun {
                            text: std::mem::take(&mut active_text),
                            style: active_style
                                .take()
                                .expect("terminal run should have style before flushing"),
                        });
                    }
                    active_style = Some(padding_style);
                }
                active_text.push_str(&"\u{00A0}".repeat((cols - visited_cells) as usize));
            }

            if !active_text.is_empty() {
                row_runs.push(TerminalRun {
                    text: active_text,
                    style: active_style.unwrap_or_else(|| TerminalTextStyle::plain(&theme)),
                });
            }

            rows.push(TerminalRow { runs: row_runs });
        }

        let mut active_screen = GhosttyTerminalScreen::Primary;
        ghostty_check("ghostty_terminal_get(active_screen)", unsafe {
            ghostty_terminal_get(
                self.terminal,
                GhosttyTerminalData::ActiveScreen,
                (&mut active_screen as *mut GhosttyTerminalScreen).cast::<c_void>(),
            )
        })?;

        let mut application_cursor = false;
        ghostty_check("ghostty_terminal_mode_get(DECCKM)", unsafe {
            ghostty_terminal_mode_get(self.terminal, GHOSTTY_MODE_DECCKM, &mut application_cursor)
        })?;

        let mut scrollbar = GhosttyTerminalScrollbar::default();
        let _ = ghostty_check("ghostty_terminal_get(scrollbar)", unsafe {
            ghostty_terminal_get(
                self.terminal,
                GhosttyTerminalData::Scrollbar,
                (&mut scrollbar as *mut GhosttyTerminalScrollbar).cast::<c_void>(),
            )
        });

        Ok(TerminalSnapshot {
            title: fallback_title.to_string(),
            rows,
            theme,
            cursor,
            cursor_blinking,
            cols,
            rows_count,
            scrollbar: TerminalScrollbar {
                total: scrollbar.total,
                offset: scrollbar.offset,
                visible: scrollbar.len,
            },
            alternate_screen: matches!(active_screen, GhosttyTerminalScreen::Alternate),
            application_cursor,
            closed: false,
            status: None,
        })
    }

    fn refresh(&mut self) -> Result<()> {
        ghostty_check("ghostty_render_state_update", unsafe {
            ghostty_render_state_update(self.render_state, self.terminal)
        })
    }
}

impl Drop for GhosttyEngine {
    fn drop(&mut self) {
        unsafe {
            ghostty_render_state_row_cells_free(self.row_cells);
            ghostty_render_state_row_iterator_free(self.row_iter);
            ghostty_render_state_free(self.render_state);
            ghostty_terminal_free(self.terminal);
        }
    }
}

impl TerminalSession {
    pub fn spawn(
        spec: &ProcessSpec,
        fallback_title: impl Into<String>,
        grid: TerminalGridSize,
    ) -> Result<Self> {
        let fallback_title = fallback_title.into();
        let spawned = spawn_process(spec, grid).context("failed to boot PTY process")?;
        let engine = Arc::new(Mutex::new(
            GhosttyEngine::new(grid).context("failed to boot ghostty-vt")?,
        ));
        let revision = Arc::new(AtomicU64::new(1));
        let closed = Arc::new(AtomicBool::new(false));
        let status = Arc::new(Mutex::new(Some("Running".to_string())));
        let agent_metadata = Arc::new(Mutex::new(None));

        let thread_engine = Arc::clone(&engine);
        let thread_revision = Arc::clone(&revision);
        let thread_closed = Arc::clone(&closed);
        let thread_status = Arc::clone(&status);
        let thread_process = spawned.process.clone();
        let mut reader = spawned.reader;

        std::thread::Builder::new()
            .name(format!("axis-terminal-{}", fallback_title))
            .spawn(move || {
                let mut buffer = [0u8; 8192];

                loop {
                    match reader.read(&mut buffer) {
                        Ok(0) => {
                            thread_closed.store(true, Ordering::SeqCst);
                            let status_text = thread_process
                                .try_wait_status()
                                .ok()
                                .flatten()
                                .unwrap_or_else(|| "PTY stream closed".to_string());
                            *thread_status
                                .lock()
                                .expect("terminal status mutex poisoned") = Some(status_text);
                            thread_revision.fetch_add(1, Ordering::SeqCst);
                            break;
                        }
                        Ok(bytes_read) => {
                            match thread_engine.lock() {
                                Ok(mut engine) => {
                                    if let Err(error) = engine.write(&buffer[..bytes_read]) {
                                        *thread_status
                                            .lock()
                                            .expect("terminal status mutex poisoned") =
                                            Some(format!("ghostty-vt update failed: {error}"));
                                        thread_revision.fetch_add(1, Ordering::SeqCst);
                                        break;
                                    }
                                }
                                Err(_) => {
                                    *thread_status
                                        .lock()
                                        .expect("terminal status mutex poisoned") =
                                        Some("ghostty-vt state mutex poisoned".to_string());
                                    thread_revision.fetch_add(1, Ordering::SeqCst);
                                    break;
                                }
                            }

                            thread_revision.fetch_add(1, Ordering::SeqCst);
                        }
                        Err(error) => {
                            thread_closed.store(true, Ordering::SeqCst);
                            *thread_status
                                .lock()
                                .expect("terminal status mutex poisoned") =
                                Some(format!("PTY read error: {error}"));
                            thread_revision.fetch_add(1, Ordering::SeqCst);
                            break;
                        }
                    }
                }
            })
            .context("failed to spawn PTY reader thread")?;

        Ok(Self {
            inner: Arc::new(TerminalSessionInner {
                fallback_title,
                engine,
                process: spawned.process,
                revision,
                closed,
                status,
                agent_metadata,
            }),
        })
    }

    pub fn set_agent_metadata(&self, meta: Option<TerminalAgentMetadata>) {
        *self
            .inner
            .agent_metadata
            .lock()
            .expect("terminal agent metadata mutex poisoned") = meta;
    }

    pub fn agent_metadata(&self) -> Option<TerminalAgentMetadata> {
        self.inner
            .agent_metadata
            .lock()
            .expect("terminal agent metadata mutex poisoned")
            .clone()
    }

    pub fn revision(&self) -> u64 {
        self.inner.revision.load(Ordering::SeqCst)
    }

    pub fn status(&self) -> Option<String> {
        self.inner
            .status
            .lock()
            .expect("terminal status mutex poisoned")
            .clone()
    }

    pub fn closed(&self) -> bool {
        self.inner.closed.load(Ordering::SeqCst)
    }

    pub fn snapshot(&self) -> TerminalSnapshot {
        let mut snapshot = match self.inner.engine.lock() {
            Ok(mut engine) => engine
                .snapshot(&self.inner.fallback_title)
                .unwrap_or_else(|error| TerminalSnapshot {
                    title: self.inner.fallback_title.clone(),
                    rows: vec![TerminalRow::plain(
                        format!("ghostty-vt snapshot failed: {error}"),
                        &TerminalTheme::default(),
                    )],
                    theme: TerminalTheme::default(),
                    cursor: (0, 0),
                    cursor_blinking: false,
                    cols: 0,
                    rows_count: 0,
                    scrollbar: TerminalScrollbar::default(),
                    alternate_screen: false,
                    application_cursor: false,
                    closed: false,
                    status: None,
                }),
            Err(_) => TerminalSnapshot {
                title: self.inner.fallback_title.clone(),
                rows: vec![TerminalRow::plain(
                    "ghostty-vt state mutex poisoned",
                    &TerminalTheme::default(),
                )],
                theme: TerminalTheme::default(),
                cursor: (0, 0),
                cursor_blinking: false,
                cols: 0,
                rows_count: 0,
                scrollbar: TerminalScrollbar::default(),
                alternate_screen: false,
                application_cursor: false,
                closed: false,
                status: None,
            },
        };

        snapshot.closed = self.inner.closed.load(Ordering::SeqCst);
        snapshot.status = self
            .inner
            .status
            .lock()
            .expect("terminal status mutex poisoned")
            .clone();
        snapshot
    }

    pub fn send_bytes(&self, bytes: &[u8]) -> Result<()> {
        self.inner.process.write_all(bytes)
    }

    pub fn send_text(&self, text: &str) -> Result<()> {
        self.send_bytes(text.as_bytes())
    }

    pub fn scroll_viewport_delta(&self, delta: isize) -> Result<()> {
        if delta == 0 {
            return Ok(());
        }

        self.inner
            .engine
            .lock()
            .expect("ghostty-vt engine mutex poisoned")
            .scroll_viewport_delta(delta)?;
        self.inner.revision.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    pub fn scroll_viewport_bottom(&self) -> Result<()> {
        self.inner
            .engine
            .lock()
            .expect("ghostty-vt engine mutex poisoned")
            .scroll_viewport_bottom()?;
        self.inner.revision.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    pub fn resize(&self, grid: TerminalGridSize) -> Result<()> {
        self.inner.process.resize(grid)?;
        self.inner
            .engine
            .lock()
            .expect("ghostty-vt engine mutex poisoned")
            .resize(grid)?;
        self.inner.revision.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    pub fn close(&self) {
        let _ = self.inner.process.kill();
        self.inner.closed.store(true, Ordering::SeqCst);
        *self
            .inner
            .status
            .lock()
            .expect("terminal status mutex poisoned") = Some("Terminated".to_string());
        self.inner.revision.fetch_add(1, Ordering::SeqCst);
    }
}

pub fn seed_workdesk(workdesk: &mut Workdesk) {
    let shell = TerminalPaneSpec::shell("Shell", Point::new(48.0, 48.0), Size::new(960.0, 600.0));
    let agent = TerminalPaneSpec::agent(
        "Agent",
        vec!["echo".to_string(), "agent pane placeholder".to_string()],
        Point::new(1060.0, 120.0),
        Size::new(720.0, 420.0),
    );

    for pane in [shell, agent] {
        workdesk.add_pane(pane.title, pane.kind, pane.position, pane.size);
    }
}

fn normalize_terminal_text(text: String) -> String {
    text.chars()
        .map(|ch| if ch == ' ' { '\u{00A0}' } else { ch })
        .collect()
}

fn color_from_rgb(color: GhosttyColorRgb) -> TerminalColor {
    TerminalColor::new(color.r, color.g, color.b)
}

fn resolve_style_color(
    color: GhosttyStyleColor,
    colors: &GhosttyRenderStateColors,
) -> Option<TerminalColor> {
    match color.tag {
        GhosttyStyleColorTag::None => None,
        GhosttyStyleColorTag::Palette => {
            let palette_index = unsafe { color.value.palette } as usize;
            colors
                .palette
                .get(palette_index)
                .copied()
                .map(color_from_rgb)
        }
        GhosttyStyleColorTag::Rgb => Some(color_from_rgb(unsafe { color.value.rgb })),
    }
}

fn resolve_cell_style(
    style: GhosttyStyle,
    colors: &GhosttyRenderStateColors,
    theme: &TerminalTheme,
) -> TerminalTextStyle {
    let default_fg = theme.foreground;
    let default_bg = theme.background;

    let mut foreground = resolve_style_color(style.fg_color, colors).unwrap_or(default_fg);
    let mut background = resolve_style_color(style.bg_color, colors);

    if style.inverse {
        let resolved_bg = background.unwrap_or(default_bg);
        background = Some(foreground);
        foreground = resolved_bg;
    }

    if style.invisible {
        foreground = background.unwrap_or(default_bg);
    }

    let underline_color = resolve_style_color(style.underline_color, colors).or(Some(foreground));

    let text_style = TerminalTextStyle {
        foreground,
        background: background.filter(|color| *color != default_bg),
        underline_color,
        bold: style.bold,
        italic: style.italic,
        faint: style.faint,
        underline: style.underline != 0,
        strikethrough: style.strikethrough,
    };

    text_style
}

pub fn default_process_spec(kind: &PaneKind) -> ProcessSpec {
    match kind {
        PaneKind::Shell => ProcessSpec::login_shell(),
        PaneKind::Agent => ProcessSpec::agent_shell(),
        PaneKind::Browser | PaneKind::Editor => ProcessSpec::login_shell(),
    }
}

pub fn grid_size_for_pane(size: Size) -> TerminalGridSize {
    let cols = ((size.width - TERMINAL_HORIZONTAL_PADDING) / TERMINAL_CELL_WIDTH)
        .floor()
        .max(f32::from(MIN_TERMINAL_COLS)) as u16;
    let rows = ((size.height - TERMINAL_VERTICAL_CHROME) / TERMINAL_CELL_HEIGHT)
        .floor()
        .max(f32::from(MIN_TERMINAL_ROWS)) as u16;

    TerminalGridSize::new(cols, rows)
}

pub fn spawn_terminal_session(
    kind: &PaneKind,
    title: impl Into<String>,
    size: Size,
) -> Result<TerminalSession> {
    if !kind.is_terminal() {
        return Err(anyhow!(
            "surface kind {:?} does not support terminal sessions",
            kind
        ));
    }
    let title = title.into();
    TerminalSession::spawn(&default_process_spec(kind), title, grid_size_for_pane(size))
}

pub fn spawn_terminal_session_with_grid(
    kind: &PaneKind,
    title: impl Into<String>,
    grid: TerminalGridSize,
) -> Result<TerminalSession> {
    if !kind.is_terminal() {
        return Err(anyhow!(
            "surface kind {:?} does not support terminal sessions",
            kind
        ));
    }
    let title = title.into();
    TerminalSession::spawn(&default_process_spec(kind), title, grid)
}

pub fn ghostty_build_info() -> GhosttyBuildInfo {
    ghostty_vt_build_info()
}

fn ghostty_check(label: &str, result: i32) -> Result<()> {
    if result == GHOSTTY_SUCCESS {
        return Ok(());
    }

    let reason = match result {
        ghostty_sys::GHOSTTY_OUT_OF_MEMORY => "out of memory",
        ghostty_sys::GHOSTTY_INVALID_VALUE => "invalid value",
        ghostty_sys::GHOSTTY_OUT_OF_SPACE => "out of space",
        _ => "unknown ghostty error",
    };

    Err(anyhow!("{label} failed: {reason} ({result})"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ghostty_snapshot_preserves_ansi_colors_and_styles() {
        let mut engine = GhosttyEngine::new(TerminalGridSize::new(24, 4)).unwrap();
        engine
            .write(
                b"plain \x1b[1;38;2;0;205;0mgreen\x1b[0m \x1b[4;38;2;255;128;0morange\x1b[0m\r\n\
\x1b[48;2;40;60;80;38;2;220;226;232mblock\x1b[0m\r\n",
            )
            .unwrap();

        let snapshot = engine.snapshot("test").unwrap();

        let first_row = &snapshot.rows[0];
        assert!(first_row.runs.iter().any(|run| {
            run.text.contains("green")
                && run.style.foreground == TerminalColor::new(0, 205, 0)
                && run.style.bold
        }));
        assert!(first_row.runs.iter().any(|run| {
            run.text.contains("orange")
                && run.style.foreground == TerminalColor::new(0xff, 0x80, 0x00)
                && run.style.underline
        }));

        let second_row = &snapshot.rows[1];
        assert!(second_row.runs.iter().any(|run| {
            run.text.contains("block")
                && run.style.background == Some(TerminalColor::new(40, 60, 80))
                && run.style.foreground == TerminalColor::new(220, 226, 232)
        }));
    }
}
