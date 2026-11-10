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

Each client has exactly one account, created the first time the client appears in the input.

Invariants that must hold after every operation:

- `total = available + held`
- `locked` becomes `true` after a chargeback and never reverts.

## 5. Operations

| Type         | References | Effect when valid |
|--------------|------------|-------------------|
| `deposit`    | new `tx` with amount | `available += amount`, `total += amount` |
| `withdrawal` | new `tx` with amount | `available -= amount`, `total -= amount`. **Rejected** if `available < amount`; balances stay unchanged |
| `dispute`    | existing `tx` | Moves the referenced transaction's amount from `available` to `held`. `total` unchanged |
| `resolve`    | disputed `tx` | Moves the amount back from `held` to `available`. `total` unchanged. Dispute ends |
| `chargeback` | disputed `tx` | `held -= amount`, `total -= amount`. Account becomes locked. Dispute ends |

`dispute`, `resolve`, and `chargeback` carry no amount of their own; they always use the amount of the transaction they reference.

### Rows to ignore

These are treated as upstream errors and skipped without failing the run:

- `dispute` whose `tx` does not exist.
- `resolve` or `chargeback` whose `tx` does not exist or is not currently under dispute.

## 6. Threat model and invariants

Card and bank transfers can be reversed after the fact, which is good for consumers but exposes the processor to fraud. The motivating attack:

1. Deposit funds through a reversible payment method.
2. Spend or withdraw those funds immediately.
3. Dispute the original deposit and get it charged back, keeping what was withdrawn.

The engine must behave like a cautious bank under this and similar abuse: it must never create money, never lose track of held funds, and must freeze an account once a chargeback happens.

This section is a living catalog. It will grow with more attack scenarios and invariants as decisions are made; each entry must be backed by at least one test (unit, sample CSV, or property-based).

### Attack scenarios

| ID | Scenario | Expected behavior |
|----|----------|-------------------|
| A1 | Deposit, withdraw, then dispute and charge back the deposit | Decided in an ADR (see open questions on disputes driving `available` negative) |

### Invariants

| ID | Invariant |
|----|-----------|
| I1 | `total = available + held` for every account after every row |
| I2 | `locked` never goes from `true` back to `false` |
| I3 | A rejected or ignored row leaves every account unchanged |

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

- Can a withdrawal be disputed, and if so, what happens to `available` and `held`?
- What happens when a dispute, resolve, or chargeback references a `tx` that belongs to a different client?
- Which operations, if any, are allowed on a locked account?
- How to handle a repeated `tx` ID on a deposit or withdrawal.
- Can a transaction be disputed again after it was resolved?
- Can a dispute drive `available` negative (funds already withdrawn)?
- How to handle malformed rows: skip and log to stderr, or abort?
- Rounding or rejection of amounts with more than 4 fractional digits, and of zero or negative amounts.
