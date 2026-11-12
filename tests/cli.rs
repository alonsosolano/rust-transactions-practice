//! CLI tests on the built binary (ADR-0014): the `sample_cases` test over
//! every case under `tests/cases/` (T3 to T8), and hand-written tests (T9) for
//! behavior a sample file cannot express, such as argument handling.
//!
//! Every fatal case checks the exit code, that stdout is empty, and that
//! stderr ends with exactly one error line (ADR-0010 V7, V11; invariant I12).
//! A fatal error raised after the input was opened comes after the
//! diagnostics summary (ADR-0017 D7).

use std::error::Error;
use std::ffi::OsStr;
use std::fs;
use std::path::Path;
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

/// The files every sample case holds, and nothing else, in sorted order
/// (ADR-0014 T3).
const CASE_FILES: [&str; 4] = [
    "expected.status",
    "expected.stderr",
    "expected.stdout",
    "input.csv",
];

/// The UTF-8 byte order mark that the A43 input must keep (ADR-0014 T8).
const UTF8_BOM: &[u8] = b"\xef\xbb\xbf";

/// ADR-0014 T3 to T6, T8: runs the binary on every case under `tests/cases/`
/// and compares stdout, stderr, and the exit code with the case's expected
/// files byte for byte, with no normalization. Every case runs, and one
/// failure at the end lists every problem found. Adding a case is adding a
/// directory; no code changes.
#[test]
fn sample_cases() -> TestResult {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/cases");
    let mut entries = fs::read_dir(&root)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(fs::DirEntry::file_name);

    let mut cases = Vec::new();
    let mut problems = Vec::new();
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !entry.file_type()?.is_dir() {
            problems.push(format!("{name}: not a case directory"));
            continue;
        }
        let case_problems =
            check_case(&entry.path()).unwrap_or_else(|error| vec![error.to_string()]);
        problems.extend(
            case_problems
                .into_iter()
                .map(|problem| format!("{name}: {problem}")),
        );
        cases.push(name);
    }

    if cases.is_empty() {
        problems.push(format!("no sample cases found in {}", root.display()));
    }
    problems.extend(check_a43_bytes(&root, &cases));

    assert!(
        problems.is_empty(),
        "{} sample-case problem(s):\n{}",
        problems.len(),
        problems.join("\n")
    );
    Ok(())
}

/// Checks one case directory and returns its problems; an empty list means
/// the case passed. An I/O error while reading the case or running the
/// binary is returned as an error and reported as a problem by the caller.
fn check_case(dir: &Path) -> Result<Vec<String>, Box<dyn Error>> {
    let mut names = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !entry.file_type()?.is_file() {
            return Ok(vec![format!("{name} is not a regular file")]);
        }
        names.push(name);
    }
    names.sort();
    if names != CASE_FILES {
        return Ok(vec![format!(
            "holds {names:?}; a case holds exactly {CASE_FILES:?}"
        )]);
    }

    let expected_stdout = fs::read(dir.join("expected.stdout"))?;
    let expected_stderr = fs::read(dir.join("expected.stderr"))?;
    let expected_status = match parse_status(&fs::read(dir.join("expected.status"))?) {
        Some(status) => status,
        None => {
            return Ok(vec![
                "expected.status must be decimal digits followed by one \\n".to_owned(),
            ]);
        }
    };
    // T6, invariant I12: a failed run leaves stdout empty, so the expected
    // files of a failing case must say so.
    if expected_status != 0 && !expected_stdout.is_empty() {
        return Ok(vec![format!(
            "expected.status is {expected_status}, so expected.stdout must be empty"
        )]);
    }

    let output = Command::new(env!("CARGO_BIN_EXE_rust-transactions-practice"))
        .arg("input.csv")
        .current_dir(dir)
        .stdin(Stdio::null())
        .output()?;

    let mut problems = Vec::new();
    if let Some(difference) = first_difference(&expected_stdout, &output.stdout) {
        problems.push(format!("stdout: {difference}"));
    }
    if let Some(difference) = first_difference(&expected_stderr, &output.stderr) {
        problems.push(format!("stderr: {difference}"));
    }
    match output.status.code() {
        Some(code) if code == expected_status => {}
        Some(code) => problems.push(format!("status: expected {expected_status}, actual {code}")),
        None => problems.push(format!(
            "status: expected {expected_status}, actual: killed by a signal"
        )),
    }
    Ok(problems)
}

/// Parses `expected.status`: ASCII digits followed by exactly one `\n`.
fn parse_status(bytes: &[u8]) -> Option<i32> {
    let digits = bytes.strip_suffix(b"\n")?;
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    std::str::from_utf8(digits).ok()?.parse().ok()
}

/// Describes the first line where `actual` differs from `expected`, or
/// returns `None` when the bytes are equal. Lines keep their `\n`, so a
/// difference in line endings is shown too. Both sides are printed with
/// control and non-ASCII bytes escaped.
fn first_difference(expected: &[u8], actual: &[u8]) -> Option<String> {
    if expected == actual {
        return None;
    }
    let mut expected_lines = expected.split_inclusive(|&byte| byte == b'\n');
    let mut actual_lines = actual.split_inclusive(|&byte| byte == b'\n');
    for line in 1_usize.. {
        let (expected_line, actual_line) = (expected_lines.next(), actual_lines.next());
        if expected_line != actual_line {
            return Some(format!(
                "line {line}: expected {}, actual {}",
                show_line(expected_line),
                show_line(actual_line)
            ));
        }
        if expected_line.is_none() {
            break;
        }
    }
    Some("bytes differ".to_owned())
}

/// Shows one line for a failure report, quoted and escaped.
fn show_line(line: Option<&[u8]>) -> String {
    line.map_or_else(
        || "<no line>".to_owned(),
        |bytes| format!("\"{}\"", bytes.escape_ascii()),
    )
}

/// ADR-0014 T8: the A43 input must still hold a byte order mark and `\r\n`
/// line endings, so an editor or a git setting that rewrites the file fails
/// the run instead of silently weakening the case.
fn check_a43_bytes(root: &Path, cases: &[String]) -> Vec<String> {
    let Some(case) = cases.iter().find(|name| name.starts_with("a43-")) else {
        return vec!["no a43-* sample case found".to_owned()];
    };
    let input = match fs::read(root.join(case).join("input.csv")) {
        Ok(input) => input,
        Err(error) => return vec![format!("{case}: cannot read input.csv: {error}")],
    };
    let mut problems = Vec::new();
    if !input.starts_with(UTF8_BOM) {
        problems.push(format!(
            "{case}: input.csv no longer starts with a UTF-8 BOM"
        ));
    }
    if !input.windows(2).any(|pair| pair == b"\r\n") {
        problems.push(format!("{case}: input.csv no longer contains \\r\\n"));
    }
    problems
}

/// Runs the binary on `input.csv` in a sample case directory.
fn run_in_case(name: &str) -> Result<Output, Box<dyn Error>> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/cases")
        .join(name);
    let output = Command::new(env!("CARGO_BIN_EXE_rust-transactions-practice"))
        .arg("input.csv")
        .current_dir(dir)
        .stdin(Stdio::null())
        .output()?;
    Ok(output)
}

/// The units of `0.0001` of a printed amount such as `-0.5000`.
fn units_of(text: &str) -> Result<i128, Box<dyn Error>> {
    let (sign, digits) = text.strip_prefix('-').map_or((1, text), |rest| (-1, rest));
    let (whole, fraction) = digits.split_once('.').ok_or("no decimal point")?;
    assert_eq!(fraction.len(), 4, "four fractional digits in {text:?}");
    whole
        .parse::<i128>()?
        .checked_mul(10_000)
        .and_then(|units| units.checked_add(fraction.parse().ok()?))
        .and_then(|units| units.checked_mul(sign))
        .ok_or_else(|| "amount out of range".into())
}

/// I10, ADR-0014 T9: running A26's input twice through the binary gives
/// byte-identical stdout, stderr, and exit code, and every printed `total`
/// equals the printed `available + held`.
#[test]
fn i10_the_binary_gives_the_same_output_twice() -> TestResult {
    let first = run_in_case("a26-example-output")?;
    let second = run_in_case("a26-example-output")?;
    assert_eq!(first.status.code(), Some(0), "exit code");
    assert_eq!(first.status.code(), second.status.code(), "exit code");
    assert_eq!(first.stdout, second.stdout, "stdout");
    assert_eq!(first.stderr, second.stderr, "stderr");

    let stdout = String::from_utf8(first.stdout)?;
    let mut rows = stdout.lines();
    assert_eq!(rows.next(), Some("client,available,held,total,locked"));
    for row in rows {
        let fields: Vec<&str> = row.split(',').collect();
        assert_eq!(fields.len(), 5, "{row}");
        assert_eq!(
            units_of(fields[1])?.checked_add(units_of(fields[2])?),
            Some(units_of(fields[3])?),
            "{row}"
        );
    }
    Ok(())
}

/// A34 (d), ADR-0010 V10, ADR-0014 T9: stdout is a pipe whose reader has
/// closed, so writing the CSV fails with a broken pipe. The run exits 74
/// without a panic, and its last stderr line is the write error, after the
/// summary (ADR-0017 D7). The reader is dropped right after `spawn`, which
/// races the child's write; the child writes stdout only after reading the
/// whole input (ADR-0011 S12), so the race is very likely won. A lost race
/// makes the write succeed and this test fail with exit 0: it can fail
/// spuriously but never pass wrongly.
#[test]
fn a34d_closed_stdout_pipe_exits_74() -> TestResult {
    let dir =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/cases/a26-example-output");
    let mut child = Command::new(env!("CARGO_BIN_EXE_rust-transactions-practice"))
        .arg("input.csv")
        .current_dir(&dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    drop(child.stdout.take());
    let output = child.wait_with_output()?;
    assert_eq!(output.status.code(), Some(74), "exit code");
    let stderr = String::from_utf8(output.stderr)?;
    let summary = std::fs::read_to_string(dir.join("expected.stderr"))?;
    let error = stderr
        .strip_prefix(&summary)
        .ok_or_else(|| format!("diagnostics and summary first: {stderr:?}"))?;
    assert_eq!(error.lines().count(), 1, "one error line: {stderr:?}");
    assert!(
        error.starts_with("error: cannot write output: "),
        "{stderr:?}"
    );
    assert!(error.ends_with('\n'), "error line ends with a newline");
    Ok(())
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
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/cases/deposits-and-withdrawals");
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
