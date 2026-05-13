//! RAII terminal-state guard.
//!
//! When the interactive UI starts it must:
//!
//! 1. Enable raw mode (so stdin produces individual key events rather than
//!    cooked lines, and so the program's own `Ctrl-C` handling supersedes the
//!    tty's).
//! 2. Reserve the bottom rows for the status bar + input area by setting the
//!    terminal's scroll region (DECSTBM, `ESC[top;bottom r`). All `Print`
//!    calls that follow flow naturally within the upper region.
//! 3. Enable bracketed paste so multi-line pastes arrive as a single bursty
//!    sequence rather than triggering Enter handling on every newline.
//! 4. Hide the cursor while painting (the renderer restores it explicitly at
//!    the input cursor when each frame ends — see `interactive.rs`).
//!
//! On drop — including panics, Ctrl-C, normal shutdown — *all* of those need
//! to be undone, otherwise the user's shell prompt comes back to a scroll-
//! constrained, raw-mode, hidden-cursor terminal. This guard owns the
//! lifetime.

use std::io::{Write, stdout};

use crossterm::{
    cursor, event,
    style::ResetColor,
    terminal::{self, ClearType},
};

/// RAII handle for the terminal's interactive-mode state. The guard's `drop`
/// implementation restores the terminal regardless of how we leave the
/// session — clean exit, Ctrl-C, panic.
pub struct TerminalGuard {
    /// Last-applied scroll region (top, bottom) using 1-based row indices, or
    /// `None` if no scroll region has been set yet. Stored so the restore
    /// path can emit a matching reset.
    scroll_region: Option<(u16, u16)>,
}

impl TerminalGuard {
    /// Enter interactive mode. Installs a panic hook chained on top of the
    /// existing one so that even an unexpected panic restores the terminal.
    pub fn enter() -> std::io::Result<Self> {
        terminal::enable_raw_mode()?;

        let mut out = stdout();
        // Best-effort bracketed paste — some terminals don't support it but
        // failing here would be hostile. Errors are swallowed.
        let _ = crossterm::execute!(out, event::EnableBracketedPaste, cursor::Hide);
        out.flush().ok();

        install_panic_hook();
        Ok(Self {
            scroll_region: None,
        })
    }

    /// Set (or update) the scroll region to rows `top..=bottom` (1-based,
    /// inclusive). All subsequent normal output scrolls within this region;
    /// rows above and below remain untouched.
    pub fn set_scroll_region(&mut self, top: u16, bottom: u16) -> std::io::Result<()> {
        // Top must be <= bottom and both within 1..=rows. The caller is
        // responsible for sanity; we just emit the escape.
        let mut out = stdout();
        write!(out, "\x1b[{top};{bottom}r")?;
        out.flush()?;
        self.scroll_region = Some((top, bottom));
        Ok(())
    }

    /// Reset the scroll region to the full screen.
    pub fn reset_scroll_region(&mut self) {
        let mut out = stdout();
        let _ = write!(out, "\x1b[r");
        let _ = out.flush();
        self.scroll_region = None;
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore_terminal();
    }
}

/// Best-effort restore: reset scroll region, show cursor, disable bracketed
/// paste, leave raw mode. Idempotent so it's safe to call from the panic hook
/// AND `Drop`.
fn restore_terminal() {
    let mut out = stdout();
    // Reset scroll region to full screen.
    let _ = write!(out, "\x1b[r");
    // Move to a sane spot, clear from cursor down so the status bar / input
    // area artefacts don't leak into the user's shell prompt.
    if let Ok((_cols, rows)) = terminal::size() {
        let _ = crossterm::queue!(out, cursor::MoveTo(0, rows.saturating_sub(1)));
        let _ = crossterm::queue!(out, terminal::Clear(ClearType::CurrentLine));
    }
    let _ = crossterm::queue!(out, ResetColor, cursor::Show, event::DisableBracketedPaste);
    let _ = out.flush();
    let _ = terminal::disable_raw_mode();
}

/// Install a panic hook that restores the terminal before delegating to the
/// previous hook. Idempotent — repeated calls replace the previous chained
/// hook with a fresh one.
fn install_panic_hook() {
    use std::sync::OnceLock;
    static INSTALLED: OnceLock<()> = OnceLock::new();
    INSTALLED.get_or_init(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            restore_terminal();
            previous(info);
        }));
    });
}
