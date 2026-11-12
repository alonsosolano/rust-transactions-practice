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
mod transaction;

use std::io::{self, Read, Write};

pub use diagnostics::Escaped;

/// A fatal error that stops the run (ADR-0010 V7).
///
/// The binary maps each variant to exactly one exit code (ADR-0010 V10, V12).
#[derive(Debug, thiserror::Error)]
pub enum RunError {
    /// Writing the output CSV failed, for example on a broken pipe.
    #[error("cannot write output: {0}")]
    WriteOutput(#[source] io::Error),
}

/// Processes one input stream and writes the account balances to `output`.
///
/// `output` receives only the output CSV, and only after all input is
/// processed (ADR-0011 S12). `diagnostics` receives the bounded reports of
/// ADR-0006 R10; writing to it is best effort and never fails the run.
///
/// The engine is not implemented yet: this skeleton reads no input, writes
/// nothing, and only flushes `output`.
///
/// # Errors
///
/// Returns a [`RunError`] when the run cannot complete, in which case
/// `output` must not be trusted.
pub fn run<R, W, E>(_input: R, mut output: W, _diagnostics: E) -> Result<(), RunError>
where
    R: Read,
    W: Write,
    E: Write,
{
    output.flush().map_err(RunError::WriteOutput)
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

    #[test]
    fn skeleton_run_completes_and_writes_nothing() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        run(&b"type,client,tx,amount\n"[..], &mut stdout, &mut stderr).unwrap();
        assert!(stdout.is_empty());
        assert!(stderr.is_empty());
    }

    #[test]
    fn failed_output_flush_is_a_write_error() {
        let result = run(&b""[..], BrokenPipe, io::sink());
        assert!(matches!(
            result,
            Err(RunError::WriteOutput(e)) if e.kind() == io::ErrorKind::BrokenPipe
        ));
    }

    #[test]
    fn failing_diagnostics_do_not_fail_the_run() {
        let mut stdout = Vec::new();
        run(&b""[..], &mut stdout, BrokenPipe).unwrap();
    }
}
