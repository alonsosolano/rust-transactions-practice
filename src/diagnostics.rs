//! Bounded, escaped stderr reporting and the end-of-run summary (ADR-0006 R10, ADR-0010 V11).
//!
//! Adapter module of the layout in ADR-0011. For now it holds the escaping
//! used for input echoed in diagnostics and a [`Diagnostics`] sink that
//! receives every skipped row but writes nothing yet. The line format, the
//! budget, and the summary come with the bounded diagnostics (E10, LYA-58).

use std::fmt::{self, Display, Formatter, Write as _};

use crate::engine::Rejection;
use crate::input::InvalidRow;
use crate::transaction::Transaction;

/// Receives every row that the run skips (ADR-0006 R10).
///
/// The `run` loop already passes each invalid row and each rejection here,
/// with its line number, so the bounded diagnostics (E10, LYA-58) only need to
/// fill in these methods. Until then nothing is written: an unbounded stream
/// of one line per row would break R10, and a provisional format would have to
/// be redone.
#[derive(Debug, Default)]
pub struct Diagnostics;

impl Diagnostics {
    /// A row that failed validation (ADR-0010 V1 to V5, V14).
    pub fn invalid(&mut self, _line: u64, _reason: InvalidRow) {}

    /// A valid row that the engine rejected or ignored.
    pub fn rejected(&mut self, _line: u64, _transaction: &Transaction, _reason: Rejection) {}
}

/// Most bytes of raw input echoed in one diagnostic. Longer input is cut off
/// and marked, so each diagnostic line stays bounded (ADR-0006 R10).
pub const MAX_ECHO_BYTES: usize = 256;

/// Raw input bytes shown on stderr as one safe, quoted line (ADR-0010 V11).
///
/// Printable ASCII is written as is. A quote and a backslash are escaped with
/// a backslash; tab, line feed, and carriage return as `\t`, `\n`, and `\r`;
/// every other control byte and every non-ASCII byte as `\xNN`. The result is
/// enclosed in double quotes, so the end of the echoed input is always
/// visible. Input longer than `MAX_ECHO_BYTES` (256) is cut off, and
/// ` (truncated)` follows the closing quote.
///
/// Escaping keeps hostile input from forging log lines (CWE-117) or sending
/// terminal control sequences to the operator's screen.
#[derive(Debug, Clone, Copy)]
pub struct Escaped<'a>(&'a [u8]);

impl<'a> Escaped<'a> {
    /// Wraps raw bytes for display.
    #[must_use]
    pub fn new(bytes: &'a [u8]) -> Self {
        Self(bytes)
    }
}

impl Display for Escaped<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_char('"')?;
        for &byte in self.0.iter().take(MAX_ECHO_BYTES) {
            match byte {
                b'"' => f.write_str("\\\"")?,
                b'\\' => f.write_str("\\\\")?,
                b'\t' => f.write_str("\\t")?,
                b'\n' => f.write_str("\\n")?,
                b'\r' => f.write_str("\\r")?,
                b' '..=b'~' => f.write_char(char::from(byte))?,
                _ => write!(f, "\\x{byte:02x}")?,
            }
        }
        f.write_char('"')?;
        if self.0.len() > MAX_ECHO_BYTES {
            f.write_str(" (truncated)")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{Escaped, MAX_ECHO_BYTES};

    fn escape(bytes: &[u8]) -> String {
        Escaped::new(bytes).to_string()
    }

    #[test]
    fn printable_ascii_is_quoted_unchanged() {
        assert_eq!(escape(b"input.csv"), r#""input.csv""#);
        assert_eq!(escape(b" ~!#$%&'()*+,-./09:;<=>?@AZ[]^_`az{|}"), {
            r#"" ~!#$%&'()*+,-./09:;<=>?@AZ[]^_`az{|}""#
        });
    }

    #[test]
    fn empty_input_is_an_empty_quoted_string() {
        assert_eq!(escape(b""), r#""""#);
    }

    #[test]
    fn quote_and_backslash_are_escaped() {
        assert_eq!(escape(br#"a"b\c"#), r#""a\"b\\c""#);
    }

    /// Part of A32: escaped input can never start a second, forged line.
    #[test]
    fn line_breaks_and_tab_are_escaped() {
        let shown = escape(b"a\nerror: forged\r\tb");
        assert_eq!(shown, r#""a\nerror: forged\r\tb""#);
        assert!(!shown.contains(['\n', '\r', '\t']));
    }

    #[test]
    fn control_bytes_are_hex_escaped() {
        assert_eq!(escape(b"\x00\x1b[2J\x7f"), r#""\x00\x1b[2J\x7f""#);
    }

    #[test]
    fn non_ascii_bytes_are_hex_escaped() {
        assert_eq!(escape("é".as_bytes()), r#""\xc3\xa9""#);
        assert_eq!(escape(b"\xff\xfe"), r#""\xff\xfe""#);
    }

    #[test]
    fn input_at_the_limit_is_not_truncated() {
        let input = vec![b'a'; MAX_ECHO_BYTES];
        let shown = escape(&input);
        assert!(!shown.ends_with(" (truncated)"));
        assert_eq!(shown.matches('a').count(), MAX_ECHO_BYTES);
    }

    #[test]
    fn input_over_the_limit_is_truncated_and_marked() {
        let input = vec![b'a'; MAX_ECHO_BYTES.checked_add(1).unwrap()];
        let shown = escape(&input);
        let quoted = shown.strip_suffix(" (truncated)").unwrap();
        assert!(quoted.ends_with('"'));
        assert_eq!(quoted.matches('a').count(), MAX_ECHO_BYTES);
    }

    #[test]
    fn every_byte_value_gives_one_printable_ascii_line() {
        let all: Vec<u8> = (0..=u8::MAX).collect();
        let shown = Escaped(&all).to_string();
        assert!(shown.bytes().all(|b| (b' '..=b'~').contains(&b)));
    }
}
