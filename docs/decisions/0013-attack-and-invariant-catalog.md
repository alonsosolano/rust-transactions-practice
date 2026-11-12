---
status: accepted
date: 2026-11-11
decision-makers: Alonso Solano
---

# Make the attack and invariant catalog executable: test types, priorities, tests-only checks, and a per-client ordering invariant

## Context and Problem Statement

[Requirements §6](../requirements.md) lists the attack scenarios and invariants the engine must satisfy. ADR-0006 to ADR-0012 each added rows, but no row said how it would be verified, the rows had no order of importance, and some abuse cases and invariants were still missing: replays of a resolve, withdrawals while funds are held, header tricks, boundary IDs, money conservation, legal state transitions, and how the result depends on the order of rows from different clients. The implementation issue for these tests (E6) must be able to work straight from the table.

How is each catalog row verified, which rows come first, are invariants also checked while the engine runs, and what exactly does the engine promise about row order across clients?

## Decision Drivers

* Every rule must be proven by a test, not claimed ([requirements §8](../requirements.md)). A row without a named test type tends to stay untested.
* Correct stdout through the fixed CLI is the top priority, because the automated gate compares it first ([requirements §1](../requirements.md)).
* No input may make the run panic, and the run fails closed ([ADR-0010](0010-input-validation-and-errors.md) V10, [ADR-0012](0012-dependencies-and-safety-policy.md) S6).
* Debug and release builds should behave the same, so what the tests observe is what ships.
* The ordering promise must hold in the CLI today and in the sharded server of [ADR-0011](0011-architecture-and-memory.md) stage 2, where order is only kept per client.
* Follow what ledgers, event streams, and testing practice do (see [More Information](#more-information)).

## Considered Options

The choices below were decided one by one. Each lists the options considered and the chosen one.

1. **Test-type vocabulary:** five types (unit, integration, CLI, property, type), or three (unit, sample CSV, property).
2. **Priority:** three levels by risk, or no priority.
3. **Runtime invariant checks:** tests only, `debug_assert!` for cheap invariants, or fail closed at run time in release builds.
4. **Money conservation:** per client, overall, or both.
5. **Order across clients:** a per-client projection invariant, a reorder invariant only, or no invariant beyond determinism.

## Decision Outcome

Chosen options: five test types, three risk-based priorities, invariants checked by tests only, money conservation per client and overall, and a per-client projection invariant. Together they turn every row of the catalog into a test that E6 can write directly, in order of risk.

### Rules

| # | Rule | One-line justification |
|---|------|------------------------|
| C1 | Every attack scenario and invariant in [requirements §6](../requirements.md) names **at least one test type and one priority**. No row is "to be decided". A row is done only when a test that names its ID exists. | E6 can be executed from the table, and a search for an ID shows its test. |
| C2 | The test types are **Unit** (a domain module on typed values, no I/O, [ADR-0011](0011-architecture-and-memory.md) S2), **Integration** (the library's `run` on in-memory bytes and writers, S1), **CLI** (the real binary on a committed sample file, checking the exit code, stdout, and stderr), **Property** (`proptest` over generated row streams, checking after every row), and **Type** (holds by construction, as listed in ADR-0011's type-system guarantees). Type never stands alone: the row also names a test that would catch a regression. Where each type lives and how sample files are laid out is decided by the testing strategy (D9). | These are the layers ADR-0011 and ADR-0012 already describe, and they follow the Rust Book's split between unit and integration tests. Exit codes and closed streams can only be observed through the binary, so CLI needs its own name. A type guarantee can be weakened by a refactor without any compile error, so it still needs a test. |
| C3 | Priorities: **P1** when a failure could create, lose, or release money, break the lock or the ownership of a `tx` ID, or change the stdout the automated gate compares; **P2** when it could accept input that must be refused, misreport a fatal error, or panic; **P3** when it affects only diagnostics on stderr. Priority orders the work; every row is still required. | Risk-based test prioritization: the rows whose failure costs money or fails the gate are built and reviewed first. |
| C4 | Invariants are **checked by tests only**. Non-test code has no `assert!`, `debug_assert!`, or other runtime check for them. A test-only `assert_invariants` helper checks every invariant that can be read from the engine's state (I1, I2, I4, I13, I14, I15) after every row of a property test and after every step of a unit scenario. Checked arithmetic that returns a rejection ([ADR-0007](0007-money-representation.md) M7) is error handling, not an invariant check, and stays. | I1, I2, I5, and I6 already hold by construction (ADR-0011 S8, S9 and ADR-0007 M7). I4 needs a scan of every stored transaction, too slow for every row. A failed assertion panics (exit `101`), which ADR-0010 V10 treats as a bug, and `debug_assert!` would make debug and release builds behave differently. |
| C5 | Money conservation is stated **per client** (I14) and **overall** (I15): `total` equals applied deposits minus applied withdrawals minus charged-back deposits, for each client and summed over all accounts. Tests compute the expected sums from the rows the engine reported as applied (`apply` returned `Ok`), using the amount of the referenced deposit for a chargeback. | The overall form is the ledger's conservation check; the per-client form says which account is wrong when it fails. Disputes and resolves move money between `available` and `held` only, so neither changes `total`. |
| C6 | Order across clients is stated as a **per-client projection** (I18). If no `tx` ID is used by `deposit` or `withdrawal` rows of two different clients, each client's output row is the same as in a run on only that client's rows. Any reordering that keeps each client's rows in their relative order therefore gives byte-identical stdout. When two clients' rows share an ID, the first one in the file owns it ([ADR-0010](0010-input-validation-and-errors.md) V6), as A27 and A41 show in both orders. stderr is not covered. | Accounts never move money to each other, so the only rule that couples clients is the shared `tx` ID namespace. Cross-client dispute rows are ignored in either position (unknown `tx` before the deposit, wrong owner after it). Event streams guarantee order per key only, and ADR-0011 stage 2 already routes by client. |
| C7 | A later decision that adds or changes a rule adds or updates rows in [requirements §6](../requirements.md) **in the same change**, with a test type and a priority. | The catalog stays complete without a separate catch-up issue. |

### Where order across clients matters

Under the accepted ADRs, rows of different clients interact in exactly three places:

| Interaction | Rule | Effect of moving one client's rows ahead of another's |
|-------------|------|-------------------------------------------------------|
| Two clients' `deposit` or `withdrawal` rows use the same `tx` ID | ADR-0010 V6 | **stdout changes**: the first row in the file owns the ID, and the other is ignored as a duplicate. This is the only case, and I18 excludes it. |
| A `dispute`, `resolve`, or `chargeback` names another client's `tx` | ADR-0006 R3 | stdout is unchanged: the row is ignored as an unknown `tx` before the deposit and as a wrong owner after it. Only the diagnostic reason differs. |
| Which rows get detailed diagnostics | ADR-0006 R10 | stdout is unchanged. The first 100 skipped rows in file order get detail, so stderr can differ. |

Everything else in an account depends only on that client's rows: balances, the lock, and the dispute states of the deposits the client owns.

### Consequences

* Good, because every row has a test type and a priority, so E6 can be planned and checked by searching for IDs.
* Good, because the money invariants (I14, I15) catch any rule that creates or loses money, even one no scenario anticipated.
* Good, because I18 is the property that makes ADR-0011's sharding by client sound, and it is tested before the server exists.
* Good, because release and debug builds behave the same, and the no-panic policy has no exceptions for assertions.
* Bad, because a broken invariant in production is not detected at run time. Accepted: the invariants that can be enforced cheaply already hold by construction, and property tests check the rest after every row.
* Bad, because I18 says nothing about inputs that reuse a `tx` ID across clients. Accepted: under V6 the file order is the only authority, and A27 and A41 fix the outcome in both orders.
* Neutral, because priorities order the work but do not make any row optional.
* Neutral, because D9 still decides where each test type lives and how sample files are laid out; this ADR only names the types.

### Confirmation

* Every row of [requirements §6](../requirements.md) has a non-empty Test and Priority column, and a search for each ID finds at least one test once E6 is done.
* Review check: non-test code contains no `assert!` or `debug_assert!`.
* The property test for I18 generates streams with a separate ID pool per client, including dispute rows that name other clients' IDs. It compares stdout of the full run with the per-client runs merged by client, and with a shuffle that keeps each client's order.
* The property test for I14 and I15 sums the rows the engine applied and compares the sums with the account totals after every row.

## Pros and Cons of the Options

### Test-type vocabulary

* **Five types (chosen).** Good, because every row maps to exactly the layer that can observe it: exit codes and closed streams need the binary, parser rules need the pipeline, and lifecycle rules need only the engine. Good, because it matches the layers ADR-0011 and ADR-0012 already describe.
* **Three types (unit, sample CSV, property).** Good, because it is shorter. Bad, because fatal exits and in-memory pipeline tests have no exact name, so E6 would have to guess.

### Priority

* **Three levels by risk (chosen).** Good, because the rows whose failure costs money or fails the gate are written first. Good, because it uses the same P1 and P2 tags reviewers already use on pull requests.
* **No priority.** Good, because every row is required anyway. Bad, because nothing tells E6 where to start, and a partial E6 could cover diagnostics before money.

### Runtime invariant checks

* **Tests only (chosen).** Good, because debug and release behave the same, and no input can trigger an assertion panic. Bad, because a broken invariant in production goes unnoticed until a test finds it.
* **`debug_assert!` for cheap invariants.** Good, because it is defense in depth, in the spirit of TigerBeetle's dense assertions. Bad, because it runs only in debug and test builds, so it adds nothing in the shipped binary, and it mostly repeats what the types already guarantee.
* **Fail closed at run time.** Good, because a broken invariant would stop the run before it prints a wrong ledger. Bad, because it needs a new exit code and a change to ADR-0010, and the cheap invariants it could check already hold by construction.

### Money conservation

* **Both (chosen).** Good, because the overall sum is the ledger's balance check and the per-client sum locates the error.
* **Overall only.** Bad, because two opposite errors in two accounts would cancel out.
* **Per client only.** Bad, because the overall statement, which is the one auditors ask for, would only follow from it implicitly.

### Order across clients

* **Per-client projection (chosen).** Good, because it states what is independent and why, and implies the reorder form. Good, because it is the property sharding by client needs.
* **Reorder invariant only.** Good, because it is easy to state. Bad, because it does not say what a client's result depends on, and it is weaker than the projection.
* **Determinism only (I10).** Bad, because it allows a client's balance to depend on unrelated clients' rows, which no rule intends.

## More Information

* Industry references:
  * Ordering per key only: event streams such as Kafka keep order within a partition, and records with the same key go to the same partition ([Confluent on message keys](https://www.confluent.io/learn/kafka-message-key/)). [ADR-0011](0011-architecture-and-memory.md) stage 2 applies the same model with client IDs as keys.
  * One ID namespace, first writer wins: [TigerBeetle `create_transfers`](https://docs.tigerbeetle.com/reference/requests/create_transfers/) answers a reused transfer `id` with `exists` or one of the `exists_with_different_*` results, including different debit or credit accounts. [Stripe idempotent requests](https://docs.stripe.com/idempotency) return an error when a key is reused with different parameters.
  * Assertions: the [Rust `debug_assert!` documentation](https://doc.rust-lang.org/std/macro.debug_assert.html) states that it runs only in non-optimized builds by default and that an unchecked assertion lets a program in an inconsistent state keep running. [TigerBeetle's Tiger Style](https://github.com/tigerbeetle/tigerbeetle/blob/main/docs/TIGER_STYLE.md) favors dense assertions and treats a failure as a programmer error; this ADR keeps that intent in the tests instead of the shipped binary.
  * Test layers: the Rust Book chapter on [test organization](https://doc.rust-lang.org/book/ch11-03-test-organization.html) (unit tests next to the code, integration tests that use the public API).
  * Risk-based prioritization: the [ISTQB Foundation Level syllabus](https://www.istqb.org/certifications/certified-tester-foundation-level/) on risk-based testing.
* Adds scenarios A38 to A45 and invariants I13 to I18, and the Test and Priority columns, to [requirements §6](../requirements.md). No earlier rule changes.
* Where each test type lives, how sample cases are laid out, and how tests are named after catalog IDs are decided in [ADR-0014](0014-testing-strategy.md).
* Tracked by issue LYA-46 (D8). The testing strategy (D9) places each test type; the tests themselves are written in E6.
