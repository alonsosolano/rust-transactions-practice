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
    use std::collections::{BTreeMap, BTreeSet, VecDeque};
    use std::io::{self, Write};

    use proptest::prelude::*;

    use super::{RunError, run};
    use crate::test_support::{amount_of_units, any_row, client_of, csv_line, row_from};
    use crate::transaction::{Transaction, TxId};

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

    // Properties of the whole pipeline (ADR-0014 T1): `run` on generated
    // input bytes, with in-memory stdout and stderr.

    const OUTPUT_HEADER: &str = "client,available,held,total,locked\n";

    /// Invalid rows of every kind in ADR-0010 V1 to V5 and V14. None is
    /// fatal, and none binds an ID or opens an account.
    const INVALID_LINES: &[&str] = &[
        "Deposit,1,2,5",
        "deposit,+1,3,5",
        "deposit,65536,4,5",
        "deposit,1,4294967296,5",
        "deposit,1,5",
        "deposit,1,6,",
        "deposit,1,7,5,",
        "deposit,1,8,-5",
        "deposit,1,9,0",
        "withdrawal,2,10,1.00005",
        "deposit,1,11,1e3",
        "deposit,1,12,\"1\"0",
        "type,client,tx,amount",
        "",
    ];

    /// Lines that end the run with a fatal error wherever they appear: a line
    /// that ends inside an open quote (V14) and a line over the size limit
    /// (V9).
    fn fatal_line() -> impl Strategy<Value = String> {
        prop_oneof![
            Just("deposit,1,1,\"5".to_owned()),
            Just(format!("deposit,1,1,{}", "1".repeat(5000))),
        ]
    }

    /// A valid row or an invalid one, as an input line.
    fn any_line() -> impl Strategy<Value = String> {
        prop_oneof![
            4 => any_row().prop_map(csv_line),
            1 => prop::sample::select(INVALID_LINES).prop_map(str::to_owned),
        ]
    }

    /// A valid row, or `None` for an invalid one, so tests can model which
    /// rows reached the engine.
    fn any_row_or_invalid() -> impl Strategy<Value = (Option<Transaction>, String)> {
        prop_oneof![
            4 => any_row().prop_map(|row| (Some(row), csv_line(row))),
            1 => prop::sample::select(INVALID_LINES).prop_map(|line| (None, line.to_owned())),
        ]
    }

    /// The input bytes: the header line, then each line ending in `\n`.
    fn input_of<S: AsRef<str>>(header: &str, lines: &[S]) -> Vec<u8> {
        let mut input = format!("{header}\n");
        for line in lines {
            input.push_str(line.as_ref());
            input.push('\n');
        }
        input.into_bytes()
    }

    /// Runs `input` and returns the result, stdout, and stderr.
    fn run_bytes(input: &[u8]) -> (Result<(), RunError>, Vec<u8>, Vec<u8>) {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let result = run(input, &mut stdout, &mut stderr);
        (result, stdout, stderr)
    }

    /// stdout of a run that must complete.
    fn stdout_of(input: &[u8]) -> String {
        let (result, stdout, _) = run_bytes(input);
        assert!(result.is_ok(), "{result:?}");
        String::from_utf8(stdout).unwrap()
    }

    /// The units of `0.0001` of a printed amount such as `-0.5000`.
    fn units_of(text: &str) -> i128 {
        let (sign, digits) = text.strip_prefix('-').map_or((1, text), |rest| (-1, rest));
        let (whole, fraction) = digits.split_once('.').unwrap();
        assert_eq!(fraction.len(), 4, "four fractional digits in {text:?}");
        whole
            .parse::<i128>()
            .unwrap()
            .checked_mul(10_000)
            .unwrap()
            .checked_add(fraction.parse::<i128>().unwrap())
            .unwrap()
            .checked_mul(sign)
            .unwrap()
    }

    /// The client of each output row, in output order.
    fn output_clients(stdout: &str) -> Vec<u16> {
        let rows = stdout.strip_prefix(OUTPUT_HEADER).unwrap();
        rows.lines()
            .map(|row| row.split(',').next().unwrap().parse().unwrap())
            .collect()
    }

    /// A row for the I18 property. Each client's `deposit` and `withdrawal`
    /// rows use only its own IDs (`client * 100 + 0..8`), so no ID is shared
    /// across clients. A `dispute`, `resolve`, or `chargeback` names an ID
    /// from any client's pool, so rows naming another client's transaction
    /// are common.
    fn pooled_row() -> impl Strategy<Value = Transaction> {
        let row = row_from(
            0_u8..5,
            0_u16..4,
            0_u32..8,
            (1_u64..=50_000).prop_map(amount_of_units),
        );
        (row, 0_u32..4).prop_map(|(row, target)| {
            let pool = |owner: u32, tx: TxId| {
                TxId(owner.checked_mul(100).unwrap().checked_add(tx.0).unwrap())
            };
            let own = u32::from(client_of(row).0);
            match row {
                Transaction::Deposit { client, tx, amount } => Transaction::Deposit {
                    client,
                    tx: pool(own, tx),
                    amount,
                },
                Transaction::Withdrawal { client, tx, amount } => Transaction::Withdrawal {
                    client,
                    tx: pool(own, tx),
                    amount,
                },
                Transaction::Dispute { client, tx } => Transaction::Dispute {
                    client,
                    tx: pool(target, tx),
                },
                Transaction::Resolve { client, tx } => Transaction::Resolve {
                    client,
                    tx: pool(target, tx),
                },
                Transaction::Chargeback { client, tx } => Transaction::Chargeback {
                    client,
                    tx: pool(target, tx),
                },
            }
        })
    }

    /// Pooled rows and a shuffled list of their clients, one entry per row,
    /// which [`interleave`] turns into a reordering that keeps each client's
    /// rows in their relative order.
    fn pooled_rows_and_order() -> impl Strategy<Value = (Vec<Transaction>, Vec<u16>)> {
        prop::collection::vec(pooled_row(), 0..64).prop_flat_map(|rows| {
            let clients: Vec<u16> = rows.iter().map(|row| client_of(*row).0).collect();
            (Just(rows), Just(clients).prop_shuffle())
        })
    }

    /// The rows reordered so that the `n`-th entry of `order` takes the next
    /// row of that client. Each client's rows keep their relative order.
    fn interleave(rows: &[Transaction], order: &[u16]) -> Vec<Transaction> {
        let mut queues: BTreeMap<u16, VecDeque<Transaction>> = BTreeMap::new();
        for row in rows {
            queues.entry(client_of(*row).0).or_default().push_back(*row);
        }
        order
            .iter()
            .map(|client| queues.get_mut(client).unwrap().pop_front().unwrap())
            .collect()
    }

    proptest! {
        /// I10: the same input always gives byte-identical stdout (and
        /// stderr), and every printed `total` equals the printed
        /// `available + held`.
        ///
        /// Covers: I10.
        #[test]
        fn i10_the_same_input_gives_the_same_output(lines in prop::collection::vec(any_line(), 0..64)) {
            let input = input_of("type, client, tx, amount", &lines);
            let (first_result, first_stdout, first_stderr) = run_bytes(&input);
            let (second_result, second_stdout, second_stderr) = run_bytes(&input);
            prop_assert!(first_result.is_ok() && second_result.is_ok());
            prop_assert_eq!(&first_stdout, &second_stdout);
            prop_assert_eq!(&first_stderr, &second_stderr);

            let stdout = String::from_utf8(first_stdout).unwrap();
            let rows = stdout.strip_prefix(OUTPUT_HEADER).unwrap();
            for row in rows.lines() {
                let fields: Vec<&str> = row.split(',').collect();
                prop_assert_eq!(fields.len(), 5, "{}", row);
                prop_assert_eq!(
                    units_of(fields[1]).checked_add(units_of(fields[2])),
                    Some(units_of(fields[3])),
                    "{}", row
                );
            }
        }

        /// I12: every run either completes with the full output CSV or fails
        /// with one fatal error and an empty stdout. The input mixes valid,
        /// invalid, and fatal lines under a valid or a wrong header, and the
        /// expected outcome follows from which of them it contains. No input
        /// makes the run panic.
        ///
        /// Covers: I12.
        #[test]
        fn i12_every_run_completes_or_fails_closed(
            valid_header in any::<bool>(),
            lines in prop::collection::vec(
                prop_oneof![8 => any_line().prop_map(|line| (false, line)), 1 => fatal_line().prop_map(|line| (true, line))],
                0..48,
            ),
        ) {
            let header = if valid_header { "type,client,tx,amount" } else { "type,client,amount,tx" };
            let texts: Vec<&str> = lines.iter().map(|(_, line)| line.as_str()).collect();
            let (result, stdout, _) = run_bytes(&input_of(header, &texts));
            let fatal = !valid_header || lines.iter().any(|(is_fatal, _)| *is_fatal);
            match result {
                Ok(()) => {
                    prop_assert!(!fatal, "a fatal input completed");
                    let stdout = String::from_utf8(stdout).unwrap();
                    prop_assert!(stdout.starts_with(OUTPUT_HEADER));
                    prop_assert!(stdout.ends_with('\n'));
                }
                Err(RunError::WriteOutput(error)) => prop_assert!(false, "stdout cannot fail: {}", error),
                Err(error) => {
                    prop_assert!(fatal, "an unexpected fatal error: {}", error);
                    prop_assert!(stdout.is_empty(), "a failed run wrote stdout");
                }
            }
        }

        /// I12 on arbitrary bytes after a valid header: the run never panics,
        /// and it either completes with the output CSV or fails with an
        /// empty stdout.
        ///
        /// Covers: I12.
        #[test]
        fn i12_arbitrary_bytes_complete_or_fail_closed(body in prop::collection::vec(any::<u8>(), 0..512)) {
            let mut input = b"type,client,tx,amount\n".to_vec();
            input.extend(body);
            let (result, stdout, _) = run_bytes(&input);
            match result {
                Ok(()) => prop_assert!(stdout.starts_with(OUTPUT_HEADER.as_bytes()) && stdout.ends_with(b"\n")),
                Err(RunError::WriteOutput(error)) => prop_assert!(false, "stdout cannot fail: {}", error),
                Err(_) => prop_assert!(stdout.is_empty(), "a failed run wrote stdout"),
            }
        }

        /// I17: the output has exactly one row per account, in ascending
        /// client order, and the accounts are exactly the clients with at
        /// least one applied or rejected `deposit` or `withdrawal`. The
        /// expected clients come from a model of ID binding (ADR-0010 V6),
        /// not from the engine: the first `deposit` or `withdrawal` to use an
        /// ID opens its client's account, and a later one is a duplicate.
        /// Invalid and dispute-related rows open nothing.
        ///
        /// Covers: I17.
        #[test]
        fn i17_one_row_per_opened_account_in_client_order(lines in prop::collection::vec(any_row_or_invalid(), 0..64)) {
            let mut bound = BTreeSet::new();
            let mut opened = BTreeSet::new();
            for (row, _) in &lines {
                if let Some(Transaction::Deposit { client, tx, .. } | Transaction::Withdrawal { client, tx, .. }) = row {
                    if bound.insert(*tx) {
                        opened.insert(client.0);
                    }
                }
            }
            let texts: Vec<&str> = lines.iter().map(|(_, line)| line.as_str()).collect();
            let stdout = stdout_of(&input_of("type,client,tx,amount", &texts));
            prop_assert_eq!(output_clients(&stdout), opened.into_iter().collect::<Vec<_>>());
        }

        /// I18: when no ID is shared by two clients' `deposit` or
        /// `withdrawal` rows, each client's output row is the same as in a
        /// run on only that client's rows, and any reordering that keeps each
        /// client's rows in their relative order gives byte-identical stdout
        /// (ADR-0013 C6). The rows include disputes, resolves, and
        /// chargebacks that name other clients' IDs.
        ///
        /// Covers: I18.
        #[test]
        fn i18_a_client_row_depends_only_on_its_own_rows((rows, order) in pooled_rows_and_order()) {
            let lines: Vec<String> = rows.iter().copied().map(csv_line).collect();
            let full = stdout_of(&input_of("type,client,tx,amount", &lines));

            let mut merged = OUTPUT_HEADER.to_owned();
            for client in 0..4 {
                let own: Vec<&String> = rows
                    .iter()
                    .zip(&lines)
                    .filter(|(row, _)| client_of(**row).0 == client)
                    .map(|(_, line)| line)
                    .collect();
                let alone = stdout_of(&input_of("type,client,tx,amount", &own));
                merged.push_str(alone.strip_prefix(OUTPUT_HEADER).unwrap());
            }
            prop_assert_eq!(&full, &merged);

            let reordered: Vec<String> = interleave(&rows, &order).into_iter().map(csv_line).collect();
            prop_assert_eq!(&full, &stdout_of(&input_of("type,client,tx,amount", &reordered)));
        }
    }
}
