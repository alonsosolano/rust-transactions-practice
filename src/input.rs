//! Line reading, the header check, strict quoting, and the row-to-`Transaction` constructor (ADR-0010, ADR-0011 S3, S4, ADR-0015).
//!
//! Adapter module of the layout in ADR-0011. It reads lines itself, into one
//! reused buffer of bounded size, and splits fields with the strict quoting
//! of ADR-0010 V14. The `csv` crate and serde are not used here (ADR-0015).

use std::io::{BufRead, BufReader, ErrorKind, Read};

use crate::RunError;
use crate::amount::{Amount, AmountError};
use crate::transaction::{ClientId, Transaction, TxId};

/// Longest line content accepted, without its line ending (ADR-0010 V9).
pub const MAX_RECORD_BYTES: usize = 4096;

/// Bytes the line buffer may hold: the content limit plus room for the `\r`
/// of a `\r\n` line (ADR-0015 P2).
const LINE_BUFFER_LIMIT: usize = MAX_RECORD_BYTES + 1;

/// The UTF-8 byte order mark, accepted only at the start of the file.
const BOM: &[u8] = b"\xEF\xBB\xBF";

/// The only valid header (ADR-0010 V8).
const HEADER: [&[u8]; 4] = [b"type", b"client", b"tx", b"amount"];

/// Why a data row is invalid (ADR-0010 V1 to V5, V14).
///
/// An invalid row is ignored: it never reaches the engine, never changes
/// state, and never binds an ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvalidRow {
    /// A bare quote, text after a closing quote, or a doubled quote
    /// (`malformed_quote`, V14).
    MalformedQuote,
    /// Not 3 or 4 fields (`field_count`, V2).
    FieldCount,
    /// Not one of the five types, matched exactly (`unknown_type`, V3).
    UnknownType,
    /// `client` is not digits within `u16` (`invalid_client`, V4).
    InvalidClient,
    /// `tx` is not digits within `u32` (`invalid_tx`, V4).
    InvalidTx,
    /// A `deposit` or `withdrawal` without an amount (`missing_amount`, V5).
    MissingAmount,
    /// A `deposit` or `withdrawal` with an invalid amount (`invalid_amount`,
    /// ADR-0007 M3 to M5).
    InvalidAmount(AmountError),
}

/// One data row: its line number and either a transaction or why it is
/// invalid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Row {
    /// The 1-based physical line number in the file.
    pub line: u64,
    /// The parsed row.
    pub outcome: Result<Transaction, InvalidRow>,
}

/// Streams the data rows of one input, one line at a time (ADR-0011 S3).
///
/// Memory is one line buffer of at most `MAX_RECORD_BYTES + 1` bytes plus the
/// fixed buffer of the `BufReader`, whatever the input.
pub struct Reader<R> {
    source: BufReader<R>,
    /// The current line, without its line ending.
    line: Vec<u8>,
    /// The number of the current line; 0 before the first.
    line_number: u64,
}

impl<R: Read> Reader<R> {
    /// Opens a reader and checks the header (ADR-0010 V8, ADR-0015 P3, P6).
    ///
    /// # Errors
    ///
    /// A fatal [`RunError`]: no header, a wrong header, a line over the size
    /// limit, or a read error.
    pub fn new(input: R) -> Result<Self, RunError> {
        let mut reader = Self {
            source: BufReader::new(input),
            line: Vec::with_capacity(LINE_BUFFER_LIMIT),
            line_number: 0,
        };
        loop {
            if !reader.read_line()? {
                return Err(RunError::MissingHeader);
            }
            if !reader.content().is_empty() {
                break;
            }
        }
        let content = reader.content();
        let is_header = match split(content) {
            Ok(fields) => !fields.malformed && fields.count == 4 && fields.first == HEADER,
            Err(Unterminated) => false,
        };
        if is_header {
            Ok(reader)
        } else {
            Err(RunError::WrongHeader {
                found: content.to_vec(),
            })
        }
    }

    /// Reads the next data row, skipping empty lines.
    ///
    /// Returns `Ok(None)` at the end of the input.
    ///
    /// # Errors
    ///
    /// A fatal [`RunError`]: a line over the size limit, a line that ends
    /// inside an open quoted field, or a read error.
    pub fn next_row(&mut self) -> Result<Option<Row>, RunError> {
        loop {
            if !self.read_line()? {
                return Ok(None);
            }
            let content = self.content();
            if content.is_empty() {
                continue;
            }
            let fields = split(content).map_err(|Unterminated| RunError::UnterminatedQuote {
                line: self.line_number,
            })?;
            return Ok(Some(Row {
                line: self.line_number,
                outcome: parse_row(&fields),
            }));
        }
    }

    /// The current line, without a byte order mark on the first line.
    fn content(&self) -> &[u8] {
        if self.line_number == 1 {
            self.line.strip_prefix(BOM).unwrap_or(&self.line)
        } else {
            &self.line
        }
    }

    /// Reads the next line into the buffer, without its `\n` or `\r\n`.
    ///
    /// Returns `false` at the end of the input. Never holds more than
    /// `LINE_BUFFER_LIMIT` bytes (ADR-0015 P1, P2).
    fn read_line(&mut self) -> Result<bool, RunError> {
        self.line.clear();
        // A diagnostic number only; 2^64 lines cannot be read.
        let number = self.line_number.saturating_add(1);
        let mut read_any = false;
        loop {
            let buffered = match self.source.fill_buf() {
                Ok(buffered) => buffered,
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                Err(source) => {
                    return Err(RunError::ReadInput {
                        line: number,
                        source,
                    });
                }
            };
            // The first piece is the rest of the line, including its `\n` if
            // the buffer holds it.
            let Some(piece) = buffered.split_inclusive(|&byte| byte == b'\n').next() else {
                break;
            };
            read_any = true;
            let consumed = piece.len();
            let (chunk, ends_line) = match piece.strip_suffix(b"\n") {
                Some(chunk) => (chunk, true),
                None => (piece, false),
            };
            // The buffer never exceeds the limit, so this cannot saturate.
            let room = LINE_BUFFER_LIMIT.saturating_sub(self.line.len());
            if chunk.len() > room {
                return Err(RunError::LineTooLong { line: number });
            }
            self.line.extend_from_slice(chunk);
            self.source.consume(consumed);
            if ends_line {
                break;
            }
        }
        if !read_any {
            return Ok(false);
        }
        self.line_number = number;
        if self.line.last() == Some(&b'\r') {
            self.line.pop();
        }
        if self.line.len() > MAX_RECORD_BYTES {
            return Err(RunError::LineTooLong { line: number });
        }
        Ok(true)
    }

    /// The capacity of the line buffer, for the memory bound test.
    #[cfg(test)]
    fn line_capacity(&self) -> usize {
        self.line.capacity()
    }
}

/// A line ends inside an open quoted field (ADR-0010 V14, fatal).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Unterminated;

/// The fields of one line (ADR-0015 P4, P5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Fields<'a> {
    /// The first four fields, trimmed and unquoted. Unused slots are empty.
    first: [&'a [u8]; 4],
    /// The number of fields in the line.
    count: usize,
    /// Whether any field breaks the quoting rules.
    malformed: bool,
}

/// Splits one line into fields under ADR-0010 V14.
///
/// A field is surrounding whitespace around either unquoted text without
/// `"`, or `"`, text without `"`, and a closing `"`. Commas inside a quoted
/// field do not split it. Any other quote marks the line as malformed, and
/// the scan continues at the next comma, so a later field that opens a quote
/// and never closes it is still found.
///
/// # Errors
///
/// [`Unterminated`] if a quoted field is still open at the end of the line.
fn split(line: &[u8]) -> Result<Fields<'_>, Unterminated> {
    let mut fields = Fields {
        first: [b""; 4],
        count: 0,
        malformed: false,
    };
    let mut rest = Some(line);
    while let Some(remaining) = rest {
        let start = remaining.trim_ascii_start();
        let (field, after) = match start.strip_prefix(b"\"") {
            Some(quoted) => {
                let (content, closed) = quoted.split_once_byte(b'"').ok_or(Unterminated)?;
                let tail = closed.trim_ascii_start();
                match tail.split_first() {
                    None => (content, None),
                    Some((b',', after)) => (content, Some(after)),
                    Some(_) => {
                        fields.malformed = true;
                        let (_, after) = tail.split_once_byte(b',').unzip();
                        (content, after)
                    }
                }
            }
            None => {
                let (field, after) = match start.split_once_byte(b',') {
                    Some((field, after)) => (field, Some(after)),
                    None => (start, None),
                };
                if field.contains(&b'"') {
                    fields.malformed = true;
                }
                (field, after)
            }
        };
        if let Some(slot) = fields.first.get_mut(fields.count) {
            *slot = field.trim_ascii();
        }
        // Bounded by the line length, so this cannot saturate.
        fields.count = fields.count.saturating_add(1);
        rest = after;
    }
    Ok(fields)
}

/// Splitting a byte slice at the first occurrence of one byte.
trait SplitOnceByte {
    /// The bytes before and after the first `byte`, or `None` if it is absent.
    fn split_once_byte(&self, byte: u8) -> Option<(&[u8], &[u8])>;
}

impl SplitOnceByte for [u8] {
    fn split_once_byte(&self, byte: u8) -> Option<(&[u8], &[u8])> {
        let mut parts = self.splitn(2, |&b| b == byte);
        let before = parts.next()?;
        let after = parts.next()?;
        Some((before, after))
    }
}

/// The row types of ADR-0010 V3.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Deposit,
    Withdrawal,
    Dispute,
    Resolve,
    Chargeback,
}

/// Turns the fields of one line into a transaction (ADR-0011 S4).
///
/// This is the one function that decides what a valid row is. Checks run in
/// the order of ADR-0010 V1, and the first failing check is the reason.
fn parse_row(fields: &Fields<'_>) -> Result<Transaction, InvalidRow> {
    if fields.malformed {
        return Err(InvalidRow::MalformedQuote);
    }
    if !matches!(fields.count, 3 | 4) {
        return Err(InvalidRow::FieldCount);
    }
    let [kind, client, tx, amount] = fields.first;
    let kind = match kind {
        b"deposit" => Kind::Deposit,
        b"withdrawal" => Kind::Withdrawal,
        b"dispute" => Kind::Dispute,
        b"resolve" => Kind::Resolve,
        b"chargeback" => Kind::Chargeback,
        _ => return Err(InvalidRow::UnknownType),
    };
    let client = parse_digits(client)
        .and_then(|value| u16::try_from(value).ok())
        .map(ClientId)
        .ok_or(InvalidRow::InvalidClient)?;
    let tx = parse_digits(tx).map(TxId).ok_or(InvalidRow::InvalidTx)?;
    // A 3-field row leaves the amount slot empty (V2, V5).
    let amount = || {
        if amount.is_empty() {
            return Err(InvalidRow::MissingAmount);
        }
        Amount::parse(amount).map_err(InvalidRow::InvalidAmount)
    };
    Ok(match kind {
        Kind::Deposit => Transaction::Deposit {
            client,
            tx,
            amount: amount()?,
        },
        Kind::Withdrawal => Transaction::Withdrawal {
            client,
            tx,
            amount: amount()?,
        },
        // The amount field of these rows is ignored: not parsed, not
        // validated, not reported (V5).
        Kind::Dispute => Transaction::Dispute { client, tx },
        Kind::Resolve => Transaction::Resolve { client, tx },
        Kind::Chargeback => Transaction::Chargeback { client, tx },
    })
}

/// Parses one or more ASCII digits into a `u32` with checked arithmetic.
///
/// Leading zeros are accepted; a sign, an empty field, or any other byte is
/// not (ADR-0010 V4). `u32::from_str` is not used because it accepts `+5`.
fn parse_digits(field: &[u8]) -> Option<u32> {
    if field.is_empty() {
        return None;
    }
    field.iter().try_fold(0_u32, |value, &byte| {
        let digit = char::from(byte).to_digit(10)?;
        value.checked_mul(10)?.checked_add(digit)
    })
}

#[cfg(test)]
mod tests {
    use std::io::{self, Read};

    use proptest::prelude::*;

    use super::{InvalidRow, MAX_RECORD_BYTES, Reader, Row, Unterminated, parse_digits, split};
    use crate::RunError;
    use crate::amount::{Amount, AmountError};
    use crate::transaction::{ClientId, Transaction, TxId};

    const HEADER: &str = "type,client,tx,amount\n";

    /// Reads every row of `input`, or the first fatal error.
    fn read_all(input: &[u8]) -> Result<Vec<Row>, RunError> {
        let mut reader = Reader::new(input)?;
        let mut rows = Vec::new();
        while let Some(row) = reader.next_row()? {
            rows.push(row);
        }
        Ok(rows)
    }

    /// The outcome of the single data row `line` under a valid header.
    fn parse_line(line: &str) -> Result<Transaction, InvalidRow> {
        let rows = read_all(format!("{HEADER}{line}\n").as_bytes()).unwrap();
        assert_eq!(rows.len(), 1, "expected one row from {line:?}");
        rows[0].outcome
    }

    fn deposit(client: u16, tx: u32, amount: &str) -> Transaction {
        Transaction::Deposit {
            client: ClientId(client),
            tx: TxId(tx),
            amount: Amount::parse(amount.as_bytes()).unwrap(),
        }
    }

    // ---- Valid rows (requirements §2) ----

    #[test]
    fn every_type_is_parsed() {
        assert_eq!(parse_line("deposit,1,2,3.5"), Ok(deposit(1, 2, "3.5")));
        assert_eq!(
            parse_line("withdrawal,1,2,3"),
            Ok(Transaction::Withdrawal {
                client: ClientId(1),
                tx: TxId(2),
                amount: Amount::parse(b"3").unwrap(),
            })
        );
        let (client, tx) = (ClientId(4), TxId(5));
        assert_eq!(
            parse_line("dispute,4,5,"),
            Ok(Transaction::Dispute { client, tx })
        );
        assert_eq!(
            parse_line("resolve,4,5"),
            Ok(Transaction::Resolve { client, tx })
        );
        assert_eq!(
            parse_line("chargeback,4,5,"),
            Ok(Transaction::Chargeback { client, tx })
        );
    }

    #[test]
    fn whitespace_around_fields_is_trimmed() {
        assert_eq!(
            parse_line("  deposit , 1,\t2 ,  3.5  "),
            Ok(deposit(1, 2, "3.5"))
        );
    }

    /// A35, V5: the amount of a dispute-related row is ignored, even when it
    /// is not a valid amount.
    #[test]
    fn a35_the_amount_of_a_dispute_related_row_is_ignored() {
        let (client, tx) = (ClientId(1), TxId(1));
        for line in [
            "dispute,1,1,3",
            "dispute,1,1,abc",
            "dispute,1,1,-50",
            "dispute,1,1,\"x\"",
        ] {
            assert_eq!(
                parse_line(line),
                Ok(Transaction::Dispute { client, tx }),
                "{line}"
            );
        }
    }

    /// A44: boundary IDs and leading zeros are accepted.
    #[test]
    fn a44_boundary_ids_and_leading_zeros_are_accepted() {
        assert_eq!(
            parse_line("deposit,65535,4294967295,1"),
            Ok(deposit(65535, 4_294_967_295, "1"))
        );
        assert_eq!(parse_line("deposit,0,0,1"), Ok(deposit(0, 0, "1")));
        assert_eq!(parse_line("deposit,007,0008,1"), Ok(deposit(7, 8, "1")));
    }

    #[test]
    fn simple_quoting_is_accepted() {
        assert_eq!(
            parse_line("\"deposit\", 1 ,6,\" 2.5 \""),
            Ok(deposit(1, 6, "2.5"))
        );
        assert_eq!(
            parse_line("\"deposit\" ,\"1\",\"2\",\"1.2345\""),
            Ok(deposit(1, 2, "1.2345"))
        );
    }

    // ---- Invalid rows (ADR-0010 V1 to V5, V14) ----

    /// A31: each invalid row reports its own reason.
    #[test]
    fn a31_each_invalid_row_has_its_reason() {
        assert_eq!(parse_line("Deposit,1,2,5"), Err(InvalidRow::UnknownType));
        assert_eq!(parse_line("deposit,+1,3,5"), Err(InvalidRow::InvalidClient));
        assert_eq!(
            parse_line("deposit,65536,4,5"),
            Err(InvalidRow::InvalidClient)
        );
        assert_eq!(
            parse_line("deposit,1,4294967296,5"),
            Err(InvalidRow::InvalidTx)
        );
        assert_eq!(parse_line("deposit,1,5"), Err(InvalidRow::MissingAmount));
        assert_eq!(parse_line("deposit,1,6,"), Err(InvalidRow::MissingAmount));
        assert_eq!(parse_line("deposit,1,7,5,"), Err(InvalidRow::FieldCount));
    }

    #[test]
    fn field_count_other_than_3_or_4_is_invalid() {
        assert_eq!(parse_line("deposit,1"), Err(InvalidRow::FieldCount));
        assert_eq!(parse_line("deposit"), Err(InvalidRow::FieldCount));
        assert_eq!(parse_line("   "), Err(InvalidRow::FieldCount));
        assert_eq!(parse_line("deposit,1,2,3,4,5"), Err(InvalidRow::FieldCount));
        assert_eq!(parse_line("dispute,1,2,,"), Err(InvalidRow::FieldCount));
    }

    #[test]
    fn types_are_matched_exactly() {
        for line in [
            "DEPOSIT,1,1,1",
            "deposits,1,1,1",
            "dep,1,1,1",
            ",1,1,1",
            "dis pute,1,1",
        ] {
            assert_eq!(parse_line(line), Err(InvalidRow::UnknownType), "{line}");
        }
    }

    #[test]
    fn ids_are_digits_only() {
        for client in ["", "-1", "1.0", "1 2", "0x1", "\u{0661}"] {
            let line = format!("deposit,{client},1,1");
            assert_eq!(parse_line(&line), Err(InvalidRow::InvalidClient), "{line}");
        }
        for tx in ["", "+1", "-1", "1e3", "1_000"] {
            let line = format!("deposit,1,{tx},1");
            assert_eq!(parse_line(&line), Err(InvalidRow::InvalidTx), "{line}");
        }
    }

    #[test]
    fn invalid_amounts_keep_their_cause() {
        assert_eq!(
            parse_line("deposit,1,1,-5"),
            Err(InvalidRow::InvalidAmount(AmountError::Syntax))
        );
        assert_eq!(
            parse_line("deposit,1,1,0"),
            Err(InvalidRow::InvalidAmount(AmountError::Zero))
        );
        assert_eq!(
            parse_line("withdrawal,1,1,1.00005"),
            Err(InvalidRow::InvalidAmount(AmountError::TooPrecise))
        );
        assert_eq!(
            parse_line("deposit,1,1,\"1,000\""),
            Err(InvalidRow::InvalidAmount(AmountError::Syntax))
        );
        // A quoted field that holds only whitespace is empty after trimming.
        assert_eq!(
            parse_line("deposit,1,1,\"  \""),
            Err(InvalidRow::MissingAmount)
        );
        // `""` is an empty quoted field, not a doubled quote inside a field.
        assert_eq!(
            parse_line("deposit,1,1,\"\""),
            Err(InvalidRow::MissingAmount)
        );
    }

    /// The checks run in column order: the first failing column is the reason.
    #[test]
    fn the_first_failing_column_is_the_reason() {
        assert_eq!(parse_line("bogus,x,y,z"), Err(InvalidRow::UnknownType));
        assert_eq!(parse_line("deposit,x,y,z"), Err(InvalidRow::InvalidClient));
        assert_eq!(parse_line("deposit,1,y,z"), Err(InvalidRow::InvalidTx));
    }

    /// A36, V14: malformed quotes make the row invalid before any other check.
    #[test]
    fn a36_malformed_quotes_are_invalid() {
        for line in [
            "deposit,1,2,\"1\"0",
            "deposit,1,3,1\"0",
            "deposit,1,4,\"\"5\"\"",
            "\"deposit\"x,1,1,1",
            "deposit,1,1,1\"",
            // A quote check fails before the field count check.
            "deposit,1\",1,1,1,1",
        ] {
            assert_eq!(parse_line(line), Err(InvalidRow::MalformedQuote), "{line}");
        }
    }

    /// A45: a repeated header is a data row with an unknown type.
    #[test]
    fn a45_a_repeated_header_is_an_unknown_type() {
        assert_eq!(
            parse_line("type,client,tx,amount"),
            Err(InvalidRow::UnknownType)
        );
    }

    #[test]
    fn invalid_utf8_fails_only_its_field() {
        let mut input = HEADER.as_bytes().to_vec();
        input.extend_from_slice(b"dep\xFFosit,1,1,1\ndeposit,1,2,1\n");
        let rows = read_all(&input).unwrap();
        assert_eq!(rows[0].outcome, Err(InvalidRow::UnknownType));
        assert_eq!(rows[1].outcome, Ok(deposit(1, 2, "1")));
    }

    // ---- Lines, header, and fatal errors ----

    /// A43: BOM, padded header, `\r\n`, an empty line, and quoted fields.
    #[test]
    fn a43_every_accepted_form() {
        let input = b"\xEF\xBB\xBF type , client , tx , amount \r\n\r\ndeposit, 1, 1, 1\r\ndeposit,1,2,1.0\r\n\"deposit\",1,3,\"1.2345\"\r\n";
        let rows = read_all(input).unwrap();
        let outcomes: Vec<_> = rows.iter().map(|row| row.outcome).collect();
        assert_eq!(
            outcomes,
            [
                Ok(deposit(1, 1, "1")),
                Ok(deposit(1, 2, "1.0")),
                Ok(deposit(1, 3, "1.2345"))
            ]
        );
        let lines: Vec<u64> = rows.iter().map(|row| row.line).collect();
        assert_eq!(lines, [3, 4, 5]);
    }

    #[test]
    fn empty_lines_are_skipped_everywhere() {
        let rows = read_all(b"\n\n\ntype,client,tx,amount\n\n\ndeposit,1,1,1\n\n").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].line, 7);
    }

    #[test]
    fn the_last_line_needs_no_line_ending() {
        let rows = read_all(b"type,client,tx,amount\ndeposit,1,1,1").unwrap();
        assert_eq!(rows[0].outcome, Ok(deposit(1, 1, "1")));
    }

    #[test]
    fn a_header_alone_has_no_rows() {
        assert!(read_all(b"type,client,tx,amount").unwrap().is_empty());
        assert!(
            read_all(b"\"type\",\"client\",\"tx\",\"amount\"\r\n")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn a_byte_order_mark_is_only_accepted_at_the_start() {
        let input = b"type,client,tx,amount\n\xEF\xBB\xBFdeposit,1,1,1\n";
        assert_eq!(
            read_all(input).unwrap()[0].outcome,
            Err(InvalidRow::UnknownType)
        );
        let input = b"\n\xEF\xBB\xBFtype,client,tx,amount\n";
        assert!(matches!(read_all(input), Err(RunError::WrongHeader { .. })));
    }

    /// A33 (a): an empty file, or one with only empty lines, has no header.
    #[test]
    fn a33a_no_header_is_fatal() {
        for input in [&b""[..], b"\n", b"\r\n\r\n", b"\xEF\xBB\xBF"] {
            assert!(
                matches!(read_all(input), Err(RunError::MissingHeader)),
                "{input:?}"
            );
        }
    }

    /// A33 (b), A42: any header other than the four names in order is fatal.
    #[test]
    fn a42_a_wrong_header_is_fatal() {
        for header in [
            "type,client,amount,tx",
            "type,client,tx,amount,note",
            "type,client,tx",
            "Type,Client,Tx,Amount",
            "type,client,tx,\"amount",
            "type,client,tx,amount\"",
            " ",
        ] {
            let input = format!("{header}\ndeposit,1,1,10\n");
            match read_all(input.as_bytes()) {
                Err(RunError::WrongHeader { found }) => assert_eq!(found, header.as_bytes()),
                other => panic!("{header:?}: expected a wrong header, got {other:?}"),
            }
        }
    }

    /// A37: a line that ends inside an open quote is fatal, on any line.
    #[test]
    fn a37_an_unterminated_quote_is_fatal() {
        let input = b"type,client,tx,amount\ndeposit,1,1,10\ndeposit,1,2,\"5";
        assert!(matches!(
            read_all(input),
            Err(RunError::UnterminatedQuote { line: 3 })
        ));
        let input = b"type,client,tx,amount\ndeposit,1,1,10\ndeposit,1,2,\"5\ndeposit,1,3,7\n8\"\n";
        assert!(matches!(
            read_all(input),
            Err(RunError::UnterminatedQuote { line: 3 })
        ));
        // Even after an earlier malformed field on the same line.
        let input = b"type,client,tx,amount\ndeposit,1\"0,2,\"5\n";
        assert!(matches!(
            read_all(input),
            Err(RunError::UnterminatedQuote { line: 2 })
        ));
    }

    /// A33 (c), V9: the line limit, counted without the line ending.
    #[test]
    fn a33c_a_line_over_the_limit_is_fatal() {
        let row = |len: usize| {
            let mut line = b"deposit,1,1,1".to_vec();
            line.resize(len, b' ');
            line
        };
        for ending in [&b"\n"[..], b"\r\n", b""] {
            let mut input = HEADER.as_bytes().to_vec();
            input.extend(row(MAX_RECORD_BYTES));
            input.extend_from_slice(ending);
            assert_eq!(read_all(&input).unwrap()[0].outcome, Ok(deposit(1, 1, "1")));

            let mut input = HEADER.as_bytes().to_vec();
            input.extend(row(MAX_RECORD_BYTES + 1));
            input.extend_from_slice(ending);
            assert!(matches!(
                read_all(&input),
                Err(RunError::LineTooLong { line: 2 })
            ));
        }
    }

    #[test]
    fn a_long_header_line_is_fatal() {
        let input = vec![b'x'; 5 * 1024];
        assert!(matches!(
            read_all(&input),
            Err(RunError::LineTooLong { line: 1 })
        ));
    }

    /// A reader that returns `Interrupted` before every read.
    struct Interrupting<'a> {
        data: &'a [u8],
        interrupt: bool,
    }

    impl Read for Interrupting<'_> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.interrupt = !self.interrupt;
            if self.interrupt {
                return Err(io::ErrorKind::Interrupted.into());
            }
            let n = buf.len().min(self.data.len()).min(3);
            buf[..n].copy_from_slice(&self.data[..n]);
            self.data = &self.data[n..];
            Ok(n)
        }
    }

    #[test]
    fn interrupted_reads_are_retried() {
        let data = b"type,client,tx,amount\ndeposit,1,1,1\n";
        let mut reader = Reader::new(Interrupting {
            data,
            interrupt: false,
        })
        .unwrap();
        assert_eq!(
            reader.next_row().unwrap().unwrap().outcome,
            Ok(deposit(1, 1, "1"))
        );
        assert_eq!(reader.next_row().unwrap(), None);
    }

    /// A reader that fails after its data.
    struct FailingAfter<'a>(&'a [u8]);

    impl Read for FailingAfter<'_> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.0.is_empty() {
                return Err(io::Error::other("disk on fire"));
            }
            let n = buf.len().min(self.0.len());
            buf[..n].copy_from_slice(&self.0[..n]);
            self.0 = &self.0[n..];
            Ok(n)
        }
    }

    #[test]
    fn a_read_error_is_fatal_with_its_line() {
        let mut reader =
            Reader::new(FailingAfter(b"type,client,tx,amount\ndeposit,1,1,1\n")).unwrap();
        assert!(reader.next_row().unwrap().is_some());
        assert!(matches!(
            reader.next_row(),
            Err(RunError::ReadInput { line: 3, .. })
        ));
    }

    // ---- Splitter and digits ----

    #[test]
    fn commas_inside_quotes_do_not_split() {
        let fields = split(b"a,\"b,c\",d").unwrap();
        assert_eq!(fields.count, 3);
        assert_eq!(fields.first, [&b"a"[..], b"b,c", b"d", b""]);
        assert!(!fields.malformed);
        assert_eq!(split(b"\"a,b"), Err(Unterminated));
    }

    #[test]
    fn digits_parse_with_checked_arithmetic() {
        assert_eq!(parse_digits(b"0"), Some(0));
        assert_eq!(parse_digits(b"4294967295"), Some(u32::MAX));
        assert_eq!(parse_digits(b"00000000000000000042"), Some(42));
        assert_eq!(parse_digits(b"4294967296"), None);
        assert_eq!(parse_digits(b""), None);
        assert_eq!(parse_digits(b"+1"), None);
    }

    /// One input byte, biased toward the bytes the reader treats specially.
    fn input_byte() -> impl Strategy<Value = u8> {
        prop_oneof![
            Just(b'\n'),
            Just(b'\r'),
            Just(b','),
            Just(b'"'),
            Just(b' '),
            Just(b'1'),
            any::<u8>()
        ]
    }

    proptest! {
        /// ADR-0012 S12, ADR-0015: arbitrary bytes never panic the reader, and
        /// the line buffer never grows past its limit.
        #[test]
        fn arbitrary_input_never_panics_and_stays_bounded(
            before in prop::collection::vec(input_byte(), 0..400),
            run_length in prop::option::of(4000_usize..4200),
            after in prop::collection::vec(input_byte(), 0..400),
        ) {
            // A long run of one byte, somewhere in the input, reaches the line
            // limit without generating thousands of bytes one by one.
            let mut input = HEADER.as_bytes().to_vec();
            input.extend(before);
            input.extend(std::iter::repeat_n(b'1', run_length.unwrap_or(0)));
            input.extend(after);
            let mut reader = Reader::new(&input[..]).unwrap();
            loop {
                prop_assert!(reader.line_capacity() <= MAX_RECORD_BYTES + 1);
                match reader.next_row() {
                    Ok(Some(_)) => {}
                    Ok(None) | Err(RunError::LineTooLong { .. } | RunError::UnterminatedQuote { .. }) => break,
                    Err(other) => prop_assert!(false, "unexpected error {other:?}"),
                }
            }
        }

        /// Quoting a valid field with optional padding never changes the row.
        #[test]
        fn quoting_and_padding_do_not_change_a_valid_row(
            client in any::<u16>(),
            tx in any::<u32>(),
            units in 1_u64..,
            pad in "[ \t]{0,3}",
            quote in any::<[bool; 4]>(),
        ) {
            let amount = format!("{}.{:04}", units / 10_000, units % 10_000);
            let values = ["deposit".to_string(), client.to_string(), tx.to_string(), amount.clone()];
            let fields: Vec<String> = values
                .iter()
                .zip(quote)
                .map(|(value, quoted)| {
                    if quoted { format!("{pad}\"{pad}{value}{pad}\"{pad}") } else { format!("{pad}{value}{pad}") }
                })
                .collect();
            let line = fields.join(",");
            prop_assert_eq!(parse_line(&line), Ok(deposit(client, tx, &amount)));
        }
    }
}
