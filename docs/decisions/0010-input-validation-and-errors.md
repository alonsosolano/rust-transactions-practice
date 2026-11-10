---
status: accepted
date: 2026-11-10
decision-makers: Alonso Solano
---

# Validate input strictly, skip bad rows, and fail closed on file-level errors

## Context and Problem Statement

The engine must never panic on bad input and must explain how it handles errors ([requirements §8](../requirements.md)). Stdout may contain only the output CSV, so every diagnostic goes to stderr ([requirements §1](../requirements.md)). [ADR-0006](0006-dispute-lifecycle.md) already decides which well-formed dispute rows are ignored and how diagnostics are bounded (R10). [ADR-0007](0007-money-representation.md) decides which amounts are valid and leaves open how an invalid row is reported (M6).

Everything else is open: malformed rows, unknown or miscased types, missing or unexpected amounts, duplicate `tx` IDs, problems with the arguments or the file itself, exit codes, and the error types in the code.

How is every kind of bad input or failure classified, how is each one reported, and which exit code does the run return?

## Decision Drivers

* No input may cause a panic or unbounded memory use ([requirements §8](../requirements.md)).
* The ledger must never be built from data the engine misread. When the engine cannot trust the structure of the file, it must stop rather than guess.
* One bad row must not deny service to every other client in the file. Rows that are wrong on their own are upstream errors, and the requirements already skip such rows without failing the run ([requirements §5](../requirements.md)).
* Every accepted row has exactly one meaning, as in [ADR-0007](0007-money-representation.md) M3. Loosening a rule later is a compatible change; tightening it is not.
* Every refused row is visible. Diagnostics stay bounded and cannot be used to inject fake log lines ([ADR-0006](0006-dispute-lifecycle.md) R10).
* The CLI contract is fixed: one positional argument, no required flags, CSV on stdout only ([requirements §1](../requirements.md)).
* Follow what payment-file processors, ledger databases, data loaders, and CLI conventions do (see [More Information](#more-information)).

## Considered Options

The choices below were decided one by one. Each lists the options considered and the chosen one.

1. **Malformed rows:** skip with a diagnostic, abort the run, or skip until an error budget is exhausted.
2. **Field strictness:** strict (exact lowercase `type`, digits only), or lenient (case-insensitive `type`, accept what Rust's integer parser accepts).
3. **Amount on `dispute`, `resolve`, or `chargeback`:** ignore the amount and process the row, or treat the row as invalid.
4. **Duplicate `tx` IDs:** the first deposit or withdrawal binds the ID even if rejected, only an applied row binds it, the last row wins, or abort the run.
5. **File-level errors:** fail closed with no output, or write the balances computed so far.
6. **Exit codes:** a subset of `sysexits.h`, `0`/`1`/`2`, or `0`/`1` only.
7. **Exit code when rows were skipped:** `0`, or a distinct non-zero code.
8. **Record size:** bounded with a fatal error, or unbounded.
9. **Error types:** hand-written enums, `thiserror` plus `anyhow`, or `Box<dyn Error>`.
10. **Malformed quoting:** strict detection on line-delimited records, the lenient parse of the `csv-core` parser, or no quoting at all.

## Decision Outcome

Chosen options: skip malformed rows with a diagnostic, strict fields, the amount on dispute-related rows is ignored, the first deposit or withdrawal binds a `tx` ID even when rejected, fail closed on file-level errors, a `sysexits.h` subset, exit `0` when the run completes, a bounded record size, typed error enums, and strict quoting on line-delimited records.

The line between the two halves is simple: **a problem confined to one row skips that row; a problem that makes the file or the run untrustworthy stops the run.**

### Outcome classes

Every row ends in exactly one class, and every run ends in success or one fatal error.

| Class | Meaning | State change | Account created | `tx` ID bound | Run continues |
|-------|---------|--------------|-----------------|---------------|---------------|
| **Applied** | The row is valid and takes effect. | Yes | Yes, for an unseen client on `deposit` or `withdrawal` | Yes, for `deposit` or `withdrawal` | Yes |
| **Rejected** | A valid `withdrawal` the engine declines: insufficient funds, or a locked account ([ADR-0006](0006-dispute-lifecycle.md) R9). | No balance change | Yes, zero balance, for an unseen client (R8) | Yes | Yes |
| **Ignored** | The row is invalid (fails validation, never reaches the engine) or is valid but not applicable (ADR-0006 R1 to R7, ADR-0007 M7, or a duplicate `tx`). | None | No | No | Yes |
| **Fatal** | The arguments, the file, or an output write cannot be trusted. | Not applicable | Not applicable | Not applicable | No, the run stops |

"Ignored" keeps its meaning from [ADR-0006](0006-dispute-lifecycle.md): no state change, no account, no failure of the run, and an entry in the bounded diagnostics. Invalid rows are ignored rows whose reason comes from validation.

### Rules

| # | Rule | One-line justification |
|---|------|------------------------|
| V1 | A problem confined to one data row makes that row **invalid**: it is ignored with a diagnostic and the run continues. The checks run in column order (`type`, `client`, `tx`, `amount`) and the first failing check is the reason reported. | Payment status reports reject single transactions inside an accepted batch, one reason per transaction. Aborting would let one bad row deny service to every client in the file. |
| V2 | A data row must have **3 or 4 fields**; any other count makes the row invalid (`field_count`). A row with 3 fields has no amount, which V5 allows only for `dispute`, `resolve`, and `chargeback`. | The schema has four columns; anything else is a row the engine cannot map with certainty. |
| V3 | `type` must equal one of `deposit`, `withdrawal`, `dispute`, `resolve`, `chargeback` **exactly**, after trimming whitespace. Matching is case-sensitive: `Deposit` is an unknown type. | The schema lists lowercase values. Codes in ISO 20022 and enum values in payment APIs are case-sensitive, and guessing an operation is the worst possible misread. |
| V4 | `client` and `tx` must be **ASCII digits only** after trimming, and fit `u16` and `u32` respectively. A sign (`+7`, `-7`), an empty value, or an out-of-range value makes the row invalid. Leading zeros are accepted (`007` is `7`), as for amounts in ADR-0007 M4. | Rust's integer parser accepts a leading `+`, so it cannot be used unguarded. The same grammar as amounts gives every field one meaning. |
| V5 | A `deposit` or `withdrawal` with a **missing or empty amount** is invalid. The amount grammar is ADR-0007 M3 to M5. On a `dispute`, `resolve`, or `chargeback`, the amount field is **ignored**: it is not parsed, not validated, and not reported, and the row is processed under ADR-0006 with the full amount of the referenced deposit. | These rows always act on the referenced transaction's amount ([requirements §5](../requirements.md)), so a value in the field cannot change the result. Refusing the row would drop a real claim over a field the engine never reads. |
| V6 | The first `deposit` or `withdrawal` row that is **applied or rejected** binds its `tx` ID for the rest of the run. Any later `deposit` or `withdrawal` with the same ID is **ignored** as a duplicate, whatever its client, type, or amount. Invalid and ignored rows never bind an ID, because they never became a transaction. | `tx` is a globally unique transaction ID ([requirements §2](../requirements.md)), not an idempotency key: the input has no retry semantics, so a reused ID is an upstream uniqueness violation, not a retry, and the engine does not replay the earlier result. Ledgers with unique transfer IDs, such as TigerBeetle, refuse a reused ID and keep it reserved even after a declined transfer. First-writer-wins also means no later row can overwrite a stored deposit or reset its dispute state. |
| V7 | **File-level problems are fatal** (see the [table](#condition-table)): bad arguments, an input that cannot be opened or read, a missing or wrong header, a record over the size limit, and a failed write to stdout. The run **fails closed**: stdout stays empty and the exit code is non-zero. | When the header is wrong or a quote is unbalanced, the engine no longer knows what each byte means. Data loaders default to stopping on such errors, and a partial ledger printed as if complete would be worse than none. |
| V8 | The header must have exactly the four names `type`, `client`, `tx`, `amount`, in that order, after trimming whitespace, case-sensitive. A UTF-8 byte order mark before it is accepted. An empty file has no header and is fatal. A file with only the header is valid and prints only the output header. | A column order the engine does not expect would read an amount as a `tx` without any error. Checking the header once is the cheapest way to rule that out. |
| V9 | A line longer than `MAX_RECORD_BYTES` (one named constant, 4 KiB) is **fatal**. The limit is enforced before the line is buffered, so memory per record is bounded. | A valid row is under 100 bytes, so a longer line means a hostile file or one that does not follow this schema. Python's `csv` module uses the same kind of limit and raises an error. Quoting problems do not depend on this limit; V14 detects them at any length. |
| V10 | Exit codes: `0` when the run completes, even if rows were rejected or ignored; `64` for a usage error; `65` for invalid file data; `66` for an input that cannot be opened; `74` for an I/O error while reading input or writing stdout. A panic exits with Rust's `101` and is always a bug. | These are the matching `sysexits.h` values, the convention the Rust CLI book recommends. Every skipped row is already counted on stderr, and a non-zero code on a completed run could make callers discard a correct output. |
| V11 | Diagnostics for invalid rows use the ADR-0006 R10 budget and summary: same cap, same counters, one reason code each. Any input echoed to stderr is **escaped** (control characters, quotes, and non-ASCII bytes are written as escapes) and truncated, so each diagnostic is exactly one line. A fatal error writes one escaped line naming the condition and, when known, the input line number. | OWASP asks to log validation failures and to sanitize logged data against CR/LF and delimiter injection (CWE-117). Escaping also stops ANSI sequences in the input from rewriting the operator's terminal. |
| V12 | Errors are **typed enums**: a closed set of row reasons in the library (which the summary counts) and one fatal error enum in `main` that maps each variant to its exit code. `main` returns `std::process::ExitCode`, not `Result`. No error is erased into `anyhow` or `Box<dyn Error>`. Whether `thiserror` derives the `Display` code is decided by D7. | Exit codes and per-reason counts need to match on the error kind. A `main` that returns `Result` prints the `Debug` form and always exits with `1`. |
| V13 | No input reaches a panic: input is handled through `Result` only, stdout is written with `write!` and an explicit `flush` whose errors map to `74`, and `println!` and `eprintln!` are not used (both panic on a failed write). The lint policy that enforces this is decided by D7. | A panic on a closed pipe or bad input would crash the run, which [requirements §8](../requirements.md) forbids. |
| V14 | Records are **line-delimited** (`\n` or `\r\n`), and quoting is **strict**. After trimming, a field is either unquoted and contains no `"`, or quoted: `"`, content that contains no `"`, then a closing `"`, with only whitespace between the closing quote and the next `,` or the end of the line. The content is trimmed again and then checked like an unquoted field. Commas inside a quoted field do not split it. Any other `"` in a terminated line (a bare quote such as `1"0`, text after the closing quote such as `"1"0`, or a doubled quote `""`) makes the row **invalid** (`malformed_quote`). A line that **ends inside an open quoted field** is **fatal** (exit `65`), including on the last line of the file. | Valid fields never contain a quote, comma, or line break, so this subset of [RFC 4180](https://www.rfc-editor.org/rfc/rfc4180) accepts every quoted form a valid row can take. Go's `encoding/csv` rejects bare and extraneous quotes by default (`ErrBareQuote`, `ErrQuote`). The `csv-core` parser is lenient by design and never reports an error, so `deposit,1,1,"5` would read as amount `5` and `"1"0` as `10`. A malformed quote inside a terminated line leaves row boundaries certain, so only that row is skipped. An unterminated quote does not: under RFC 4180 the field continues onto the following lines, so the engine cannot tell rows from field content, and it stops instead of guessing. |

### Condition table

Exit code `0` means the run completed and stdout has the full output CSV. Every fatal row leaves stdout empty, except a failed stdout write, where some output may already be written.

| Condition | Class | Report on stderr | Exit code |
|-----------|-------|------------------|-----------|
| No argument, or more than one | Fatal | Usage line | 64 |
| Input path not found, not readable, or a directory | Fatal | Path (escaped) and OS error | 66 |
| I/O error while reading the input | Fatal | OS error and line number | 74 |
| Empty file (no header) | Fatal | "missing header" | 65 |
| Header other than `type,client,tx,amount` | Fatal | Expected and actual header (escaped) | 65 |
| Line longer than `MAX_RECORD_BYTES` (V9) | Fatal | Line number | 65 |
| Line ends inside an open quoted field, including the last line of the file (V14) | Fatal | Line number where the quoted field opens | 65 |
| Write to stdout fails (closed pipe, full disk) | Fatal | OS error | 74 |
| Bare quote, text after a closing quote, or doubled quote in a terminated line (V14) | Ignored, invalid | Diagnostic, reason `malformed_quote` | 0 |
| Wrong field count (V2) | Ignored, invalid | Diagnostic, reason `field_count` | 0 |
| Unknown or miscased `type` (V3) | Ignored, invalid | Diagnostic, reason `unknown_type` | 0 |
| `client` not digits or out of `u16` range (V4) | Ignored, invalid | Diagnostic, reason `invalid_client` | 0 |
| `tx` not digits or out of `u32` range (V4) | Ignored, invalid | Diagnostic, reason `invalid_tx` | 0 |
| `deposit` or `withdrawal` with a missing or empty amount (V5) | Ignored, invalid | Diagnostic, reason `missing_amount` | 0 |
| `deposit` or `withdrawal` with an invalid amount (ADR-0007 M3 to M5) | Ignored, invalid | Diagnostic, reason `invalid_amount` | 0 |
| `dispute`, `resolve`, or `chargeback` with an amount (V5) | Not a condition: the amount is ignored and the row is processed | Only what ADR-0006 reports for the row | 0 |
| `deposit` or `withdrawal` reusing a bound `tx` (V6) | Ignored | Diagnostic, reason `duplicate_tx`, owning client | 0 |
| Dispute-related row not applicable (ADR-0006 R1 to R7) | Ignored | Diagnostic, reason per ADR-0006 | 0 |
| Row whose effect would overflow (ADR-0007 M7) | Ignored | Diagnostic, reason `overflow` | 0 |
| `withdrawal` with insufficient funds | Rejected | Diagnostic, reason `insufficient_funds` | 0 |
| `withdrawal` on a locked account ([ADR-0008](0008-locked-account-policy.md) L3, L4: the lock is checked before funds) | Rejected | Diagnostic, reason `account_locked` | 0 |
| Write to stderr fails | Not a condition of the run | Nothing (ADR-0006 R10) | Unchanged |

Reason codes are named here for clarity; the code may spell them differently as long as each one is a distinct, countable variant.

### Parsing details

* Fields are trimmed of surrounding ASCII whitespace before every check ([requirements §2](../requirements.md)). Inner whitespace stays and makes the field invalid.
* Simple quoting is accepted under V14, so `"deposit"` is `deposit` and `" 2.5 "` is `2.5`. Valid input never needs quotes. Quoted fields that span lines, which RFC 4180 allows, are not supported: no valid field contains a line break.
* Quote checks run before field checks, so a row with a malformed quote always reports `malformed_quote`, whatever else is wrong with it.
* The reader must implement V14 itself or add a strict check in front of a CSV crate. The `csv` crate alone does not satisfy V14, because `csv-core` accepts malformed quoting without an error. Which of the two is used is decided by D7.
* Empty lines are not rows: they are skipped without a diagnostic. A line holding only whitespace is a row with one field and is invalid under V2.
* Data rows are read as bytes, not as UTF-8 strings. Every valid field is ASCII, so invalid UTF-8 in a data row simply fails the field check where it appears (V1) instead of aborting the run.
* A data row equal to the header (for example two files concatenated) has the type `type` and is invalid under V3.
* The input path is taken as an `OsString`, so a non-UTF-8 path works. The single argument is always a path: `-h`, `--help`, and `-` are file names like any other.

### Consequences

* Good, because a single bad row costs one diagnostic and one count, while every other client is still processed.
* Good, because the engine never builds a ledger from a file it cannot parse with certainty: header problems, unterminated quotes, and read errors stop the run before anything reaches stdout, and no malformed quote is ever read as a value.
* Good, because a stored deposit can never be overwritten or reset by a later row with the same `tx`, which closes the "reuse the ID after a chargeback" path (A28).
* Good, because exit codes say why a run failed, and a script can tell a usage mistake (`64`) from bad data (`65`) and from an environment problem (`66`, `74`).
* Good, because diagnostics stay one line each, so hostile input cannot forge log entries or terminal output.
* Bad, because a skipped `deposit` or `withdrawal` means the balances differ from what the upstream intended. Accepted: the row is counted and reported, and aborting would hide every other client's correct balance as well.
* Bad, because strict parsing refuses forms a lenient upstream might send, such as `Deposit` or `+7`. Accepted for the same reason as ADR-0007: loosening later is compatible.
* Bad, because rejected withdrawals must be remembered to detect duplicates, so withdrawals now take per-transaction state (an ID marker, not a lifecycle state). In the worst case that is up to 2^32 IDs; the storage layout (for example a set of IDs or a 512 MiB bitmap) is left to the implementation and its resource analysis.
* Neutral, because a run that completes exits `0` even if every row was skipped. Callers that need a stricter gate read the end-of-run summary on stderr. A configurable error budget would need an optional flag and a new ADR.
* Bad, because a file that uses RFC 4180 quoting beyond simple fields (multi-line quoted fields, escaped `""`) is refused, by row or as a whole. Accepted: no valid row needs either, and a lenient parse is what let `"1"0` become `10`.
* Neutral, because a failed stdout write may leave partial output behind. The exit code `74` is the signal; output is written only after all input is processed, so input errors can never cause partial output.

### Confirmation

* Scenarios A27 to A37 and invariants I3, I11, and I12 in [requirements §6](../requirements.md) cover duplicates, invalid rows, malformed quoting, ignored amounts on dispute-related rows, diagnostics escaping, and every fatal condition. They are acceptance criteria for the implementation until the tests exist.
* Parser unit tests cover every row-level reason in the condition table, including `Deposit`, `+1`, `65536` as a client, `4294967296` as a `tx`, and 2 and 5 fields. A further test checks that a dispute with an amount, including an unparsable one, has the same effect as the same dispute without it.
* Integration tests run the binary for each fatal condition and check the exit code, that stdout is empty, and that stderr has exactly one error line.
* Reader unit tests cover V14 with `deposit,1,1,"5` as the last line (fatal), the same line followed by more rows (fatal, no later row applied), `deposit,1,1,"1"0`, `deposit,1,1,1"0`, and `deposit,1,1,""5""` (each `malformed_quote`), and `"deposit",1,1," 2.5 "` and `deposit,1,1,"1,000"` (valid quoting; the second fails as `invalid_amount`).
* A fuzz or property-based test feeds arbitrary bytes to the reader and checks that the run never panics, that memory per record stays under the limit, and that every outcome is either exit `0` with a valid CSV or a fatal exit code with empty stdout.
* Review check: no `unwrap`, `expect`, indexing, `println!`, or `eprintln!` on any input or output path, and every fatal error variant maps to exactly one exit code.

## Pros and Cons of the Options

### Malformed rows

* **Skip with a diagnostic (chosen).** Good, because it matches transaction-level rejection in ISO 20022 status reports (a batch can be partially accepted) and the skip-and-continue rule the requirements already use for upstream errors. Good, because the automated gate still gets an output for every other client. Bad, because the ledger then misses whatever the bad row meant.
* **Abort the run.** Good, because no balance is ever computed from incomplete data; it is the default of Snowflake `COPY` (`ABORT_STATEMENT`) and BigQuery load jobs (`maxBadRecords = 0`). Bad, because one stray row denies service to every client, and the CLI has no flag to override it.
* **Skip until an error budget is exhausted.** Good, because it is how Snowflake (`SKIP_FILE_<n>`) and BigQuery (`maxBadRecords`) let operators trade completeness for availability. Bad, because the budget needs an optional flag or an arbitrary built-in number. Kept as a future extension.

### Field strictness

* **Strict (chosen).** Good, because every accepted row has one meaning and the grammar matches ADR-0007. Bad, because a miscased type or a `+` sign is refused.
* **Lenient.** Good, because more upstream variants are processed. Bad, because guessing that `DEPOSIT` or `Dep` is a deposit turns upstream bugs into money movements, and a lenient grammar cannot be tightened later without breaking callers.

### Amount on `dispute`, `resolve`, or `chargeback`

* **Ignore the amount (chosen).** Good, because the dispute still holds the funds, which protects the platform, and the result is the same with or without the field. Good, because an upstream that repeats the original amount on its dispute rows still has every claim applied. Bad, because if an upstream ever means a partial dispute, the engine holds the full amount without saying so. Accepted: partial disputes are out of scope (ADR-0006, Known limits), and holding more is the cautious side.
* **Invalid row.** Good, because it matches strict schema validation in payment APIs, which refuse parameters an operation does not take. Bad, because a real dispute or chargeback would be dropped over a field the engine never uses, leaving funds unheld or a reversal unrecorded.

### Duplicate `tx` IDs

* **First deposit or withdrawal binds the ID, even when rejected (chosen).** Good, because it enforces the uniqueness the input format promises for `tx`. Good, because it matches TigerBeetle, which refuses a reused transfer ID (`exists`, `exists_with_different_*`) and keeps the ID of a declined transfer reserved (`id_already_failed`). Good, because a second withdrawal with a rejected withdrawal's ID cannot succeed just because funds arrived in between.
* **Only an applied row binds the ID.** Good, because no state is kept for rejected withdrawals. Bad, because a rejected withdrawal's ID could be used again by a later withdrawal, so two rows would share one "unique" ID and the second would move money.
* **Last row wins.** Bad, because a later row could overwrite a stored deposit, change its owner or amount, or reset a charged-back deposit to `Processed`.
* **Abort the run.** Bad, because a single duplicated row denies service to every client, while ignoring it is already safe.
* **Idempotent replay (return the first result for a repeated request).** Not applicable: it needs an idempotency key and a request that can be retried, and the input has neither. If upstream ever sends idempotency keys, a new ADR decides how they relate to `tx`.

### File-level errors

* **Fail closed (chosen).** Good, because output appears only for a file the engine read completely and understood. Bad, because a late read error discards a long run.
* **Write the balances computed so far.** Bad, because a partial ledger looks the same as a complete one to the automated checker and to any downstream system.

### Exit codes

* **`sysexits.h` subset (chosen).** Good, because the codes are documented, distinguish usage, data, input, and I/O failures, and are the convention the Rust CLI book points to. Neutral, because FreeBSD marks the header as deprecated; the values are still widely used and only four constants are needed, defined in this crate without a dependency.
* **`0`/`1`/`2`.** Good, because `2` for usage errors is what `clap` and many Unix tools return. Bad, because every runtime failure collapses into `1`.
* **`0`/`1` only.** Bad, because a caller cannot tell bad data from a missing file.

### Exit code when rows were skipped

* **`0` (chosen).** Good, because a completed run with a correct output is a success, and the requirements say skipped rows do not fail the run. Bad, because a caller that ignores stderr cannot see that rows were skipped.
* **A distinct non-zero code.** Good, because skips are visible to scripts. Bad, because harnesses and pipelines often treat any non-zero code as failure and would discard a correct output.

### Record size

* **Bounded, fatal (chosen).** Good, because memory per record is constant whatever the input. Bad, because a file with a single absurdly long row cannot be processed at all.
* **Unbounded.** Bad, because a single long line, or a lenient parser that lets an open quote run on, can make one record as large as the rest of the file.

### Malformed quoting

* **Strict detection on line-delimited records (chosen).** Good, because no malformed quote is ever read as a value, whatever the record length. Good, because it matches strict CSV readers such as Go's `encoding/csv`, which reports `ErrBareQuote` and `ErrQuote` unless `LazyQuotes` is set. Good, because a malformed quote in a terminated line costs one row, while an unterminated one, where boundaries are lost, stops the run. Bad, because quoted fields cannot span lines, and the reader needs code or a pre-check of its own.
* **Lenient parse (the `csv-core` default).** Good, because it needs no code. Bad, because it always produces some parse: `"1"0` becomes `10`, and an unterminated quote at the end of the file becomes a valid amount. That is exactly the misread V7 forbids.
* **No quoting at all.** Good, because every `"` simply fails the field check and records are always lines. Bad, because an upstream that quotes every field, which many CSV writers can do, would have every row refused.

### Error types

* **Hand-written typed enums (chosen).** Good, because every reason and every fatal kind is a variant the compiler checks, which the summary counters and the exit-code mapping need. Good, because no dependency is required; `thiserror` can be adopted later through D7 without changing the variants.
* **`thiserror` in the library plus `anyhow` in `main`.** Good, because it is the common Rust split for applications. Bad, because `anyhow` erases the error kind, so mapping to exit codes needs downcasts that the compiler cannot check for completeness.
* **`Box<dyn Error>`.** Bad, for the same reason, and it allocates on every error.

## More Information

* Industry practice consulted:
  * Payment batches: [ISO 20022 payment status reports](https://docs.oracle.com/en/cloud/saas/financials/24b/faofc/iso-20022-payment-status-reports.html) separate group-level rejection (`RJCT` for the whole batch) from transaction-level rejection inside a partially accepted batch (`PART`), with [one reason per rejected transaction](https://www.handelsbanken.com/tron/xgpu/info/contents/v1/document/72-111389).
  * Duplicate IDs: [TigerBeetle `create_transfers`](https://docs.tigerbeetle.com/reference/requests/create_transfers/) refuses a reused transfer `id` with `exists`, `exists_with_different_*`, or `id_already_failed`. For contrast, [Stripe idempotent requests](https://docs.stripe.com/api/idempotent_requests) replay the first result for a repeated key; that model needs idempotency keys, which this input does not have.
  * Data loading: Snowflake [`COPY INTO`](https://docs.snowflake.com/en/user-guide/data-load-transform) defaults to `ON_ERROR = ABORT_STATEMENT`; [BigQuery load jobs](https://docs.cloud.google.com/bigquery/docs/reference/rest/v2/Job) default to `maxBadRecords = 0`. Both offer skip modes and error budgets.
  * Record limits: Python's [`csv.field_size_limit`](https://docs.python.org/3/library/csv.html) (default 128 KiB) raises an error on longer fields.
  * Quoting: Go's [`encoding/csv`](https://pkg.go.dev/encoding/csv) reports `ErrBareQuote` and `ErrQuote` unless `LazyQuotes` is set. The [`csv-core` reader](https://docs.rs/csv-core/latest/csv_core/struct.Reader.html#rfc-4180) states that it can never return an error and prefers some parse over none.
  * Exit codes: [`sysexits(3)`](https://man.freebsd.org/cgi/man.cgi?query=sysexits) and the [Rust CLI book on exit codes](https://rust-cli.github.io/book/in-depth/exit-code.html).
  * Logging: the [OWASP Logging Cheat Sheet](https://cheatsheetseries.owasp.org/cheatsheets/Logging_Cheat_Sheet.html) on logging validation failures and preventing log injection.
  * Rust: [`println!`](https://doc.rust-lang.org/std/macro.println.html) panics when the write fails; integer `FromStr` accepts a leading `+` (checked locally: `"+42".parse::<u32>()` is `Ok(42)`). The `csv-core` parser strips a leading UTF-8 BOM and skips empty lines (checked in the `csv-core` 0.1.13 source).
* Refines [ADR-0006](0006-dispute-lifecycle.md) without changing it: R8 is unchanged (duplicates are ignored rows and never create an account), and R10 now also covers invalid rows. Answers ADR-0006's note that a failed stdout write is handled by the error-handling decision (V7, V10), and the reporting question left open by [ADR-0007](0007-money-representation.md) M6.
* Fills in what [ADR-0008](0008-locked-account-policy.md) L7 and [ADR-0009](0009-output-format.md) O10 leave to this decision: the reason codes (`account_locked` and `insufficient_funds` are counted apart), the fatal conditions, and their exit codes. A fatal error leaves stdout empty, as O10 requires.
* Answers the open questions on malformed rows and duplicate `tx` IDs in [requirements §10](../requirements.md). Dependencies (`csv`, `thiserror`) and lint policy are decided by D7 (LYA-45); the attack catalog by D8 (LYA-46); the CLI entry point is built in E1 (LYA-49).
* Revisit if an upstream needs case-insensitive types or partial disputes (the amount on dispute-related rows would then need a meaning and validation), if an error budget becomes necessary, if an upstream needs multi-line quoted fields, or if the CLI ever gains optional flags.
* Tracked by issue LYA-43 (D5).
