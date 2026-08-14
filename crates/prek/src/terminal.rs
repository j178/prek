//! Terminal capability setup, progress rendering, and output sanitization.
//!
//! Terminal streams contain instructions, not just text. Before captured hook
//! output is displayed, it is converted into a linear transcript that retains
//! SGR styling and resolves line overwrites. Other terminal instructions have
//! no useful meaning in that transcript and are discarded instead of being
//! replayed on the user's terminal.
//!
//! # Terminology
//!
//! - **PTY** (pseudo-terminal) makes a subprocess behave as if it were attached
//!   to an interactive terminal, so its output may include terminal instructions.
//! - **ANSI/ECMA-48 sequences** encode terminal instructions in the same byte
//!   stream as printable text. `ANSI` is the common shorthand used here.
//! - **ESC** is the escape byte (`0x1b`) that begins most terminal sequences.
//! - **CSI** (Control Sequence Introducer) starts with `ESC [` and ends with an
//!   action byte; optional numeric parameters configure that action. CSI sequences
//!   cover styling, cursor movement, and erasure.
//! - **SGR** (Select Graphic Rendition) is a CSI sequence ending in `m`, such as
//!   `\x1b[31m`. It changes text attributes including color, bold, and dimming.
//! - **EL** (Erase in Line) is a CSI sequence ending in `K`, such as `\x1b[2K`.
//!   Modes 0, 1, and 2 erase after the cursor, before the cursor, or the whole line.
//! - **OSC** (Operating System Command, beginning with `ESC ]`) carries commands
//!   such as window titles and hyperlinks. **DCS** (Device Control String,
//!   beginning with `ESC P`) carries device-specific data.
//! - **C0/C1 controls** are non-printing control characters. Of these, LF (`\n`),
//!   CR (`\r`), and tab (`\t`) affect the transcript's text layout.
//! - **CR** returns the cursor to the start of the current line without advancing
//!   it; progress displays commonly use it to overwrite a previous frame. **LF**
//!   advances to the next line.

use std::fmt::Write as _;
use std::io;
use std::sync::LazyLock;

use anstream::ColorChoice;
use anstyle_parse::Params;
use anstyle_parse::{DefaultCharAccumulator, Parser, Perform};
use console::Term;
use indicatif::{ProgressDrawTarget, TermLike};

/// Whether stderr's resolved color choice permits ANSI styling.
pub(crate) static USE_COLOR: LazyLock<bool> = LazyLock::new(|| {
    matches!(
        anstream::Stderr::choice(&std::io::stderr()),
        ColorChoice::Always | ColorChoice::AlwaysAnsi
    )
});

/// Enables virtual terminal processing on platforms that require it for ANSI output.
pub(crate) fn enable_ansi_colors() {
    let _ = anstyle_query::windows::enable_ansi_colors();
}

// Windows console mode belongs to the shared screen buffer, so a child process
// can disable virtual terminal processing while prek's spinner is active.
// Indicatif buffers its ANSI output until flush; re-enable VT immediately before
// that output reaches the console. See https://github.com/j178/prek/issues/1237.
#[derive(Debug)]
struct WindowsVtTerm {
    inner: Term,
}

impl WindowsVtTerm {
    fn stderr() -> Self {
        Self {
            inner: Term::buffered_stderr(),
        }
    }
}

impl TermLike for WindowsVtTerm {
    fn width(&self) -> u16 {
        self.inner.size().1
    }

    fn height(&self) -> u16 {
        self.inner.size().0
    }

    fn move_cursor_up(&self, n: usize) -> io::Result<()> {
        self.inner.move_cursor_up(n)
    }

    fn move_cursor_down(&self, n: usize) -> io::Result<()> {
        self.inner.move_cursor_down(n)
    }

    fn move_cursor_right(&self, n: usize) -> io::Result<()> {
        self.inner.move_cursor_right(n)
    }

    fn move_cursor_left(&self, n: usize) -> io::Result<()> {
        self.inner.move_cursor_left(n)
    }

    fn write_line(&self, s: &str) -> io::Result<()> {
        self.inner.write_line(s)
    }

    fn write_str(&self, s: &str) -> io::Result<()> {
        self.inner.write_str(s)
    }

    fn clear_line(&self) -> io::Result<()> {
        self.inner.clear_line()
    }

    fn flush(&self) -> io::Result<()> {
        enable_ansi_colors();
        self.inner.flush()
    }
}

/// Returns the progress draw target for the current terminal.
pub(crate) fn progress_draw_target() -> ProgressDrawTarget {
    if cfg!(windows) {
        let term = WindowsVtTerm::stderr();
        if term.inner.features().colors_supported() {
            ProgressDrawTarget::term_like_with_hz(Box::new(term), 20)
        } else {
            ProgressDrawTarget::hidden()
        }
    } else {
        ProgressDrawTarget::stderr()
    }
}

/// Converts captured terminal bytes into text that is safe to display later.
pub(crate) fn sanitize_output(input: &[u8]) -> String {
    let mut parser = Parser::<DefaultCharAccumulator>::default();
    let mut output = TerminalOutput::default();
    let input = String::from_utf8_lossy(input);
    for byte in input.bytes() {
        parser.advance(&mut output, byte);
    }
    output.text
}

/// Applies supported terminal actions while accumulating a linear transcript.
#[derive(Default)]
struct TerminalOutput {
    text: String,
    line_start: usize,
    pending_cr: bool,
    active_style: String,
    line_start_style: String,
    restore_style: bool,
}

impl TerminalOutput {
    /// Resolves a pending overwrite and restores the active style before writing text.
    fn prepare_for_text(&mut self) {
        if self.pending_cr {
            self.clear_current_line();
            self.pending_cr = false;
        }

        if self.restore_style {
            self.text.push_str("\x1b[0m");
            self.text.push_str(&self.active_style);
            self.restore_style = false;
        }
    }

    /// Removes the buffered current line without changing the terminal's active style.
    fn clear_current_line(&mut self) {
        self.text.truncate(self.line_start);
        self.restore_style = self.active_style != self.line_start_style;
    }

    /// Commits the current line and records the style inherited by the next line.
    fn push_newline(&mut self) {
        self.pending_cr = false;
        self.text.push('\n');
        self.line_start = self.text.len();
        if !self.restore_style {
            self.line_start_style.clone_from(&self.active_style);
        }
    }

    /// Preserves an SGR sequence and updates the style state used after overwrites.
    fn push_sgr(&mut self, params: &Params) {
        if !self.pending_cr {
            self.prepare_for_text();
        }

        let Some(sequence) = sgr_sequence(params.iter()) else {
            return;
        };
        self.text.push_str(&sequence);

        let mut last_reset = None;
        for (index, param) in params.iter().enumerate() {
            if param == [0] {
                last_reset = Some(index);
            }
        }

        // A full reset makes every earlier SGR irrelevant when reconstructing
        // the style after a line overwrite.
        if let Some(last_reset) = last_reset {
            let style = sgr_sequence(params.iter().skip(last_reset + 1)).unwrap_or_default();
            self.active_style.clone_from(&style);
        } else {
            self.active_style.push_str(&sequence);
        }
    }

    /// Applies EL modes that erase text already present in the transcript buffer.
    fn erase_line(&mut self, params: &Params) {
        let mode = params
            .iter()
            .next()
            .and_then(|param| param.first())
            .copied()
            .unwrap_or(0);
        // EL after CR and EL modes 1/2 erase text already present in the buffer.
        if self.pending_cr || matches!(mode, 1 | 2) {
            self.clear_current_line();
            self.pending_cr = false;
        }
    }
}

impl Perform for TerminalOutput {
    /// Writes printable text to the transcript.
    fn print(&mut self, c: char) {
        self.prepare_for_text();
        self.text.push(c);
    }

    /// Applies line-layout controls and discards other C0 and C1 controls.
    fn execute(&mut self, byte: u8) {
        match byte {
            b'\n' => self.push_newline(),
            b'\r' => self.pending_cr = true,
            b'\t' => {
                self.prepare_for_text();
                self.text.push('\t');
            }
            _ => {}
        }
    }

    /// Preserves SGR, applies EL, and discards all other CSI actions.
    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], ignore: bool, action: u8) {
        if ignore || !intermediates.is_empty() {
            return;
        }

        match action {
            b'm' => self.push_sgr(params),
            b'K' => self.erase_line(params),
            _ => {}
        }
    }
}

/// Encodes parsed SGR parameters as a canonical CSI `m` sequence.
fn sgr_sequence<'a>(params: impl Iterator<Item = &'a [u16]>) -> Option<String> {
    let mut sequence = String::from("\x1b[");
    let mut has_params = false;
    for (param_index, param) in params.enumerate() {
        if param_index != 0 {
            sequence.push(';');
        }
        has_params = true;
        for (subparam_index, subparam) in param.iter().enumerate() {
            if subparam_index != 0 {
                sequence.push(':');
            }
            write!(sequence, "{subparam}").ok()?;
        }
    }
    if !has_params {
        return None;
    }
    sequence.push('m');
    Some(sequence)
}

#[cfg(test)]
mod tests {
    use super::sanitize_output;

    #[test]
    fn preserves_sgr_colors() {
        let output = sanitize_output(b"\x1b[1;32mgreen\x1b[0m");

        assert_eq!(output, "\x1b[1;32mgreen\x1b[0m");
    }

    #[test]
    fn replaces_invalid_utf8() {
        let output = sanitize_output(b"before\xffafter");

        assert_eq!(output, "before\u{fffd}after");
    }

    #[test]
    fn filters_terminal_controls() {
        let output = sanitize_output(
            b"discarded\r\x1b[2K\x1b[31mred\x1b[0m\n\
              \x1b[1A\x1b[1B\x1b]0;title\x07\x1bPdata\x1b\\plain\x1b[?25l",
        );

        assert_eq!(output, "\x1b[31mred\x1b[0m\nplain");
    }

    #[test]
    fn treats_bare_carriage_return_as_line_overwrite() {
        let output = sanitize_output(b"first\r\nold\rnew");

        assert_eq!(output, "first\nnew");
    }

    #[test]
    fn reapplies_color_after_line_overwrite() {
        let output = sanitize_output(b"\x1b[31mold\rnew\x1b[0m");

        assert_eq!(output, "\x1b[0m\x1b[31mnew\x1b[0m");
    }

    #[test]
    fn restores_reset_after_overwriting_inherited_style() {
        let output = sanitize_output(b"\x1b[1mfirst\n\x1b[0mold\rnew");

        assert_eq!(output, "\x1b[1mfirst\n\x1b[0mnew");
    }
}
