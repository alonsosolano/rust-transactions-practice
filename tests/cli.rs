//! Hand-written CLI tests (ADR-0014 T9): behavior only observable through the
//! process, such as argument handling and exit codes.
//!
//! Every fatal case checks the exit code, that stdout is empty, and that
//! stderr has exactly one line (ADR-0010 V7, V11; invariant I12).

use std::error::Error;
use std::ffi::OsStr;
use std::process::{Command, Output, Stdio};

type TestResult = Result<(), Box<dyn Error>>;

const USAGE_LINE: &str = "usage: rust-transactions-practice <input.csv>\n";

/// Runs the binary from the crate root with the given arguments and stdin
/// closed.
fn run_binary<I, S>(args: I) -> Result<Output, Box<dyn Error>>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = Command::new(env!("CARGO_BIN_EXE_rust-transactions-practice"))
        .args(args)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .stdin(Stdio::null())
        .output()?;
    Ok(output)
}

/// Checks the shape every fatal run shares and returns stderr as text.
fn assert_fatal(output: &Output, code: i32) -> Result<String, Box<dyn Error>> {
    assert_eq!(output.status.code(), Some(code), "exit code");
    assert!(output.stdout.is_empty(), "stdout must be empty");
    let stderr = String::from_utf8(output.stderr.clone())?;
    assert_eq!(stderr.lines().count(), 1, "one stderr line: {stderr:?}");
    assert!(stderr.ends_with('\n'), "stderr line ends with a newline");
    Ok(stderr)
}

#[test]
fn a34a_no_argument_exits_64() -> TestResult {
    let output = run_binary::<[&str; 0], &str>([])?;
    let stderr = assert_fatal(&output, 64)?;
    assert_eq!(stderr, USAGE_LINE);
    Ok(())
}

#[test]
fn a34b_two_arguments_exit_64() -> TestResult {
    let output = run_binary(["Cargo.toml", "Cargo.toml"])?;
    let stderr = assert_fatal(&output, 64)?;
    assert_eq!(stderr, USAGE_LINE);
    Ok(())
}

#[test]
fn a34c_missing_path_exits_66() -> TestResult {
    let output = run_binary(["does-not-exist.csv"])?;
    let stderr = assert_fatal(&output, 66)?;
    assert!(
        stderr.starts_with(r#"error: cannot open input "does-not-exist.csv": "#),
        "{stderr:?}"
    );
    Ok(())
}

/// The path is input, so it is escaped before it reaches stderr: a line break
/// in it cannot start a second line (ADR-0010 V11).
#[test]
fn a34c_missing_path_is_escaped() -> TestResult {
    let output = run_binary(["missing\nerror: forged \"line\".csv"])?;
    let stderr = assert_fatal(&output, 66)?;
    assert!(
        stderr.starts_with(r#"error: cannot open input "missing\nerror: forged \"line\".csv": "#),
        "{stderr:?}"
    );
    Ok(())
}

#[test]
fn directory_input_exits_66() -> TestResult {
    let output = run_binary(["src"])?;
    let stderr = assert_fatal(&output, 66)?;
    assert!(
        stderr.starts_with(r#"error: cannot open input "src": "#),
        "{stderr:?}"
    );
    Ok(())
}

/// The single argument is always a path; flags have no meaning (ADR-0010).
#[test]
fn help_flag_is_a_path_exits_66() -> TestResult {
    for flag in ["-h", "--help", "-"] {
        let output = run_binary([flag])?;
        let stderr = assert_fatal(&output, 66)?;
        assert!(
            stderr.starts_with(&format!("error: cannot open input \"{flag}\": ")),
            "{stderr:?}"
        );
    }
    Ok(())
}

/// A non-UTF-8 path neither panics nor reaches stderr unescaped.
#[cfg(unix)]
#[test]
fn non_utf8_path_exits_66_and_is_escaped() -> TestResult {
    use std::os::unix::ffi::OsStrExt;

    let output = run_binary([OsStr::from_bytes(b"bad-\xff.csv")])?;
    let stderr = assert_fatal(&output, 66)?;
    assert!(
        stderr.starts_with(r#"error: cannot open input "bad-\xff.csv": "#),
        "{stderr:?}"
    );
    Ok(())
}
