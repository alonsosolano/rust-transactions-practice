//! Command-line entry point: `rust-transactions-practice <input.csv>`.
//!
//! The binary is thin (ADR-0011 S1): it reads the single argument, opens the
//! file, calls [`run`] with the file, stdout, and stderr, and maps the result
//! to an exit code (ADR-0010 V10). The output CSV is the only thing written to
//! stdout; a fatal error writes one escaped line to stderr.

use std::env;
use std::ffi::OsString;
use std::fs::File;
use std::io::{self, Write};
use std::process::ExitCode;

use rust_transactions_practice::{Escaped, RunError, run};

/// The usage line, written with a fixed program name so that stderr is the
/// same on every machine and for every way the binary is started.
const USAGE: &str = "usage: rust-transactions-practice <input.csv>";

/// Every way a run can fail. Each variant maps to exactly one exit code
/// (ADR-0010 V10, V12), and its `Display` is the one line written to stderr.
#[derive(Debug, thiserror::Error)]
enum Fatal {
    /// No argument, or more than one.
    #[error("{USAGE}")]
    Usage,

    /// The input path cannot be opened: not found, not readable, or a
    /// directory. The path is escaped, because it is input (ADR-0010 V11).
    #[error("error: cannot open input {}: {source}", Escaped::new(.path.as_encoded_bytes()))]
    Open { path: OsString, source: io::Error },

    /// The run itself stopped on a fatal error.
    #[error("error: {0}")]
    Run(#[from] RunError),
}

impl Fatal {
    /// The exit code of ADR-0010 V10, using the `sysexits.h` values.
    fn exit_code(&self) -> ExitCode {
        const EX_USAGE: u8 = 64;
        const EX_NOINPUT: u8 = 66;
        const EX_IOERR: u8 = 74;

        ExitCode::from(match self {
            Self::Usage => EX_USAGE,
            Self::Open { .. } => EX_NOINPUT,
            Self::Run(RunError::WriteOutput(_)) => EX_IOERR,
        })
    }
}

fn main() -> ExitCode {
    match try_main() {
        Ok(()) => ExitCode::SUCCESS,
        Err(fatal) => {
            // Best effort (ADR-0012 S8): a failing stderr must not panic or
            // change the exit code, so the write result is dropped.
            let _ = writeln!(io::stderr().lock(), "{fatal}");
            fatal.exit_code()
        }
    }
}

fn try_main() -> Result<(), Fatal> {
    let path = single_argument(env::args_os())?;
    let input = open_input(path)?;
    run(input, io::stdout().lock(), io::stderr().lock())?;
    Ok(())
}

/// Returns the one argument after the program name.
///
/// Arguments are taken as `OsString`, so a non-UTF-8 path works and nothing
/// panics. The argument is always a path: `-h`, `--help`, and `-` are file
/// names like any other (ADR-0010).
fn single_argument(args: impl IntoIterator<Item = OsString>) -> Result<OsString, Fatal> {
    let mut args = args.into_iter().skip(1);
    match (args.next(), args.next()) {
        (Some(path), None) => Ok(path),
        _ => Err(Fatal::Usage),
    }
}

/// Opens the input file for reading.
///
/// On Unix, opening a directory succeeds and only reading it fails, which
/// would surface as a read error (`74`). A directory is refused here instead,
/// so it gets the "cannot open" exit code (`66`) on every platform.
fn open_input(path: OsString) -> Result<File, Fatal> {
    let opened = File::open(&path).and_then(|file| {
        if file.metadata()?.is_dir() {
            Err(io::Error::from(io::ErrorKind::IsADirectory))
        } else {
            Ok(file)
        }
    });
    opened.map_err(|source| Fatal::Open { path, source })
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::{Fatal, single_argument};

    fn args(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    #[test]
    fn one_argument_is_the_path() {
        let path = single_argument(args(&["bin", "input.csv"])).unwrap();
        assert_eq!(path, "input.csv");
    }

    #[test]
    fn a34a_no_argument_is_a_usage_error() {
        assert!(matches!(single_argument(args(&["bin"])), Err(Fatal::Usage)));
        assert!(matches!(single_argument(args(&[])), Err(Fatal::Usage)));
    }

    #[test]
    fn a34b_two_or_more_arguments_are_a_usage_error() {
        assert!(matches!(
            single_argument(args(&["bin", "a.csv", "b.csv"])),
            Err(Fatal::Usage)
        ));
        assert!(matches!(
            single_argument(args(&["bin", "a.csv", "b.csv", "c.csv"])),
            Err(Fatal::Usage)
        ));
    }

    #[test]
    fn flags_are_paths() {
        for flag in ["-h", "--help", "-", "--"] {
            let path = single_argument(args(&["bin", flag])).unwrap();
            assert_eq!(path, flag);
        }
    }
}
