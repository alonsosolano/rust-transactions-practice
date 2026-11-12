---
status: accepted
date: 2026-11-11
decision-makers: Alonso Solano
---

# Read the input with a hand-written strict line reader and use the `csv` crate only for output

## Context and Problem Statement

Three accepted decisions describe how the input is read, and they do not agree.

* [ADR-0010](0010-input-validation-and-errors.md) requires strict quoting (V14), a record size limit enforced before the record is buffered (V9), and a header with the four names in a fixed order (V8). It notes that the `csv` crate alone does not satisfy V14 and leaves the choice between "our own reader" and "a strict check in front of a CSV crate" to D7.
* [ADR-0011](0011-architecture-and-memory.md) requires one reused line buffer that refuses a long line before buffering it (S3) and raw byte fields validated by our own code, with no serde or standard-library parser deciding the grammar (S4).
* [ADR-0012](0012-dependencies-and-safety-policy.md) (D7) lists the `csv` crate's purpose as reading the input with `Trim::All` and `flexible`, and serde's purpose as deserializing each record into a row struct by header name. It does not mention V9, V14, or S4.

The `csv` reader cannot meet ADR-0010 and ADR-0011. Run against `csv` 1.4.0 with `Trim::All` and `flexible`:

| Input line | `csv` result | Required result |
|------------|--------------|-----------------|
| `deposit,1,1,"1"0` | amount `10` | invalid row, `malformed_quote` |
| `deposit,1,1,""5""` | amount `5""` | invalid row, `malformed_quote` |
| `deposit,1,1,"5` (last line) | amount `5` | fatal, exit `65` |
| a 1 MiB field | read without error, 1 MiB buffered | fatal before buffering more than 4 KiB |

Matching columns by header name would also accept a header in another order, which V8 makes fatal.

How is the input read so that ADR-0010 and ADR-0011 hold, and what are the `csv` crate and serde used for?

A second, smaller question came up while building the account: ADR-0011 S8 derives `total` on every read with checked arithmetic, which returns an `Option`. The output then needs an error path for a value that can never overflow, and ADR-0010 has no exit code for it.

## Decision Drivers

* No malformed quote may ever be read as a value ([ADR-0010](0010-input-validation-and-errors.md) V14), and no input may grow a buffer past a fixed bound (V9, [ADR-0011](0011-architecture-and-memory.md) S3).
* One function owns the row grammar, as `Amount::parse` owns the amount grammar ([ADR-0007](0007-money-representation.md) M3, ADR-0011 S4).
* No allocation per row, and no panic on any input ([ADR-0012](0012-dependencies-and-safety-policy.md) S6, S12).
* Every dependency has a recorded purpose that matches how it is used (requirements §9, ADR-0012 S1).
* Clean, readable code over clever code (requirements §8).

## Considered Options

1. **Hand-written line reader and field splitter (chosen).** Read lines with `BufRead::fill_buf` into one bounded buffer, split fields with a small state machine that implements V14, and validate the raw bytes.
2. **Strict pre-check in front of the `csv` reader.** Our line reader and quote check as in option 1, then hand each accepted line to a `csv::Reader` to split it.
3. **The `csv` reader with serde, as ADR-0012 describes.**

## Decision Outcome

Chosen option: "Hand-written line reader and field splitter", because it is the only option that meets V9, V14, and S4 without parsing every line twice, and the code it needs is small: no valid field contains a quote, comma, or line break, so the splitter only has to recognize the simple quoted form of V14.

The `csv` crate and serde stay in the dependency list with a narrower purpose: they write the output CSV. The dependency list of ADR-0012 S1 does not change.

### Rules

| # | Rule | One-line justification |
|---|------|------------------------|
| P1 | The `input` adapter reads lines itself from a `BufRead` with `fill_buf` and `consume`, into one reused buffer. A line ends at `\n`; one `\r` before it is removed. The `csv` crate is not used for reading. | `BufRead::read_until` has no length limit, so the limit must sit in our loop. A line is the record of V14. |
| P2 | A line whose content (without its `\n`, and without a `\r` before it) is longer than `MAX_RECORD_BYTES` (4096) is fatal. The reader stops copying once the buffer holds `MAX_RECORD_BYTES + 1` bytes, so input memory never exceeds that, whatever the line length. | ADR-0010 V9 and ADR-0011 S3. The extra byte leaves room for the `\r` of a `\r\n` line of exactly 4096 bytes. |
| P3 | A UTF-8 byte order mark is removed only at the very start of the file. Empty lines are skipped everywhere, including before the header. The first non-empty line is the header. | ADR-0010 V8 and its parsing details. The `csv-core` parser also skips empty lines before the header, so this matches what a CSV tool would do. |
| P4 | Fields are split by one state machine that implements V14 and nothing more. At the start of a field, surrounding whitespace is skipped. A field that starts with `"` runs to the next `"`, and only whitespace may follow before the next `,` or the end of the line. A field that does not start with `"` runs to the next `,` and must not contain `"`. The whole line is scanned before any field is checked. | V14 and ADR-0010's rule that quote checks run before field checks. Scanning the whole line means a line that ends inside an open quote is always fatal, even if an earlier field is already malformed. |
| P5 | Fields stay byte slices into the line buffer; at most four are kept, and the rest are only counted. The checks then run in the order of ADR-0010 V1: quoting, field count (V2), `type` (V3), `client` and `tx` (V4), and the amount (V5, ADR-0007). `client` and `tx` use a digits-only parser with checked arithmetic. | No allocation per row, and one function owns what a valid row is (ADR-0011 S4). Rust's `u32::from_str` accepts `+5`, which V4 forbids. |
| P6 | The header line goes through the same splitter. It is valid only if it has exactly the four fields `type`, `client`, `tx`, `amount`, in that order, after trimming. Anything else, including a malformed or unterminated quote, is a wrong header (fatal, exit `65`). | ADR-0010 V8. One splitter means the header and the rows follow the same quoting rules. |
| P7 | A read error of kind `Interrupted` is retried; any other read error is fatal (exit `74`) and names the line being read. | `Interrupted` is not a failure under the `std::io` contract, and `BufReader` does not retry it on `fill_buf`. |
| P8 | The `output` adapter writes with `csv::Writer`. The header is written once with `write_record`, so it appears even with no accounts (ADR-0009 O1). Each row is a struct with `#[derive(serde::Serialize)]`, and the amounts are serialized through their `Display` form, so the domain types do not depend on serde. | The `csv` writer ends every record with `\n` and quotes only when a field needs it, which never happens here (ADR-0009 O9). The derive keeps the column order in one place. |
| P9 | This refines ADR-0011 S8: an `Account` keeps `total` next to `available` and `held`. All three are set only by one private constructor that computes `total` as `available + held` with checked arithmetic and refuses the change if any value overflows. `total()` then cannot fail. | I1 still holds by construction, because nothing else ever sets `total`. The output needs no error path for a value that cannot overflow. The cost is 16 bytes per account: 64 instead of 48, at most 4 MiB for all 65,536 accounts. |

### Consequences

* Good, because no malformed quote is ever read as a value, and memory per line is bounded by a constant (A33, A36, A37 in [requirements §6](../requirements.md)).
* Good, because one module owns the whole row grammar, and no default parser can widen it.
* Good, because rows allocate nothing: fields are slices into one buffer.
* Good, because the dependency list is unchanged, and each crate still has a real use.
* Neutral, because the `csv` crate now does very little: our output never needs quoting. Writing the rows by hand would be just as short, but would need a new ADR to remove two dependencies, and the crate is already vetted.
* Bad, because the reader and splitter are code we maintain. Accepted: they are short, and their unit and property tests cover every condition of ADR-0010.
* Bad, because ADR-0012's description of the `csv` crate and serde (reading with `Trim::All`, `flexible`, and decoding by header name) no longer matches the code. ADR-0012 links here for that part.

### Confirmation

* Reader unit tests cover every row-level reason of ADR-0010's condition table, the V14 cases listed in ADR-0010's Confirmation, a line of exactly 4096 bytes with `\n` and with `\r\n` (accepted), a line of 4097 bytes (fatal), the byte order mark, empty lines before and after the header, and a last line without `\n`.
* A property test feeds arbitrary bytes to the reader and checks that it never panics and never holds more than `MAX_RECORD_BYTES + 1` bytes in its line buffer.
* Review check: `input` does not use the `csv` crate, serde, or any `FromStr` implementation on input fields.

## Pros and Cons of the Options

### Hand-written line reader and field splitter

* Good, because it meets V9, V14, and S4 directly and reads each byte once.
* Good, because it allocates nothing per row.
* Bad, because the splitter is our own code, which ADR-0012 counted against a hand-written parser. That concern was about full RFC 4180 parsing; the V14 subset needs no escaped quotes and no multi-line fields.

### Strict pre-check in front of the `csv` reader

* Good, because the `csv` crate still splits the fields.
* Bad, because the pre-check must already find every field boundary to check the quotes, so the `csv` reader would repeat work that is done.
* Bad, because the line must be fed to a second reader, which adds a buffer copy per row and a second place that decides what a field is.

### The `csv` reader with serde

* Good, because it is the shortest code and the style the `csv` documentation teaches.
* Bad, because the `csv-core` parser accepts malformed quoting without an error by design, so `"1"0` becomes `10` (checked above), which V14 forbids.
* Bad, because the reader buffers a record of any length (checked above with a 1 MiB field), which V9 forbids.
* Bad, because matching columns by header name accepts a header in another order, which V8 forbids, and serde's integer parsing accepts `+5`, which V4 forbids.

## More Information

* Checked on 2026-11-11 with a scratch crate on `csv` 1.4.0, `ReaderBuilder` with `has_headers(false)`, `flexible(true)`, and `Trim::All`. Results are in the table above. The same crate, with headers on, skips empty lines before the header and removes a byte order mark only at the very start of the input, as P3 does.
* [`csv-core` `Reader`](https://docs.rs/csv-core/latest/csv_core/struct.Reader.html#rfc-4180): the parser never returns an error and prefers some parse over none.
* Go's [`encoding/csv`](https://pkg.go.dev/encoding/csv) reports `ErrBareQuote` and `ErrQuote` unless `LazyQuotes` is set; P4 follows the same strict reading.
* [RFC 4180](https://www.rfc-editor.org/rfc/rfc4180), section 2: the quoted-field grammar that V14 restricts.
* Rust [`BufRead::fill_buf`](https://doc.rust-lang.org/std/io/trait.BufRead.html#tymethod.fill_buf) and [`read_until`](https://doc.rust-lang.org/std/io/trait.BufRead.html#method.read_until) (no length limit), and [`ErrorKind::Interrupted`](https://doc.rust-lang.org/std/io/enum.ErrorKind.html#variant.Interrupted).
* Resolves the reading choice that [ADR-0010](0010-input-validation-and-errors.md) left to D7 and that [ADR-0012](0012-dependencies-and-safety-policy.md) did not address. Refines [ADR-0011](0011-architecture-and-memory.md) S8 (P9) and its account size of 48 bytes.
* Tracked by issue LYA-51 (E3).
