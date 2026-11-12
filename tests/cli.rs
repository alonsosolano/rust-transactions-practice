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

/// ADR-0010 V8, V10: a file that is not this CSV (here `Cargo.toml`) has the
/// wrong header and exits 65 with one escaped stderr line.
#[test]
fn wrong_header_exits_65() -> TestResult {
    let output = run_binary(["Cargo.toml"])?;
    let stderr = assert_fatal(&output, 65)?;
    assert_eq!(
        stderr,
        "error: wrong header: expected \"type,client,tx,amount\", found \"[package]\"\n"
    );
    Ok(())
}
