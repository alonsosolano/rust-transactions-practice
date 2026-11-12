//! Hand-written CLI tests (ADR-0014 T9): behavior only observable through the
//! process, such as argument handling and exit codes.
//!
//! Every fatal case checks the exit code, that stdout is empty, and that
//! stderr ends with exactly one error line (ADR-0010 V7, V11; invariant I12).
//! A fatal error raised after the input was opened comes after the
//! diagnostics summary (ADR-0017 D7).

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

/// The summary of a run that skipped no row (ADR-0017 D6).
const EMPTY_SUMMARY: &str = "summary: skipped=0 rejected=0 ignored=0\n";

/// Checks a fatal run that failed after opening its input: the summary comes
/// first and the error line is the last line. Returns the error line.
fn assert_fatal_after_summary(output: &Output, code: i32) -> Result<String, Box<dyn Error>> {
    assert_eq!(output.status.code(), Some(code), "exit code");
    assert!(output.stdout.is_empty(), "stdout must be empty");
    let stderr = String::from_utf8(output.stderr.clone())?;
    let error = stderr
        .strip_prefix(EMPTY_SUMMARY)
        .ok_or_else(|| format!("summary first: {stderr:?}"))?;
    assert_eq!(error.lines().count(), 1, "one error line: {stderr:?}");
    assert!(error.starts_with("error: "), "{stderr:?}");
    assert!(error.ends_with('\n'), "error line ends with a newline");
    Ok(error.to_owned())
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

/// Runs one sample case under `tests/cases/` and compares stdout, stderr, and
/// the exit code with its expected files byte for byte (ADR-0014 T3, T4).
/// E5 generalizes this into the `sample_cases` test over every case.
fn run_case(name: &str) -> TestResult {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/cases")
        .join(name);
    let output = Command::new(env!("CARGO_BIN_EXE_rust-transactions-practice"))
        .arg("input.csv")
        .current_dir(&dir)
        .stdin(Stdio::null())
        .output()?;
    let status = format!("{}\n", output.status.code().ok_or("killed by a signal")?);
    assert_eq!(
        String::from_utf8(output.stdout)?,
        std::fs::read_to_string(dir.join("expected.stdout"))?,
        "{name}: stdout"
    );
    assert_eq!(
        String::from_utf8(output.stderr)?,
        std::fs::read_to_string(dir.join("expected.stderr"))?,
        "{name}: stderr"
    );
    assert_eq!(
        status,
        std::fs::read_to_string(dir.join("expected.status"))?,
        "{name}: status"
    );
    Ok(())
}

/// Interleaved clients, padded fields, mixed precision, and a withdrawal
/// rejected for insufficient funds.
#[test]
fn deposits_and_withdrawals_sample_case() -> TestResult {
    run_case("deposits-and-withdrawals")
}

/// A1, ADR-0006 R2, ADR-0008 L6: a deposit is withdrawn, then disputed and
/// charged back. The locked account is printed like any other, with its
/// negative balance and `locked` set to `true`; another client is untouched.
#[test]
fn a1_withdraw_then_chargeback_sample_case() -> TestResult {
    run_case("a1-withdraw-then-chargeback")
}

/// ADR-0006: held funds cannot be withdrawn while disputed, and the resolve
/// releases them.
#[test]
fn dispute_then_resolve_sample_case() -> TestResult {
    run_case("dispute-then-resolve")
}

/// ADR-0006, ADR-0008 L2, L3: a chargeback locks the account, a later deposit
/// is applied, and a withdrawal is rejected. Another client's dispute stays
/// open in `held`.
#[test]
fn dispute_then_chargeback_sample_case() -> TestResult {
    run_case("dispute-then-chargeback")
}

/// ADR-0010 V8, V10: a file that is not this CSV (here `Cargo.toml`) has the
/// wrong header and exits 65 with one escaped error line after the summary
/// (ADR-0017 D7).
#[test]
fn wrong_header_exits_65() -> TestResult {
    let output = run_binary(["Cargo.toml"])?;
    let stderr = assert_fatal_after_summary(&output, 65)?;
    assert_eq!(
        stderr,
        "error: wrong header: expected \"type,client,tx,amount\", found \"[package]\"\n"
    );
    Ok(())
}

/// A16, ADR-0014 T9: stderr is a pipe whose reader has closed, so every
/// diagnostic write fails with a broken pipe. The run still exits 0 with the
/// same stdout as the sample case. The reader is dropped right after `spawn`,
/// which races the child's first stderr write; a lost race makes the write
/// succeed and the test still pass, so this test can only miss the failure,
/// never report a false one.
#[test]
fn a16_closed_stderr_pipe_changes_nothing() -> TestResult {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/cases/deposits-and-withdrawals");
    let mut child = Command::new(env!("CARGO_BIN_EXE_rust-transactions-practice"))
        .arg("input.csv")
        .current_dir(&dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    drop(child.stderr.take());
    let output = child.wait_with_output()?;
    assert_eq!(output.status.code(), Some(0), "exit code");
    assert_eq!(
        String::from_utf8(output.stdout)?,
        std::fs::read_to_string(dir.join("expected.stdout"))?
    );
    Ok(())
}
