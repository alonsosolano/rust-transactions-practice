//! Integration tests: the public `run` function on in-memory input, stdout,
//! and stderr (ADR-0014). Covers the Integration rows of the requirements §6
//! catalog; the dispute lifecycle itself is unit-tested in `engine`.

use std::error::Error;
use std::io;

use rust_transactions_practice::{RunError, run};

type TestResult = Result<(), Box<dyn Error>>;

const OUTPUT_HEADER: &str = "client,available,held,total,locked\n";

/// Runs `input` and returns stdout as text.
fn stdout_of(input: &str) -> Result<String, Box<dyn Error>> {
    let mut stdout = Vec::new();
    run(input.as_bytes(), &mut stdout, io::sink())?;
    Ok(String::from_utf8(stdout)?)
}

/// The expected stdout: the header followed by `rows`, one per line.
fn expected(rows: &[&str]) -> String {
    let mut text = OUTPUT_HEADER.to_owned();
    for row in rows {
        text.push_str(row);
        text.push('\n');
    }
    text
}

#[test]
fn interleaved_clients_with_a_rejected_withdrawal() -> TestResult {
    let input = "type, client, tx, amount\n\
                 deposit, 2, 1, 5\n\
                 deposit, 1, 2, 10.0\n\
                 withdrawal, 2, 3, 7.25\n\
                 withdrawal, 1, 4, 2.5\n\
                 deposit, 2, 5, 0.0001\n";
    assert_eq!(
        stdout_of(input)?,
        expected(&[
            "1,7.5000,0.0000,7.5000,false",
            "2,5.0001,0.0000,5.0001,false",
        ])
    );
    Ok(())
}

/// R8: a rejected withdrawal opens a zero-balance account.
#[test]
fn a_rejected_withdrawal_opens_a_zero_account() -> TestResult {
    let input = "type,client,tx,amount\nwithdrawal,3,1,1.0\n";
    assert_eq!(
        stdout_of(input)?,
        expected(&["3,0.0000,0.0000,0.0000,false"])
    );
    Ok(())
}

/// A6, ADR-0006 R7, R8: a dispute on an unknown `tx` opens no account, so
/// the output has only the header.
#[test]
fn a6_dispute_on_unknown_tx_prints_only_the_header() -> TestResult {
    let input = "type,client,tx,amount\ndispute,9,7,\n";
    assert_eq!(stdout_of(input)?, expected(&[]));
    Ok(())
}

/// A35, ADR-0010 V5, ADR-0006 R2: the amount on a dispute-related row is
/// ignored, even when it is invalid, and every row uses the full deposit.
#[test]
fn a35_amounts_on_dispute_rows_are_ignored() -> TestResult {
    let input = "type,client,tx,amount\n\
                 deposit,1,1,10\n\
                 dispute,1,1,3\n\
                 resolve,1,1,abc\n\
                 dispute,1,1,-50\n\
                 chargeback,1,1,10\n";
    assert_eq!(
        stdout_of(input)?,
        expected(&["1,0.0000,0.0000,0.0000,true"])
    );
    Ok(())
}

/// A17: a negative withdrawal is invalid and cannot create money.
#[test]
fn a17_negative_withdrawal_is_invalid() -> TestResult {
    let input = "type,client,tx,amount\ndeposit,1,1,10\nwithdrawal,1,2,-5\nwithdrawal,2,3,-5\n";
    assert_eq!(
        stdout_of(input)?,
        expected(&["1,10.0000,0.0000,10.0000,false"])
    );
    Ok(())
}

/// A18: a negative deposit is invalid and cannot bypass the funds check.
#[test]
fn a18_negative_deposit_is_invalid() -> TestResult {
    let input = "type,client,tx,amount\ndeposit,1,1,5\ndeposit,1,2,-10\nwithdrawal,1,3,10\n";
    assert_eq!(
        stdout_of(input)?,
        expected(&["1,5.0000,0.0000,5.0000,false"])
    );
    Ok(())
}

/// A19: excess precision is refused, never rounded; trailing zeros are fine.
#[test]
fn a19_excess_precision_is_never_rounded() -> TestResult {
    let input = "type,client,tx,amount\ndeposit,1,1,1.00005\ndeposit,1,2,1.23450\n";
    assert_eq!(
        stdout_of(input)?,
        expected(&["1,1.2345,0.0000,1.2345,false"])
    );
    Ok(())
}

/// A20: zero amounts are invalid and open no account.
#[test]
fn a20_zero_amounts_are_invalid() -> TestResult {
    let input = "type,client,tx,amount\ndeposit,1,1,0\nwithdrawal,2,2,0.0000\n";
    assert_eq!(stdout_of(input)?, OUTPUT_HEADER);
    Ok(())
}

/// A21: the largest amount twice does not overflow; one unit more is invalid.
#[test]
fn a21_maximum_amounts_do_not_overflow() -> TestResult {
    let input = "type,client,tx,amount\n\
                 deposit,1,1,1844674407370955.1615\n\
                 deposit,1,2,1844674407370955.1615\n\
                 deposit,1,3,1844674407370955.1616\n";
    assert_eq!(
        stdout_of(input)?,
        expected(&["1,3689348814741910.3230,0.0000,3689348814741910.3230,false"])
    );
    Ok(())
}

/// A22: only the plain decimal form is accepted.
#[test]
fn a22_only_plain_decimals_are_accepted() -> TestResult {
    let input = "type,client,tx,amount\n\
                 deposit,1,1,.5\n\
                 deposit,1,2,5.\n\
                 deposit,1,3,+5\n\
                 deposit,1,4,1e3\n\
                 deposit,1,5,1_000\n\
                 deposit,1,6,\" 2.5 \"\n";
    assert_eq!(
        stdout_of(input)?,
        expected(&["1,2.5000,0.0000,2.5000,false"])
    );
    Ok(())
}

/// A30: an invalid row does not bind its `tx` ID.
#[test]
fn a30_invalid_row_does_not_bind_its_id() -> TestResult {
    let input = "type,client,tx,amount\ndeposit,1,1,abc\ndeposit,1,1,5\n";
    assert_eq!(
        stdout_of(input)?,
        expected(&["1,5.0000,0.0000,5.0000,false"])
    );
    Ok(())
}

/// A31: every invalid row is skipped and the run continues.
#[test]
fn a31_invalid_rows_are_skipped() -> TestResult {
    let input = "type,client,tx,amount\n\
                 deposit,1,1,10\n\
                 Deposit,1,2,5\n\
                 deposit,+1,3,5\n\
                 deposit,65536,4,5\n\
                 deposit,1,4294967296,5\n\
                 deposit,1,5\n\
                 deposit,1,6,\n\
                 deposit,1,7,5,\n\
                 deposit,1,8,1\n";
    assert_eq!(
        stdout_of(input)?,
        expected(&["1,11.0000,0.0000,11.0000,false"])
    );
    Ok(())
}

/// A36: malformed quotes are never read leniently.
#[test]
fn a36_malformed_quotes_are_never_read_leniently() -> TestResult {
    let input = "type,client,tx,amount\n\
                 deposit,1,1,10\n\
                 deposit,1,2,\"1\"0\n\
                 deposit,1,3,1\"0\n\
                 deposit,1,4,\"\"5\"\"\n\
                 deposit,1,5,\"1,000\"\n\
                 \"deposit\", 1 ,6,\" 2.5 \"\n";
    assert_eq!(
        stdout_of(input)?,
        expected(&["1,12.5000,0.0000,12.5000,false"])
    );
    Ok(())
}

/// A43: BOM, padded header, `\r\n`, an empty line, and quoted fields.
#[test]
fn a43_every_accepted_form() -> TestResult {
    let input = "\u{feff} type , client , tx , amount \r\n\r\n\
                 deposit, 1, 1, 1\r\n\
                 deposit,1,2,1.0\r\n\
                 \"deposit\",1,3,\"1.2345\"\r\n";
    assert_eq!(
        stdout_of(input)?,
        expected(&["1,3.2345,0.0000,3.2345,false"])
    );
    Ok(())
}

/// A44: boundary IDs and leading zeros, printed in ascending client order.
#[test]
fn a44_boundary_ids_and_leading_zeros() -> TestResult {
    let input = "type,client,tx,amount\n\
                 deposit,65535,4294967295,1\n\
                 deposit,0,0,1\n\
                 deposit,007,0008,1\n";
    assert_eq!(
        stdout_of(input)?,
        expected(&[
            "0,1.0000,0.0000,1.0000,false",
            "7,1.0000,0.0000,1.0000,false",
            "65535,1.0000,0.0000,1.0000,false",
        ])
    );
    Ok(())
}

/// A45: a repeated header is an invalid row, and the run continues.
#[test]
fn a45_repeated_header_is_invalid() -> TestResult {
    let input = "type,client,tx,amount\ndeposit,1,1,10\ntype,client,tx,amount\ndeposit,1,2,5\n";
    assert_eq!(
        stdout_of(input)?,
        expected(&["1,15.0000,0.0000,15.0000,false"])
    );
    Ok(())
}

/// Recognizes the fatal error a case expects.
type IsExpected = fn(&RunError) -> bool;

/// ADR-0010 V7, ADR-0009 O10: each fatal input condition leaves stdout empty.
#[test]
fn fatal_input_errors_leave_stdout_empty() -> TestResult {
    let long_line = format!(
        "type,client,tx,amount\ndeposit,1,1,1{}\n",
        " ".repeat(5 * 1024)
    );
    let cases: [(&str, IsExpected); 4] = [
        ("", |e| matches!(e, RunError::MissingHeader)),
        ("type,client,amount,tx\ndeposit,1,1,10\n", |e| {
            matches!(e, RunError::WrongHeader { .. })
        }),
        (&long_line, |e| {
            matches!(e, RunError::LineTooLong { line: 2 })
        }),
        (
            "type,client,tx,amount\ndeposit,1,1,10\ndeposit,1,2,\"5",
            |e| matches!(e, RunError::UnterminatedQuote { line: 3 }),
        ),
    ];
    for (input, is_expected) in cases {
        let mut stdout = Vec::new();
        match run(input.as_bytes(), &mut stdout, io::sink()) {
            Err(error) => assert!(is_expected(&error), "{input:?}: got {error:?}"),
            Ok(()) => panic!("{input:?}: expected a fatal error"),
        }
        assert!(stdout.is_empty(), "{input:?}: stdout is not empty");
    }
    Ok(())
}

/// Runs `input` with a recording stderr and returns stdout and stderr as text.
fn streams_of(input: &[u8]) -> Result<(String, String), Box<dyn Error>> {
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    run(input, &mut stdout, &mut stderr)?;
    Ok((String::from_utf8(stdout)?, String::from_utf8(stderr)?))
}

/// A15, ADR-0006 R10, ADR-0017 D5, D6: a million ignored rows give 100
/// detailed lines, one notice, and a summary that counts every one of them.
/// The account is untouched and the ignored rows open no account.
#[test]
fn a15_a_million_ignored_rows_give_bounded_stderr() -> TestResult {
    const IGNORED: usize = 1_000_000;
    let mut input = String::from("type,client,tx,amount\ndeposit,1,1,10\n");
    input.push_str(&"dispute,2,1,\n".repeat(IGNORED));
    let (stdout, stderr) = streams_of(input.as_bytes())?;
    assert_eq!(stdout, expected(&["1,10.0000,0.0000,10.0000,false"]));

    let lines: Vec<&str> = stderr.lines().collect();
    let detailed = lines.iter().filter(|l| l.starts_with("ignored: ")).count();
    let notices = lines.iter().filter(|l| l.starts_with("notice: ")).count();
    assert_eq!(detailed, 100);
    assert_eq!(notices, 1);
    assert_eq!(
        lines.first(),
        Some(&"ignored: line 3: client_mismatch type=dispute client=2 tx=1 owner=1")
    );
    assert_eq!(
        lines.get(100),
        Some(
            &"notice: line 103: more than 100 skipped rows; \
              further skipped rows are counted in the summary only"
        )
    );
    assert_eq!(
        lines.get(101..),
        Some(
            &[
                "summary: skipped=1000000 rejected=0 ignored=1000000",
                "summary: reason=client_mismatch count=1000000",
            ][..]
        )
    );
    assert!(stderr.len() < 16 * 1024, "stderr is {} bytes", stderr.len());
    Ok(())
}

/// A writer whose every write and flush fails with a broken pipe.
struct BrokenPipe;

impl io::Write for BrokenPipe {
    fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
        Err(io::ErrorKind::BrokenPipe.into())
    }

    fn flush(&mut self) -> io::Result<()> {
        Err(io::ErrorKind::BrokenPipe.into())
    }
}

/// A16, ADR-0006 R10, ADR-0012 S8: with stderr failing on every write, the
/// run still completes and stdout is the same as with a working stderr.
#[test]
fn a16_failing_stderr_changes_nothing() -> TestResult {
    let inputs = [
        // A1: withdraw, then dispute and charge back.
        "type,client,tx,amount\ndeposit,1,1,100\nwithdrawal,1,2,100\n\
         dispute,1,1,\nchargeback,1,1,\n",
        // A2: a withdrawal rejected while available is negative.
        "type,client,tx,amount\ndeposit,1,1,100\nwithdrawal,1,2,100\n\
         dispute,1,1,\nwithdrawal,1,3,10\nresolve,1,1,\n",
        // A25 and more: rejected, ignored, and invalid rows past the budget.
        &format!(
            "type,client,tx,amount\ndeposit,1,1,10\ndispute,1,1,\nchargeback,1,1,\n\
             withdrawal,1,3,100\n{}",
            "dispute,2,9,\nbogus\n".repeat(150)
        ),
    ];
    for input in inputs {
        let (working, _) = streams_of(input.as_bytes())?;
        let mut stdout = Vec::new();
        run(input.as_bytes(), &mut stdout, BrokenPipe)?;
        assert_eq!(String::from_utf8(stdout)?, working, "{input:?}");
    }
    Ok(())
}

/// A32, ADR-0010 V11, ADR-0017 D4: a quoted `type` holding a bare carriage
/// return and an ANSI escape sequence makes the row invalid, and its
/// diagnostic is one line with both written as escapes.
#[test]
fn a32_hostile_input_is_escaped_on_one_line() -> TestResult {
    let input = b"type,client,tx,amount\ndeposit,1,1,10\n\"dep\rosit\x1b[31m\",1,2,5\n";
    let (stdout, stderr) = streams_of(input)?;
    assert_eq!(stdout, expected(&["1,10.0000,0.0000,10.0000,false"]));
    assert_eq!(
        stderr,
        "ignored: line 3: unknown_type input=\"\\\"dep\\rosit\\x1b[31m\\\",1,2,5\"\n\
         summary: skipped=1 rejected=0 ignored=1\n\
         summary: reason=unknown_type count=1\n"
    );
    assert!(!stderr.contains(['\r', '\x1b']));
    Ok(())
}

/// ADR-0017 D7: when reading stops on a fatal error, the summary still
/// counts the rows skipped before it, so the caller's error line follows it.
#[test]
fn a_fatal_error_still_writes_the_summary_first() -> TestResult {
    let input = "type,client,tx,amount\nwithdrawal,1,1,5\ndispute,1,7,\ndeposit,1,2,\"5";
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let result = run(input.as_bytes(), &mut stdout, &mut stderr);
    assert!(matches!(
        result,
        Err(RunError::UnterminatedQuote { line: 4 })
    ));
    assert!(stdout.is_empty());
    assert_eq!(
        String::from_utf8(stderr)?,
        "rejected: line 2: insufficient_funds type=withdrawal client=1 tx=1 amount=5.0000\n\
         ignored: line 3: unknown_tx type=dispute client=1 tx=7\n\
         summary: skipped=2 rejected=1 ignored=1\n\
         summary: reason=unknown_tx count=1\n\
         summary: reason=insufficient_funds count=1\n"
    );
    Ok(())
}

/// ADR-0017 D6, D7: a header error stops the run before any row, and the
/// summary of zero skipped rows is still written.
#[test]
fn a_header_error_writes_a_zero_summary() -> TestResult {
    for input in ["", "type,client,amount,tx\ndeposit,1,1,10\n"] {
        let mut stderr = Vec::new();
        assert!(run(input.as_bytes(), io::sink(), &mut stderr).is_err());
        assert_eq!(
            String::from_utf8(stderr)?,
            "summary: skipped=0 rejected=0 ignored=0\n",
            "{input:?}"
        );
    }
    Ok(())
}
