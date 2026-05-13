//! Multi-line input editor with readline-style keybindings.
//!
//! Maintained as a `Vec<String>` of logical lines plus a `(line_idx, col)`
//! cursor. Pure data structure — no terminal I/O. The interactive renderer
//! draws this view; this module just mutates state in response to
//! `KeyEvent`s and reports the resulting `EditOutcome`.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

/// Outcome of feeding a [`KeyEvent`] to the editor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditOutcome {
    /// Buffer or cursor changed; the renderer should redraw the input area.
    Redraw,
    /// Buffer is being submitted as a single multi-line string. The editor
    /// is cleared.
    Submit(String),
    /// Ctrl-C — the session should shut down.
    Interrupt,
    /// Ctrl-D on an empty buffer — propagate as EOF.
    Eof,
    /// User wants a full repaint (`Ctrl-L`).
    ForceRepaint,
    /// No change (e.g. an unmapped key).
    Noop,
}

/// Multi-line input editor. Initially one empty line, cursor at column 0.
pub struct InputEditor {
    /// Logical lines. Always non-empty; an empty buffer is `vec![String::new()]`.
    lines: Vec<String>,
    /// Cursor row (`0..lines.len()`).
    row: usize,
    /// Cursor column within `lines[row]` (`0..=lines[row].chars().count()`).
    col: usize,
}

impl Default for InputEditor {
    fn default() -> Self {
        Self::new()
    }
}

impl InputEditor {
    pub fn new() -> Self {
        Self {
            lines: vec![String::new()],
            row: 0,
            col: 0,
        }
    }

    /// Number of logical lines (always ≥ 1).
    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    /// Logical lines, for the renderer to draw.
    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    /// Current cursor position (row, column).
    pub fn cursor(&self) -> (usize, usize) {
        (self.row, self.col)
    }

    /// True if there's nothing typed.
    pub fn is_empty(&self) -> bool {
        self.lines.len() == 1 && self.lines[0].is_empty()
    }

    /// Insert a literal character at the cursor (e.g. for pasted content).
    pub fn insert_char(&mut self, ch: char) {
        if ch == '\n' {
            self.split_line();
            return;
        }
        let line = &mut self.lines[self.row];
        let byte_idx = byte_index(line, self.col);
        line.insert(byte_idx, ch);
        self.col += 1;
    }

    /// Insert a multi-line string at the cursor (used for bracketed paste).
    pub fn insert_str(&mut self, s: &str) {
        for ch in s.chars() {
            self.insert_char(ch);
        }
    }

    /// Apply a key event. Mutates state and returns what the renderer should do.
    pub fn handle_key(&mut self, ev: KeyEvent) -> EditOutcome {
        if !matches!(ev.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            return EditOutcome::Noop;
        }
        let ctrl = ev.modifiers.contains(KeyModifiers::CONTROL);
        let shift = ev.modifiers.contains(KeyModifiers::SHIFT);
        let alt = ev.modifiers.contains(KeyModifiers::ALT);

        match ev.code {
            KeyCode::Char(c) if ctrl => match c {
                'a' => {
                    self.col = 0;
                    EditOutcome::Redraw
                }
                'e' => {
                    self.col = self.lines[self.row].chars().count();
                    EditOutcome::Redraw
                }
                'u' => {
                    let line = &mut self.lines[self.row];
                    let byte_idx = byte_index(line, self.col);
                    line.replace_range(..byte_idx, "");
                    self.col = 0;
                    EditOutcome::Redraw
                }
                'k' => {
                    let line = &mut self.lines[self.row];
                    let byte_idx = byte_index(line, self.col);
                    line.truncate(byte_idx);
                    EditOutcome::Redraw
                }
                'w' => {
                    self.delete_prev_word();
                    EditOutcome::Redraw
                }
                'l' => EditOutcome::ForceRepaint,
                'c' => EditOutcome::Interrupt,
                'd' => {
                    if self.is_empty() {
                        EditOutcome::Eof
                    } else {
                        // Forward delete
                        self.delete_forward();
                        EditOutcome::Redraw
                    }
                }
                _ => EditOutcome::Noop,
            },
            KeyCode::Enter => {
                if shift || alt {
                    self.split_line();
                    EditOutcome::Redraw
                } else {
                    let text = self.take_buffer();
                    if text.is_empty() {
                        EditOutcome::Noop
                    } else {
                        EditOutcome::Submit(text)
                    }
                }
            }
            KeyCode::Char(c) => {
                self.insert_char(c);
                EditOutcome::Redraw
            }
            KeyCode::Backspace => {
                self.delete_backward();
                EditOutcome::Redraw
            }
            KeyCode::Delete => {
                self.delete_forward();
                EditOutcome::Redraw
            }
            KeyCode::Left => {
                self.move_left();
                EditOutcome::Redraw
            }
            KeyCode::Right => {
                self.move_right();
                EditOutcome::Redraw
            }
            KeyCode::Up => {
                self.move_up();
                EditOutcome::Redraw
            }
            KeyCode::Down => {
                self.move_down();
                EditOutcome::Redraw
            }
            KeyCode::Home => {
                self.col = 0;
                EditOutcome::Redraw
            }
            KeyCode::End => {
                self.col = self.lines[self.row].chars().count();
                EditOutcome::Redraw
            }
            _ => EditOutcome::Noop,
        }
    }

    /// Drain the buffer into a single string with `\n` between logical lines,
    /// resetting the editor to empty.
    fn take_buffer(&mut self) -> String {
        let joined = self.lines.join("\n");
        self.lines.clear();
        self.lines.push(String::new());
        self.row = 0;
        self.col = 0;
        joined
    }

    fn split_line(&mut self) {
        let line = &mut self.lines[self.row];
        let byte_idx = byte_index(line, self.col);
        let rest = line.split_off(byte_idx);
        self.lines.insert(self.row + 1, rest);
        self.row += 1;
        self.col = 0;
    }

    fn delete_backward(&mut self) {
        if self.col > 0 {
            let line = &mut self.lines[self.row];
            let from = byte_index(line, self.col - 1);
            let to = byte_index(line, self.col);
            line.replace_range(from..to, "");
            self.col -= 1;
        } else if self.row > 0 {
            // Join with previous line
            let cur = self.lines.remove(self.row);
            self.row -= 1;
            self.col = self.lines[self.row].chars().count();
            self.lines[self.row].push_str(&cur);
        }
    }

    fn delete_forward(&mut self) {
        let line_len = self.lines[self.row].chars().count();
        if self.col < line_len {
            let line = &mut self.lines[self.row];
            let from = byte_index(line, self.col);
            let to = byte_index(line, self.col + 1);
            line.replace_range(from..to, "");
        } else if self.row + 1 < self.lines.len() {
            // Join with next line
            let next = self.lines.remove(self.row + 1);
            self.lines[self.row].push_str(&next);
        }
    }

    fn delete_prev_word(&mut self) {
        // Walk backwards over whitespace, then over non-whitespace.
        let line = &mut self.lines[self.row];
        if self.col == 0 {
            // Same as backspace on a line boundary.
            if self.row > 0 {
                let cur = self.lines.remove(self.row);
                self.row -= 1;
                self.col = self.lines[self.row].chars().count();
                self.lines[self.row].push_str(&cur);
            }
            return;
        }
        let chars: Vec<char> = line.chars().collect();
        let mut i = self.col;
        while i > 0 && chars[i - 1].is_whitespace() {
            i -= 1;
        }
        while i > 0 && !chars[i - 1].is_whitespace() {
            i -= 1;
        }
        let from = byte_index(line, i);
        let to = byte_index(line, self.col);
        line.replace_range(from..to, "");
        self.col = i;
    }

    fn move_left(&mut self) {
        if self.col > 0 {
            self.col -= 1;
        } else if self.row > 0 {
            self.row -= 1;
            self.col = self.lines[self.row].chars().count();
        }
    }

    fn move_right(&mut self) {
        let line_len = self.lines[self.row].chars().count();
        if self.col < line_len {
            self.col += 1;
        } else if self.row + 1 < self.lines.len() {
            self.row += 1;
            self.col = 0;
        }
    }

    fn move_up(&mut self) {
        if self.row > 0 {
            self.row -= 1;
            let line_len = self.lines[self.row].chars().count();
            if self.col > line_len {
                self.col = line_len;
            }
        }
    }

    fn move_down(&mut self) {
        if self.row + 1 < self.lines.len() {
            self.row += 1;
            let line_len = self.lines[self.row].chars().count();
            if self.col > line_len {
                self.col = line_len;
            }
        }
    }
}

/// Convert a char-index into a byte-index inside `s` for safe `String::insert`
/// / `replace_range`. Saturates at `s.len()` on out-of-range.
fn byte_index(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map(|(i, _)| i)
        .unwrap_or(s.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn key_mod(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: mods,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    #[test]
    fn typing_inserts_chars() {
        let mut ed = InputEditor::new();
        for c in "hi".chars() {
            assert_eq!(ed.handle_key(key(KeyCode::Char(c))), EditOutcome::Redraw);
        }
        assert_eq!(ed.lines(), &["hi".to_string()]);
        assert_eq!(ed.cursor(), (0, 2));
    }

    #[test]
    fn enter_submits_and_clears() {
        let mut ed = InputEditor::new();
        ed.handle_key(key(KeyCode::Char('h')));
        ed.handle_key(key(KeyCode::Char('i')));
        assert_eq!(
            ed.handle_key(key(KeyCode::Enter)),
            EditOutcome::Submit("hi".to_string())
        );
        assert!(ed.is_empty());
    }

    #[test]
    fn shift_enter_inserts_newline() {
        let mut ed = InputEditor::new();
        ed.handle_key(key(KeyCode::Char('a')));
        ed.handle_key(key_mod(KeyCode::Enter, KeyModifiers::SHIFT));
        ed.handle_key(key(KeyCode::Char('b')));
        assert_eq!(ed.lines(), &["a".to_string(), "b".to_string()]);
        assert_eq!(ed.cursor(), (1, 1));
    }

    #[test]
    fn alt_enter_inserts_newline_as_fallback() {
        let mut ed = InputEditor::new();
        ed.handle_key(key_mod(KeyCode::Enter, KeyModifiers::ALT));
        assert_eq!(ed.lines(), &["".to_string(), "".to_string()]);
    }

    #[test]
    fn enter_on_multiline_submits_with_newlines() {
        let mut ed = InputEditor::new();
        for c in "a".chars() {
            ed.handle_key(key(KeyCode::Char(c)));
        }
        ed.handle_key(key_mod(KeyCode::Enter, KeyModifiers::SHIFT));
        for c in "b".chars() {
            ed.handle_key(key(KeyCode::Char(c)));
        }
        let out = ed.handle_key(key(KeyCode::Enter));
        assert_eq!(out, EditOutcome::Submit("a\nb".to_string()));
    }

    #[test]
    fn backspace_within_line() {
        let mut ed = InputEditor::new();
        ed.insert_str("hello");
        assert_eq!(ed.cursor(), (0, 5));
        ed.handle_key(key(KeyCode::Backspace));
        assert_eq!(ed.lines(), &["hell".to_string()]);
        assert_eq!(ed.cursor(), (0, 4));
    }

    #[test]
    fn backspace_at_line_start_joins() {
        let mut ed = InputEditor::new();
        ed.insert_str("ab\ncd");
        assert_eq!(ed.lines(), &["ab".to_string(), "cd".to_string()]);
        // Move cursor to start of line 1
        ed.row = 1;
        ed.col = 0;
        ed.handle_key(key(KeyCode::Backspace));
        assert_eq!(ed.lines(), &["abcd".to_string()]);
        assert_eq!(ed.cursor(), (0, 2));
    }

    #[test]
    fn ctrl_u_clears_to_start() {
        let mut ed = InputEditor::new();
        ed.insert_str("hello world");
        ed.col = 6; // after "hello "
        ed.handle_key(key_mod(KeyCode::Char('u'), KeyModifiers::CONTROL));
        assert_eq!(ed.lines(), &["world".to_string()]);
        assert_eq!(ed.cursor(), (0, 0));
    }

    #[test]
    fn ctrl_w_deletes_word() {
        let mut ed = InputEditor::new();
        ed.insert_str("hello world ");
        ed.handle_key(key_mod(KeyCode::Char('w'), KeyModifiers::CONTROL));
        assert_eq!(ed.lines(), &["hello ".to_string()]);
    }

    #[test]
    fn ctrl_c_interrupts() {
        let mut ed = InputEditor::new();
        ed.insert_str("hi");
        assert_eq!(
            ed.handle_key(key_mod(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            EditOutcome::Interrupt
        );
    }

    #[test]
    fn ctrl_d_on_empty_is_eof() {
        let mut ed = InputEditor::new();
        assert_eq!(
            ed.handle_key(key_mod(KeyCode::Char('d'), KeyModifiers::CONTROL)),
            EditOutcome::Eof
        );
    }

    #[test]
    fn ctrl_d_on_nonempty_is_forward_delete() {
        let mut ed = InputEditor::new();
        ed.insert_str("ab");
        ed.col = 0;
        assert_eq!(
            ed.handle_key(key_mod(KeyCode::Char('d'), KeyModifiers::CONTROL)),
            EditOutcome::Redraw
        );
        assert_eq!(ed.lines(), &["b".to_string()]);
    }

    #[test]
    fn arrow_keys() {
        let mut ed = InputEditor::new();
        ed.insert_str("a\nbc");
        ed.handle_key(key(KeyCode::Up));
        assert_eq!(ed.cursor(), (0, 1));
        ed.handle_key(key(KeyCode::Home));
        assert_eq!(ed.cursor(), (0, 0));
        ed.handle_key(key(KeyCode::End));
        assert_eq!(ed.cursor(), (0, 1));
        ed.handle_key(key(KeyCode::Down));
        assert_eq!(ed.cursor(), (1, 1));
    }

    #[test]
    fn unicode_byte_indexing() {
        // Multi-byte chars must not corrupt indexing.
        let mut ed = InputEditor::new();
        for c in "café".chars() {
            ed.handle_key(key(KeyCode::Char(c)));
        }
        ed.handle_key(key(KeyCode::Backspace));
        assert_eq!(ed.lines(), &["caf".to_string()]);
    }
}
