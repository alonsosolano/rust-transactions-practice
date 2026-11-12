//! A streaming transaction engine.
//!
//! [`run`] reads a CSV of transactions, applies them to client accounts, and
//! writes the final account balances as CSV. The binary in `main.rs` only
//! handles the command line and exit codes (ADR-0011 S1).
//!
//! Module layout (ADR-0011): the domain modules `amount`, `transaction`,
//! `account`, `store`, and `engine` perform no I/O. Only the adapter modules
//! `input`, `output`, and `diagnostics` read or write (ADR-0011 S2).

mod account;
mod amount;
mod diagnostics;
mod engine;
mod input;
mod output;
mod store;
#[cfg(test)]
mod test_support;
mod transaction;

use std::io::{self, Read, Write};

pub use diagnostics::Escaped;
pub use input::MAX_RECORD_BYTES;

use diagnostics::Diagnostics;
use engine::Engine;
use input::Reader;

/// A fatal error that stops the run (ADR-0010 V7).
///
/// The binary maps each variant to exactly one exit code (ADR-0010 V10, V12).
/// Every variant except [`RunError::WriteOutput`] happens before anything is
/// written to the output, so the output stays empty (ADR-0009 O10).
#[derive(Debug, thiserror::Error)]
pub enum RunError {
    /// The input has no header: it is empty or holds only empty lines (V8).
    #[error("missing header: the input has no header line")]
    MissingHeader,

    /// The first non-empty line is not the expected header (V8).
    #[error("wrong header: expected \"type,client,tx,amount\", found {}", Escaped::new(.found))]
    WrongHeader {
        /// The header line as read, without its line ending.
        found: Vec<u8>,
    },

    /// A line is longer than [`MAX_RECORD_BYTES`] (V9).
    #[error("line {line}: longer than {MAX_RECORD_BYTES} bytes")]
    LineTooLong {
        /// The 1-based line number.
        line: u64,
    },

    /// A line ends inside an open quoted field (V14).
    #[error("line {line}: a quoted field is not closed before the end of the line")]
    UnterminatedQuote {
        /// The 1-based line number where the quote opens.
        line: u64,
    },

    /// Reading the input failed.
    #[error("cannot read input at line {line}: {source}")]
    ReadInput {
        /// The 1-based number of the line being read.
        line: u64,
        /// The I/O error.
        source: io::Error,
    },

    /// Writing the output CSV failed, for example on a broken pipe.
    #[error("cannot write output: {0}")]
    WriteOutput(#[source] io::Error),
}

/// Processes one input stream and writes the account balances to `output`.
///
/// The input is read one line at a time and never held in full (ADR-0011
/// S3). `output` receives only the output CSV, and only after all input is
/// processed (ADR-0011 S12). `diagnostics` receives the bounded reports of
/// ADR-0006 R10 in the format of ADR-0017: a line for each of the first
/// skipped rows, then a summary. The summary is written once the input is no
/// longer read, even when reading stopped on a fatal error, so the caller's
/// fatal line comes after it (ADR-0017 D7). Every write to `diagnostics` is
/// best effort; a failing `diagnostics` never changes the result.
///
/// # Errors
///
/// Returns a [`RunError`] when the run cannot complete. Unless the error is
/// [`RunError::WriteOutput`], nothing was written to `output`; after a
/// `WriteOutput` error, `output` must not be trusted.
pub fn run<R, W, E>(input: R, output: W, diagnostics: E) -> Result<(), RunError>
where
    R: Read,
    W: Write,
    E: Write,
{
    let mut diagnostics = Diagnostics::new(diagnostics);
    let processed = process(input, &mut diagnostics);
    diagnostics.finish();
    let engine = processed?;
    output::write_accounts(output, engine.accounts()).map_err(RunError::WriteOutput)
}

/// Reads every row of `input` into a new engine, reporting each skipped row.
fn process<R: Read, E: Write>(
    input: R,
    diagnostics: &mut Diagnostics<E>,
) -> Result<Engine, RunError> {
    let mut reader = Reader::new(input)?;
    let mut engine = Engine::new();
    while let Some(row) = reader.next_row()? {
        match row.outcome {
            Ok(transaction) => {
                if let Err(rejection) = engine.apply(transaction) {
                    diagnostics.rejected(row.line, &transaction, rejection);
                }
            }
            Err(reason) => diagnostics.invalid(row.line, reader.current_line(), reason),
        }
    }
    Ok(engine)
}

#[cfg(test)]
mod tests {
    use std::io::{self, Write};

    use super::{RunError, run};

    /// A writer whose every write and flush fails with a broken pipe.
    struct BrokenPipe;

    impl Write for BrokenPipe {
        fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
            Err(io::ErrorKind::BrokenPipe.into())
        }

        fn flush(&mut self) -> io::Result<()> {
            Err(io::ErrorKind::BrokenPipe.into())
        }
    }

    const HEADER: &[u8] = b"type,client,tx,amount\n";

    #[test]
    fn a_header_alone_prints_only_the_output_header() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        run(HEADER, &mut stdout, &mut stderr).unwrap();
        assert_eq!(stdout, b"client,available,held,total,locked\n");
        assert_eq!(stderr, b"summary: skipped=0 rejected=0 ignored=0\n");
    }

    #[test]
    fn failed_output_write_is_a_write_error() {
        let result = run(HEADER, BrokenPipe, io::sink());
        assert!(matches!(
            result,
            Err(RunError::WriteOutput(e)) if e.kind() == io::ErrorKind::BrokenPipe
        ));
    }

    #[test]
    fn failing_diagnostics_do_not_fail_the_run() {
        let mut stdout = Vec::new();
        run(
            &b"type,client,tx,amount\nwithdrawal,1,1,5\n"[..],
            &mut stdout,
            BrokenPipe,
        )
        .unwrap();
        assert_eq!(
            stdout,
            b"client,available,held,total,locked\n1,0.0000,0.0000,0.0000,false\n"
        );
    }

    /// ADR-0010 V7, ADR-0009 O10: a fatal input error leaves the output empty,
    /// even when valid rows came before it.
    #[test]
    fn a_fatal_error_leaves_the_output_empty() {
        let mut stdout = Vec::new();
        let input = b"type,client,tx,amount\ndeposit,1,1,10\ndeposit,1,2,\"5";
        let result = run(&input[..], &mut stdout, io::sink());
        assert!(matches!(
            result,
            Err(RunError::UnterminatedQuote { line: 3 })
        ));
        assert!(stdout.is_empty());
    }

    #[test]
    fn fatal_messages_name_the_condition() {
        let wrong = RunError::WrongHeader {
            found: b"type,client,amount,tx\x1b[2J".to_vec(),
        };
        assert_eq!(
            wrong.to_string(),
            r#"wrong header: expected "type,client,tx,amount", found "type,client,amount,tx\x1b[2J""#
        );
        assert_eq!(
            RunError::LineTooLong { line: 7 }.to_string(),
            "line 7: longer than 4096 bytes"
        );
        assert_eq!(
            RunError::UnterminatedQuote { line: 3 }.to_string(),
            "line 3: a quoted field is not closed before the end of the line"
        );
    }
}
