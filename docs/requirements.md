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
| `amount` | decimal | Up to 4 fractional digits. Present for `deposit` and `withdrawal`; empty or missing for the other three types |

Parsing must tolerate:

- Whitespace around fields and headers.
- Any decimal precision up to 4 fractional digits (`1`, `1.0`, `1.2345`).

Ordering:

- Rows are in chronological order; a row happens after every row above it.
- Client IDs and transaction IDs are **not** sorted. Clients can appear in any order and interleave.

## 3. Output

CSV with the columns `client, available, held, total, locked`, one row per client seen in the input.

- Amounts are printed with 4 fractional digits of precision.
- Row order, whitespace, and trailing zeros on round values are not significant to the checker.

Example:

```
client,available,held,total,locked
1,1.5,0,1.5,false
2,2,0,2,false
```

## 4. Account model

Each client has exactly one account, created by the first `deposit` or `withdrawal` row for that client, even if the withdrawal is rejected. Rows that are ignored never create an account ([ADR-0006](decisions/0006-dispute-lifecycle.md)).

Invariants that must hold after every operation:

- `total = available + held`
- `locked` becomes `true` after a chargeback and never reverts.
- `held` equals the sum of the amounts of the deposits currently under dispute.
- `available` and `total` may be negative: a dispute or chargeback on funds that were already withdrawn leaves the debt visible ([ADR-0006](decisions/0006-dispute-lifecycle.md)).

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

These are treated as upstream errors and skipped without failing the run. "Skipped" means no state change, no balance change, no account created, and a diagnostic on stderr. Diagnostics are bounded: full detail (row type, client, `tx`, owning client when known, reason, input line) for the first 100 rows, then only an end-of-run summary with one count per reason. A failure to write stderr is ignored and never affects stdout, account state, or the exit code (ADR-0006, R10).

- `dispute`, `resolve`, or `chargeback` whose `tx` does not exist or was never stored (for example a rejected withdrawal).
- `dispute`, `resolve`, or `chargeback` on a withdrawal.
- `dispute`, `resolve`, or `chargeback` whose `client` differs from the client that owns the `tx`.
- `dispute` on a `tx` that is already under dispute or already charged back. A dispute on a resolved `tx` is not ignored; it opens a new cycle.
- `resolve` or `chargeback` whose `tx` is not currently under dispute.
- `withdrawal` on a locked account (rejected; balances unchanged). Deposits on a locked account are still accepted, so the client can repay a debt.

Only a `deposit` or `withdrawal` row creates a client account, including a rejected withdrawal. Ignored rows never do.

## 6. Threat model and invariants

Card and bank transfers can be reversed after the fact, which is good for consumers but exposes the processor to fraud. The motivating attack:

1. Deposit funds through a reversible payment method.
2. Spend or withdraw those funds immediately.
3. Dispute the original deposit and get it charged back, keeping what was withdrawn.

The engine must behave like a cautious bank under this and similar abuse: it must never create money, never lose track of held funds, and must freeze an account once a chargeback happens.

### Engine boundary

The engine applies accounting and withdrawal rules to the supplied events. It cannot determine whether a deposit is irrevocable or safe to withdraw. The full split is in [ADR-0006](decisions/0006-dispute-lifecycle.md#engine-boundary-and-residual-risk):

- **Enforced by the engine:** the invariants below, ownership checks, full-amount holds with visible debt, withdrawal rejection when funds are insufficient and permanently after a chargeback, and bounded diagnostics.
- **Required from upstream (deployment prerequisite):** only deposits that are irrevocable, or fully backed by enforceable collateral or a funding guarantee, are admitted; events are authoritative, chronologically ordered, and deduplicated. Settlement or a fixed waiting period alone is not proof that a payment cannot be reversed.
- **Residual risk accepted for this scope:** without upstream protection, the attack above remains a financial exposure. Recording a negative balance after a reversal does not prevent the initial loss: a client can withdraw a reversible deposit before the dispute arrives, and the later lock cannot recover funds already paid out. A deposit accepted on a locked account is not final recovery if it can itself be reversed.

Enforcing withdrawal eligibility inside the engine would need additional input information and a separate ADR.

This section is a living catalog. It will grow with more attack scenarios and invariants as decisions are made; each entry must be backed by at least one test (unit, sample CSV, or property-based). Entries are acceptance criteria for the implementation until those tests exist.

### Attack scenarios

The `Rule` column refers to the rules R1 to R10 of [ADR-0006](decisions/0006-dispute-lifecycle.md). Rows are listed in input order; `c1`/`c2` are client IDs. Results are `available / held / total / locked`.

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
| A16 | R10 | Any A1 to A14 input, run with stderr closed or failing on write | The run completes without a panic, stdout has the same CSV as with a working stderr, and the exit code is unchanged. |

### Invariants

| ID | Rule | Invariant |
|----|------|-----------|
| I1 | all | `total = available + held` for every account after every row |
| I2 | R6, R9 | `locked` never goes from `true` back to `false` |
| I3 | R3 to R8 | A rejected or ignored row leaves every account and every transaction state unchanged. The only exception is that a rejected withdrawal still creates a zero-balance account for an unseen client (R8). |
| I4 | R2, R4, R5 | `held` equals the sum of the amounts of the deposits currently in `Disputed` |
| I5 | R1 | Only deposits ever reach `Disputed`, `Resolved`, or `ChargedBack` |

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

- How to handle a repeated `tx` ID on a deposit or withdrawal.
- How to handle malformed rows: skip and log to stderr, or abort?
- Rounding or rejection of amounts with more than 4 fractional digits, and of zero or negative amounts.
