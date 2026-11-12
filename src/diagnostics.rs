//! Bounded, escaped stderr reporting and the end-of-run summary (ADR-0006 R10, ADR-0010 V11, ADR-0017).
//!
//! Adapter module of the layout in ADR-0011. [`Diagnostics`] receives every
//! skipped row, writes a detailed line for the first [`MAX_DETAILED_ROWS`],
//! one notice when the budget runs out, and counts every row by reason for
//! the summary. [`Escaped`] shows raw input as one safe line.
//!
//! Every write is best effort (ADR-0012 S8): its result is dropped, so a
//! failing stderr never changes stdout, account state, or the exit code.

use std::fmt::{self, Display, Formatter, Write as _};
use std::io::{self, Write};

use crate::amount::{Amount, AmountError};
use crate::engine::Rejection;
use crate::input::InvalidRow;
use crate::transaction::{ClientId, Transaction, TxId};

/// Skipped rows that get a detailed line; later ones are only counted
/// (ADR-0006 R10, ADR-0017 D5).
pub const MAX_DETAILED_ROWS: u64 = 100;

/// Why a row was skipped: one code per row of the ADR-0010 condition table.
///
/// The variants are declared in the order of that table, which is the order
/// of the summary (ADR-0017 D6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reason {
    MalformedQuote,
    FieldCount,
    UnknownType,
    InvalidClient,
    InvalidTx,
    MissingAmount,
    InvalidAmount,
    DuplicateTx,
    UnknownTx,
    ClientMismatch,
    NotADeposit,
    AlreadyDisputed,
    AlreadyChargedBack,
    NotDisputed,
    Overflow,
    InsufficientFunds,
    AccountLocked,
}

/// The number of reasons.
const REASONS: usize = 17;

impl Reason {
    /// Every reason, in summary order. `index` returns each one's position
    /// here, which a unit test checks.
    const ALL: [Self; REASONS] = [
        Self::MalformedQuote,
        Self::FieldCount,
        Self::UnknownType,
        Self::InvalidClient,
        Self::InvalidTx,
        Self::MissingAmount,
        Self::InvalidAmount,
        Self::DuplicateTx,
        Self::UnknownTx,
        Self::ClientMismatch,
        Self::NotADeposit,
        Self::AlreadyDisputed,
        Self::AlreadyChargedBack,
        Self::NotDisputed,
        Self::Overflow,
        Self::InsufficientFunds,
        Self::AccountLocked,
    ];

    /// The position of this reason's counter. An exhaustive match, so a new
    /// reason fails to compile until it has a counter (ADR-0011 S6).
    fn index(self) -> usize {
        match self {
            Self::MalformedQuote => 0,
            Self::FieldCount => 1,
            Self::UnknownType => 2,
            Self::InvalidClient => 3,
            Self::InvalidTx => 4,
            Self::MissingAmount => 5,
            Self::InvalidAmount => 6,
            Self::DuplicateTx => 7,
            Self::UnknownTx => 8,
            Self::ClientMismatch => 9,
            Self::NotADeposit => 10,
            Self::AlreadyDisputed => 11,
            Self::AlreadyChargedBack => 12,
            Self::NotDisputed => 13,
            Self::Overflow => 14,
            Self::InsufficientFunds => 15,
            Self::AccountLocked => 16,
        }
    }

    /// The reason code of the ADR-0010 condition table.
    fn code(self) -> &'static str {
        match self {
            Self::MalformedQuote => "malformed_quote",
            Self::FieldCount => "field_count",
            Self::UnknownType => "unknown_type",
            Self::InvalidClient => "invalid_client",
            Self::InvalidTx => "invalid_tx",
            Self::MissingAmount => "missing_amount",
            Self::InvalidAmount => "invalid_amount",
            Self::DuplicateTx => "duplicate_tx",
            Self::UnknownTx => "unknown_tx",
            Self::ClientMismatch => "client_mismatch",
            Self::NotADeposit => "not_a_deposit",
            Self::AlreadyDisputed => "already_disputed",
            Self::AlreadyChargedBack => "already_charged_back",
            Self::NotDisputed => "not_disputed",
            Self::Overflow => "overflow",
            Self::InsufficientFunds => "insufficient_funds",
            Self::AccountLocked => "account_locked",
        }
    }

    /// Whether the row is rejected (a declined withdrawal) rather than
    /// ignored, as in the ADR-0010 outcome classes.
    fn is_rejected(self) -> bool {
        matches!(self, Self::InsufficientFunds | Self::AccountLocked)
    }

    /// The class prefix of a detailed line (ADR-0017 D2).
    fn class(self) -> &'static str {
        if self.is_rejected() {
            "rejected"
        } else {
            "ignored"
        }
    }
}

impl From<InvalidRow> for Reason {
    fn from(invalid: InvalidRow) -> Self {
        match invalid {
            InvalidRow::MalformedQuote => Self::MalformedQuote,
            InvalidRow::FieldCount => Self::FieldCount,
            InvalidRow::UnknownType => Self::UnknownType,
            InvalidRow::InvalidClient => Self::InvalidClient,
            InvalidRow::InvalidTx => Self::InvalidTx,
            InvalidRow::MissingAmount => Self::MissingAmount,
            InvalidRow::InvalidAmount(_) => Self::InvalidAmount,
        }
    }
}

impl From<Rejection> for Reason {
    fn from(rejection: Rejection) -> Self {
        match rejection {
            Rejection::InsufficientFunds => Self::InsufficientFunds,
            Rejection::AccountLocked => Self::AccountLocked,
            Rejection::DuplicateTx { .. } => Self::DuplicateTx,
            Rejection::Overflow => Self::Overflow,
            Rejection::UnknownTx => Self::UnknownTx,
            Rejection::ClientMismatch { .. } => Self::ClientMismatch,
            Rejection::NotADeposit => Self::NotADeposit,
            Rejection::AlreadyDisputed => Self::AlreadyDisputed,
            Rejection::AlreadyChargedBack => Self::AlreadyChargedBack,
            Rejection::NotDisputed => Self::NotDisputed,
        }
    }
}

/// The `detail=` value of an `invalid_amount` line (ADR-0017 D4).
fn amount_detail(error: AmountError) -> &'static str {
    match error {
        AmountError::Empty => "empty",
        AmountError::Syntax => "syntax",
        AmountError::TooPrecise => "too_precise",
        AmountError::Zero => "zero",
        AmountError::TooLarge => "too_large",
    }
}

/// The row type, client, `tx`, and amount of a transaction, as shown in a
/// detailed line (ADR-0017 D3).
fn describe(transaction: &Transaction) -> (&'static str, ClientId, TxId, Option<Amount>) {
    match *transaction {
        Transaction::Deposit { client, tx, amount } => ("deposit", client, tx, Some(amount)),
        Transaction::Withdrawal { client, tx, amount } => ("withdrawal", client, tx, Some(amount)),
        Transaction::Dispute { client, tx } => ("dispute", client, tx, None),
        Transaction::Resolve { client, tx } => ("resolve", client, tx, None),
        Transaction::Chargeback { client, tx } => ("chargeback", client, tx, None),
    }
}

/// Receives every row that the run skips and writes the bounded diagnostics
/// of ADR-0006 R10 in the format of ADR-0017.
///
/// Call [`Diagnostics::finish`] once the input is no longer read, before the
/// output CSV and before any fatal line (ADR-0017 D7).
pub struct Diagnostics<E> {
    /// Where the lines go: stderr in the binary.
    sink: E,
    /// One line, formatted in full before it is written (ADR-0017 D1).
    line: Vec<u8>,
    /// Skipped rows so far.
    skipped: u64,
    /// Skipped rows per reason, at the position given by `Reason::index`.
    counts: [u64; REASONS],
}

impl<E: Write> Diagnostics<E> {
    /// Diagnostics that write to `sink`.
    pub fn new(sink: E) -> Self {
        Self {
            sink,
            line: Vec::new(),
            skipped: 0,
            counts: [0; REASONS],
        }
    }

    /// A row that failed validation (ADR-0010 V1 to V5, V14). `input` is the
    /// raw line, which the detailed line echoes escaped (ADR-0017 D4).
    pub fn invalid(&mut self, line: u64, input: &[u8], invalid: InvalidRow) {
        let reason = Reason::from(invalid);
        if self.begin(line, reason) {
            if let InvalidRow::InvalidAmount(error) = invalid {
                self.push(format_args!(" detail={}", amount_detail(error)));
            }
            self.push(format_args!(" input={}\n", Escaped::new(input)));
            self.flush_line();
        }
        self.count(reason);
    }

    /// A valid row that the engine rejected or ignored (ADR-0017 D3).
    pub fn rejected(&mut self, line: u64, transaction: &Transaction, rejection: Rejection) {
        let reason = Reason::from(rejection);
        if self.begin(line, reason) {
            let (kind, client, tx, amount) = describe(transaction);
            self.push(format_args!(" type={kind} client={client} tx={}", tx.0));
            if let Some(amount) = amount {
                self.push(format_args!(" amount={amount}"));
            }
            if let Rejection::DuplicateTx { owner } | Rejection::ClientMismatch { owner } =
                rejection
            {
                self.push(format_args!(" owner={owner}"));
            }
            self.push(format_args!("\n"));
            self.flush_line();
        }
        self.count(reason);
    }

    /// Writes the summary (ADR-0017 D6): the totals, then one line for each
    /// reason with a non-zero count, in summary order.
    pub fn finish(mut self) {
        let mut rejected: u64 = 0;
        for reason in Reason::ALL {
            if reason.is_rejected() {
                // Saturation is harmless: a count cannot reach u64::MAX,
                // because each row takes at least one byte of input.
                rejected = rejected.saturating_add(self.count_of(reason));
            }
        }
        let skipped = self.skipped;
        let ignored = skipped.saturating_sub(rejected);
        self.line.clear();
        self.push(format_args!(
            "summary: skipped={skipped} rejected={rejected} ignored={ignored}\n"
        ));
        self.flush_line();
        for reason in Reason::ALL {
            let count = self.count_of(reason);
            if count > 0 {
                self.line.clear();
                self.push(format_args!(
                    "summary: reason={} count={count}\n",
                    reason.code()
                ));
                self.flush_line();
            }
        }
        // Best effort, like every write here.
        let _ = self.sink.flush();
    }

    /// Starts the line for one skipped row, if the budget allows.
    ///
    /// Returns `true` when the caller should finish and write a detailed
    /// line, whose prefix is already in the buffer. Writes the single notice
    /// for the first row over the budget (ADR-0017 D5).
    fn begin(&mut self, line: u64, reason: Reason) -> bool {
        self.line.clear();
        if self.skipped < MAX_DETAILED_ROWS {
            self.push(format_args!(
                "{}: line {line}: {}",
                reason.class(),
                reason.code()
            ));
            true
        } else {
            if self.skipped == MAX_DETAILED_ROWS {
                self.push(format_args!(
                    "notice: line {line}: more than {MAX_DETAILED_ROWS} skipped rows; \
                     further skipped rows are counted in the summary only\n"
                ));
                self.flush_line();
            }
            false
        }
    }

    /// Counts one skipped row.
    fn count(&mut self, reason: Reason) {
        // Saturation is harmless (ADR-0012 S7): a count cannot reach
        // u64::MAX, because each row takes at least one byte of input.
        self.skipped = self.skipped.saturating_add(1);
        if let Some(count) = self.counts.get_mut(reason.index()) {
            *count = count.saturating_add(1);
        }
    }

    /// The number of rows skipped for `reason`.
    fn count_of(&self, reason: Reason) -> u64 {
        self.counts.get(reason.index()).copied().unwrap_or(0)
    }

    /// Appends formatted text to the line buffer.
    fn push(&mut self, text: fmt::Arguments<'_>) {
        // Writing to a `Vec` cannot fail, and every `Display` used here
        // returns `Ok`, so there is no error to handle.
        let _ = io::Write::write_fmt(&mut self.line, text);
    }

    /// Writes the buffered line in one call and drops the result
    /// (ADR-0017 D1, ADR-0012 S8).
    fn flush_line(&mut self) {
        let _ = self.sink.write_all(&self.line);
    }
}

/// Most bytes of raw input echoed in one diagnostic. Longer input is cut off
/// and marked, so each diagnostic line stays bounded (ADR-0006 R10).
pub const MAX_ECHO_BYTES: usize = 256;

/// Raw input bytes shown on stderr as one safe, quoted line (ADR-0010 V11).
///
/// Printable ASCII is written as is. A quote and a backslash are escaped with
/// a backslash; tab, line feed, and carriage return as `\t`, `\n`, and `\r`;
/// every other control byte and every non-ASCII byte as `\xNN`. The result is
/// enclosed in double quotes, so the end of the echoed input is always
/// visible. Input longer than `MAX_ECHO_BYTES` (256) is cut off, and
/// ` (truncated)` follows the closing quote.
///
/// Escaping keeps hostile input from forging log lines (CWE-117) or sending
/// terminal control sequences to the operator's screen.
#[derive(Debug, Clone, Copy)]
pub struct Escaped<'a>(&'a [u8]);

impl<'a> Escaped<'a> {
    /// Wraps raw bytes for display.
    #[must_use]
    pub fn new(bytes: &'a [u8]) -> Self {
        Self(bytes)
    }
}

impl Display for Escaped<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_char('"')?;
        for &byte in self.0.iter().take(MAX_ECHO_BYTES) {
            match byte {
                b'"' => f.write_str("\\\"")?,
                b'\\' => f.write_str("\\\\")?,
                b'\t' => f.write_str("\\t")?,
                b'\n' => f.write_str("\\n")?,
                b'\r' => f.write_str("\\r")?,
                b' '..=b'~' => f.write_char(char::from(byte))?,
                _ => write!(f, "\\x{byte:02x}")?,
            }
        }
        f.write_char('"')?;
        if self.0.len() > MAX_ECHO_BYTES {
            f.write_str(" (truncated)")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::io::{self, Write};

    use super::{Diagnostics, Escaped, MAX_DETAILED_ROWS, MAX_ECHO_BYTES, REASONS, Reason};
    use crate::amount::{Amount, AmountError};
    use crate::engine::Rejection;
    use crate::input::InvalidRow;
    use crate::transaction::{ClientId, Transaction, TxId};

    fn escape(bytes: &[u8]) -> String {
        Escaped::new(bytes).to_string()
    }

    /// Runs `report` against fresh diagnostics, finishes them, and returns
    /// everything written.
    fn render(report: impl FnOnce(&mut Diagnostics<&mut Vec<u8>>)) -> String {
        let mut written = Vec::new();
        let mut diagnostics = Diagnostics::new(&mut written);
        report(&mut diagnostics);
        diagnostics.finish();
        String::from_utf8(written).unwrap()
    }

    fn amount(text: &str) -> Amount {
        Amount::parse(text.as_bytes()).unwrap()
    }

    fn withdrawal(client: u16, tx: u32, value: &str) -> Transaction {
        Transaction::Withdrawal {
            client: ClientId(client),
            tx: TxId(tx),
            amount: amount(value),
        }
    }

    fn dispute(client: u16, tx: u32) -> Transaction {
        Transaction::Dispute {
            client: ClientId(client),
            tx: TxId(tx),
        }
    }

    #[test]
    fn reasons_are_listed_in_index_order() {
        assert_eq!(Reason::ALL.len(), REASONS);
        for (position, reason) in Reason::ALL.into_iter().enumerate() {
            assert_eq!(reason.index(), position, "{reason:?}");
        }
    }

    #[test]
    fn reason_codes_are_distinct() {
        let mut codes: Vec<&str> = Reason::ALL.into_iter().map(Reason::code).collect();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), REASONS);
    }

    /// ADR-0017 D6: a run that skipped nothing still writes its summary.
    #[test]
    fn nothing_skipped_writes_a_zero_summary() {
        assert_eq!(render(|_| {}), "summary: skipped=0 rejected=0 ignored=0\n");
    }

    /// One test per invalid-row reason: its detailed line, with the raw line
    /// echoed escaped, and its summary count (ADR-0017 D4, D6).
    #[test]
    fn every_invalid_row_reason_is_reported_and_counted() {
        let cases: [(InvalidRow, &str, &str); 7] = [
            (
                InvalidRow::MalformedQuote,
                r#"deposit,1,1,1"0"#,
                r#"ignored: line 2: malformed_quote input="deposit,1,1,1\"0""#,
            ),
            (
                InvalidRow::FieldCount,
                "deposit,1",
                r#"ignored: line 2: field_count input="deposit,1""#,
            ),
            (
                InvalidRow::UnknownType,
                "Deposit,1,1,5",
                r#"ignored: line 2: unknown_type input="Deposit,1,1,5""#,
            ),
            (
                InvalidRow::InvalidClient,
                "deposit,65536,1,5",
                r#"ignored: line 2: invalid_client input="deposit,65536,1,5""#,
            ),
            (
                InvalidRow::InvalidTx,
                "deposit,1,+1,5",
                r#"ignored: line 2: invalid_tx input="deposit,1,+1,5""#,
            ),
            (
                InvalidRow::MissingAmount,
                "withdrawal,1,1,",
                r#"ignored: line 2: missing_amount input="withdrawal,1,1,""#,
            ),
            (
                InvalidRow::InvalidAmount(AmountError::TooPrecise),
                "deposit,1,1,1.00005",
                r#"ignored: line 2: invalid_amount detail=too_precise input="deposit,1,1,1.00005""#,
            ),
        ];
        for (invalid, input, detail) in cases {
            let code = Reason::from(invalid).code();
            let written = render(|d| d.invalid(2, input.as_bytes(), invalid));
            assert_eq!(
                written,
                format!(
                    "{detail}\n\
                     summary: skipped=1 rejected=0 ignored=1\n\
                     summary: reason={code} count=1\n"
                ),
                "{invalid:?}"
            );
        }
    }

    /// Every cause of an invalid amount has its own `detail=` value, and all
    /// of them count as one `invalid_amount` reason (ADR-0017 D4).
    #[test]
    fn every_amount_error_has_a_detail() {
        let cases = [
            (AmountError::Empty, "empty"),
            (AmountError::Syntax, "syntax"),
            (AmountError::TooPrecise, "too_precise"),
            (AmountError::Zero, "zero"),
            (AmountError::TooLarge, "too_large"),
        ];
        let written = render(|d| {
            for (error, _) in cases {
                d.invalid(9, b"x", InvalidRow::InvalidAmount(error));
            }
        });
        for (_, detail) in cases {
            assert!(
                written.contains(&format!(
                    "ignored: line 9: invalid_amount detail={detail} input=\"x\"\n"
                )),
                "{detail}: {written}"
            );
        }
        assert!(written.ends_with("summary: reason=invalid_amount count=5\n"));
    }

    /// One test per engine reason: its class, fields, owner when known, and
    /// summary count (ADR-0017 D3, D6).
    #[test]
    fn every_engine_reason_is_reported_and_counted() {
        let cases: [(Transaction, Rejection, &str); 10] = [
            (
                withdrawal(1, 4, "1.5"),
                Rejection::InsufficientFunds,
                "rejected: line 3: insufficient_funds type=withdrawal client=1 tx=4 amount=1.5000",
            ),
            (
                withdrawal(1, 4, "1.5"),
                Rejection::AccountLocked,
                "rejected: line 3: account_locked type=withdrawal client=1 tx=4 amount=1.5000",
            ),
            (
                Transaction::Deposit {
                    client: ClientId(2),
                    tx: TxId(7),
                    amount: amount("100"),
                },
                Rejection::DuplicateTx { owner: ClientId(1) },
                "ignored: line 3: duplicate_tx type=deposit client=2 tx=7 amount=100.0000 owner=1",
            ),
            (
                Transaction::Deposit {
                    client: ClientId(1),
                    tx: TxId(7),
                    amount: amount("0.0001"),
                },
                Rejection::Overflow,
                "ignored: line 3: overflow type=deposit client=1 tx=7 amount=0.0001",
            ),
            (
                dispute(2, 1),
                Rejection::UnknownTx,
                "ignored: line 3: unknown_tx type=dispute client=2 tx=1",
            ),
            (
                dispute(2, 1),
                Rejection::ClientMismatch { owner: ClientId(1) },
                "ignored: line 3: client_mismatch type=dispute client=2 tx=1 owner=1",
            ),
            (
                Transaction::Resolve {
                    client: ClientId(1),
                    tx: TxId(2),
                },
                Rejection::NotADeposit,
                "ignored: line 3: not_a_deposit type=resolve client=1 tx=2",
            ),
            (
                dispute(1, 1),
                Rejection::AlreadyDisputed,
                "ignored: line 3: already_disputed type=dispute client=1 tx=1",
            ),
            (
                Transaction::Chargeback {
                    client: ClientId(1),
                    tx: TxId(1),
                },
                Rejection::AlreadyChargedBack,
                "ignored: line 3: already_charged_back type=chargeback client=1 tx=1",
            ),
            (
                Transaction::Chargeback {
                    client: ClientId(65535),
                    tx: TxId(u32::MAX),
                },
                Rejection::NotDisputed,
                "ignored: line 3: not_disputed type=chargeback client=65535 tx=4294967295",
            ),
        ];
        for (transaction, rejection, detail) in cases {
            let reason = Reason::from(rejection);
            let (rejected, ignored) = if reason.is_rejected() { (1, 0) } else { (0, 1) };
            let written = render(|d| d.rejected(3, &transaction, rejection));
            assert_eq!(
                written,
                format!(
                    "{detail}\n\
                     summary: skipped=1 rejected={rejected} ignored={ignored}\n\
                     summary: reason={} count=1\n",
                    reason.code()
                ),
                "{rejection:?}"
            );
        }
    }

    /// ADR-0017 D6: only reasons that occurred are listed, in summary order,
    /// whatever the order of the rows.
    #[test]
    fn summary_lists_reasons_in_table_order() {
        let written = render(|d| {
            d.rejected(2, &withdrawal(1, 1, "5"), Rejection::AccountLocked);
            d.rejected(3, &dispute(1, 9), Rejection::UnknownTx);
            d.invalid(4, b"bogus", InvalidRow::FieldCount);
            d.rejected(5, &dispute(1, 8), Rejection::UnknownTx);
        });
        let summary: Vec<&str> = written
            .lines()
            .filter(|line| line.starts_with("summary:"))
            .collect();
        assert_eq!(
            summary,
            [
                "summary: skipped=4 rejected=1 ignored=3",
                "summary: reason=field_count count=1",
                "summary: reason=unknown_tx count=2",
                "summary: reason=account_locked count=1",
            ]
        );
    }

    /// ADR-0006 R10, ADR-0017 D5: detail for the first 100 rows, one notice
    /// at the 101st, and nothing more until the summary, which counts all.
    #[test]
    fn detail_stops_after_the_budget_with_one_notice() {
        let rows: u64 = 250;
        let written = render(|d| {
            for line in 2..rows.checked_add(2).unwrap() {
                d.rejected(line, &dispute(2, 1), Rejection::UnknownTx);
            }
        });
        let lines: Vec<&str> = written.lines().collect();
        let detailed = lines.iter().filter(|l| l.starts_with("ignored:")).count();
        assert_eq!(u64::try_from(detailed).unwrap(), MAX_DETAILED_ROWS);
        assert_eq!(
            lines.first(),
            Some(&"ignored: line 2: unknown_tx type=dispute client=2 tx=1")
        );
        assert_eq!(
            lines.get(100),
            Some(
                &"notice: line 102: more than 100 skipped rows; \
                  further skipped rows are counted in the summary only"
            )
        );
        assert_eq!(
            lines.get(101..),
            Some(
                &[
                    "summary: skipped=250 rejected=0 ignored=250",
                    "summary: reason=unknown_tx count=250",
                ][..]
            )
        );
    }

    /// Exactly 100 skipped rows fit the budget: no notice.
    #[test]
    fn a_full_budget_writes_no_notice() {
        let written = render(|d| {
            for line in 1..=MAX_DETAILED_ROWS {
                d.invalid(line, b"x", InvalidRow::FieldCount);
            }
        });
        assert!(!written.contains("notice:"));
        assert!(written.contains("summary: skipped=100 rejected=0 ignored=100\n"));
    }

    /// A writer that fails its first `failures` calls, then records.
    struct Flaky {
        failures: usize,
        written: Vec<u8>,
    }

    impl Write for Flaky {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if let Some(left) = self.failures.checked_sub(1) {
                self.failures = left;
                Err(io::ErrorKind::BrokenPipe.into())
            } else {
                self.written.extend_from_slice(buf);
                Ok(buf.len())
            }
        }

        fn flush(&mut self) -> io::Result<()> {
            Err(io::ErrorKind::BrokenPipe.into())
        }
    }

    /// Part of A16: failed writes are dropped without a panic, every row is
    /// still counted, and a line is written whole or not at all.
    #[test]
    fn failed_writes_are_dropped_and_counting_continues() {
        let mut sink = Flaky {
            failures: 2,
            written: Vec::new(),
        };
        let mut diagnostics = Diagnostics::new(&mut sink);
        diagnostics.rejected(2, &dispute(2, 1), Rejection::UnknownTx);
        diagnostics.rejected(3, &dispute(2, 1), Rejection::UnknownTx);
        diagnostics.rejected(4, &dispute(2, 1), Rejection::UnknownTx);
        diagnostics.finish();
        assert_eq!(
            String::from_utf8(sink.written).unwrap(),
            "ignored: line 4: unknown_tx type=dispute client=2 tx=1\n\
             summary: skipped=3 rejected=0 ignored=3\n\
             summary: reason=unknown_tx count=3\n"
        );
    }

    /// Part of A32: hostile input in an invalid row stays on one line.
    #[test]
    fn echoed_input_cannot_forge_a_line() {
        let written = render(|d| {
            d.invalid(
                2,
                b"\"dep\rosit\x1b[2J\",1,1,5\nsummary: skipped=0",
                InvalidRow::MalformedQuote,
            );
        });
        let first = written.lines().next().unwrap();
        assert_eq!(
            first,
            r#"ignored: line 2: malformed_quote input="\"dep\rosit\x1b[2J\",1,1,5\nsummary: skipped=0""#
        );
        assert_eq!(written.lines().count(), 3);
        assert!(
            written
                .bytes()
                .all(|b| b == b'\n' || (b' '..=b'~').contains(&b))
        );
    }

    /// ADR-0017 bound: a detailed line stays short whatever the input.
    #[test]
    fn a_detailed_line_is_bounded() {
        let input = vec![0xff_u8; 4096];
        let written = render(|d| d.invalid(2, &input, InvalidRow::UnknownType));
        let first = written.lines().next().unwrap();
        assert!(first.ends_with(" (truncated)"));
        assert!(first.len() < 1100, "{}", first.len());
    }

    #[test]
    fn printable_ascii_is_quoted_unchanged() {
        assert_eq!(escape(b"input.csv"), r#""input.csv""#);
        assert_eq!(escape(b" ~!#$%&'()*+,-./09:;<=>?@AZ[]^_`az{|}"), {
            r#"" ~!#$%&'()*+,-./09:;<=>?@AZ[]^_`az{|}""#
        });
    }

    #[test]
    fn empty_input_is_an_empty_quoted_string() {
        assert_eq!(escape(b""), r#""""#);
    }

    #[test]
    fn quote_and_backslash_are_escaped() {
        assert_eq!(escape(br#"a"b\c"#), r#""a\"b\\c""#);
    }

    /// Part of A32: escaped input can never start a second, forged line.
    #[test]
    fn line_breaks_and_tab_are_escaped() {
        let shown = escape(b"a\nerror: forged\r\tb");
        assert_eq!(shown, r#""a\nerror: forged\r\tb""#);
        assert!(!shown.contains(['\n', '\r', '\t']));
    }

    #[test]
    fn control_bytes_are_hex_escaped() {
        assert_eq!(escape(b"\x00\x1b[2J\x7f"), r#""\x00\x1b[2J\x7f""#);
    }

    #[test]
    fn non_ascii_bytes_are_hex_escaped() {
        assert_eq!(escape("é".as_bytes()), r#""\xc3\xa9""#);
        assert_eq!(escape(b"\xff\xfe"), r#""\xff\xfe""#);
    }

    #[test]
    fn input_at_the_limit_is_not_truncated() {
        let input = vec![b'a'; MAX_ECHO_BYTES];
        let shown = escape(&input);
        assert!(!shown.ends_with(" (truncated)"));
        assert_eq!(shown.matches('a').count(), MAX_ECHO_BYTES);
    }

    #[test]
    fn input_over_the_limit_is_truncated_and_marked() {
        let input = vec![b'a'; MAX_ECHO_BYTES.checked_add(1).unwrap()];
        let shown = escape(&input);
        let quoted = shown.strip_suffix(" (truncated)").unwrap();
        assert!(quoted.ends_with('"'));
        assert_eq!(quoted.matches('a').count(), MAX_ECHO_BYTES);
    }

    #[test]
    fn every_byte_value_gives_one_printable_ascii_line() {
        let all: Vec<u8> = (0..=u8::MAX).collect();
        let shown = Escaped(&all).to_string();
        assert!(shown.bytes().all(|b| (b' '..=b'~').contains(&b)));
    }
}
