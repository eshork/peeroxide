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

use std::collections::{HashSet, VecDeque};
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
    /// Show a transient overlay text on the status-bar row for `duration`.
    /// While active the overlay replaces the normal bar with yellow-on-black
    /// styling. Used by the keyboard task to surface the "press Ctrl-C
    /// again…" prompt without disturbing the chat scrollback. A new overlay
    /// while one is already active simply replaces it (new deadline).
    ShowTransientOverlay { text: String, duration: std::time::Duration },
    /// Clear any active transient overlay (e.g. user typed something so the
    /// armed Ctrl-C window should be cancelled).
    ClearTransientOverlay,
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
                &mut out, &status, cols, rows, input_height, &snap, &mut slots, None,
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

    // In-memory chat-history ring buffer. Every `Message` / `System` line we
    // write into the scroll region is pushed here too. On `FullRepaint`
    // (terminal resize / Ctrl-L) we replay the tail of this buffer into the
    // freshly-laid-out scroll region so the user's visible chat history
    // survives the resize, instead of being wiped along with stale bar /
    // input artifacts.
    //
    // Bounded to `HISTORY_CAP` lines. A chat session can run for hours; we
    // only need enough to refill the largest reasonable terminal a few
    // times. 10_000 is loose-enough to cover even an enormous screen and
    // cheap in memory (a few MB worst-case).
    let mut history: VecDeque<String> = VecDeque::with_capacity(HISTORY_CAP);

    // Cache the last-rendered status snapshot so the idle timer arm can
    // detect "the rendered bar would now differ" (e.g. the `recv_active`
    // flash just decayed back to false) and trigger a repaint. Without
    // this, the flash would stay on screen until the next inbound message
    // forces a paint.
    let mut last_rendered: Option<status::StatusSnapshot> = None;

    // Transient overlay painted in place of the status bar (e.g. the
    // "press Ctrl-C again…" prompt). Cleared automatically once
    // `expires_at` has elapsed (the idle tick checks every ~100ms).
    let mut transient_overlay: Option<(String, std::time::Instant)> = None;

    loop {
        tokio::select! {
            biased;
            op = ops_rx.recv() => {
                let Some(op) = op else { break };
                match op {
                    UiOp::Shutdown => break,
                    UiOp::Message(line) | UiOp::System(line) => {
                        write_into_scroll_region(&mut out, &line, rows, input_height)?;
                        push_history(&mut history, line);
                        // After a scroll-region write the cursor sits at the
                        // bottom of the region; we still need to repaint the
                        // status bar (in case `Receiving...` count changed)
                        // and put the cursor back in the input area.
                        let editor_snap = editor.read().await.clone();
                        paint_status_and_input(
                            &mut out, &status, cols, rows, input_height, &editor_snap, &mut slots,
                            overlay_text(&transient_overlay),
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
                                overlay_text(&transient_overlay),
                            )?;
                        } else {
                            paint_status_and_input(
                                &mut out, &status, cols, rows, input_height, &editor_snap, &mut slots,
                                overlay_text(&transient_overlay),
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
                        apply_layout(&mut guard, &mut out, cols, rows, input_height)?;
                        crossterm::queue!(
                            out,
                            cursor::MoveTo(0, 0),
                            Clear(ClearType::All),
                        )?;
                        replay_history(&mut out, &history, rows, input_height)?;
                        paint_full(
                            &mut out, &status, cols, rows, input_height, &editor_snap, &mut slots,
                            overlay_text(&transient_overlay),
                        )?;
                        last_rendered = Some(status.snapshot());
                    }
                    UiOp::ShowTransientOverlay { text, duration } => {
                        transient_overlay = Some((text, std::time::Instant::now() + duration));
                        let editor_snap = editor.read().await.clone();
                        paint_status_and_input(
                            &mut out, &status, cols, rows, input_height, &editor_snap, &mut slots,
                            overlay_text(&transient_overlay),
                        )?;
                        // Don't touch last_rendered — the next status-based
                        // tick should still trigger a real paint if the bar
                        // would otherwise differ.
                    }
                    UiOp::ClearTransientOverlay => {
                        if transient_overlay.is_some() {
                            transient_overlay = None;
                            let editor_snap = editor.read().await.clone();
                            paint_status_and_input(
                                &mut out, &status, cols, rows, input_height, &editor_snap, &mut slots,
                                None,
                            )?;
                        }
                    }
                }
            }
            _ = status.dirty.notified() => {
                let editor_snap = editor.read().await.clone();
                paint_status_and_input(
                    &mut out, &status, cols, rows, input_height, &editor_snap, &mut slots,
                    overlay_text(&transient_overlay),
                )?;
                last_rendered = Some(status.snapshot());
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {
                // Auto-expire the transient overlay if its deadline has
                // passed. Triggers a repaint of the normal bar.
                if let Some((_, expires_at)) = transient_overlay
                    && std::time::Instant::now() >= expires_at
                {
                    transient_overlay = None;
                    let editor_snap = editor.read().await.clone();
                    paint_status_and_input(
                        &mut out, &status, cols, rows, input_height, &editor_snap, &mut slots,
                        None,
                    )?;
                    last_rendered = Some(status.snapshot());
                    continue;
                }

                // Idle tick: if the snapshot would now render differently
                // from the last paint (e.g. the recv_active flash just
                // decayed), repaint. Skipping the paint when nothing
                // changed keeps the terminal quiet between activity.
                let snap = status.snapshot();
                if last_rendered.as_ref() != Some(&snap) {
                    let editor_snap = editor.read().await.clone();
                    paint_status_and_input(
                        &mut out, &status, cols, rows, input_height, &editor_snap, &mut slots,
                        overlay_text(&transient_overlay),
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

/// Project an `Option<(String, Instant)>` to an `Option<&str>` for the
/// paint helpers. Returns `None` if the overlay has already expired so the
/// callers paint the normal bar even if the auto-expire tick hasn't fired
/// yet (defensive — the tick should clear it within ~100 ms anyway).
fn overlay_text(overlay: &Option<(String, std::time::Instant)>) -> Option<&str> {
    overlay
        .as_ref()
        .filter(|(_, expires_at)| std::time::Instant::now() < *expires_at)
        .map(|(text, _)| text.as_str())
}

/// Full repaint: clears the bar + input rows then paints both.
#[allow(clippy::too_many_arguments)]
fn paint_full(
    out: &mut Stdout,
    status: &StatusState,
    cols: u16,
    rows: u16,
    input_height: u16,
    editor: &EditorSnapshot,
    slots: &mut SlotWidths,
    overlay: Option<&str>,
) -> std::io::Result<()> {
    // Clear status + input rows (just paint them fresh).
    paint_status_and_input(out, status, cols, rows, input_height, editor, slots, overlay)
}

#[allow(clippy::too_many_arguments)]
fn paint_status_and_input(
    out: &mut Stdout,
    status: &StatusState,
    cols: u16,
    rows: u16,
    input_height: u16,
    editor: &EditorSnapshot,
    slots: &mut SlotWidths,
    overlay: Option<&str>,
) -> std::io::Result<()> {
    paint_status_bar(out, status, cols, rows, input_height, slots, overlay)?;
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
    overlay: Option<&str>,
) -> std::io::Result<()> {
    // Row index (1-based for DECSTBM, 0-based for crossterm). The bar lives
    // at `rows - input_height - 1` in 0-based coords.
    let bar_row = rows.saturating_sub(input_height + 1);
    // Clear the row before painting so that on a terminal resize (when the
    // previous bar was wider, in different columns, or at a different row
    // index) no leftover bytes remain past the new bar's right edge.
    queue!(
        out,
        cursor::Hide,
        cursor::MoveTo(0, bar_row),
        ResetColor,
        Clear(ClearType::CurrentLine),
    )?;

    if let Some(text) = overlay {
        // Transient overlay: yellow background, black foreground. Pad/
        // truncate to exactly `cols` so the row is fully covered.
        let body = fit_overlay(text, cols as usize);
        queue!(
            out,
            SetBackgroundColor(Color::Yellow),
            SetForegroundColor(Color::Black),
        )?;
        out.write_all(body.as_bytes())?;
    } else {
        // Normal status bar (grey).
        let snap = status.snapshot();
        let level = status::pick_level(&snap, cols as usize);
        let bar = status::render_bar(&snap, level, cols as usize, slots);
        queue!(
            out,
            SetBackgroundColor(Color::Grey),
            SetForegroundColor(Color::Black),
        )?;
        out.write_all(bar.as_bytes())?;
    }
    queue!(out, ResetColor)?;
    Ok(())
}

/// Pure helper: format a transient overlay text to exactly `cols` cells
/// (truncate if too long, right-pad with spaces if too short). One space
/// of left padding for visual breathing room when there's room.
fn fit_overlay(text: &str, cols: usize) -> String {
    if cols == 0 {
        return String::new();
    }
    // Truncate by char count, then pad with spaces. We don't have a true
    // visible-width crate in scope; chat content is ASCII-dominant so
    // chars().count() approximates well for our overlay text.
    let with_pad = format!(" {text} ");
    let n = with_pad.chars().count();
    if n >= cols {
        with_pad.chars().take(cols).collect()
    } else {
        let mut s = with_pad;
        for _ in 0..(cols - n) {
            s.push(' ');
        }
        s
    }
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

/// Maximum number of chat lines kept in the in-memory history buffer.
/// Lines older than this are evicted FIFO as new ones are pushed.
///
/// Sized to comfortably cover any plausible terminal height (a 4K display
/// at a tiny font is on the order of 250-300 rows) with headroom. Bigger
/// values don't help — only the tail up to `region_height` is ever
/// replayed; older history is never visible again once it falls past the
/// top of the region.
const HISTORY_CAP: usize = 500;

/// Push a chat line onto the bounded history buffer, evicting the oldest
/// line when the capacity is reached.
fn push_history(history: &mut VecDeque<String>, line: String) {
    if history.len() == HISTORY_CAP {
        history.pop_front();
    }
    history.push_back(line);
}

/// Replay the tail of the in-memory chat history into the new scroll region.
/// Called from the `FullRepaint` arm after the screen has been cleared and
/// the scroll region applied for the new geometry.
///
/// Each replayed line is emitted exactly the way fresh chat messages are
/// written by [`write_into_scroll_region`]: `MoveTo` the bottom row of the
/// region, write the bytes, then `\r\n`. The terminal handles wrap-on-
/// overflow and scroll-within-region naturally — the same machinery that
/// handles in-flight messages — so a long line that no longer fits in the
/// new `cols` simply wraps to multiple visible rows and the oldest replayed
/// content scrolls past the top of the region (which is fine: at most
/// `region_height` rows can ever be visible at once).
///
/// We replay exactly `region_height` lines (or all of history, whichever
/// is smaller). That's the most that could ever be visible at one time;
/// any additional lines would just scroll off the top and add latency
/// without changing the final visible state.
fn replay_history(
    out: &mut Stdout,
    history: &VecDeque<String>,
    rows: u16,
    input_height: u16,
) -> std::io::Result<()> {
    let region_bottom_zero = rows.saturating_sub(input_height + 2);
    let region_height = (region_bottom_zero + 1) as usize;
    if history.is_empty() || region_height == 0 {
        return Ok(());
    }
    let replay_count = replay_count(history.len(), region_height);
    let start = history.len() - replay_count;
    queue!(out, cursor::Hide, ResetColor)?;
    for line in history.iter().skip(start) {
        queue!(out, cursor::MoveTo(0, region_bottom_zero), ResetColor)?;
        out.write_all(line.as_bytes())?;
        out.write_all(b"\r\n")?;
    }
    out.flush()?;
    Ok(())
}

/// Pure helper: how many history entries [`replay_history`] should replay
/// given the buffer length and the new region height. At most
/// `region_height` lines can be visible in the region at once, so that's
/// the natural upper bound; extras would only scroll off the top.
fn replay_count(history_len: usize, region_height: usize) -> usize {
    history_len.min(region_height)
}

// ===== Keyboard task =====

/// Text shown when the user presses Ctrl-C on an empty input buffer, arming
/// the 2-second force-quit window.
const CTRL_C_ARM_OVERLAY: &str =
    "*** press Ctrl-C again within 2 seconds to force quit — press Ctrl-D for graceful exit";

/// How long the force-quit arming stays hot after the first empty-input
/// Ctrl-C. A second Ctrl-C inside this window triggers `UiInput::Interrupt`;
/// after expiry a fresh double-press is required.
const CTRL_C_ARM_WINDOW: std::time::Duration = std::time::Duration::from_secs(2);

/// Decision for a Ctrl-C press given the current editor / arming state.
/// Pure-function output so the logic is unit-testable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CtrlCAction {
    /// Editor had unsent text — clear it and disarm any pending window.
    ClearBuffer,
    /// Editor was empty and the arming window is hot — force-quit.
    ForceQuit,
    /// Editor was empty and no hot arming — show the prompt overlay and
    /// arm the window.
    ArmAndPrompt,
}

/// Classify the current Ctrl-C press given editor emptiness, whether the
/// force-quit window is armed, and the time elapsed since arming.
fn classify_ctrl_c(
    editor_empty: bool,
    armed_at: Option<std::time::Instant>,
    now: std::time::Instant,
) -> CtrlCAction {
    if !editor_empty {
        return CtrlCAction::ClearBuffer;
    }
    match armed_at {
        Some(t) if now.duration_since(t) < CTRL_C_ARM_WINDOW => CtrlCAction::ForceQuit,
        _ => CtrlCAction::ArmAndPrompt,
    }
}

async fn keyboard_loop(
    input_tx: mpsc::UnboundedSender<UiInput>,
    ops_tx: mpsc::UnboundedSender<UiOp>,
    editor_view: Arc<RwLock<EditorSnapshot>>,
) {
    let mut editor = InputEditor::new();
    let mut events = EventStream::new();
    // Hot-timestamp for the double-Ctrl-C force-quit window. `Some(t)` means
    // the user pressed Ctrl-C at `t` on an empty buffer; another Ctrl-C
    // within `CTRL_C_ARM_WINDOW` confirms the exit. Cleared on any other
    // editing action (typing, paste, submit, Ctrl-D, etc.) so the prompt
    // disappears as soon as the user shows they're still active.
    let mut ctrl_c_armed_at: Option<std::time::Instant> = None;
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
                        // Any active force-quit arming is cancelled — user
                        // clearly didn't mean to quit.
                        if ctrl_c_armed_at.take().is_some() {
                            let _ = ops_tx.send(UiOp::ClearTransientOverlay);
                        }
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
                        let action = classify_ctrl_c(
                            editor.is_empty(),
                            ctrl_c_armed_at,
                            std::time::Instant::now(),
                        );
                        match action {
                            CtrlCAction::ClearBuffer => {
                                editor.clear();
                                if ctrl_c_armed_at.take().is_some() {
                                    let _ = ops_tx.send(UiOp::ClearTransientOverlay);
                                }
                                publish_view(&editor_view, &editor).await;
                                let _ = ops_tx.send(UiOp::InputRedraw);
                            }
                            CtrlCAction::ForceQuit => {
                                let _ = input_tx.send(UiInput::Interrupt);
                                return;
                            }
                            CtrlCAction::ArmAndPrompt => {
                                ctrl_c_armed_at = Some(std::time::Instant::now());
                                let _ = ops_tx.send(UiOp::ShowTransientOverlay {
                                    text: CTRL_C_ARM_OVERLAY.to_string(),
                                    duration: CTRL_C_ARM_WINDOW,
                                });
                            }
                        }
                    }
                    EditOutcome::Eof => {
                        // User chose graceful exit while armed — drop the
                        // overlay so the prompt doesn't linger past EOF.
                        if ctrl_c_armed_at.take().is_some() {
                            let _ = ops_tx.send(UiOp::ClearTransientOverlay);
                        }
                        let _ = input_tx.send(UiInput::Eof);
                        // Don't return — user may continue if --stay-after-eof.
                    }
                    EditOutcome::Redraw => {
                        // Any active arming is invalidated — the user is
                        // editing again, so the prompt should disappear.
                        if ctrl_c_armed_at.take().is_some() {
                            let _ = ops_tx.send(UiOp::ClearTransientOverlay);
                        }
                        publish_view(&editor_view, &editor).await;
                        let _ = ops_tx.send(UiOp::InputRedraw);
                    }
                    EditOutcome::ForceRepaint => {
                        if ctrl_c_armed_at.take().is_some() {
                            let _ = ops_tx.send(UiOp::ClearTransientOverlay);
                        }
                        publish_view(&editor_view, &editor).await;
                        let _ = ops_tx.send(UiOp::FullRepaint);
                    }
                    EditOutcome::Noop => {}
                }
            }
            Event::Paste(s) => {
                if ctrl_c_armed_at.take().is_some() {
                    let _ = ops_tx.send(UiOp::ClearTransientOverlay);
                }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replay_count_capped_by_history_len() {
        // Small history fits entirely.
        assert_eq!(replay_count(3, 24), 3);
        assert_eq!(replay_count(0, 24), 0);
    }

    #[test]
    fn replay_count_capped_by_region_height() {
        // Large history is trimmed to region_height — extras would scroll
        // off the top of the region anyway.
        assert_eq!(replay_count(1000, 24), 24);
        assert_eq!(replay_count(HISTORY_CAP, 200), 200);
    }

    #[test]
    fn replay_count_handles_zero_region() {
        assert_eq!(replay_count(1000, 0), 0);
    }

    #[test]
    fn replay_count_handles_exact_fit() {
        assert_eq!(replay_count(50, 50), 50);
    }

    #[test]
    fn push_history_evicts_oldest_when_full() {
        let mut h: VecDeque<String> = VecDeque::with_capacity(HISTORY_CAP);
        // Fill to capacity.
        for i in 0..HISTORY_CAP {
            push_history(&mut h, format!("line{i}"));
        }
        assert_eq!(h.len(), HISTORY_CAP);
        assert_eq!(h.front().unwrap(), "line0");
        // One more push evicts the oldest.
        push_history(&mut h, "new".to_string());
        assert_eq!(h.len(), HISTORY_CAP);
        assert_eq!(h.front().unwrap(), "line1");
        assert_eq!(h.back().unwrap(), "new");
    }

    #[test]
    fn push_history_grows_below_cap() {
        let mut h: VecDeque<String> = VecDeque::new();
        push_history(&mut h, "a".to_string());
        push_history(&mut h, "b".to_string());
        push_history(&mut h, "c".to_string());
        assert_eq!(h.len(), 3);
        assert_eq!(h.iter().collect::<Vec<_>>(), vec!["a", "b", "c"]);
    }

    // ── classify_ctrl_c ────────────────────────────────────────────────

    #[test]
    fn classify_ctrl_c_clears_when_buffer_nonempty() {
        let now = std::time::Instant::now();
        // Even with a hot arming, non-empty buffer means "clear".
        assert_eq!(
            classify_ctrl_c(false, Some(now), now),
            CtrlCAction::ClearBuffer
        );
        // Without arming too.
        assert_eq!(
            classify_ctrl_c(false, None, now),
            CtrlCAction::ClearBuffer
        );
    }

    #[test]
    fn classify_ctrl_c_arms_when_empty_and_cold() {
        let now = std::time::Instant::now();
        assert_eq!(
            classify_ctrl_c(true, None, now),
            CtrlCAction::ArmAndPrompt
        );
    }

    #[test]
    fn classify_ctrl_c_quits_when_empty_and_hot() {
        let armed = std::time::Instant::now();
        let now = armed + std::time::Duration::from_millis(500);
        assert_eq!(
            classify_ctrl_c(true, Some(armed), now),
            CtrlCAction::ForceQuit
        );
    }

    #[test]
    fn classify_ctrl_c_re_arms_after_window_expires() {
        let armed = std::time::Instant::now();
        let now = armed + CTRL_C_ARM_WINDOW + std::time::Duration::from_millis(1);
        assert_eq!(
            classify_ctrl_c(true, Some(armed), now),
            CtrlCAction::ArmAndPrompt
        );
    }

    #[test]
    fn classify_ctrl_c_quits_at_exactly_one_ms_before_window_end() {
        // Boundary check: within the window means strictly less than.
        let armed = std::time::Instant::now();
        let now = armed + CTRL_C_ARM_WINDOW - std::time::Duration::from_millis(1);
        assert_eq!(
            classify_ctrl_c(true, Some(armed), now),
            CtrlCAction::ForceQuit
        );
    }

    // ── fit_overlay ────────────────────────────────────────────────────

    #[test]
    fn fit_overlay_zero_cols_returns_empty() {
        assert_eq!(fit_overlay("hello", 0), "");
    }

    #[test]
    fn fit_overlay_pads_short_text() {
        // " hi " padded to 10 cells -> " hi       ".
        let out = fit_overlay("hi", 10);
        assert_eq!(out.chars().count(), 10);
        assert!(out.starts_with(" hi "));
        assert!(out.ends_with("       ") || out.ends_with(" ")); // padding trail
    }

    #[test]
    fn fit_overlay_truncates_long_text() {
        let text = "this overlay is longer than the available room";
        let out = fit_overlay(text, 12);
        assert_eq!(out.chars().count(), 12);
    }

    #[test]
    fn fit_overlay_exact_fit() {
        let out = fit_overlay("hi", 4); // " hi " is exactly 4
        assert_eq!(out, " hi ");
    }

    // ── overlay_text helper ────────────────────────────────────────────

    #[test]
    fn overlay_text_returns_none_when_expired() {
        let past = std::time::Instant::now()
            .checked_sub(std::time::Duration::from_secs(1))
            .unwrap_or_else(std::time::Instant::now);
        let o = Some(("x".to_string(), past));
        assert!(overlay_text(&o).is_none());
    }

    #[test]
    fn overlay_text_returns_some_when_active() {
        let future = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let o = Some(("hello".to_string(), future));
        assert_eq!(overlay_text(&o), Some("hello"));
    }

    #[test]
    fn overlay_text_none_when_absent() {
        let o: Option<(String, std::time::Instant)> = None;
        assert!(overlay_text(&o).is_none());
    }
}
