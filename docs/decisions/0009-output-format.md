---
status: accepted
date: 2026-11-10
decision-makers: Alonso Solano
---

# Print a canonical, sorted output CSV with fixed 4-digit amounts

## Context and Problem Statement

The output CSV is checked automatically before any human review ([requirements §1](../requirements.md)). The checker ignores row order, whitespace, and trailing zeros on round values, and amounts must carry 4 fractional digits of precision ([requirements §3](../requirements.md)). Within those limits, the exact shape of the output is ours to choose.

What exactly does the engine write to stdout: the header, the order of rows, which clients get a row, how amounts and booleans are printed, and what is printed when there are no accounts or the run fails?

## Decision Drivers

* Passing the automated gate comes first. Nothing on stdout may be anything other than the CSV ([requirements §1](../requirements.md)).
* The same input must always produce byte-identical output, so tests can compare files exactly and a diff between two runs shows only real changes.
* Every value must have exactly one printed form. No value may be rounded, clamped, or reformatted on the way out ([ADR-0007](0007-money-representation.md) M8).
* The printed ledger must tell the truth, including debt ([ADR-0006](0006-dispute-lifecycle.md) R2).
* A failed run must not be mistaken for a run with no accounts.
* Follow common practice for financial data files and for CSV.
* The CLI, input format, and output columns stay as they are ([requirements §1](../requirements.md)).

## Considered Options

The choices below were decided one by one. Each lists the options considered and the chosen one.

1. **Decimal formatting:** always 4 fractional digits (`1.5000`, `2.0000`), or trimmed (`1.5`, `2`).
2. **Row order:** ascending client ID, first-seen order, or storage order.
3. **Which clients get a row:** every client named in any row, or only clients whose account was opened under [ADR-0006](0006-dispute-lifecycle.md) R8.
4. **Output with no accounts:** the header only, or nothing.
5. **Output on a fatal error:** nothing, or whatever was written so far.
6. **Line terminator:** `\n` or `\r\n`.

## Decision Outcome

Chosen options: fixed 4 fractional digits, rows sorted by client ID, one row per account opened under R8, the header even with no accounts, nothing on stdout when the run fails, and `\n` line endings. Together they give exactly one valid output for each input.

### Rules

| # | Rule | One-line justification |
|---|------|------------------------|
| O1 | The first line is exactly `client,available,held,total,locked`: lowercase, in this order, with no spaces. It is printed on every successful run, even when there are no accounts. | The header is the schema. A header-only file is an unambiguous "no accounts"; an empty file is not. |
| O2 | After the header there is **one row per account** and nothing else: no summary row, no comment line, no blank line, no byte-order mark. An account exists only if it was opened under [ADR-0006](0006-dispute-lifecycle.md) R8: by a `deposit` or `withdrawal` with a valid amount, including a rejected withdrawal (zero balances). Ignored rows and invalid rows ([ADR-0007](0007-money-representation.md) M6) never add a row. | The output reports accounts, not client IDs that happened to appear in garbage rows. |
| O3 | Rows are sorted by `client` in **ascending numeric order**. | The order is independent of how clients interleave in the input and of hash-map iteration, which the Rust standard library leaves unspecified and seeds randomly per run. |
| O4 | `client` is printed as a plain decimal `u16`: no sign, no leading zeros, no padding (`1`, `65535`). | One printed form per ID. |
| O5 | `available`, `held`, and `total` are printed as an optional `-`, the integer part, `.`, and **exactly 4 fractional digits**. The integer part has no leading zeros (a single `0` for values below 1), no grouping separators, and no `+`. Exponent notation is never used. Zero is `0.0000` and never `-0.0000`. Values between `-1` and `0` keep their sign (`-0.5000`). | One printed form per value, and every printed amount shows its full precision. |
| O6 | Amounts are formatted from the integer unit count with integer arithmetic only ([ADR-0007](0007-money-representation.md) M8): the sign comes from the unit count, and the digits come from its unsigned absolute value (`unsigned_abs`, which is defined for `i128::MIN`) divided by 10,000, with the remainder zero-padded to 4 digits. | Dividing the signed count would print `-0.5` as `0.5000`, a plain absolute value overflows on `i128::MIN`, and a float conversion would reintroduce rounding. |
| O7 | Printed balances are the **ledger values unchanged**. Negative balances are printed as they are ([ADR-0006](0006-dispute-lifecycle.md) R2), and `total` is the stored total, so `total = available + held` holds in the printed output. | Clamping or recomputing on the way out would hide debt or break the invariant. |
| O8 | `locked` is printed as `true` or `false`, lowercase. | Matches the format in the requirements and the usual CSV encoding of booleans. |
| O9 | The output is ASCII. Fields are separated by `,` with no whitespace around them and are never quoted. Every record, including the last, ends with `\n`. | No field can contain a comma, quote, or line break, so quoting is never needed. `\n` is the Unix convention and the default of the Rust `csv` crate writer. |
| O10 | The CSV is written to stdout only **after the whole input has been processed**. If the run stops on a fatal error before that point, **nothing** is written to stdout. Which conditions are fatal and which exit code is used are decided by the input-validation decision (D5). | A consumer must never take a partial result, or a header-only output from a failed run, for the final state of the accounts. |

### Examples

Input (whitespace and short amounts are accepted under [requirements §2](../requirements.md)):

```
type, client, tx, amount
deposit, 2, 1, 2.0
deposit, 1, 2, 0.5
withdrawal, 3, 3, 1.0
withdrawal, 1, 4, 0.5
dispute, 1, 2,
chargeback, 1, 2,
dispute, 4, 99,
deposit, 2, 5, 0.0001
dispute, 2, 5,
```

What happens, row by row:

1. Client 2 deposits `2.0`.
2. Client 1 deposits `0.5`.
3. Client 3 tries to withdraw `1.0` with no funds. The withdrawal is rejected, but it opens a zero-balance account (R8).
4. Client 1 withdraws `0.5`, leaving `0`.
5. Client 1 disputes tx 2. The full `0.5` moves to `held`, so `available` becomes `-0.5` (R2).
6. The chargeback removes the held `0.5` from `total` and locks the account. The debt stays visible.
7. Client 4 disputes an unknown tx. The row is ignored and no account is opened for client 4 (R7, R8).
8. Client 2 deposits `0.0001`.
9. Client 2 disputes tx 5, moving `0.0001` to `held`.

Exact expected output, byte for byte (each line ends with `\n`):

```
client,available,held,total,locked
1,-0.5000,0.0000,-0.5000,true
2,2.0000,0.0001,2.0001,false
3,0.0000,0.0000,0.0000,false
```

The row for client 1 comes first although client 2 appeared first in the input (O3). The value `-0.5000` shows the sign rule for values between `-1` and `0` (O5, O6).

An input with no accounts, such as a header only or only ignored rows (scenarios A6 and A20 in [requirements §6](../requirements.md)), produces exactly:

```
client,available,held,total,locked
```

### Consequences

* Good, because each input has exactly one valid output, so golden-file tests can compare bytes, and two runs on the same input always produce identical files.
* Good, because every printed amount shows its full 4-digit precision, and a round amount is never printed as a bare integer such as `2`.
* Good, because the formatter has no trimming branch: it always prints the same structure, which is less code to test.
* Good, because a failed run leaves stdout empty, so the shell redirect produces an empty file that no consumer can mistake for a valid ledger.
* Good, because debt is printed as is, and the printed columns satisfy `total = available + held`.
* Bad, because the layout differs from the trimmed example in the original requirements (`1.5`, `2`). Accepted: the checker ignores trailing zeros on round values ([requirements §3](../requirements.md)), and the precision rule asks for 4 fractional digits.
* Bad, because fixed digits make the output a few bytes longer per row. At most 65,536 rows exist, so this is negligible.
* Neutral, because sorting adds work proportional to `n log n` for at most 65,536 accounts, or none if the account store is already ordered. The data structure is decided by the architecture decision (D6).
* Neutral, because the output is buffered until the end of the run. The account table is bounded by the `u16` client range, so this does not conflict with streaming the input ([requirements §8](../requirements.md)).
* Neutral, because `\n` differs from the `\r\n` that RFC 4180 describes. Common CSV parsers accept both, and the checker ignores whitespace.

### Confirmation

* An end-to-end test runs the binary on the example input above and compares stdout with the expected output byte for byte. It also runs the binary twice and checks that both outputs are identical. Where sample files live is decided by the testing decision (D9).
* Formatter unit tests cover `0`, `0.0001`, `-0.0001`, `-0.5`, `-0.9999`, `-1`, `1.5`, `1844674407370955.1616` (one unit above `u64::MAX` units, the amount limit), the largest reachable balance from scenario A21 (`3689348814741910.3230`), and `i128::MIN` and `i128::MAX` units. No case may print `-0.0000`, a leading `+`, grouping separators, or an exponent.
* A property-based test runs over any `i128` balance, with generated cases that always include zero, negative balances between `-1` and `0`, and balances above `u64::MAX` units. For each balance it checks that:
  * the formatted string matches `-?(0|[1-9][0-9]*)\.[0-9]{4}`;
  * the string starts with `-` exactly when the balance is negative;
  * a balance decoder rebuilds the signed `i128` unit count from the string with checked integer arithmetic only, and the result equals the original balance.
* The balance decoder is a test helper, separate from the [ADR-0007](0007-money-representation.md) transaction amount parser, which stays unchanged. The two cover different domains: an amount is strictly positive and at most `u64::MAX` units, while a balance is signed, can be zero, and can exceed that limit (A21). The amount parser would reject valid balances such as `0.0000` or `3689348814741910.3230`, so it must not be used to verify the formatter.
* A test with a fatal input error (once D5 defines one) checks that stdout is empty.
* Review check: nothing else writes to stdout, and no code path writes account rows before the input is fully processed.

## Pros and Cons of the Options

### Decimal formatting

* **Always 4 fractional digits (chosen).** Good, because every value has one printed form, which makes tests and diffs exact. Good, because it is how ledgers and bank statements show amounts: with the fixed number of minor-unit digits of the currency, as ISO 4217 defines them, rather than trimming. Good, because the precision is visible in every value. Bad, because it does not match the trimmed example layout, which the checker tolerates.
* **Trimmed (`1.5`, `2`).** Good, because it matches the example in the requirements and is shorter. Bad, because the width depends on the value, and a round amount such as `2` looks like an integer count. Bad, because the formatter needs a trimming branch with its own edge cases (`2.` versus `2`, `-0` versus `0`).

### Row order

* **Ascending client ID (chosen).** Good, because the order depends only on the set of accounts, not on the input order or on hashing. Good, because a sorted list is the easiest to scan and to compare by eye.
* **First-seen order.** Good, because it is also deterministic. Bad, because it needs extra bookkeeping (insertion order) and changes whenever the input order of clients changes, even when the final state does not.
* **Storage order.** Bad, because with the standard `HashMap` the order changes from run to run, so outputs cannot be compared byte for byte.

### Which clients get a row

* **Only accounts opened under R8 (chosen).** Good, because it reports what the ledger holds and matches [ADR-0006](0006-dispute-lifecycle.md) and [ADR-0007](0007-money-representation.md) M6. Neutral, because a rejected withdrawal still opens a zero-balance account; that rule belongs to ADR-0006 and is only printed here.
* **Every client named in any row.** Bad, because garbage rows such as a dispute on an unknown `tx` could add accounts to the output, which R8 forbids.

### Output with no accounts

* **Header only (chosen).** Good, because it is still a valid CSV with the expected schema, and it is what scenarios A6 and A20 require.
* **Nothing.** Bad, because it cannot be told apart from a failed run.

### Output on a fatal error

* **Nothing (chosen).** Good, because a partial or header-only result can never be taken for the final state. Good, because it costs nothing: the output is already written only at the end. Neutral, because a failure while writing stdout itself can still leave a partial file; that is a write error, handled by D5, and the exit code is how the caller detects it.
* **Whatever was written so far.** Bad, because a truncated ledger can look like a complete one.

### Line terminator

* **`\n` (chosen).** Good, because it is the Unix text convention and the default of the Rust `csv` crate writer. Neutral, because RFC 4180 describes `\r\n`; that RFC is informational, and CSV parsers accept both.
* **`\r\n`.** Good, because it follows RFC 4180 literally. Bad, because it adds a byte per line that most Unix tools show as noise, with no benefit for the checker.

## More Information

* References consulted:
  * [RFC 4180](https://www.rfc-editor.org/rfc/rfc4180) on the common CSV format: header line, comma separators, and quoting only when a field needs it.
  * The [Rust `csv` crate `WriterBuilder`](https://docs.rs/csv/latest/csv/struct.WriterBuilder.html): the default record terminator is `\n` and the default quote style quotes only when necessary, so its defaults already produce O9.
  * The [Rust `HashMap` documentation](https://doc.rust-lang.org/std/collections/struct.HashMap.html): the hasher is randomly seeded and iteration order is arbitrary.
  * [ISO 4217](https://www.iso.org/iso-4217-currency-codes.html) on the fixed number of minor-unit digits per currency.
* Whether the writer uses the `csv` crate or plain formatted writes is an implementation detail under the dependency decision (D7). Either way, the output must follow O1 to O10.
* Revisit this decision if the checker turns out to require a different layout, or if the output gains columns. Both would need a superseding ADR, and the second would also change the CLI contract.
* Answers the output-format details of issue LYA-40 (D2). Account creation comes from [ADR-0006](0006-dispute-lifecycle.md) (D3), the output of locked accounts from [ADR-0008](0008-locked-account-policy.md) L6 (D4), exact formatting and negative values from [ADR-0007](0007-money-representation.md) (D1), and fatal errors and exit codes from D5 (LYA-43). The output writer (E3) builds on these rules.
