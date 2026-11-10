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

Follow [ADR-0010](decisions/0010-input-validation-and-errors.md) V10. A fatal error writes one line to stderr and leaves stdout empty (a failed stdout write may leave partial output).

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
- A UTF-8 byte order mark before the header, standard CSV quoting, and empty lines.

Validation follows [ADR-0010](decisions/0010-input-validation-and-errors.md):

- The header must be exactly `type,client,tx,amount` after trimming, case-sensitive. A missing or different header is fatal (exit `65`).
- A record longer than 4 KiB is fatal (exit `65`); memory per record is bounded.
- A data row must have 4 fields, or 3 for `dispute`, `resolve`, and `chargeback`.
- `type` is matched exactly and case-sensitively: `Deposit` is unknown.
- `client` and `tx` are ASCII digits only, within `u16` and `u32`. A sign (`+7`) makes the row invalid; leading zeros are accepted.
- A `deposit` or `withdrawal` needs a non-empty amount. On a `dispute`, `resolve`, or `chargeback`, any amount is ignored (not parsed or reported) and the row is processed normally.
- A row that fails any of these checks is **invalid**: it is skipped with a diagnostic and the run continues (see §5).

Amounts follow [ADR-0007](decisions/0007-money-representation.md):

- Stored as exact fixed-point integers in units of `0.0001`. No floating point is used, and no amount is ever rounded or truncated.
- Valid form: ASCII digits, optionally followed by `.` and more digits. Extra fractional digits beyond the fourth are accepted only if they are zeros (`1.23450`); leading zeros are accepted (`007.5`).
- Valid range: `0.0001` to `1844674407370955.1615`.
- Anything else makes the `deposit` or `withdrawal` row invalid: a sign (`-5`, `+5`), `.5`, `5.`, exponents (`1e3`), separators (`1_000`, `1,000`), zero, more than 4 non-zero fractional digits, or a value above the range. An invalid row never changes state and never creates an account. It is reported like any other skipped row ([ADR-0010](decisions/0010-input-validation-and-errors.md) V1, V11).

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
- Any invalid row (§2): wrong field count, unknown or miscased `type`, bad `client` or `tx`, or a missing or invalid amount on a `deposit` or `withdrawal` ([ADR-0010](decisions/0010-input-validation-and-errors.md) V1 to V5).
- `deposit` or `withdrawal` whose `tx` ID was already used by an earlier `deposit` or `withdrawal` that was applied or rejected, whatever its client, type, or amount. Invalid and ignored rows never use up an ID ([ADR-0010](decisions/0010-input-validation-and-errors.md) V6).

Diagnostics escape any echoed input, so every entry is exactly one line ([ADR-0010](decisions/0010-input-validation-and-errors.md) V11).

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

This section is a living catalog. It will grow with more attack scenarios and invariants as decisions are made; each entry must be backed by at least one test (unit, sample CSV, or property-based). Entries are acceptance criteria for the implementation until those tests exist.

### Attack scenarios

The `Rule` column refers to the rules R1 to R10 of [ADR-0006](decisions/0006-dispute-lifecycle.md), M1 to M8 of [ADR-0007](decisions/0007-money-representation.md), L1 to L7 of [ADR-0008](decisions/0008-locked-account-policy.md), O1 to O10 of [ADR-0009](decisions/0009-output-format.md), and V1 to V13 of [ADR-0010](decisions/0010-input-validation-and-errors.md). Rows are listed in input order; `c1`/`c2` are client IDs. Results are `available / held / total / locked`.

| ID | Rule | Scenario (input rows) | Expected behavior |
|----|------|-----------------------|-------------------|
| A1 | R2 | `c1` deposit tx1 100; `c1` withdrawal tx2 100; `c1` dispute tx1; `c1` chargeback tx1 | After the dispute: `-100 / 100 / 0 / false`. After the chargeback: `-100 / 0 / -100 / true`. The debt stays visible. |
| A2 | R2 | `c1` deposit tx1 100; `c1` withdrawal tx2 100; `c1` dispute tx1; `c1` withdrawal tx3 10; `c1` resolve tx1 | The withdrawal is rejected while `available` is negative (`-100 / 100 / 0 / false`). The resolve restores `0 / 0 / 0 / false`. |
| A3 | R1 | `c1` deposit tx1 100; `c1` withdrawal tx2 40; `c1` dispute tx2; `c1` resolve tx2; `c1` chargeback tx2 | All three rows on the withdrawal are ignored. Result: `60 / 0 / 60 / false`. |
| A4 | R3 | `c1` deposit tx1 10; `c2` dispute tx1 | Ignored with a log naming `c1` as owner. `c1`: `10 / 0 / 10 / false`. `c2` has no account. |
| A5 | R3 | `c1` deposit tx1 10; `c1` dispute tx1; `c2` resolve tx1; `c2` chargeback tx1 | Both `c2` rows are ignored. `c1` stays disputed: `0 / 10 / 10 / false`. |
| A6 | R8 | `c9` dispute tx7 | Ignored. No account for `c9`; the output has only the header. |
| A7 | R4 | `c1` deposit tx1 10; `c1` dispute tx1; `c1` dispute tx1 | The second dispute is ignored. `0 / 10 / 10 / false`; the deposit is held once. |
| A8 | R5 | `c1` deposit tx1 10; `c1` dispute tx1; `c1` resolve tx1; `c1` dispute tx1; `c1` chargeback tx1 | The second dispute opens a new cycle (`0 / 10 / 10 / false`) and the chargeback applies. `0 / 0 / 0 / true`. |
| A9 | R6 | `c1` deposit tx1 10; `c1` deposit tx2 5; `c1` dispute tx1; `c1` chargeback tx1; `c1` chargeback tx1; `c1` dispute tx1; `c1` resolve tx1 | Only the first chargeback applies. `5 / 0 / 5 / true`. |
| A10 | R7 | `c1` deposit tx1 5; `c1` withdrawal tx2 10 (rejected); `c1` dispute tx2; `c1` dispute tx99; `c1` resolve tx1; `c1` chargeback tx1 | All dispute-related rows are ignored (rejected withdrawal, unknown `tx`, deposit not under dispute). `5 / 0 / 5 / false`. |
| A11 | R9 | `c1` deposit tx1 10; `c1` deposit tx2 5; `c1` dispute tx1; `c1` chargeback tx1; `c1` deposit tx3 20; `c1` withdrawal tx4 5 | Locked after the chargeback (`5 / 0 / 5 / true`). The deposit is accepted and the withdrawal is rejected. `25 / 0 / 25 / true`. |
| A12 | R9 | `c1` deposit tx1 10; `c1` deposit tx2 10; `c1` dispute tx1; `c1` dispute tx2; `c1` chargeback tx1; `c1` resolve tx2 | Dispute-related rows still work on a locked account. After the chargeback: `0 / 10 / 10 / true`. After the resolve: `10 / 0 / 10 / true`. |
| A13 | R2, R9 | A1 rows, then `c1` deposit tx3 150; `c1` withdrawal tx4 10 | The deposit repays the debt on the locked account: `50 / 0 / 50 / true`. The withdrawal is rejected and the lock stays. |
| A14 | R2, R5 | `c1` deposit tx1 100; `c1` dispute tx1; `c1` resolve tx1; `c1` withdrawal tx2 100; `c1` dispute tx1; `c1` chargeback tx1 | After the resolve and withdrawal: `0 / 0 / 0 / false`. The second dispute is recorded: `-100 / 100 / 0 / false`. The chargeback records the debt and locks: `-100 / 0 / -100 / true`. |
| A15 | R10 | `c1` deposit tx1 10, then 1,000,000 rows that are each ignored (for example `c2` dispute tx1) | `c1` stays `10 / 0 / 10 / false`; no account for `c2`. stderr has at most 100 detailed entries, one suppression notice, and a summary that counts all 1,000,000 ignored rows. |
| A16 | R10 | Any A1 to A14 input, run with stderr closed or failing on write | The run completes without a panic, stdout has the same CSV as with a working stderr, and the exit code is unchanged. This covers closed or failing stderr only; a reader that stops consuming a pipe can block the run and is an accepted limitation. |
| A17 | M3, M6 | `c1` deposit tx1 10; `c1` withdrawal tx2 -5; `c2` withdrawal tx3 -5 | Both withdrawals are invalid, so a negative withdrawal cannot create money. `c1`: `10 / 0 / 10 / false`. `c2` has no account. |
| A18 | M3, M6 | `c1` deposit tx1 5; `c1` deposit tx2 -10; `c1` withdrawal tx3 10 | The negative deposit is invalid and cannot bypass the funds check, so the withdrawal is rejected for insufficient funds. `5 / 0 / 5 / false`. |
| A19 | M4 | `c1` deposit tx1 1.00005; `c1` deposit tx2 1.23450 | tx1 is invalid and is never rounded. tx2 is accepted as `1.2345`. `1.2345 / 0 / 1.2345 / false`. |
| A20 | M5, M6 | `c1` deposit tx1 0; `c2` withdrawal tx2 0.0000 | Both rows are invalid. No accounts; the output has only the header. |
| A21 | M2, M5 | `c1` deposit tx1 1844674407370955.1615; `c1` deposit tx2 1844674407370955.1615; `c1` deposit tx3 1844674407370955.1616 | tx1 and tx2 are accepted; their sum does not overflow the balance. tx3 is above the range and invalid. `3689348814741910.3230 / 0 / 3689348814741910.3230 / false`. |
| A22 | M3 | `c1` deposits with amounts `.5`, `5.`, `+5`, `1e3`, `1_000`, and `" 2.5 "` (tx1 to tx6) | Only tx6 is accepted, after trimming the whitespace. `2.5 / 0 / 2.5 / false`. |
| A23 | L5, R2 | `c1` deposit tx1 100; `c1` deposit tx2 100; `c1` deposit tx3 100; `c1` withdrawal tx4 300; `c1` dispute tx1; `c1` dispute tx2; `c1` dispute tx3; `c1` chargeback tx1; `c1` chargeback tx2; `c1` chargeback tx3 | Several disputed deposits: the first chargeback locks (`-300 / 200 / -100 / true`) and the later ones still apply, so the full debt shows. `-300 / 0 / -300 / true`. |
| A24 | L1, L2, L5 | `c1` deposit tx1 10; `c1` dispute tx1; `c1` chargeback tx1; `c1` deposit tx2 50; `c1` dispute tx2; `c1` chargeback tx2 | Locked at `0 / 0 / 0 / true`. The deposit after the lock is applied (`50 / 0 / 50 / true`) and stays disputable (`0 / 50 / 50 / true`). Its chargeback returns the account to `0 / 0 / 0 / true`; the lock stays. |
| A25 | L3, L4, L7, R7 | `c1` deposit tx1 10; `c1` deposit tx2 5; `c1` dispute tx1; `c1` chargeback tx1; `c1` withdrawal tx3 100; `c1` withdrawal tx4 1; `c1` dispute tx3 | Locked at `5 / 0 / 5 / true`. Both withdrawals are rejected with the reason "account locked", including tx3, which also lacks funds. The dispute on tx3 is ignored because a rejected withdrawal is never stored. `5 / 0 / 5 / true`. |
| A26 | O1 to O9 | The example input in §3 | stdout is exactly the example output in §3, byte for byte: sorted rows, fixed 4-digit amounts, `-0.5000` for client 1, a zero-balance row for client 3, and no row for client 4. |
| A27 | V6 | `c1` deposit tx1 10; `c2` deposit tx1 50; `c2` dispute tx1 | The second deposit is a duplicate and is ignored, so `c2` cannot take over tx1. The dispute is ignored with `c1` as owner. `c1`: `10 / 0 / 10 / false`. `c2` has no account. |
| A28 | V6, R6 | `c1` deposit tx1 10; `c1` dispute tx1; `c1` chargeback tx1; `c1` deposit tx1 10; `c1` dispute tx1 | The repeated deposit is ignored, so the charged-back deposit is not reset. The last dispute is ignored under R6. `0 / 0 / 0 / true`. |
| A29 | V6 | `c1` deposit tx1 5; `c1` withdrawal tx2 10 (rejected); `c1` deposit tx3 10; `c1` withdrawal tx2 10 | The rejected withdrawal binds tx2, so a later withdrawal reusing tx2 is ignored as a duplicate and cannot succeed after the deposit. `15 / 0 / 15 / false`. |
| A30 | V6, M6 | `c1` deposit tx1 `abc`; `c1` deposit tx1 5 | The invalid row does not bind tx1, so the corrected row is applied. `5 / 0 / 5 / false`. |
| A31 | V1 to V5 | `c1` deposit tx1 10, then invalid rows: `Deposit,1,2,5`; `deposit,+1,3,5`; `deposit,65536,4,5`; `deposit,1,4294967296,5`; `deposit,1,5`; `deposit,1,6,`; `deposit,1,7,5,`; then `c1` deposit tx8 1 | Every invalid row is skipped with its own reason and the run continues. `c1`: `11 / 0 / 11 / false`; no other account. |
| A32 | V11 | `c1` deposit tx1 10; a row whose `type` is a quoted field holding a newline and an ANSI escape sequence | The row is invalid. Its diagnostic is a single line with the newline and escape written as escapes. `c1`: `10 / 0 / 10 / false`. |
| A33 | V7 to V9 | (a) an empty file; (b) header `type,client,amount,tx`; (c) a valid header followed by `deposit,1,1,"5` and 10 KiB of further rows | Each run exits `65` with one error line on stderr and nothing on stdout. In (c) the unbalanced quote hits the record limit; memory stays bounded. |
| A34 | V7, V10 | (a) no argument; (b) two arguments; (c) a path that does not exist; (d) a valid input with stdout closed | (a) and (b) exit `64`, (c) exits `66`, each with empty stdout. (d) exits `74` without a panic. |
| A35 | V5, R2 | `c1` deposit tx1 10; `c1` dispute tx1 with amount `3`; `c1` resolve tx1 with amount `abc`; `c1` dispute tx1 with amount `-50`; `c1` chargeback tx1 with amount `10` | Every amount is ignored and every row applies to the full deposit. After the first dispute: `0 / 10 / 10 / false`. After the resolve: `10 / 0 / 10 / false`. After the chargeback: `0 / 0 / 0 / true`. |

### Invariants

| ID | Rule | Invariant |
|----|------|-----------|
| I1 | all | `total = available + held` for every account after every row |
| I2 | R6, R9 | `locked` never goes from `true` back to `false` |
| I3 | R3 to R8, V6 | A rejected or ignored row leaves every balance and every transaction state unchanged. An ignored row, including an invalid one, has no effect at all. A rejected withdrawal has exactly two effects: it creates a zero-balance account for an unseen client (R8) and it binds its `tx` ID (V6). |
| I4 | R2, R4, R5 | `held` equals the sum of the amounts of the deposits currently in `Disputed` |
| I5 | R1 | Only deposits ever reach `Disputed`, `Resolved`, or `ChargedBack` |
| I6 | M2, M7 | No balance ever overflows silently: every change to `available`, `held`, or `total` is checked, and a row whose effect would overflow leaves every account unchanged |
| I7 | M1, M4, M5 | Every stored amount is strictly positive and equals exactly the value in the input; no amount is ever rounded or truncated |
| I8 | L3 | No withdrawal is ever applied to an account whose `locked` is `true` |
| I9 | L2, L5 | A `deposit`, `dispute`, `resolve`, or `chargeback` row has the same effect on balances and transaction states whether or not the account is locked |
| I10 | O1 to O10 | The output is a function of the input alone: the same input always produces byte-identical stdout, and every printed `total` equals the printed `available + held` |
| I11 | V6 | Each `tx` ID is bound to at most one `deposit` or `withdrawal` for the whole run; once bound, no later row changes its client, amount, or type |
| I12 | V7, V10, V13 | Every run ends with exit `0` and the full output CSV, or with one fatal exit code and an empty stdout (except a failed stdout write); no input makes the run panic |

## 7. Given assumptions

- One asset account per client; every operation targets that account.
- Clients are identified only by their `u16` ID. No other profile data.
- When a rule is ambiguous, choose the behavior a bank or ATM processor would choose, and document it as an ADR.

## 8. Quality goals

Listed in the order reviewers will weigh them when the output is already correct.

1. **Correct output** for the CLI contract above. Prove it with sample CSVs committed to the repo and unit tests for the non-trivial logic; use the type system to rule out invalid states where practical.
2. **Completeness**: cover all five operation types.
3. **Safety and robustness**: no panics on bad input, explicit error handling, and a written justification for anything risky (for example `unsafe`, `unwrap`, or lossy numeric conversions).
4. **Resource use**: stream the input instead of loading it all into memory. Transaction IDs span the full `u32` range, so per-transaction state must be sized with that in mind. Keep the design compatible with a future where the engine runs inside a server processing many concurrent input streams.
5. **Maintainability**: clean, readable code is preferred over clever or micro-optimized code. Reviewers read the code without the author present, so the README must explain the design, the assumptions, and how correctness was verified.

## 9. Deliverable constraints

- A plain crate created with `cargo new` / `cargo init`, buildable with `cargo build` and runnable with `cargo run`.
- Published as a public GitHub repository.
- Dependencies limited to common, well-maintained crates that we consider secure (for example `serde` and `csv`). Every dependency must have a stated reason, recorded in an ADR, covering why it is needed and why it is trusted (maintenance, adoption, no `unsafe`-heavy or unvetted transitive dependencies).

## 10. Open questions

Not answered by the brief. Each must be decided in an ADR before or while implementing it.

None are open. Repeated `tx` IDs and malformed rows are decided by [ADR-0010](decisions/0010-input-validation-and-errors.md).
