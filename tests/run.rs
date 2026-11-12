//! Integration tests: the public `run` function on in-memory input, stdout,
//! and stderr (ADR-0014). Covers the catalog rows of requirements §6 that
//! involve only deposits and withdrawals; the dispute rows come with E4.

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
