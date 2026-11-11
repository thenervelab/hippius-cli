//! A secret typed at the terminal, read without echo: the token `hipc auth login` asks for
//! when it runs at a terminal without `--token-file` (tenets 5 and 8).
//!
//! The terminal is put in a mode where `hipc` reads each byte as it is typed, with echo and
//! the signal keys off, and is put back as it was before the read returns. With the signal
//! keys off, Ctrl-C reaches `hipc` as a byte, so the read ends and the terminal is restored,
//! rather than the process dying with echo still off. A signal sent from elsewhere (`kill`,
//! a closed terminal) still ends the process with the terminal as it was during the read.
//!
//! The bytes are read from the descriptor itself, not through the standard library's stdin
//! buffer, which would keep a copy of the token for the life of the process; and whatever
//! was typed past the line is discarded when the mode is put back, so a line pasted after
//! the token cannot answer a question asked later in the same run.
//!
//! The unbuffered reader and the discard of what was typed ahead serve the confirmation
//! question and a secret file read from stdin too.

use std::io::{self, Read};
use std::os::fd::{AsFd as _, BorrowedFd};

use rustix::termios::{
    InputModes, LocalModes, OptionalActions, QueueSelector, SpecialCodeIndex, Termios, tcflush,
    tcgetattr, tcsetattr,
};
use zeroize::Zeroizing;

/// The longest line read, in bytes: far longer than a console token (40 characters), and
/// allocated whole up front, so the buffer never grows and leaves a copy of a part behind.
const MAX_LINE: usize = 4096;

/// How a hidden read ended.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Typed {
    /// A line ended with Enter, without the line break; cleared when dropped.
    Line(Zeroizing<Vec<u8>>),
    /// Ctrl-C, Ctrl-Z, or Ctrl-\, the keys that would interrupt or suspend a command.
    Interrupted,
    /// More than [`MAX_LINE`] bytes before the end of the line.
    TooLong,
    /// Ctrl-D on an empty line, or the end of input, before anything was typed.
    Ended,
}

/// Reads one line from the terminal on stdin with echo off. The caller has checked that
/// stdin is a terminal ([`crate::output::Terminal::can_prompt`]).
///
/// # Errors
///
/// When the terminal's mode cannot be read or set, or the read fails.
pub(crate) fn read_line(ask: impl FnOnce()) -> io::Result<Typed> {
    read_line_from(io::stdin().as_fd(), ask)
}

/// [`read_line`] from the terminal on `fd`. `ask` prints the prompt once echo is off, so
/// nothing typed after the prompt appears is shown.
fn read_line_from(fd: BorrowedFd<'_>, ask: impl FnOnce()) -> io::Result<Typed> {
    let _quiet = Quiet::new(fd)?;
    ask();
    read_typed(&mut Unbuffered(fd))
}

/// Discards what was typed at the terminal on stdin and not read yet, before a question is
/// shown, so only an answer typed after it can answer it. Best effort: a terminal that
/// refuses the flush is asked anyway. The caller has
/// checked that stdin is a terminal ([`crate::output::Terminal::can_prompt`]).
pub(crate) fn discard_typed_ahead() {
    let _best_effort = tcflush(io::stdin().as_fd(), QueueSelector::IFlush);
}

/// A descriptor read with `read(2)` and no buffer, so no copy of what it reads is kept
/// anywhere but the caller's buffer.
pub(crate) struct Unbuffered<'a>(pub(crate) BorrowedFd<'a>);

impl Read for Unbuffered<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        Ok(rustix::io::read(self.0, buf)?)
    }
}

/// The terminal on stdin in the mode of a hidden read, until dropped, which discards what is
/// left unread and puts the mode it had back.
struct Quiet<'a> {
    stdin: BorrowedFd<'a>,
    saved: Termios,
}

impl<'a> Quiet<'a> {
    fn new(stdin: BorrowedFd<'a>) -> io::Result<Self> {
        let saved = tcgetattr(stdin)?;
        let mut quiet = saved.clone();
        // Byte by byte (no `ICANON`), not shown (no `ECHO`), and Ctrl-C, Ctrl-Z, Ctrl-\, and
        // Ctrl-V as bytes (no `ISIG`, no `IEXTEN`). The other input flags stay, such as
        // the carriage return to line feed mapping, so Enter reads as typed.
        quiet.local_modes -=
            LocalModes::ICANON | LocalModes::ECHO | LocalModes::ISIG | LocalModes::IEXTEN;
        // Ctrl-S and Ctrl-Q as bytes too (no `IXON`, no `IXOFF`): a Ctrl-S would otherwise stop
        // the terminal's output, and putting the mode back would not start it again.
        quiet.input_modes -= InputModes::IXON | InputModes::IXOFF;
        quiet.special_codes[SpecialCodeIndex::VMIN] = 1;
        quiet.special_codes[SpecialCodeIndex::VTIME] = 0;
        // `Flush`: what was typed before the prompt, which the terminal has already shown, is
        // dropped rather than read as part of the token.
        tcsetattr(stdin, OptionalActions::Flush, &quiet)?;
        Ok(Self { stdin, saved })
    }
}

impl Drop for Quiet<'_> {
    fn drop(&mut self) {
        // `Flush`: what was typed or pasted past the line is dropped, not left for the next
        // read, such as a confirmation's. Best effort: nothing more can be done for a
        // terminal that refuses its own mode.
        let _best_effort = tcsetattr(self.stdin, OptionalActions::Flush, &self.saved);
    }
}

/// The line typed into `input`, read a byte at a time, as a terminal in canonical mode
/// would edit it: Backspace (DEL or Ctrl-H) removes the last UTF-8 character, Ctrl-U the
/// whole line, Enter (a line feed or a carriage return) ends it, Ctrl-C, Ctrl-Z, and Ctrl-\
/// interrupt the read, and Ctrl-D ends the line, or the input when the line is empty, as does
/// the end of input. A line longer than [`MAX_LINE`] ends the read.
fn read_typed(input: &mut dyn Read) -> io::Result<Typed> {
    let mut line = Zeroizing::new(Vec::with_capacity(MAX_LINE));
    let mut byte = [0_u8; 1];
    loop {
        let read = match input.read(&mut byte) {
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        match (read, byte[0]) {
            (0, _) | (_, 0x04) if line.is_empty() => return Ok(Typed::Ended),
            (0, _) | (_, b'\n' | b'\r' | 0x04) => return Ok(Typed::Line(line)),
            (_, 0x03 | 0x1a | 0x1c) => return Ok(Typed::Interrupted),
            (_, 0x7f | 0x08) => {
                // Back to the start of the last character, as a terminal with `IUTF8` erases
                // it: its continuation bytes are `10xxxxxx`.
                while line.pop().is_some_and(|byte| byte & 0xC0 == 0x80) {}
            }
            (_, 0x15) => line.clear(),
            (_, _) if line.len() == MAX_LINE => return Ok(Typed::TooLong),
            (_, typed) => line.push(typed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(bytes: &[u8]) -> Typed {
        read_typed(&mut &bytes[..]).expect("a slice reads")
    }

    fn line(text: &str) -> Typed {
        Typed::Line(Zeroizing::new(text.as_bytes().to_vec()))
    }

    #[test]
    fn enter_ends_the_line_and_what_follows_is_not_read() {
        assert_eq!(typed(b"t0ken\nmore"), line("t0ken"));
        assert_eq!(typed(b"t0ken\r"), line("t0ken"));
        assert_eq!(typed(b"\n"), line(""));
    }

    #[test]
    fn backspace_and_ctrl_u_edit_the_line() {
        assert_eq!(typed(b"t0kx\x7fen\n"), line("t0ken"));
        assert_eq!(typed(b"t0kx\x08en\n"), line("t0ken"));
        assert_eq!(typed(b"\x7f\x7ft0ken\n"), line("t0ken"));
        assert_eq!(typed(b"wrong\x15t0ken\n"), line("t0ken"));
        // A character of several bytes is erased whole.
        assert_eq!(typed("t0ké\x7fen\n".as_bytes()), line("t0ken"));
        assert_eq!(typed("t0k\u{1F600}\x08en\n".as_bytes()), line("t0ken"));
    }

    #[test]
    fn ctrl_c_interrupts_and_ctrl_d_or_the_end_ends() {
        assert_eq!(typed(b"t0k\x03en\n"), Typed::Interrupted);
        assert_eq!(typed(b"t0k\x1aen\n"), Typed::Interrupted);
        assert_eq!(typed(b"t0k\x1cen\n"), Typed::Interrupted);
        assert_eq!(typed(b"\x04"), Typed::Ended);
        assert_eq!(typed(b""), Typed::Ended);
        assert_eq!(typed(b"t0ken\x04"), line("t0ken"));
        assert_eq!(typed(b"t0ken"), line("t0ken"));
    }

    /// The buffer never grows: a line that would outgrow it ends the read, and editing keys
    /// still work at the limit.
    #[test]
    fn a_line_longer_than_the_buffer_ends_the_read() {
        let mut full = vec![b'a'; MAX_LINE];
        full.push(b'\n');
        assert_eq!(typed(&full), line(&"a".repeat(MAX_LINE)));
        let mut over = vec![b'a'; MAX_LINE + 1];
        over.push(b'\n');
        assert_eq!(typed(&over), Typed::TooLong);
        let mut edited = vec![b'a'; MAX_LINE];
        edited.extend_from_slice(b"\x7fb\n");
        assert_eq!(
            typed(&edited),
            line(&format!("{}b", "a".repeat(MAX_LINE - 1)))
        );
    }

    #[test]
    fn an_interrupted_read_is_retried_and_another_error_is_returned() {
        struct Flaky(Vec<io::Result<u8>>);
        impl Read for Flaky {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                match self.0.remove(0) {
                    Ok(byte) => {
                        buf[0] = byte;
                        Ok(1)
                    }
                    Err(error) => Err(error),
                }
            }
        }
        let mut input = Flaky(vec![
            Ok(b'a'),
            Err(io::ErrorKind::Interrupted.into()),
            Ok(b'\n'),
        ]);
        assert_eq!(read_typed(&mut input).unwrap(), line("a"));
        let mut input = Flaky(vec![Ok(b'a'), Err(io::Error::other("gone"))]);
        assert_eq!(read_typed(&mut input).unwrap_err().to_string(), "gone");
    }

    /// Off a terminal, the mode cannot be read, and the read says so instead of reading.
    #[test]
    fn a_descriptor_that_is_not_a_terminal_is_an_error() {
        let file = std::fs::File::open("/dev/null").expect("open /dev/null");
        let mut asked = false;
        assert!(read_line_from(file.as_fd(), || asked = true).is_err());
        assert!(!asked, "nobody is asked when the terminal cannot be set up");
    }
}
