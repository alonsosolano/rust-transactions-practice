---
status: accepted
date: 2026-11-10
decision-makers: Alonso Solano
---

# Treat a locked account as "post no debits" while every open claim keeps running

## Context and Problem Statement

A chargeback locks the account, and `locked` never reverts ([requirements §4](../requirements.md), invariant I2). The brief does not say what a locked account may still do. [ADR-0006](0006-dispute-lifecycle.md) R9 already fixes the outline: dispute-related rows and deposits are processed, withdrawals are rejected. It leaves the details open: whether a deposit made after the lock can itself be disputed, which reason a rejected withdrawal reports when it also lacks funds, whether anything can unlock the account, and how the rows are reported.

For each row type, what happens when it targets a locked account, and why?

## Decision Drivers

* No money may ever leave an account after a chargeback. The lock exists to stop the deposit, withdraw, charge back attack ([requirements §6](../requirements.md)) from being repeated.
* The ledger must keep telling the truth after the lock. A reversal that happens outside the engine must still be recorded, or the loss is hidden.
* Every open claim must be able to finish. Funds must not stay in `held` forever because the account was locked.
* A client must be able to repay a debt left by a chargeback.
* Follow what banks and payment processors do with frozen accounts (see [More Information](#more-information)).
* Rules must be simple enough to state as invariants and test with property-based tests.
* The CLI, CSV schema, and output columns stay as they are ([requirements §1](../requirements.md)).

## Considered Options

The choices below were decided one by one. Each lists the options considered and the chosen one.

1. **Deposits:** accept and store as a normal deposit, accept into `held`, or reject.
2. **Withdrawals:** reject always, allow up to a positive `available`, or allow once the debt is repaid.
3. **Dispute-related rows:** process all, process only those on deposits made before the lock, or ignore all.
4. **Reason for a rejected withdrawal:** lock first, insufficient funds first, or both.
5. **Unlocking:** never, or when the balance is no longer negative.
6. **Reporting accepted deposits on a locked account:** not reported, or reported as an informational diagnostic.

## Decision Outcome

Chosen options: a locked account accepts deposits as normal deposits, rejects every withdrawal and reports the lock as the reason, processes every dispute-related row exactly as on an unlocked account, never unlocks, and reports only rejected and ignored rows. Banks call this restriction "post no debits": credits post, debits do not.

### Rules

| # | Rule | One-line justification |
|---|------|------------------------|
| L1 | An account becomes locked only when a chargeback is applied to it. **No row unlocks it**: not a deposit that repays the debt, not a resolve, not a positive balance. | Reinstating a frozen account takes a review outside the engine, and the input has no row for it. Invariant I2 fixes this. |
| L2 | A `deposit` on a locked account is **applied** and stored like any other deposit. It can be disputed, resolved, and charged back under [ADR-0006](0006-dispute-lifecycle.md). | Credits keep posting under a "post no debits" freeze, so the client can repay a debt. The funds can never leave the account, so the deposit cannot raise the platform's exposure. If the payment behind it is reversed, the ledger must record that too. |
| L3 | A `withdrawal` on a locked account is **rejected**, whatever the balance. It is not stored, so later dispute-related rows on its `tx` are ignored (ADR-0006 R7). | The lock exists so that no money leaves a charged-back account. |
| L4 | The lock is checked **before** the funds check. A withdrawal on a locked account always reports the reason "account locked", even when `available` is also too low. | One fixed order makes diagnostics deterministic, and it reports the permanent cause instead of one that a deposit could remove. |
| L5 | `dispute`, `resolve`, and `chargeback` rows on a locked account follow ADR-0006 **exactly as on an unlocked account**, on deposits made before or after the lock. A chargeback on an account that is already locked applies its balance effect and leaves `locked` at `true`. | Banks must finish investigating a claim even on a closed or blocked account, and a processor's liability for disputes outlives the account. Ignoring these rows would leave funds in `held` forever or hide an external reversal. |
| L6 | A locked account appears in the output like any other: final `available`, `held`, and `total`, which may be negative, and `locked` set to `true`. | The output is the ledger. Hiding or clamping a locked account would hide the debt. |
| L7 | A rejected withdrawal on a locked account is reported under ADR-0006 R10 with its own reason, "account locked", and is counted apart from "insufficient funds" in the summary. Accepted deposits on a locked account are not reported. The exact wording and format of reasons follow the input-validation decision (D5). | Diagnostics cover rows that change nothing. A separate reason lets an operator tell attempts to drain a locked account from ordinary insufficient funds. |

Any row that is invalid or malformed is handled the same way on a locked account as on any other (D5). The lock adds no rule for it.

### Row type by account state

| Row type | Unlocked account | Locked account | Reason on the locked account |
|----------|------------------|----------------|------------------------------|
| `deposit` | Applied | **Applied** (L2) | Credits post under a "post no debits" freeze; the client can repay the debt and the funds can never leave. |
| `withdrawal` | Applied, or rejected for insufficient funds | **Rejected** for "account locked" (L3, L4) | No money may leave a charged-back account. |
| `dispute` | Applied or ignored under ADR-0006 | **Same as unlocked** (L5) | A new claim must still hold its funds, or a reversal is hidden. |
| `resolve` | Applied or ignored under ADR-0006 | **Same as unlocked** (L5) | A closed claim must release its hold, or funds stay in `held` forever. The released funds still cannot be withdrawn. |
| `chargeback` | Applied (locks) or ignored under ADR-0006 | **Same as unlocked**; `locked` stays `true` (L5) | The debit happened outside the engine and must be recorded. |

"Applied", "rejected", and "ignored" mean what they mean in [requirements §5](../requirements.md). A rejected withdrawal is a skipped row: no state change and a diagnostic.

### How an account gets unlocked

Inside the engine it never does (L1, I2). Two facts make that the right scope:

* **The input cannot express it.** No row type means "this account was reviewed and cleared". The only alternative is an automatic condition, such as a balance that is no longer negative, and that reopens the attack: a client repays with a deposit that can itself be reversed, gets unlocked, and withdraws again.
* **The engine keeps no state between runs.** Every run starts empty and replays its input from the start, so `locked` means "this input contains an applied chargeback for this client". Whether the client may transact again is decided by the system that produces the next input.

In practice, banks and processors lift a freeze only through a review outside the ledger. A risk or fraud team reviews the account, usually after the client verifies their identity. The debt is repaid, and the team waits until that repayment can no longer be reversed. Then they either reinstate the account or close it and pay out the remaining balance. Both steps are human decisions.

Supporting this in the engine would need a new input event that only an operator can issue (for example an `unlock` row from an upstream review system), and a new ADR that supersedes I2. Even then, no deposit or balance condition should unlock an account by itself.

### Consequences

* Good, because no withdrawal can ever be applied after a chargeback, which is a single invariant that property-based tests can check.
* Good, because an attacker with several disputed deposits gets every chargeback recorded after the first one locks the account, so the full debt shows (A23).
* Good, because the lock has no effect on the dispute rules, so the dispute code does not need to know about it, and the lock lives in the withdrawal path only.
* Good, because a client can repay a debt, which is how processors recover a negative balance after a chargeback.
* Bad, because a deposit made on a locked account is not final recovery if its own payment is reversed (A24). Accepted: the deposit can never be withdrawn, so the reversal returns the account to its earlier balance and adds no new loss.
* Neutral, because funds released by a resolve on a locked account stay in `available` with no way out through the engine. Returning a balance to the client after review is an out-of-band process.
* Neutral, because accepted deposits on a locked account leave no diagnostic. If operators need to see repayments, a new ADR can add an informational entry under R10.

### Confirmation

* Rules L1 to L7 are covered by scenarios A11 to A13 and A23 to A25, and by invariants I2, I8, and I9 in [requirements §6](../requirements.md). These are acceptance criteria for the implementation; they are not executable tests yet.
* A property-style test checks, after every row of a randomized stream, that no withdrawal is applied to a locked account (I8), and that dispute-related rows have the same effect whether or not the account is locked (I9).
* Review check: the lock is read only on the withdrawal path and set only on the chargeback path.

### Known limits vs industry practice

Standard controls that the input format cannot express. Adding any of them needs a new ADR.

| Industry control | Why it is out of scope | Effect on the engine today |
|------------------|------------------------|----------------------------|
| **Review and reinstatement**: a bank can lift a freeze after it reviews the account. | No row type exists for it, and I2 forbids unlocking. | A locked account stays locked for the rest of the run. |
| **Closure and payout of the remaining balance** after review. | No row type exists for it. | A positive balance on a locked account stays in the account. |
| **Collections and reserves** on a negative balance, such as debiting a linked bank account. | The engine has no external accounts. | The debt is recorded; only deposits can reduce it. |
| **Credit restrictions** ("post no credits"), used for example under sanctions or court orders. | The brief describes a single kind of lock, set by a chargeback. | Deposits are always accepted on a locked account. |

## Pros and Cons of the Options

### Deposits

* **Accept and store as a normal deposit (chosen).** Good, because it matches "post no debits" and lets the client repay. Good, because the deposit stays disputable, so a reversal of the repayment is recorded. Neutral, because such a deposit is not final recovery while it can still be reversed.
* **Accept into `held`.** Bad, because `held` would no longer equal the sum of open disputes (I4), and the funds would need a release rule the input cannot trigger.
* **Reject.** Good, because "frozen" would mean frozen for every row. Bad, because a debt such as A1 could never be repaid through the engine.

### Withdrawals

* **Reject always (chosen).** Good, because no money ever leaves a charged-back account. Bad, because a client who repaid in full still cannot withdraw. Accepted: reinstatement is out of scope.
* **Allow up to a positive `available`.** Bad, because the client who just caused a loss could start withdrawing again as soon as a deposit lands, which repeats the attack.
* **Allow once the debt is repaid.** Bad, because it amounts to unlocking (L1, I2), and a repayment can itself be reversed.

### Dispute-related rows

* **Process all (chosen).** Good, because every open claim can finish and every external reversal is recorded, as banks and processors must do on closed accounts. Good, because the dispute rules do not depend on the lock.
* **Process only those on deposits made before the lock.** Bad, because a reversed repayment (A24) would be hidden, and the engine would need to remember when each deposit arrived relative to the lock.
* **Ignore all.** Bad, because open disputes would keep funds in `held` forever, and later chargebacks would go unrecorded, hiding most of the loss in A23.

### Reason for a rejected withdrawal

* **Lock first (chosen).** Good, because the result is deterministic and names the permanent cause. Good, because a locked account needs no funds check at all.
* **Insufficient funds first.** Bad, because the reported reason would change with the balance for the same rejected row, and an operator could miss an attempt to drain a locked account.
* **Both reasons.** Bad, because one row would count twice in the summary, or the summary would need combined reasons.

### Unlocking

* **Never (chosen).** Good, because it matches I2 and leaves reinstatement to a review outside the engine.
* **When the balance is no longer negative.** Bad, because a client could unlock the account with a reversible deposit and withdraw again.

### Reporting accepted deposits on a locked account

* **Not reported (chosen).** Good, because diagnostics stay limited to rows that change nothing, which keeps R10 simple.
* **Reported as an informational diagnostic.** Good, because operators would see repayments. Bad, because it mixes accepted rows into the diagnostics and uses up the 100 detailed entries. Kept as a future extension.

## More Information

* Refines [ADR-0006](0006-dispute-lifecycle.md) R9 without changing it. The locked-account question in [requirements §10](../requirements.md) was closed by ADR-0006; this ADR adds the per-row-type table, the reason order, and the reporting rule.
* Industry references:
  * "Post no debits" freezes, where credits post and debits are blocked: [Apache Fineract account freeze (FINERACT-482)](https://issues.apache.org/jira/browse/FINERACT-482), [J.P. Morgan embedded payments, post transaction restrictions](https://developer.payments.jpmorgan.com/docs/embedded-finance-solutions/embedded-payments/capabilities/accounts/manage-accounts/post-transaction-restrictions), and [Moniepoint, PND (Post No Debit)](https://support.moniepoint.com/topics/pnd-post-no-debit-9).
  * Claims continue after an account is closed or blocked: [Regulation E error resolution, 12 CFR 1005.11](https://www.ecfr.gov/current/title-12/chapter-X/part-1005/subpart-A/section-1005.11) and its commentary, the [ABA Banking Journal answer on error resolution for closed or blocked accounts](https://bankingjournal.aba.com/2025/04/aba-regulatory-policy-and-compliance-inbox-account-error-resolution-requirements/), and [Stripe, refunds and disputes after closing an account](https://support.stripe.com/questions/refunds-and-disputes-after-closing-a-stripe-account).
  * Recovering a negative balance after a dispute: [Stripe, handling negative balances](https://support.stripe.com/embedded-connect/questions/handling-negative-balances).
* The test type and priority of each scenario and invariant are set by [ADR-0013](0013-attack-and-invariant-catalog.md).
* Tracked by issue LYA-42 (D4). Reason wording and reporting format are coordinated with D5 (LYA-43) and decided in [ADR-0010](0010-input-validation-and-errors.md). The attack catalog (D8) and the locking implementation (E4) build on the scenarios in requirements §6.
