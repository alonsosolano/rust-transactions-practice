# Requirements

Behavior contract for the transaction engine. Written for this repo from the project brief; any interpretation beyond what is stated here must be recorded as an ADR.

**Precedence:** where the prose of the brief is inconsistent, the described input and output file formats take precedence over the prose.

## 1. Command-line interface

```sh
cargo run -- transactions.csv > accounts.csv
```

- Exactly one positional argument: the path to the input CSV.
- Account state is written to **stdout** as CSV. Nothing else may go to stdout.
- Errors and diagnostics go to stderr.
- The interface is checked by automated tests, so it must not change.
- The single argument is always a path; `-h`, `--help`, and `-` have no special meaning ([ADR-0010](decisions/0010-input-validation-and-errors.md)).

### Exit codes

Follow [ADR-0010](decisions/0010-input-validation-and-errors.md) V10. A fatal error writes one line to stderr and leaves stdout empty (a failed stdout write may leave partial output). When the error happens after the input was opened, the diagnostics summary comes first and the fatal line is the last line ([ADR-0017](decisions/0017-diagnostic-line-format-and-summary.md) D7).

| Code | Meaning |
|------|---------|
| `0`  | The run completed and stdout has the full output, even if rows were rejected or ignored |
| `64` | Usage error: no argument or more than one |
| `65` | Invalid file data: missing or wrong header, or a record longer than the size limit |
| `66` | The input cannot be opened: not found, not readable, or a directory |
| `74` | I/O error while reading the input or writing stdout |

### How the submission is evaluated

1. **Automated gate first.** The binary is run against a set of sample inputs and its output is compared with the expected output. Only if this passes does manual review happen.
2. **Manual review** then covers everything else (see section 8).

Consequences:

- Correct output through the exact CLI above is the top priority; any other interface, flag, or entry point is ignored by reviewers.
- Nothing may be written to stdout except the output CSV, or the comparison fails.

## 2. Input

CSV with a header row and the columns `type, client, tx, amount`.

| Column   | Type    | Notes |
|----------|---------|-------|
| `type`   | string  | `deposit`, `withdrawal`, `dispute`, `resolve`, `chargeback` |
| `client` | `u16`   | Client identifier |
| `tx`     | `u32`   | Transaction identifier, globally unique |
| `amount` | decimal | Up to 4 fractional digits, strictly positive. Present for `deposit` and `withdrawal`; empty or missing for the other three types |

Parsing must tolerate:

- Whitespace around fields and headers.
- Any decimal precision up to 4 fractional digits (`1`, `1.0`, `1.2345`).
- A UTF-8 byte order mark before the header, empty lines, `\r\n` line endings, and a whole field in double quotes (`"deposit"`, `" 2.5 "`).

Validation follows [ADR-0010](decisions/0010-input-validation-and-errors.md):

- The header must be exactly `type,client,tx,amount` after trimming, case-sensitive. A missing or different header is fatal (exit `65`).
- A line longer than 4 KiB is fatal (exit `65`); memory per record is bounded.
- Quoting is strict and records are lines. A line that ends inside an open quoted field is fatal (exit `65`), including the last line. Any other stray quote (`1"0`, `"1"0`, `""5""`) makes the row invalid.
- A data row must have 4 fields, or 3 for `dispute`, `resolve`, and `chargeback`.
- `type` is matched exactly and case-sensitively: `Deposit` is unknown.
- `client` and `tx` are ASCII digits only, within `u16` and `u32`. A sign (`+7`) makes the row invalid; leading zeros are accepted.
- A `deposit` or `withdrawal` needs a non-empty amount. On a `dispute`, `resolve`, or `chargeback`, any amount is ignored (not parsed or reported) and the row is processed normally.
- A row that fails any of these checks is **invalid**: it is skipped with a diagnostic and the run continues (see §5).

Amounts follow [ADR-0007](decisions/0007-money-representation.md):

- Stored as exact fixed-point integers in units of `0.0001`. No floating point is used, and no amount is ever rounded or truncated.
- Valid form: ASCII digits, optionally followed by `.` and more digits. Extra fractional digits beyond the fourth are accepted only if they are zeros (`1.23450`); leading zeros are accepted (`007.5`).
- Valid range: `0.0001` to `1844674407370955.1615`.
- Anything else makes the `deposit` or `withdrawal` row invalid: a sign (`-5`, `+5`), `.5`, `5.`, exponents (`1e3`), separators (`1_000`, `1,000`), zero, a non-zero digit after the fourth decimal place (`1.00005`), or a value above the range. An invalid row never changes state and never creates an account. It is reported like any other skipped row ([ADR-0010](decisions/0010-input-validation-and-errors.md) V1, V11).

Ordering:

- Rows are in chronological order; a row happens after every row above it.
- Client IDs and transaction IDs are **not** sorted. Clients can appear in any order and interleave.

## 3. Output

CSV with the columns `client, available, held, total, locked`. The exact format follows [ADR-0009](decisions/0009-output-format.md):

- **Header:** exactly `client,available,held,total,locked`, with no spaces. Always printed on a successful run, even when there are no accounts.
- **Rows:** one per account, and nothing else. An account exists only if it was opened under §4; ignored and invalid rows never add a row.
- **Order:** ascending by `client`.
- **Amounts:** `available`, `held`, and `total` are printed with exactly 4 fractional digits (`2.0000`, `-0.5000`), an optional `-`, no `+`, no leading zeros, no grouping separators, and no exponent. Zero is `0.0000`, never `-0.0000`. Formatting uses integer arithmetic only and keeps the sign of values between `-1` and `0` ([ADR-0007](decisions/0007-money-representation.md) M8). Negative balances are printed as they are.
- **`locked`:** `true` or `false`.
- **Encoding:** ASCII, `,` separators with no surrounding whitespace, no quoting, and `\n` after every record including the last.
- **When:** the CSV is written only after the whole input is processed. If the run fails before that, stdout stays empty.

The checker ignores row order, whitespace, and trailing zeros on round values; the rules above still fix one exact output per input, so tests can compare bytes.

Example input:

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

Exact output:

```
client,available,held,total,locked
1,-0.5000,0.0000,-0.5000,true
2,2.0000,0.0001,2.0001,false
3,0.0000,0.0000,0.0000,false
```

Client 3's withdrawal is rejected but still opens a zero-balance account; client 4's dispute is ignored and opens none. The ADR walks through each row.

## 4. Account model

Each client has exactly one account, created by the first `deposit` or `withdrawal` row for that client, even if the withdrawal is rejected. Rows that are ignored never create an account ([ADR-0006](decisions/0006-dispute-lifecycle.md)). Neither do invalid rows: a withdrawal with an invalid amount is not a rejected withdrawal ([ADR-0007](decisions/0007-money-representation.md) M6). Neither does a `deposit` or `withdrawal` that reuses a `tx` ID ([ADR-0010](decisions/0010-input-validation-and-errors.md) V6).

Invariants that must hold after every operation:

- `total = available + held`
- `locked` becomes `true` after a chargeback and never reverts. What a locked account still allows is set by [ADR-0008](decisions/0008-locked-account-policy.md).
- `held` equals the sum of the amounts of the deposits currently under dispute.
- `available` and `total` may be negative: a dispute or chargeback on funds that were already withdrawn leaves the debt visible ([ADR-0006](decisions/0006-dispute-lifecycle.md)).
- No balance ever overflows silently. Balances are wide enough that valid input cannot overflow them, and every change is still checked ([ADR-0007](decisions/0007-money-representation.md)).

## 5. Operations

| Type         | References | Effect when valid |
|--------------|------------|-------------------|
| `deposit`    | new `tx` with amount | `available += amount`, `total += amount` |
| `withdrawal` | new `tx` with amount | `available -= amount`, `total -= amount`. **Rejected** if `available < amount`; balances stay unchanged |
| `dispute`    | existing `tx` | Moves the referenced transaction's amount from `available` to `held`. `total` unchanged |
| `resolve`    | disputed `tx` | Moves the amount back from `held` to `available`. `total` unchanged. Dispute ends |
| `chargeback` | disputed `tx` | `held -= amount`, `total -= amount`. Account becomes locked. Dispute ends |

`dispute`, `resolve`, and `chargeback` carry no amount of their own; they always use the amount of the transaction they reference.

The dispute lifecycle follows [ADR-0006](decisions/0006-dispute-lifecycle.md): `Processed → Disputed → Resolved | ChargedBack`. A new dispute on a `Resolved` deposit opens another cycle (`Resolved → Disputed`); `ChargedBack` is terminal. Only stored deposits are disputable. A dispute moves the full amount to `held` even if `available` goes negative.

### Rows to ignore

These are treated as upstream errors and skipped without failing the run. "Skipped" means no state change, no balance change, no account created, and a diagnostic on stderr. Diagnostics are bounded: full detail (row type, client, `tx`, owning client when known, reason, input line) for the first 100 rows, then only an end-of-run summary with one count per reason. A failure to write stderr is ignored and never affects stdout, account state, or the exit code. Writes are still blocking: the cap bounds volume, not write latency, so the caller must drain piped stderr or redirect it. A stalled stderr reader is an accepted limitation (ADR-0006, R10).

- `dispute`, `resolve`, or `chargeback` whose `tx` does not exist or was never stored (for example a rejected withdrawal).
- `dispute`, `resolve`, or `chargeback` on a withdrawal.
- `dispute`, `resolve`, or `chargeback` whose `client` differs from the client that owns the `tx`.
- `dispute` on a `tx` that is already under dispute or already charged back. A dispute on a resolved `tx` is not ignored; it opens a new cycle.
- `resolve` or `chargeback` whose `tx` is not currently under dispute.
- `withdrawal` on a locked account (rejected; balances unchanged), whatever the balance. The lock is checked before the funds check, so the reason is always "account locked" ([ADR-0008](decisions/0008-locked-account-policy.md) L3, L4). Deposits on a locked account are still accepted, so the client can repay a debt, and `dispute`, `resolve`, and `chargeback` behave as on an unlocked account (L2, L5).
- Any row whose effect would overflow a balance ([ADR-0007](decisions/0007-money-representation.md) M7). Unreachable for valid input; kept as defense in depth.
- Any invalid row (§2): wrong field count, unknown or miscased `type`, bad `client` or `tx`, a missing or invalid amount on a `deposit` or `withdrawal`, or a stray quote in a terminated line ([ADR-0010](decisions/0010-input-validation-and-errors.md) V1 to V5, V14).
- `deposit` or `withdrawal` whose `tx` ID was already used by an earlier `deposit` or `withdrawal` that was applied or rejected, whatever its client, type, or amount. Invalid and ignored rows never use up an ID ([ADR-0010](decisions/0010-input-validation-and-errors.md) V6).

When a `dispute`, `resolve`, or `chargeback` row breaks several of these rules, the checks run in a fixed order and the first that fails is the reason: the `tx` is stored, then the row's client owns it, then it is a deposit, then its state accepts the row. Each rule has its own reason code ([ADR-0016](decisions/0016-dispute-row-reasons-and-check-order.md) G1, G2).

Diagnostics escape any echoed input, so every entry is exactly one line ([ADR-0010](decisions/0010-input-validation-and-errors.md) V11). The exact line format, the notice, and the summary, which every run that opens its input writes, are set by [ADR-0017](decisions/0017-diagnostic-line-format-and-summary.md).

Only a `deposit` or `withdrawal` row creates a client account, including a rejected withdrawal. Ignored rows never do.

## 6. Threat model and invariants

Card and bank transfers can be reversed after the fact, which is good for consumers but exposes the processor to fraud. The motivating attack:

1. Deposit funds through a reversible payment method.
2. Spend or withdraw those funds immediately.
3. Dispute the original deposit and get it charged back, keeping what was withdrawn.

The engine must behave like a cautious bank under this and similar abuse: it must never create money, never lose track of held funds, and must freeze an account once a chargeback happens.

### Engine boundary

The engine applies accounting and withdrawal rules to the supplied events. It cannot determine whether a deposit is irrevocable or safe to withdraw. The full split is in [ADR-0006](decisions/0006-dispute-lifecycle.md#engine-boundary-and-residual-risk):

- **Enforced by the engine:** the invariants below, ownership checks, full-amount holds with visible debt, withdrawal rejection when funds are insufficient and permanently after a chargeback, and bounded diagnostic volume.
- **Required from upstream (deployment prerequisite):** only deposits that are irrevocable, or fully backed by enforceable collateral or a funding guarantee, are admitted; events are authoritative, chronologically ordered, and deduplicated; piped stderr is drained or redirected. Settlement or a fixed waiting period alone is not proof that a payment cannot be reversed.
- **Residual risk accepted for this scope:** without upstream protection, the attack above remains a financial exposure. Recording a negative balance after a reversal does not prevent the initial loss: a client can withdraw a reversible deposit before the dispute arrives, and the later lock cannot recover funds already paid out. A deposit accepted on a locked account is not final recovery if it can itself be reversed. A stderr reader that stops consuming can block the run.

Enforcing withdrawal eligibility inside the engine would need additional input information and a separate ADR.

This section is the catalog of attack scenarios and invariants. It follows [ADR-0013](decisions/0013-attack-and-invariant-catalog.md): every row names the test types that cover it and a priority, and every entry is an acceptance criterion for the implementation until its tests exist. A later decision that adds or changes a rule adds or updates rows here in the same change (C7).

**Test types** (C2). Where each type lives, how sample cases are laid out and compared, and how tests are named after these IDs follow [ADR-0014](decisions/0014-testing-strategy.md).

| Type | What it runs |
|------|--------------|
| Unit | A domain module (`amount`, `account`, `store`, `engine`) on typed values, with no I/O ([ADR-0011](decisions/0011-architecture-and-memory.md) S2) |
| Integration | The library's `run` function on in-memory input bytes, with in-memory stdout and stderr ([ADR-0011](decisions/0011-architecture-and-memory.md) S1) |
| CLI | The real binary on a committed sample file, checking the exit code, stdout, and stderr ([ADR-0012](decisions/0012-dependencies-and-safety-policy.md)) |
| Property | `proptest` over generated row streams, checking the property after every row ([ADR-0012](decisions/0012-dependencies-and-safety-policy.md)) |
| Type | The guarantee holds by construction ([ADR-0011](decisions/0011-architecture-and-memory.md), type-system guarantees). Never alone: the row also names a test that would catch a regression |

**Priority** (C3) orders the work; every row is still required.

| Priority | A failure could |
|----------|-----------------|
| P1 | Create, lose, or release money; break the lock or the ownership of a `tx` ID; or change the stdout the automated gate compares |
| P2 | Accept input that must be refused, misreport a fatal error, or panic |
| P3 | Affect only diagnostics on stderr |

Invariants are checked by tests only, never at run time (C4): non-test code has no `assert!` or `debug_assert!` for them. A test-only `assert_invariants` helper checks every invariant that can be read from the engine's state after every row of a property test and after every step of a unit scenario.

### Attack scenarios

The `Rule` column refers to the rules R1 to R10 of [ADR-0006](decisions/0006-dispute-lifecycle.md), M1 to M8 of [ADR-0007](decisions/0007-money-representation.md), L1 to L7 of [ADR-0008](decisions/0008-locked-account-policy.md), O1 to O10 of [ADR-0009](decisions/0009-output-format.md), V1 to V14 of [ADR-0010](decisions/0010-input-validation-and-errors.md), G1 to G3 of [ADR-0016](decisions/0016-dispute-row-reasons-and-check-order.md), S1 to S13 of [ADR-0011](decisions/0011-architecture-and-memory.md), and C1 to C7 of [ADR-0013](decisions/0013-attack-and-invariant-catalog.md); `§5` is the operation table and `§2` the input rules above. Rows are listed in input order; `c1`/`c2` are client IDs. Results are `available / held / total / locked`.

| ID | Rule | Scenario (input rows) | Expected behavior | Test | Priority |
|----|------|-----------------------|-------------------|------|----------|
| A1 | R2 | `c1` deposit tx1 100; `c1` withdrawal tx2 100; `c1` dispute tx1; `c1` chargeback tx1 | After the dispute: `-100 / 100 / 0 / false`. After the chargeback: `-100 / 0 / -100 / true`. The debt stays visible. | Unit | P1 |
| A2 | R2 | `c1` deposit tx1 100; `c1` withdrawal tx2 100; `c1` dispute tx1; `c1` withdrawal tx3 10; `c1` resolve tx1 | The withdrawal is rejected while `available` is negative (`-100 / 100 / 0 / false`). The resolve restores `0 / 0 / 0 / false`. | Unit | P1 |
| A3 | R1 | `c1` deposit tx1 100; `c1` withdrawal tx2 40; `c1` dispute tx2; `c1` resolve tx2; `c1` chargeback tx2 | All three rows on the withdrawal are ignored. Result: `60 / 0 / 60 / false`. | Unit | P1 |
| A4 | R3 | `c1` deposit tx1 10; `c2` dispute tx1 | Ignored with a log naming `c1` as owner. `c1`: `10 / 0 / 10 / false`. `c2` has no account. | Unit | P1 |
| A5 | R3 | `c1` deposit tx1 10; `c1` dispute tx1; `c2` resolve tx1; `c2` chargeback tx1 | Both `c2` rows are ignored. `c1` stays disputed: `0 / 10 / 10 / false`. | Unit | P1 |
| A6 | R8 | `c9` dispute tx7 | Ignored. No account for `c9`; the output has only the header. | Unit, Integration | P1 |
| A7 | R4 | `c1` deposit tx1 10; `c1` dispute tx1; `c1` dispute tx1 | The second dispute is ignored. `0 / 10 / 10 / false`; the deposit is held once. | Unit | P1 |
| A8 | R5 | `c1` deposit tx1 10; `c1` dispute tx1; `c1` resolve tx1; `c1` dispute tx1; `c1` chargeback tx1 | The second dispute opens a new cycle (`0 / 10 / 10 / false`) and the chargeback applies. `0 / 0 / 0 / true`. | Unit | P1 |
| A9 | R6 | `c1` deposit tx1 10; `c1` deposit tx2 5; `c1` dispute tx1; `c1` chargeback tx1; `c1` chargeback tx1; `c1` dispute tx1; `c1` resolve tx1 | Only the first chargeback applies. `5 / 0 / 5 / true`. | Unit | P1 |
| A10 | R7 | `c1` deposit tx1 5; `c1` withdrawal tx2 10 (rejected); `c1` dispute tx2; `c1` dispute tx99; `c1` resolve tx1; `c1` chargeback tx1 | All dispute-related rows are ignored (rejected withdrawal, unknown `tx`, deposit not under dispute). `5 / 0 / 5 / false`. | Unit | P1 |
| A11 | R9 | `c1` deposit tx1 10; `c1` deposit tx2 5; `c1` dispute tx1; `c1` chargeback tx1; `c1` deposit tx3 20; `c1` withdrawal tx4 5 | Locked after the chargeback (`5 / 0 / 5 / true`). The deposit is accepted and the withdrawal is rejected. `25 / 0 / 25 / true`. | Unit | P1 |
| A12 | R9 | `c1` deposit tx1 10; `c1` deposit tx2 10; `c1` dispute tx1; `c1` dispute tx2; `c1` chargeback tx1; `c1` resolve tx2 | Dispute-related rows still work on a locked account. After the chargeback: `0 / 10 / 10 / true`. After the resolve: `10 / 0 / 10 / true`. | Unit | P1 |
| A13 | R2, R9 | A1 rows, then `c1` deposit tx3 150; `c1` withdrawal tx4 10 | The deposit repays the debt on the locked account: `50 / 0 / 50 / true`. The withdrawal is rejected and the lock stays. | Unit | P1 |
| A14 | R2, R5 | `c1` deposit tx1 100; `c1` dispute tx1; `c1` resolve tx1; `c1` withdrawal tx2 100; `c1` dispute tx1; `c1` chargeback tx1 | After the resolve and withdrawal: `0 / 0 / 0 / false`. The second dispute is recorded: `-100 / 100 / 0 / false`. The chargeback records the debt and locks: `-100 / 0 / -100 / true`. | Unit | P1 |
| A15 | R10 | `c1` deposit tx1 10, then 1,000,000 rows that are each ignored (for example `c2` dispute tx1) | `c1` stays `10 / 0 / 10 / false`; no account for `c2`. stderr has at most 100 detailed entries, one suppression notice, and a summary that counts all 1,000,000 ignored rows. | Integration | P3 |
| A16 | R10 | Any A1 to A14 input, run with stderr failing on write: a pipe whose reader has closed, or a writer that returns an error | The run completes without a panic, stdout has the same CSV as with a working stderr, and the exit code is unchanged. This covers failing stderr only; a reader that stops consuming a pipe can block the run and is an accepted limitation. A stderr descriptor closed before the run starts never fails a write ([ADR-0014](decisions/0014-testing-strategy.md) T10). | Integration, CLI | P3 |
| A17 | M3, M6 | `c1` deposit tx1 10; `c1` withdrawal tx2 -5; `c2` withdrawal tx3 -5 | Both withdrawals are invalid, so a negative withdrawal cannot create money. `c1`: `10 / 0 / 10 / false`. `c2` has no account. | Integration | P1 |
| A18 | M3, M6 | `c1` deposit tx1 5; `c1` deposit tx2 -10; `c1` withdrawal tx3 10 | The negative deposit is invalid and cannot bypass the funds check, so the withdrawal is rejected for insufficient funds. `5 / 0 / 5 / false`. | Integration | P1 |
| A19 | M4 | `c1` deposit tx1 1.00005; `c1` deposit tx2 1.23450 | tx1 is invalid and is never rounded. tx2 is accepted as `1.2345`. `1.2345 / 0 / 1.2345 / false`. | Integration | P1 |
| A20 | M5, M6 | `c1` deposit tx1 0; `c2` withdrawal tx2 0.0000 | Both rows are invalid. No accounts; the output has only the header. | Integration | P2 |
| A21 | M2, M5 | `c1` deposit tx1 1844674407370955.1615; `c1` deposit tx2 1844674407370955.1615; `c1` deposit tx3 1844674407370955.1616 | tx1 and tx2 are accepted; their sum does not overflow the balance. tx3 is above the range and invalid. `3689348814741910.3230 / 0 / 3689348814741910.3230 / false`. | Integration | P1 |
| A22 | M3 | `c1` deposits with amounts `.5`, `5.`, `+5`, `1e3`, `1_000`, and `" 2.5 "` (tx1 to tx6) | Only tx6 is accepted, after trimming the whitespace. `2.5 / 0 / 2.5 / false`. | Integration | P2 |
| A23 | L5, R2 | `c1` deposit tx1 100; `c1` deposit tx2 100; `c1` deposit tx3 100; `c1` withdrawal tx4 300; `c1` dispute tx1; `c1` dispute tx2; `c1` dispute tx3; `c1` chargeback tx1; `c1` chargeback tx2; `c1` chargeback tx3 | Several disputed deposits: the first chargeback locks (`-300 / 200 / -100 / true`) and the later ones still apply, so the full debt shows. `-300 / 0 / -300 / true`. | Unit | P1 |
| A24 | L1, L2, L5 | `c1` deposit tx1 10; `c1` dispute tx1; `c1` chargeback tx1; `c1` deposit tx2 50; `c1` dispute tx2; `c1` chargeback tx2 | Locked at `0 / 0 / 0 / true`. The deposit after the lock is applied (`50 / 0 / 50 / true`) and stays disputable (`0 / 50 / 50 / true`). Its chargeback returns the account to `0 / 0 / 0 / true`; the lock stays. | Unit | P1 |
| A25 | L3, L4, L7, R7 | `c1` deposit tx1 10; `c1` deposit tx2 5; `c1` dispute tx1; `c1` chargeback tx1; `c1` withdrawal tx3 100; `c1` withdrawal tx4 1; `c1` dispute tx3 | Locked at `5 / 0 / 5 / true`. Both withdrawals are rejected with the reason "account locked", including tx3, which also lacks funds. The dispute on tx3 is ignored because a rejected withdrawal is never stored. `5 / 0 / 5 / true`. | Unit | P1 |
| A26 | O1 to O9 | The example input in §3 | stdout is exactly the example output in §3, byte for byte: sorted rows, fixed 4-digit amounts, `-0.5000` for client 1, a zero-balance row for client 3, and no row for client 4. | CLI | P1 |
| A27 | V6 | `c1` deposit tx1 10; `c2` deposit tx1 50; `c2` dispute tx1 | The second deposit is a duplicate and is ignored, so `c2` cannot take over tx1. The dispute is ignored with `c1` as owner. `c1`: `10 / 0 / 10 / false`. `c2` has no account. | Unit | P1 |
| A28 | V6, R6 | `c1` deposit tx1 10; `c1` dispute tx1; `c1` chargeback tx1; `c1` deposit tx1 10; `c1` dispute tx1 | The repeated deposit is ignored, so the charged-back deposit is not reset. The last dispute is ignored under R6. `0 / 0 / 0 / true`. | Unit | P1 |
| A29 | V6 | `c1` deposit tx1 5; `c1` withdrawal tx2 10 (rejected); `c1` deposit tx3 10; `c1` withdrawal tx2 10 | The rejected withdrawal binds tx2, so a later withdrawal reusing tx2 is ignored as a duplicate and cannot succeed after the deposit. `15 / 0 / 15 / false`. | Unit | P1 |
| A30 | V6, M6 | `c1` deposit tx1 `abc`; `c1` deposit tx1 5 | The invalid row does not bind tx1, so the corrected row is applied. `5 / 0 / 5 / false`. | Integration | P1 |
| A31 | V1 to V5 | `c1` deposit tx1 10, then invalid rows: `Deposit,1,2,5`; `deposit,+1,3,5`; `deposit,65536,4,5`; `deposit,1,4294967296,5`; `deposit,1,5`; `deposit,1,6,`; `deposit,1,7,5,`; then `c1` deposit tx8 1 | Every invalid row is skipped with its own reason and the run continues. `c1`: `11 / 0 / 11 / false`; no other account. | Integration | P2 |
| A32 | V11 | `c1` deposit tx1 10; a row whose `type` is a quoted field holding a bare carriage return and an ANSI escape sequence | The row is invalid. Its diagnostic is a single line with the carriage return and escape written as escapes. (A quoted line break would end the line inside an open quote, which is fatal under V14.) `c1`: `10 / 0 / 10 / false`. | Integration | P3 |
| A33 | V7 to V9 | (a) an empty file; (b) header `type,client,amount,tx`; (c) a valid header followed by a 5 KiB line | Each run exits `65` with one error line on stderr and nothing on stdout. In (c) memory stays bounded. | CLI | P2 |
| A34 | V7, V10 | (a) no argument; (b) two arguments; (c) a path that does not exist; (d) a valid input with stdout connected to a pipe whose reader has closed | (a) and (b) exit `64`, (c) exits `66`, each with empty stdout. (d) exits `74` without a panic. A stdout descriptor closed before the run starts is not detected: the standard library reports writes to it as successful, so that run exits `0` with the output discarded ([ADR-0014](decisions/0014-testing-strategy.md) T10). | CLI | P2 |
| A35 | V5, R2 | `c1` deposit tx1 10; `c1` dispute tx1 with amount `3`; `c1` resolve tx1 with amount `abc`; `c1` dispute tx1 with amount `-50`; `c1` chargeback tx1 with amount `10` | Every amount is ignored and every row applies to the full deposit. After the first dispute: `0 / 10 / 10 / false`. After the resolve: `10 / 0 / 10 / false`. After the chargeback: `0 / 0 / 0 / true`. | Integration | P1 |
| A36 | V14 | `c1` deposit tx1 10, then `deposit,1,2,"1"0`; `deposit,1,3,1"0`; `deposit,1,4,""5""`; `deposit,1,5,"1,000"`; `"deposit", 1 ,6," 2.5 "` | The first three rows are invalid (`malformed_quote`), so the lenient readings `10`, `10`, and `5` never apply. tx5 is quoted correctly and fails as `invalid_amount`. tx6 is applied. `12.5 / 0 / 12.5 / false`. | Integration | P1 |
| A37 | V7, V14 | (a) a valid header, `c1` deposit tx1 10, and `deposit,1,2,"5` as the last line; (b) the same rows with `deposit,1,3,7` and `8"` after the open quote | Each run exits `65` with one error line naming the line where the quote opens, and nothing on stdout. A lenient parser would read (a) as a deposit of `5`; in (b) the tx3 line is never applied, because it may be part of the quoted field. | CLI | P1 |
| A38 | R7 | `c1` deposit tx1 10; `c1` dispute tx1; `c1` resolve tx1; `c1` resolve tx1; `c1` chargeback tx1 | The first resolve releases the funds (`10 / 0 / 10 / false`). The second resolve and the chargeback are ignored because tx1 is no longer under dispute, so the account is not locked. `10 / 0 / 10 / false`. | Unit | P1 |
| A39 | §5 | `c1` deposit tx1 100; `c1` deposit tx2 50; `c1` dispute tx1; `c1` withdrawal tx3 50.0001; `c1` withdrawal tx4 50 | After the dispute: `50 / 100 / 150 / false`. Held funds cannot be withdrawn, so tx3 is rejected for insufficient funds. tx4 equals `available` and is applied. `0 / 100 / 100 / false`. | Unit | P1 |
| A40 | V6 | `c1` deposit tx1 20; `c1` withdrawal tx2 5; `c1` withdrawal tx2 5; `c1` deposit tx2 100 | tx2 is bound by the applied withdrawal, so the replayed withdrawal and the deposit that reuses tx2 are both ignored as duplicates. `15 / 0 / 15 / false`. | Unit | P1 |
| A41 | V6, R3, C6 | `c2` deposit tx1 50; `c1` deposit tx1 10; `c1` dispute tx1 | A27 in the other order: the first row in the file owns tx1. `c1`'s deposit is ignored as a duplicate and opens no account, and the dispute is ignored with `c2` as owner. `c2`: `50 / 0 / 50 / false`. `c1` has no account. | Unit | P1 |
| A42 | V7, V8 | Headers (a) `type,client,tx,amount,note`; (b) `type,client,tx`; (c) `Type,Client,Tx,Amount`, each followed by `deposit,1,1,10` | Each run exits `65` with one error line on stderr and nothing on stdout; no row is read under a header the engine does not recognize. | CLI | P2 |
| A43 | §2 | A UTF-8 byte order mark, the header ` type , client , tx , amount `, `\r\n` line endings, one empty line, and the rows `deposit, 1, 1, 1`; `deposit,1,2,1.0`; `"deposit",1,3,"1.2345"` | Every form is accepted. `c1`: `3.2345 / 0 / 3.2345 / false`. | Integration, CLI | P1 |
| A44 | V4, O3, O4 | `deposit,65535,4294967295,1`; `deposit,0,0,1`; `deposit,007,0008,1` | The boundary IDs and the leading zeros are accepted. The output has rows for clients `0`, `7`, and `65535`, in that order, each `1.0000,0.0000,1.0000,false`. | Integration | P2 |
| A45 | V3 | `c1` deposit tx1 10; the line `type,client,tx,amount` (two files joined); `c1` deposit tx2 5 | The repeated header is a data row with the unknown type `type`, so it is invalid and the run continues. `15 / 0 / 15 / false`. | Integration | P2 |
| A46 | G1 to G3, R3 | `c1` deposit tx1 10; `c1` withdrawal tx2 1; `c1` deposit tx3 1; `c1` dispute tx3; `c1` chargeback tx3; `c1` withdrawal tx4 50 (rejected); then `c2` dispute tx2; `c2` resolve tx1; `c2` dispute tx3; `c2` dispute tx4 | Every `c2` row is ignored. The first three report `client_mismatch` naming `c1`, not the withdrawal, "not disputed", or "charged back" reasons, so `c2` learns nothing about the kind or state of `c1`'s transactions. tx4 is a rejected withdrawal and reports `unknown_tx`. `c1`: `9 / 0 / 9 / true`; `c2` has no account. | Unit | P3 |

### Invariants

| ID | Rule | Invariant | Test | Priority |
|----|------|-----------|------|----------|
| I1 | all | `total = available + held` for every account after every row | Type, Property | P1 |
| I2 | R6, R9 | `locked` never goes from `true` back to `false` | Type, Property | P1 |
| I3 | R3 to R8, V6 | A rejected or ignored row leaves every balance and every transaction state unchanged. An ignored row, including an invalid one, has no effect at all. A rejected withdrawal has exactly two effects: it creates a zero-balance account for an unseen client (R8) and it binds its `tx` ID (V6). | Unit, Property | P1 |
| I4 | R2, R4, R5 | `held` equals the sum of the amounts of the deposits currently in `Disputed` | Property | P1 |
| I5 | R1 | Only deposits ever reach `Disputed`, `Resolved`, or `ChargedBack` | Type, Property | P1 |
| I6 | M2, M7 | No balance ever overflows silently: every change to `available`, `held`, or `total` is checked, and a row whose effect would overflow leaves every account unchanged | Unit, Property | P1 |
| I7 | M1, M4, M5 | Every stored amount is strictly positive and equals exactly the value in the input; no amount is ever rounded or truncated | Unit, Property | P1 |
| I8 | L3 | No withdrawal is ever applied to an account whose `locked` is `true` | Property | P1 |
| I9 | L2, L5 | A `deposit`, `dispute`, `resolve`, or `chargeback` row has the same effect on balances and transaction states whether or not the account is locked | Property | P1 |
| I10 | O1 to O10 | The output is a function of the input alone: the same input always produces byte-identical stdout, and every printed `total` equals the printed `available + held` | CLI, Property | P1 |
| I11 | V6 | Each `tx` ID is bound to at most one `deposit` or `withdrawal` for the whole run; once bound, no later row changes its client, amount, or type | Unit, Property | P1 |
| I12 | V7, V10, V13 | Every run ends with exit `0` and the full output CSV, or with one fatal exit code and an empty stdout (except a failed stdout write); no input makes the run panic | Property, CLI | P2 |
| I13 | R2, R4 | `held` is never negative for any account after any row | Property | P1 |
| I14 | C5 | For every client, `total` equals the sum of that client's applied deposits, minus its applied withdrawals, minus its charged-back deposits | Property | P1 |
| I15 | C5 | Money is conserved overall: the sum of `total` over all accounts equals the sum of all applied deposits, minus all applied withdrawals, minus all charged-back deposits | Property | P1 |
| I16 | R1 to R7, S11 | A deposit's dispute state only moves along `Processed → Disputed`, `Disputed → Resolved`, `Resolved → Disputed`, and `Disputed → ChargedBack`. `ChargedBack` never changes, and every other (state, row type) pair leaves the state as it was | Unit, Property | P1 |
| I17 | R8, O2, O3 | The output has exactly one row per account, in ascending client order, and the accounts are exactly the clients with at least one applied or rejected `deposit` or `withdrawal` | Integration, Property | P1 |
| I18 | C6, V6 | Per-client projection: if no `tx` ID is used by `deposit` or `withdrawal` rows of two different clients, each client's output row is the same as its row in a run on only that client's rows, and a client has a row in one run exactly when it has one in the other. So any reordering that keeps each client's rows in their relative order gives byte-identical stdout. stderr is not covered. When two clients' rows share an ID, the first one in the file owns it (A27, A41) | Property | P1 |

## 7. Given assumptions

- One asset account per client; every operation targets that account.
- Clients are identified only by their `u16` ID. No other profile data.
- When a rule is ambiguous, choose the behavior a bank or ATM processor would choose, and document it as an ADR.

## 8. Quality goals

Listed in the order reviewers will weigh them when the output is already correct.

1. **Correct output** for the CLI contract above. Prove it with sample CSVs committed to the repo and unit tests for the non-trivial logic; use the type system to rule out invalid states where practical. The test layers and sample-case layout are in [ADR-0014](decisions/0014-testing-strategy.md).
2. **Completeness**: cover all five operation types.
3. **Safety and robustness**: no panics on bad input, explicit error handling, and a written justification for anything risky (for example `unsafe`, `unwrap`, or lossy numeric conversions).
4. **Resource use**: stream the input instead of loading it all into memory. Transaction IDs span the full `u32` range, so per-transaction state must be sized with that in mind. Keep the design compatible with a future where the engine runs inside a server processing many concurrent input streams.
5. **Maintainability**: clean, readable code is preferred over clever or micro-optimized code. Reviewers read the code without the author present, so the README must explain the design, the assumptions, and how correctness was verified.

## 9. Deliverable constraints

- A plain crate created with `cargo new` / `cargo init`, buildable with `cargo build` and runnable with `cargo run`.
- Published as a public GitHub repository.
- Dependencies limited to common, well-maintained crates that we consider secure (for example `serde` and `csv`). Every dependency must have a stated reason, recorded in an ADR, covering why it is needed and why it is trusted (maintenance, adoption, no `unsafe`-heavy or unvetted transitive dependencies). The dependency list, the lint policy that enforces §8, and the supply-chain checks are in [ADR-0012](decisions/0012-dependencies-and-safety-policy.md).

## 10. Open questions

Not answered by the brief. Each must be decided in an ADR before or while implementing it.

None are open. Repeated `tx` IDs and malformed rows are decided by [ADR-0010](decisions/0010-input-validation-and-errors.md).
