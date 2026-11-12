---
status: accepted
date: 2026-11-12
decision-makers: Alonso Solano
---

# Report ignored dispute rows with six reasons, checking ownership right after the lookup

## Context and Problem Statement

[ADR-0006](0006-dispute-lifecycle.md) R1 to R7 decide which `dispute`, `resolve`, and `chargeback` rows are ignored. [ADR-0010](0010-input-validation-and-errors.md) gives each skipped row one countable reason code, but for these rows its condition table only says "reason per ADR-0006". [ADR-0011](0011-architecture-and-memory.md) S6 makes each reason a variant of `Rejection`, counted by an exhaustive match in the end-of-run summary.

Two things are still open, and the dispute implementation (E4) needs both:

* **Which reasons exist.** One code for every ignored dispute row, a few, or one per rule.
* **Which reason wins.** One row can break several rules. For example, client 2 sends a `dispute` for a withdrawal owned by client 1: the row is both cross-client (R3) and not on a deposit (R1). The order of the checks decides which reason is reported and counted.

The account balances do not depend on either choice: every such row is ignored. Only stderr and the summary counts do.

## Decision Drivers

* An operator must be able to tell an attack from an upstream ordering error in the summary counts (ADR-0006 R10). Cross-client attempts and replayed chargebacks in particular must stand out.
* A row from a client that does not own a `tx` must not learn anything about that transaction beyond "not yours". Access to one object of a type is not access to every object of that type.
* The result must be deterministic: the same row in the same state always gives the same reason.
* Reasons map one to one onto rules that already exist, so no new behavior is introduced.
* Rejected withdrawals keep the meaning [ADR-0011](0011-architecture-and-memory.md) S10 gives them: treated as never stored.

## Considered Options

1. **Granularity:** one reason per rule (six), three reasons (unknown, cross-client, invalid state), or a single "not applicable" reason.
2. **Check order:** existence, ownership, kind, state; or existence, kind, ownership, state.

## Decision Outcome

Chosen options: **six reasons**, checked in the order **existence, ownership, kind, state**.

### Rules

| # | Rule | Justification |
|---|------|---------------|
| G1 | An ignored `dispute`, `resolve`, or `chargeback` row is reported with exactly one of six reasons: `unknown_tx`, `client_mismatch` (with the owning client), `not_a_deposit`, `already_disputed`, `already_charged_back`, `not_disputed`. Each is its own `Rejection` variant and its own count in the summary. | One reason per rule of ADR-0006 keeps the summary as precise as the rules. TigerBeetle reports the same distinctions for pending transfers with separate codes (`pending_transfer_not_found`, `pending_transfer_not_pending`, `pending_transfer_already_posted`, `pending_transfer_already_voided`). |
| G2 | The checks run in a fixed order, and the first that fails is the reason: (1) the `tx` is stored, (2) the row's client owns it, (3) it is a deposit, (4) its dispute state accepts the row. | Ownership right after the lookup means a non-owner only ever sees `client_mismatch`; the transaction's kind and state are not disclosed to them. This follows the OWASP rule of checking access to the specific object on every request. |
| G3 | A `tx` bound by a rejected withdrawal fails check (1) and reports `unknown_tx`, whoever sends the row. | ADR-0011 S10 treats a rejected withdrawal as never stored for the dispute lifecycle. It never moved money, so there is no owner whose data needs protecting. |

### Reasons by condition

| Check | Condition | Reason | ADR-0006 rule |
|-------|-----------|--------|---------------|
| 1 | `tx` unbound, or bound by a rejected withdrawal | `unknown_tx` | R7 |
| 2 | The row's `client` is not the owner of the `tx` | `client_mismatch`, owning client | R3 |
| 3 | The `tx` is an applied withdrawal | `not_a_deposit` | R1 |
| 4 | `dispute` on a deposit in `Disputed` | `already_disputed` | R4 |
| 4 | Any row on a deposit in `ChargedBack` | `already_charged_back` | R6 |
| 4 | `resolve` or `chargeback` on a deposit in `Processed` or `Resolved` | `not_disputed` | R7 |

A row that passes every check can still be ignored with `overflow` ([ADR-0007](0007-money-representation.md) M7) if its balance effect would overflow. Reason codes are spelled here as in ADR-0010; the code may spell them differently as long as each is a distinct variant.

### Consequences

* Good, because the summary separates cross-client attempts, replayed chargebacks, and ordinary ordering errors, which is the hook ADR-0006 keeps for flagging clients later.
* Good, because a client probing other clients' `tx` IDs learns only whether an ID is in use, never whether it is a withdrawal or the state of a dispute.
* Good, because each reason maps to one arm of the transition match of ADR-0011 S11, so the state checks need no extra code.
* Neutral, because `unknown_tx` versus `client_mismatch` still tells a client whether an ID is in use. The diagnostics go to the operator's stderr, not back to the client, so this is not a disclosure in the current CLI.
* Neutral, because the order differs from TigerBeetle's, which checks that a transfer is pending before it checks the accounts. TigerBeetle has no per-client ownership to protect at that point; here ownership is the security boundary.
* Bad, because six reasons mean six more summary lines in the worst case. Accepted: the summary is bounded by the number of reasons, not by the input (R10).

### Confirmation

* Unit tests in `src/engine.rs` produce every reason (A3 to A10, A25, A27, A28, A38, A41) and check the order on rows that break several rules at once (A46).
* Unit tests in `src/store.rs` cover each of the twelve (state, row type) pairs of the transition function.
* Review check: the engine's dispute path looks the `tx` up once and runs the four checks in the order of G2 before it computes any new balance.

## Pros and Cons of the Options

### Granularity

* **Six reasons (chosen).** Good, because each count names exactly one rule. Bad, because the summary has more lines.
* **Three reasons.** Good, because it is shorter. Bad, because a replayed chargeback is counted with every other state error, which hides the one ADR-0006 R6 exists to stop.
* **One reason.** Good, because it is the simplest. Bad, because the summary cannot tell a cross-client attack from an upstream ordering error.

### Check order

* **Existence, ownership, kind, state (chosen).** Good, because the authorization check comes before anything about the object is revealed. Good, because one rule decides every multi-violation row.
* **Existence, kind, ownership, state.** Good, because it matches TigerBeetle's order for pending transfers. Bad, because a non-owner learns that a `tx` is a withdrawal.

## More Information

* Fills in the "reason per ADR-0006" row of the [ADR-0010](0010-input-validation-and-errors.md) condition table. Rules R1 to R10 of [ADR-0006](0006-dispute-lifecycle.md) and the balance effects are unchanged.
* Adds scenario A46 to [requirements §6](../requirements.md) under [ADR-0013](0013-attack-and-invariant-catalog.md) C7.
* Industry references: [TigerBeetle `create_transfers` result codes](https://docs.tigerbeetle.com/reference/requests/create_transfers/), listed in order of precedence, and the [OWASP Authorization Cheat Sheet](https://cheatsheetseries.owasp.org/cheatsheets/Authorization_Cheat_Sheet.html) on checking access to the specific object on every request.
* Decided with the human while planning E4 (LYA-52).
