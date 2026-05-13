//! Interactive TTY chat UI: status bar pinned at the bottom of the terminal,
//! multi-line input area above it, chat history scrolling in the region above
//! that.
//!
//! ## Architecture
//!
//! Three concurrent tokio tasks (plus the caller's `join.rs` event loop):
//!
//! - **Renderer task** (`render_loop`): sole writer to stdout. Receives
//!   [`UiOp`]s (incoming chat messages to print into the scroll region,
//!   input-area repaint requests, resize events, shutdown signal) and
//!   `StatusState::dirty` notifications. Coalesces work into ~30 fps idle
//!   redraws.
//! - **Keyboard task** (`keyboard_loop`): reads `crossterm::event::Event`s,
//!   feeds them to [`InputEditor`], and sends:
//!   - to the renderer: an `InputRedraw` op so the cursor/text repaints
//!   - to the consumer (`InteractiveUi::next_input`): a `UiInput` event when
//!     the user submits a line, hits Ctrl-C, or hits Ctrl-D on empty input
//! - The **consumer** (`join.rs`) pulls `UiInput`s via `next_input()` and
//!   pushes messages-to-render through `render_message` / `render_system`
//!   (which produce `UiOp::Message` / `UiOp::System`).
//!
//! The scroll region (DECSTBM) reserves the bottom rows of the terminal for
//! the bar + input. Stdout writes into the upper region are managed by the
//! renderer with `MoveTo(0, region_bottom)` + `Print(line)` + `\n`; the
//! terminal handles the scroll. After every write the renderer repaints the
//! status bar and input area, then `MoveTo`s the cursor back to the editor
//! position. This way an inbound message never disturbs what the user is
//! typing.

use std::collections::HashSet;
use std::io::{Stdout, Write, stdout};
use std::sync::Arc;

use crossterm::{
    cursor,
    event::{Event, EventStream},
    queue,
    style::{Color, ResetColor, SetBackgroundColor, SetForegroundColor},
    terminal::{self, Clear, ClearType},
};
use futures::StreamExt;
use futures::future::BoxFuture;
use tokio::sync::{Mutex, RwLock, mpsc};
use tokio::task::JoinHandle;

use crate::cmd::chat::display::{DisplayMessage, render_message_line};
use crate::cmd::chat::tui::commands;
use crate::cmd::chat::tui::input::{EditOutcome, InputEditor};
use crate::cmd::chat::tui::status::{self, SlotWidths, StatusState};
use crate::cmd::chat::tui::{ChatUi, IgnoreSet, UiInput, UiOptions};

/// Renderer ops. Funneled through a single mpsc so only the renderer task
/// writes to stdout.
enum UiOp {
    /// Print a chat message into the scroll region.
    Message(String),
    /// Print a system notice into the scroll region.
    System(String),
    /// Repaint the input area (cursor moved, text changed, etc.).
    InputRedraw,
    /// Full repaint (terminal resize, Ctrl-L).
    FullRepaint,
    /// Renderer should exit.
    Shutdown,
}

/// Snapshot of the editor state, passed renderer-bound so the renderer
/// doesn't need a lock on the editor.
#[derive(Clone, Default)]
struct EditorSnapshot {
    lines: Vec<String>,
    row: usize,
    col: usize,
}

/// Public interactive UI handle. Owns the renderer + keyboard tasks; cleanup
/// on `shutdown` restores the terminal.
pub struct InteractiveUi {
    status: Arc<StatusState>,
    ignore: IgnoreSet,
    ops_tx: mpsc::UnboundedSender<UiOp>,
    input_rx: Mutex<mpsc::UnboundedReceiver<UiInput>>,
    renderer_handle: Option<JoinHandle<()>>,
    keyboard_handle: Option<JoinHandle<()>>,
    /// Shared editor state — written by the keyboard task, read (and only
    /// read) by the renderer task to paint the input area.
    editor_view: Arc<RwLock<EditorSnapshot>>,
}

impl InteractiveUi {
    /// Attempt to enter interactive mode. Returns `Err` (with the original
    /// error message) if the terminal does not support the required
    /// operations — the factory will fall back to line mode.
    ///
    /// **Synchronously** completes the terminal setup (raw mode, scroll
    /// region, initial paint) before returning, so by the time the caller
    /// gets a handle the bottom rows are already claimed by the bar + input
    /// area. Without this, the renderer task (started via `tokio::spawn`)
    /// could be scheduled later than the caller's first `render_system`
    /// call, and although the queued messages would still be processed
    /// in-order, any *third-party* stderr write from a spawned task in the
    /// gap would land at the cursor wherever the shell left it.
    pub fn new(opts: &UiOptions) -> Result<Self, String> {
        let status = StatusState::new(opts.channel_name.clone());
        let ignore: IgnoreSet = Arc::new(RwLock::new(HashSet::new()));

        let (ops_tx, ops_rx) = mpsc::unbounded_channel::<UiOp>();
        let (input_tx, input_rx) = mpsc::unbounded_channel::<UiInput>();

        let editor_view = Arc::new(RwLock::new(EditorSnapshot {
            lines: vec![String::new()],
            row: 0,
            col: 0,
        }));

        // Do the terminal setup *synchronously* on this thread (we're inside
        // a sync `new`, called from an async context). The TerminalGuard +
        // initial layout happen here; the spawned renderer task only owns
        // the steady-state paint loop. This guarantees that by the time
        // `new()` returns, the scroll region is already in place.
        use crate::cmd::chat::tui::terminal::TerminalGuard;
        let mut guard =
            TerminalGuard::enter().map_err(|e| format!("terminal init failed: {e}"))?;
        let (cols, rows) =
            crossterm::terminal::size().map_err(|e| format!("terminal::size failed: {e}"))?;
        let input_height: u16 = 1;
        // Reserve the bottom 1+input_height rows.
        let reserved = 1 + input_height;
        let region_bottom = if reserved < rows { rows - reserved } else { 1 };
        guard
            .set_scroll_region(1, region_bottom.max(1))
            .map_err(|e| format!("scroll region setup failed: {e}"))?;

        // Initial paint of status bar + input area so the divider is visible
        // immediately. Errors here aren't fatal — we'll just look ugly.
        {
            let mut out = stdout();
            let mut slots = SlotWidths::default();
            let snap = EditorSnapshot {
                lines: vec![String::new()],
                row: 0,
                col: 0,
            };
            let _ = paint_status_and_input(
                &mut out, &status, cols, rows, input_height, &snap, &mut slots,
            );
        }

        let renderer_status = status.clone();
        let renderer_editor = editor_view.clone();
        let renderer_handle = tokio::spawn(async move {
            if let Err(e) = render_loop(
                ops_rx,
                renderer_status,
                renderer_editor,
                guard,
                cols,
                rows,
                input_height,
            )
            .await
            {
                // Renderer error: TerminalGuard's drop will fire from inside
                // the failing await chain, so the terminal restores cleanly.
                eprintln!("*** interactive renderer error: {e}");
            }
        });

        let ops_tx_kb = ops_tx.clone();
        let editor_view_kb = editor_view.clone();
        let keyboard_handle = tokio::spawn(async move {
            keyboard_loop(input_tx, ops_tx_kb, editor_view_kb).await;
        });

        Ok(Self {
            status,
            ignore,
            ops_tx,
            input_rx: Mutex::new(input_rx),
            renderer_handle: Some(renderer_handle),
            keyboard_handle: Some(keyboard_handle),
            editor_view,
        })
    }
}

impl ChatUi for InteractiveUi {
    fn render_message(&self, msg: &DisplayMessage) {
        let rendered = render_message_line(msg);
        for notice in rendered.system_notices {
            let _ = self.ops_tx.send(UiOp::System(notice));
        }
        let _ = self.ops_tx.send(UiOp::Message(rendered.message_line));
    }

    fn render_system(&self, line: &str) {
        // System notices may already contain embedded newlines (e.g. the
        // multi-line ignore-list dump from `dispatch_slash`). Split so each
        // line independently scrolls into the region — otherwise the renderer
        // would `\n` once and rely on terminal wrapping for the rest, which
        // looks ragged.
        for sub in line.split('\n') {
            let _ = self.ops_tx.send(UiOp::System(sub.to_string()));
        }
    }

    fn status(&self) -> Arc<StatusState> {
        self.status.clone()
    }

    fn ignore_set(&self) -> IgnoreSet {
        self.ignore.clone()
    }

    fn next_input(&mut self) -> BoxFuture<'_, Option<UiInput>> {
        Box::pin(async move {
            let mut rx = self.input_rx.lock().await;
            rx.recv().await
        })
    }

    fn shutdown(mut self: Box<Self>) -> BoxFuture<'static, ()> {
        Box::pin(async move {
            let _ = self.ops_tx.send(UiOp::Shutdown);
            if let Some(h) = self.keyboard_handle.take() {
                h.abort();
                let _ = h.await;
            }
            if let Some(h) = self.renderer_handle.take() {
                let _ = tokio::time::timeout(std::time::Duration::from_millis(200), h).await;
            }
            // Belt-and-suspenders: even if the renderer's TerminalGuard didn't
            // drop cleanly (e.g. it panicked), the panic hook installed inside
            // `terminal::enter` will have run. Nothing to do here.
            // Drop reference to the editor so the keyboard task's snapshot
            // owner is the only one left; the keyboard task's abort releases
            // it on its own.
            let _ = self.editor_view;
        })
    }
}

// ===== Renderer task =====

/// Renderer entry point. Owns the terminal guard (already set up by
/// [`InteractiveUi::new`]), the editor view, and the status state. Returns
/// only after receiving [`UiOp::Shutdown`] or on a fatal I/O error.
async fn render_loop(
    mut ops_rx: mpsc::UnboundedReceiver<UiOp>,
    status: Arc<StatusState>,
    editor: Arc<RwLock<EditorSnapshot>>,
    mut guard: crate::cmd::chat::tui::terminal::TerminalGuard,
    mut cols: u16,
    mut rows: u16,
    mut input_height: u16,
) -> std::io::Result<()> {
    let mut out = stdout();
    let mut slots = SlotWidths::default();

    // Cache the last-rendered status snapshot so the idle timer arm can
    // detect "the rendered bar would now differ" (e.g. the `recv_active`
    // flash just decayed back to false) and trigger a repaint. Without
    // this, the flash would stay on screen until the next inbound message
    // forces a paint.
    let mut last_rendered: Option<status::StatusSnapshot> = None;

    loop {
        tokio::select! {
            biased;
            op = ops_rx.recv() => {
                let Some(op) = op else { break };
                match op {
                    UiOp::Shutdown => break,
                    UiOp::Message(line) | UiOp::System(line) => {
                        write_into_scroll_region(&mut out, &line, rows, input_height)?;
                        // After a scroll-region write the cursor sits at the
                        // bottom of the region; we still need to repaint the
                        // status bar (in case `Receiving...` count changed)
                        // and put the cursor back in the input area.
                        let editor_snap = editor.read().await.clone();
                        paint_status_and_input(
                            &mut out, &status, cols, rows, input_height, &editor_snap, &mut slots,
                        )?;
                        last_rendered = Some(status.snapshot());
                    }
                    UiOp::InputRedraw => {
                        let editor_snap = editor.read().await.clone();
                        let needed = compute_input_height(&editor_snap, rows);
                        if needed != input_height {
                            input_height = needed;
                            apply_layout(&mut guard, &mut out, cols, rows, input_height)?;
                            paint_full(
                                &mut out, &status, cols, rows, input_height, &editor_snap, &mut slots,
                            )?;
                        } else {
                            paint_status_and_input(
                                &mut out, &status, cols, rows, input_height, &editor_snap, &mut slots,
                            )?;
                        }
                        last_rendered = Some(status.snapshot());
                    }
                    UiOp::FullRepaint => {
                        let new_size = terminal::size()?;
                        cols = new_size.0;
                        rows = new_size.1;
                        slots.reset();
                        let editor_snap = editor.read().await.clone();
                        input_height = compute_input_height(&editor_snap, rows);
                        // Reset the scroll region for the new geometry, then
                        // clear the entire visible screen and repaint. This
                        // is necessary on resize because the old status-bar
                        // and input-area text is at the OLD (row, col)
                        // positions — when `cols` shrinks, those characters
                        // remain visible past the new bar's right edge; when
                        // `rows` changes, the old bar lingers above or below
                        // the new bar's position. Clearing the visible
                        // screen wipes those artifacts. Chat history is
                        // preserved in the terminal's native scrollback
                        // (above the visible region) and remains reachable
                        // via mouse wheel / PgUp.
                        apply_layout(&mut guard, &mut out, cols, rows, input_height)?;
                        crossterm::queue!(
                            out,
                            cursor::MoveTo(0, 0),
                            Clear(ClearType::All),
                        )?;
                        paint_full(
                            &mut out, &status, cols, rows, input_height, &editor_snap, &mut slots,
                        )?;
                        last_rendered = Some(status.snapshot());
                    }
                }
            }
            _ = status.dirty.notified() => {
                let editor_snap = editor.read().await.clone();
                paint_status_and_input(
                    &mut out, &status, cols, rows, input_height, &editor_snap, &mut slots,
                )?;
                last_rendered = Some(status.snapshot());
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {
                // Idle tick: if the snapshot would now render differently
                // from the last paint (e.g. the recv_active flash just
                // decayed), repaint. Skipping the paint when nothing
                // changed keeps the terminal quiet between activity.
                let snap = status.snapshot();
                if last_rendered.as_ref() != Some(&snap) {
                    let editor_snap = editor.read().await.clone();
                    paint_status_and_input(
                        &mut out, &status, cols, rows, input_height, &editor_snap, &mut slots,
                    )?;
                    last_rendered = Some(snap);
                }
            }
        }
    }

    // TerminalGuard restores the terminal on drop.
    drop(guard);
    Ok(())
}

/// Compute desired input area height for the given editor snapshot, capped
/// at half the screen.
fn compute_input_height(snap: &EditorSnapshot, rows: u16) -> u16 {
    let want = snap.lines.len().max(1) as u16;
    let cap = (rows / 2).max(1);
    want.min(cap)
}

/// Apply (or re-apply) the scroll region for the current input height. The
/// bottom `1 + input_height` rows become the status bar + input area; the
/// rest is the chat scroll region.
fn apply_layout(
    guard: &mut crate::cmd::chat::tui::terminal::TerminalGuard,
    _out: &mut Stdout,
    _cols: u16,
    rows: u16,
    input_height: u16,
) -> std::io::Result<()> {
    let reserved = 1 + input_height;
    if reserved >= rows {
        // Pathological: terminal too short. Drop the bar; use the last row
        // for input only.
        guard.set_scroll_region(1, rows.saturating_sub(1).max(1))?;
        return Ok(());
    }
    let region_bottom = rows - reserved;
    guard.set_scroll_region(1, region_bottom)?;
    Ok(())
}

/// Full repaint: clears the bar + input rows then paints both.
fn paint_full(
    out: &mut Stdout,
    status: &StatusState,
    cols: u16,
    rows: u16,
    input_height: u16,
    editor: &EditorSnapshot,
    slots: &mut SlotWidths,
) -> std::io::Result<()> {
    // Clear status + input rows (just paint them fresh).
    paint_status_and_input(out, status, cols, rows, input_height, editor, slots)
}

fn paint_status_and_input(
    out: &mut Stdout,
    status: &StatusState,
    cols: u16,
    rows: u16,
    input_height: u16,
    editor: &EditorSnapshot,
    slots: &mut SlotWidths,
) -> std::io::Result<()> {
    paint_status_bar(out, status, cols, rows, input_height, slots)?;
    paint_input_area(out, cols, rows, input_height, editor)?;
    Ok(())
}

fn paint_status_bar(
    out: &mut Stdout,
    status: &StatusState,
    cols: u16,
    rows: u16,
    input_height: u16,
    slots: &mut SlotWidths,
) -> std::io::Result<()> {
    let snap = status.snapshot();
    let level = status::pick_level(&snap, cols as usize);
    let bar = status::render_bar(&snap, level, cols as usize, slots);

    // Row index (1-based for DECSTBM, 0-based for crossterm). The bar lives
    // at `rows - input_height - 1` in 0-based coords.
    let bar_row = rows.saturating_sub(input_height + 1);
    // Clear the row before painting so that on a terminal resize (when the
    // previous bar was wider, in different columns, or at a different row
    // index) no leftover bytes remain past the new bar's right edge. We
    // then reset background to default — the grey bar paint that follows
    // will set its own background; the cleared area outside `cols` becomes
    // terminal-default rather than stale grey.
    queue!(
        out,
        cursor::Hide,
        cursor::MoveTo(0, bar_row),
        ResetColor,
        Clear(ClearType::CurrentLine),
        SetBackgroundColor(Color::Grey),
        SetForegroundColor(Color::Black),
    )?;
    // Write directly; `bar` is already `cols` wide.
    out.write_all(bar.as_bytes())?;
    queue!(out, ResetColor)?;
    Ok(())
}

fn paint_input_area(
    out: &mut Stdout,
    cols: u16,
    rows: u16,
    input_height: u16,
    editor: &EditorSnapshot,
) -> std::io::Result<()> {
    let first_row = rows.saturating_sub(input_height);
    // Render up to `input_height` editor lines, starting from the row
    // containing the cursor and walking back. If there are fewer logical
    // lines than `input_height`, pad with blanks.
    let editor_lines = &editor.lines;
    let total = editor_lines.len();
    // Determine the window of editor lines to display: keep the cursor
    // visible. Strategy: start at line 0 if `total <= input_height`,
    // otherwise scroll so the cursor row is the last visible line.
    let start = if total as u16 <= input_height {
        0
    } else if editor.row as u16 >= input_height {
        editor.row as u16 - (input_height - 1)
    } else {
        0
    };
    for i in 0..input_height {
        let row = first_row + i;
        queue!(
            out,
            cursor::MoveTo(0, row),
            Clear(ClearType::CurrentLine),
        )?;
        let line_idx = start as usize + i as usize;
        if line_idx < total {
            let line = &editor_lines[line_idx];
            // Truncate to cols-1 to keep room for the cursor at end-of-line.
            let max_len = cols.saturating_sub(1) as usize;
            let truncated: String = line.chars().take(max_len).collect();
            out.write_all(truncated.as_bytes())?;
        }
    }
    // Position cursor.
    let cursor_row_window = (editor.row as u16).saturating_sub(start);
    let cursor_col = (editor.col as u16).min(cols.saturating_sub(1));
    let cursor_row = first_row + cursor_row_window.min(input_height.saturating_sub(1));
    queue!(out, cursor::MoveTo(cursor_col, cursor_row), cursor::Show)?;
    out.flush()?;
    Ok(())
}

/// Print `line` into the chat scroll region. The terminal handles the
/// scroll for us — we just `MoveTo` the last row of the region and emit the
/// text plus `\n`.
fn write_into_scroll_region(
    out: &mut Stdout,
    line: &str,
    rows: u16,
    input_height: u16,
) -> std::io::Result<()> {
    let region_bottom_zero = rows.saturating_sub(input_height + 2);
    queue!(
        out,
        cursor::Hide,
        cursor::MoveTo(0, region_bottom_zero),
        ResetColor,
    )?;
    // Write content (truncate visually if needed; the terminal will wrap
    // otherwise, which is fine for chat history).
    out.write_all(line.as_bytes())?;
    // Newline so the next message starts on a fresh line; this triggers the
    // scroll within the region.
    out.write_all(b"\r\n")?;
    out.flush()?;
    Ok(())
}

// ===== Keyboard task =====

async fn keyboard_loop(
    input_tx: mpsc::UnboundedSender<UiInput>,
    ops_tx: mpsc::UnboundedSender<UiOp>,
    editor_view: Arc<RwLock<EditorSnapshot>>,
) {
    let mut editor = InputEditor::new();
    let mut events = EventStream::new();
    while let Some(event) = events.next().await {
        let event = match event {
            Ok(e) => e,
            Err(_) => continue,
        };
        match event {
            Event::Key(k) => {
                let outcome = editor.handle_key(k);
                match outcome {
                    EditOutcome::Submit(text) => {
                        // Snapshot the cleared editor and trigger a redraw
                        // so the input area visibly empties before the
                        // server round-trip.
                        publish_view(&editor_view, &editor).await;
                        let _ = ops_tx.send(UiOp::InputRedraw);
                        // Slash command? Parse, otherwise it's a chat message.
                        let trimmed = text.trim().to_string();
                        if trimmed.is_empty() {
                            continue;
                        }
                        let ev = match commands::parse(&trimmed) {
                            Some(cmd) => UiInput::Command(cmd),
                            None => UiInput::Message(trimmed),
                        };
                        if input_tx.send(ev).is_err() {
                            return;
                        }
                    }
                    EditOutcome::Interrupt => {
                        let _ = input_tx.send(UiInput::Interrupt);
                        return;
                    }
                    EditOutcome::Eof => {
                        let _ = input_tx.send(UiInput::Eof);
                        // Don't return — user may continue if --stay-after-eof.
                    }
                    EditOutcome::Redraw => {
                        publish_view(&editor_view, &editor).await;
                        let _ = ops_tx.send(UiOp::InputRedraw);
                    }
                    EditOutcome::ForceRepaint => {
                        publish_view(&editor_view, &editor).await;
                        let _ = ops_tx.send(UiOp::FullRepaint);
                    }
                    EditOutcome::Noop => {}
                }
            }
            Event::Paste(s) => {
                editor.insert_str(&s);
                publish_view(&editor_view, &editor).await;
                let _ = ops_tx.send(UiOp::InputRedraw);
            }
            Event::Resize(_, _) => {
                let _ = ops_tx.send(UiOp::FullRepaint);
            }
            _ => {}
        }
    }
}

async fn publish_view(view: &Arc<RwLock<EditorSnapshot>>, editor: &InputEditor) {
    let (row, col) = editor.cursor();
    let lines = editor.lines().to_vec();
    let mut w = view.write().await;
    w.lines = lines;
    w.row = row;
    w.col = col;
}
