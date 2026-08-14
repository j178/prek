//! Terminal capability setup, progress rendering, and captured-output filtering.
//!
//! Terminal streams contain instructions, not just text. Preview output keeps
//! only decoded text. PTY capture uses the same parse to retain SGR styling for
//! final replay and resolve line-overwrite instructions in a private buffer.
//! Other terminal instructions have no useful meaning in a linear transcript
//! and are discarded instead of being replayed on the user's terminal.
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
//! - **C0/C1 controls** are non-printing control characters. Of these, the linear
//!   output keeps LF (`\n`), CR (`\r`), and tab (`\t`) for text layout.
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
pub(crate) static USE_COLOR: LazyLock<bool> =
    LazyLock::new(|| match anstream::Stderr::choice(&std::io::stderr()) {
        ColorChoice::Always | ColorChoice::AlwaysAnsi => true,
        ColorChoice::Never => false,
        // We just asked anstream for a choice, that can't be auto.
        ColorChoice::Auto => unreachable!(),
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

/// Advances an incremental parser while retaining incomplete input for the next chunk.
fn parse(parser: &mut Parser<DefaultCharAccumulator>, output: &mut impl Perform, input: &[u8]) {
    for byte in input {
        parser.advance(output, *byte);
    }
}

/// Plain text emitted while parsing one captured-output chunk.
#[derive(Default)]
struct PreviewText {
    text: String,
}

impl PreviewText {
    /// Appends one decoded printable character.
    fn push_char(&mut self, c: char) {
        if !c.is_control() {
            self.text.push(c);
        }
    }

    /// Retains only controls needed to lay out the preview as text lines.
    fn push_control(&mut self, byte: u8) {
        if matches!(byte, b'\n' | b'\r' | b'\t') {
            self.text.push(char::from(byte));
        }
    }
}

impl Perform for PreviewText {
    fn print(&mut self, c: char) {
        self.push_char(c);
    }

    fn execute(&mut self, byte: u8) {
        self.push_control(byte);
    }
}

/// Incrementally decodes captured bytes into ANSI-free preview text.
#[derive(Default)]
pub(crate) struct PreviewFilter {
    parser: Parser<DefaultCharAccumulator>,
    preview: PreviewText,
}

impl PreviewFilter {
    /// Parses one chunk and returns the plain text produced by that chunk.
    ///
    /// Incomplete UTF-8 and terminal sequences remain buffered until a later call.
    pub(crate) fn push(&mut self, input: &[u8]) -> &str {
        self.preview.text.clear();
        parse(&mut self.parser, &mut self.preview, input);
        &self.preview.text
    }
}

/// Produces a safe linear transcript and plain preview from a PTY byte stream.
#[derive(Default)]
pub(crate) struct TerminalOutputFilter {
    parser: Parser<DefaultCharAccumulator>,
    output: TerminalOutput,
}

impl TerminalOutputFilter {
    /// Parses one PTY chunk and returns its ANSI-free preview text.
    ///
    /// The same parse appends printable text and SGR styling to the final transcript.
    pub(crate) fn push(&mut self, input: &[u8]) -> &str {
        self.output.preview.text.clear();
        parse(&mut self.parser, &mut self.output, input);
        &self.output.preview.text
    }

    /// Returns the completed transcript with terminal movement already resolved or removed.
    pub(crate) fn finish(self) -> String {
        self.output.text
    }
}

/// SGR sequence chain needed to recreate a terminal's current text style.
#[derive(Clone, Default, Eq, PartialEq)]
struct StyleState(String);

/// Applies supported terminal actions while accumulating a linear transcript.
#[derive(Default)]
struct TerminalOutput {
    text: String,
    line_start: usize,
    pending_cr: bool,
    active_style: StyleState,
    emitted_style: StyleState,
    // Truncating the current line returns the retained output to this style.
    line_start_style: StyleState,
    preview: PreviewText,
}

impl TerminalOutput {
    /// Resolves a pending overwrite and restores the active style before writing text.
    fn prepare_for_text(&mut self) {
        if self.pending_cr {
            self.clear_current_line();
            self.pending_cr = false;
        }

        if self.emitted_style != self.active_style {
            // Line erasure removes buffered SGR bytes, but the child terminal
            // keeps their effect. Restore that style before replacement text.
            self.text.push_str("\x1b[0m");
            self.text.push_str(&self.active_style.0);
            self.emitted_style.clone_from(&self.active_style);
        }
    }

    /// Removes the buffered current line and restores its starting style state.
    fn clear_current_line(&mut self) {
        self.text.truncate(self.line_start);
        self.emitted_style.clone_from(&self.line_start_style);
    }

    /// Commits the current line and records the style inherited by the next line.
    fn push_newline(&mut self) {
        self.pending_cr = false;
        self.text.push('\n');
        self.line_start = self.text.len();
        self.line_start_style.clone_from(&self.emitted_style);
    }

    /// Preserves an SGR sequence and updates the style state used after overwrites.
    fn push_sgr(&mut self, params: &Params) {
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
            self.active_style.0.clone_from(&style);
            self.emitted_style.0 = style;
        } else {
            self.active_style.0.push_str(&sequence);
            self.emitted_style.0.push_str(&sequence);
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
    /// Writes printable text to both the transcript and live preview.
    fn print(&mut self, c: char) {
        self.prepare_for_text();
        self.text.push(c);
        self.preview.push_char(c);
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
        self.preview.push_control(byte);
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
    use super::{PreviewFilter, TerminalOutputFilter};

    #[test]
    fn preview_filter_handles_sequences_split_across_chunks() {
        let mut filter = PreviewFilter::default();
        let mut preview = String::new();
        for chunk in [
            b"\x1b[1;3".as_slice(),
            b"2mgreen\x1b[0m \xe7".as_slice(),
            b"\xbb\xbf\n".as_slice(),
        ] {
            preview.push_str(filter.push(chunk));
        }

        assert_eq!(preview, "green \u{7eff}\n");
    }

    fn filter(input: &[u8]) -> (String, String) {
        let mut filter = TerminalOutputFilter::default();
        let preview = filter.push(input).to_owned();
        (filter.finish(), preview)
    }

    #[test]
    fn preserves_sgr_colors() {
        let (output, _) = filter(b"\x1b[1;32mgreen\x1b[0m");

        assert_eq!(output, "\x1b[1;32mgreen\x1b[0m");
    }

    #[test]
    fn filters_terminal_controls() {
        let output = filter(
            b"discarded\r\x1b[2K\x1b[31mred\x1b[0m\n\
              \x1b[1A\x1b[1B\x1b]0;title\x07\x1bPdata\x1b\\plain\x1b[?25l",
        );

        assert_eq!(
            output,
            (
                "\x1b[31mred\x1b[0m\nplain".to_owned(),
                "discarded\rred\nplain".to_owned(),
            )
        );
    }

    #[test]
    fn treats_bare_carriage_return_as_line_overwrite() {
        let (output, _) = filter(b"first\r\nold\rnew");

        assert_eq!(output, "first\nnew");
    }

    #[test]
    fn reapplies_color_after_line_overwrite() {
        let (output, _) = filter(b"\x1b[31mold\rnew\x1b[0m");

        assert_eq!(output, "\x1b[0m\x1b[31mnew\x1b[0m");
    }
}
