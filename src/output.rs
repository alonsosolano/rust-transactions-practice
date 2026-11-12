//! Writes the account rows as CSV, once, after all input is applied (ADR-0009, ADR-0011 S12, ADR-0015 P8).
//!
//! Adapter module of the layout in ADR-0011.

use std::fmt::Display;
use std::io::{self, Write};

use serde::{Serialize, Serializer};

use crate::account::Account;
use crate::amount::Balance;
use crate::transaction::ClientId;

/// The output header (ADR-0009 O1).
const HEADER: [&str; 5] = ["client", "available", "held", "total", "locked"];

/// One output row, in the column order of ADR-0009 O1.
#[derive(Serialize)]
struct OutputRow {
    client: u16,
    #[serde(serialize_with = "as_display")]
    available: Balance,
    #[serde(serialize_with = "as_display")]
    held: Balance,
    #[serde(serialize_with = "as_display")]
    total: Balance,
    locked: bool,
}

/// Serializes a value through its `Display` form, so the domain types do not
/// depend on serde (ADR-0011 S2). `Balance` prints the layout of ADR-0009 O5.
fn as_display<T: Display, S: Serializer>(value: &T, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.collect_str(value)
}

/// Writes the header and one row per account, then flushes.
///
/// `accounts` must be in ascending client order (ADR-0009 O3), which the
/// engine's iterator guarantees. The header is written even when there are
/// no accounts (O1). Each balance is the ledger value unchanged (O7).
///
/// # Errors
///
/// Any write or flush error of `output`.
pub fn write_accounts<'a, W: Write>(
    output: W,
    accounts: impl IntoIterator<Item = (ClientId, &'a Account)>,
) -> io::Result<()> {
    let mut writer = csv::WriterBuilder::new()
        .has_headers(false)
        .from_writer(output);
    writer.write_record(HEADER)?;
    for (client, account) in accounts {
        writer.serialize(OutputRow {
            client: client.0,
            available: account.available(),
            held: account.held(),
            total: account.total(),
            locked: account.is_locked(),
        })?;
    }
    writer.flush()
}

#[cfg(test)]
mod tests {
    use std::io;

    use super::write_accounts;
    use crate::account::Account;
    use crate::amount::{Amount, Balance};
    use crate::transaction::ClientId;

    fn balance(units: i128) -> Balance {
        Balance::from_units(units)
    }

    fn written(accounts: &[(u16, Account)]) -> String {
        let mut output = Vec::new();
        write_accounts(
            &mut output,
            accounts
                .iter()
                .map(|(client, account)| (ClientId(*client), account)),
        )
        .unwrap();
        String::from_utf8(output).unwrap()
    }

    /// ADR-0009 O1: the header alone when there are no accounts.
    #[test]
    fn no_accounts_prints_only_the_header() {
        assert_eq!(written(&[]), "client,available,held,total,locked\n");
    }

    /// ADR-0009 O2 to O9, with the rows of its worked example.
    #[test]
    fn rows_follow_the_exact_format() {
        let accounts = [
            (
                1,
                Account::with_balances(balance(-5_000), balance(0), true).unwrap(),
            ),
            (
                2,
                Account::with_balances(balance(20_000), balance(1), false).unwrap(),
            ),
            (3, Account::default()),
            (65535, Account::default().deposited(Amount::MAX).unwrap()),
        ];
        assert_eq!(
            written(&accounts),
            "client,available,held,total,locked\n\
             1,-0.5000,0.0000,-0.5000,true\n\
             2,2.0000,0.0001,2.0001,false\n\
             3,0.0000,0.0000,0.0000,false\n\
             65535,1844674407370955.1615,0.0000,1844674407370955.1615,false\n"
        );
    }

    /// A writer whose every write fails.
    struct Failing;

    impl io::Write for Failing {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::ErrorKind::BrokenPipe.into())
        }

        fn flush(&mut self) -> io::Result<()> {
            Err(io::ErrorKind::BrokenPipe.into())
        }
    }

    #[test]
    fn a_failed_write_is_an_error() {
        let accounts = [(ClientId(1), &Account::default())];
        let error = write_accounts(Failing, accounts).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
    }
}
