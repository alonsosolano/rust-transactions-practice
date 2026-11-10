---
status: accepted
date: 2026-11-10
decision-makers: Alonso Solano
---

# Represent money as fixed-point integers with strict parsing and checked arithmetic

## Context and Problem Statement

Input amounts are decimals with up to 4 fractional digits ([requirements §2](../requirements.md)). Every balance operation depends on how they are stored. Binary floating point cannot represent values such as `0.1` exactly, so it would make `total = available + held` (I1) drift and could create or lose money.

How are amounts and balances represented, which input strings are valid amounts, and what happens when an amount is out of range or an operation would overflow?

## Decision Drivers

* The engine must never create or lose money. No value may be rounded, truncated, wrapped, or saturated without the row being refused.
* No input may cause a panic ([requirements §8](../requirements.md)), and no single field may cost unbounded memory.
* Per-transaction state must stay small: up to 2^32 deposits may be stored ([requirements §8](../requirements.md)).
* Invalid values should be unrepresentable in the type system where practical.
* Follow what payment APIs, ledger databases, and banking standards do (see [More Information](#more-information)).
* Every dependency must be justified ([requirements §9](../requirements.md)); fewer is better.
* The choice must be reversible: changing it later should touch one module only.

## Considered Options

The choices below were decided one by one. Each lists the options considered and the chosen one.

1. **Numeric representation:** binary floating point (`f64`), `rust_decimal`, arbitrary precision, or own fixed-point integer newtypes.
2. **Balance width:** `i64` or `i128`.
3. **Sign and zero:** a signed amount checked at run time, or a type that cannot hold zero or a negative value.
4. **More than 4 fractional digits:** reject, round half-even, round half-up, or truncate.
5. **Overflow:** wrap, saturate, panic, or refuse the row.
6. **Parsing grammar:** permissive (what a general decimal parser accepts) or strict.

## Decision Outcome

Chosen options: own fixed-point integer newtypes, `i128` balances, a non-zero unsigned amount, rejection of excess precision, refusal of any row that would overflow, and a strict grammar. Together they make every accepted amount exact and every balance change checked.

### Rules

| # | Rule | One-line justification |
|---|------|------------------------|
| M1 | An `Amount` is a count of **1/10,000 units**, stored as a non-zero `u64` (for example `NonZeroU64`). The scale (4 fractional digits) is one named constant. | Integers in a fixed smallest unit are exact and are what payment APIs and ledger databases use. A non-zero unsigned type makes zero and negative amounts unrepresentable. |
| M2 | **Balances** (`available`, `held`, `total`) are signed `i128` counts of the same unit. | Balances may be negative ([ADR-0006](0006-dispute-lifecycle.md) R2). With `i128`, overflow is unreachable for any valid input (see [Range](#range)), and it costs about 3 MiB for all 65,536 possible accounts. |
| M3 | A valid amount, after the surrounding whitespace is trimmed, matches `digits [ "." digits ]` with ASCII digits only. Everything else is rejected: a sign (`+`, `-`), a leading or trailing dot (`.5`, `5.`), exponent notation (`1e3`), separators (`1_000`, `1,000`), inner whitespace, non-ASCII digits, `NaN`, `inf`, and the empty string. | Every accepted string has exactly one meaning. Forms like `1_000` or `1e3` are more likely an upstream bug than a real amount, and an exponent can encode a huge value in a few bytes. |
| M4 | At most **4 fractional digits**, not counting trailing zeros. Extra fractional digits are accepted only if they are all zeros (`1.23450` is `1.2345`). Any other excess precision rejects the amount. Amounts are **never rounded or truncated**. Leading zeros are accepted (`007.5` is `7.5`). | Accepting lossless forms costs nothing. Rounding or truncating would change the amount the upstream sent, and repeated small roundings can create or lose money. |
| M5 | The amount must be at least `0.0001` and at most `1844674407370955.1615` (`u64::MAX` units). Anything outside that range, including `0` and `0.0000`, is rejected. | Zero is a valid number but not a valid deposit or withdrawal: it would create a disputable transaction and an account with no economic meaning. The upper bound is the type limit, checked while parsing. |
| M6 | A `deposit` or `withdrawal` whose amount is rejected under M3 to M5 is an **invalid row**. It never reaches the engine, never changes state, and never creates an account. How it is reported (skipped with a diagnostic or aborting the run) is the input-validation decision (D5). | A negative withdrawal would create money and a negative deposit would bypass the insufficient-funds check. Refusing the row before it touches state closes both. |
| M7 | Every arithmetic operation on amounts and balances is **checked** (`checked_add`, `checked_sub`). Wrapping, saturating, and unchecked operators are not used, and widening uses lossless conversions (`i128::from`), never `as`. A row whose effect would overflow is **ignored** under [ADR-0006](0006-dispute-lifecycle.md): no state change, a diagnostic with its own reason, and a count in the summary. | Wrapping or saturating would corrupt a balance silently, and a panic would let hostile input crash the run. Under M2 this path is unreachable for valid input; it remains as defense in depth. |
| M8 | Conversion to and from text uses integer arithmetic only. **No floating-point type appears** anywhere on the money path: parsing, arithmetic, or formatting. | A single float conversion anywhere reintroduces the rounding errors this ADR rules out. |

Output layout (for example whether trailing zeros are printed) is decided by the output-format decision (D2). This ADR only requires that formatting is exact under M8, including for negative values between `-1` and `0` (for example `-0.5`), whose sign an integer division of the unit count would lose.

### Range

The bound below shows why M2 makes overflow unreachable. Let `D` be the sum of a client's stored deposits. Each transaction ID stores at most one deposit, so a client has fewer than 2^32 deposits of less than 2^64 units each, and `D < 2^96`.

* `held` is the sum of the deposits currently disputed (I4), so `0 ≤ held ≤ D`.
* Each deposit is charged back at most once ([ADR-0006](0006-dispute-lifecycle.md) R6), so the sum of chargebacks is at most `D`.
* A withdrawal is accepted only if `available` stays non-negative, so the sum of accepted withdrawals is at most `D`.
* `total` is deposits minus accepted withdrawals minus chargebacks, so `|total| ≤ D`, and `available = total - held` gives `|available| ≤ 2D`. Both are below 2^97, far from `i128::MAX` (about 2^127).

An `i64` balance (maximum `922337203685477.5807`) would overflow on a single deposit of the maximum amount, or on about 10,000 deposits of `100000000000` each, which is valid input. The bound assumes each `tx` ID stores at most one deposit; duplicate IDs are decided by D5. If that assumption ever breaks, M7 still refuses the overflowing row.

### Consequences

* Good, because every accepted amount is exact, so `total = available + held` holds by integer arithmetic.
* Good, because negative and zero amounts are unrepresentable, which closes the "negative withdrawal creates money" and "negative deposit skips the funds check" attacks at the type level.
* Good, because a stored amount takes 8 bytes. For 2^32 stored deposits that is half the memory of a 16-byte decimal.
* Good, because no dependency is added, so the dependency decision (D7) has one crate fewer to justify.
* Good, because the representation is hidden behind the `Amount` and balance newtypes. Switching to a decimal crate, a wider amount, or a different scale later touches one module only.
* Bad, because the parser and formatter are written and tested in this crate instead of being taken from a widely used library.
* Bad, because the strict grammar rejects forms a lenient upstream might send, such as `.5` or `+5`. Those rows are lost to the engine, but they stay visible through the D5 diagnostics. Loosening the grammar later is a compatible change; tightening it would not be.
* Neutral, because the scale is fixed at 4 fractional digits. Finer precision or multiple currencies would need a new ADR.
* Neutral, because balances and amounts are two integer widths, so the code converts between them. The conversion is always widening and lossless.

### Confirmation

* Parser unit tests cover every accepted and rejected form in M3 to M5, including the boundaries `0.0001`, `1844674407370955.1615`, and `1844674407370955.1616`, and lossless forms such as `1.23450` and `007.5`.
* A property-based test checks that formatting then parsing returns the same value for any `Amount`, and that formatting a balance never loses its sign.
* Scenarios A17 to A22 in [requirements §6](../requirements.md) cover rejected amounts and maximum-size deposits. Invariants I6 and I7 cover overflow and exactness.
* The money module is checked with clippy's `float_arithmetic`, `arithmetic_side_effects`, and `as_conversions` lints denied. Where they apply crate-wide is decided by D7.
* Review check: no `f32` or `f64` on the money path, and no `+`, `-`, or `as` on amounts or balances outside the checked helpers.

## Pros and Cons of the Options

### Numeric representation

* **Own fixed-point integer newtypes (chosen).** Good, because integer arithmetic is exact, simple, and fast, and the memory cost is 8 bytes per stored amount. Good, because it matches Stripe, Square, Modern Treasury, TigerBeetle, and Formance, which all store amounts as integers in a smallest unit. Good, because the parser accepts exactly the grammar chosen here. Bad, because the parser and formatter are code this crate must test.
* **`rust_decimal`.** Good, because it is widely used, exact, has a large range, and offers checked operations. Bad, because each value takes 16 bytes. Bad, because its parser is more permissive than this input needs: it accepts `+1`, `.5`, `5.`, and `1_000`, and it rounds half-up without an error once a number has more than 28 digits, so it would need a strict wrapper anyway. Neutral, because its parser has had overflow bugs that were found and fixed (a silent overflow in 1.36.0), which shows both active maintenance and the risk of depending on parsing code we do not control.
* **Arbitrary precision (for example `bigdecimal`).** Good, because values can never overflow. Bad, because every value allocates on the heap and its size is driven by the input, so a hostile input can make a single field cost unbounded memory and CPU.
* **Binary floating point (`f64`).** Good, because it is native and needs no code. Bad, because values like `0.1` are inexact, so balances drift and I1 fails. Ruled out.

### Balance width

* **`i128` (chosen).** Good, because overflow is unreachable for valid input (see [Range](#range)). Good, because the cost is about 3 MiB for all possible accounts. TigerBeetle uses 128-bit balances for the same reason. Neutral, because 128-bit addition and subtraction compile to a few extra instructions.
* **`i64`.** Good, because amounts and balances share one type. Bad, because the maximum balance is `922337203685477.5807`, below the largest valid amount, so valid input can reach it and the overflow path becomes a normal case. Limiting amounts to the `i64` range would not help, because many valid deposits still add up past it.

### Sign and zero

* **Non-zero unsigned type (chosen).** Good, because invalid amounts cannot be constructed, so no engine function has to check them. Good, because the direction of money comes from the row type, as with Stripe and Square, which require positive amounts.
* **Signed amount checked at run time.** Bad, because every function that receives an amount must trust that the check ran.

### More than 4 fractional digits

* **Reject, accepting only extra zeros (chosen).** Good, because the ledger always holds the exact value the upstream sent. ISO 20022 schemas and Square's API also reject amounts that do not fit their precision. Bad, because a row with excess precision is refused instead of processed.
* **Round half-even.** Good, because it is the usual unbiased mode for computed values such as interest. Bad, because it changes an input value. A deposit of `0.00015` would become `0.0002`, creating `0.00005` from nothing, and repeating that creates money.
* **Round half-up.** Bad, for the same reason, with a bias toward creating money.
* **Truncate.** Bad, because a withdrawal of `0.00019` would be recorded as `0.0001`, so more leaves the system than the ledger records.

### Overflow

* **Refuse the row (chosen).** Good, because the state stays valid and the event is visible in diagnostics.
* **Panic.** Bad, because hostile input could crash the run, which [requirements §8](../requirements.md) forbids.
* **Saturate.** Bad, because the balance becomes silently wrong and I1 can break.
* **Wrap.** Bad, because a large positive balance becomes a large negative one without any trace.

### Parsing grammar

* **Strict (chosen).** Good, because every accepted string has one meaning, and the parser does work proportional to the field's length with no allocation. Bad, because forms a lenient upstream might send, such as `.5`, are refused.
* **Permissive.** Good, because more upstream variations are accepted. Bad, because forms such as `1_000`, `1e3`, or `+5` are ambiguous upstream data, and exponents make huge values cheap to write.

## More Information

* Industry practice consulted:
  * Payment APIs store amounts as integers in the currency's smallest unit: [Stripe](https://docs.stripe.com/currencies), [Square](https://developer.squareup.com/docs/build-basics/common-data-types/working-with-monetary-amounts), and [Modern Treasury](https://moderntreasury.com/journal/floats-dont-work-for-storing-cents), which also stores a [currency exponent](https://docs.moderntreasury.com/docs/currencies) next to each amount.
  * Ledger databases do the same: [TigerBeetle](https://tigerbeetle.com/blog/2023-09-19-64-bit-bank-balances-ought-to-be-enough-for-anybody/) uses 128-bit integers and refuses transfers that would overflow ([`overflows_*` results](https://docs.tigerbeetle.com/reference/requests/create_transfers/)), and [Formance](https://docs-git-main-formance.vercel.app/stack/unambiguous-monetary-notation) uses integers with an explicit scale.
  * [Google's `Money` type](https://fuchsia.googlesource.com/third_party/googleapis/+/refs/heads/main/google/type/money.proto) splits an amount into integer units and integer nanos.
  * Banking messages in [ISO 20022](https://www.iso20022.org/) carry amounts as decimals with a fixed maximum of fractional digits, enforced by schema validation.
  * The `rust_decimal` behavior described above comes from its [parser source](https://docs.rs/rust_decimal/latest/src/rust_decimal/str.rs.html) and its [1.36.0 changelog](https://rust-digger.code-maven.com/src/rust_decimal-1.36.0/CHANGELOG.md).
* Revisit this decision if amounts need more than 4 fractional digits, if multiple currencies are introduced, or if a decimal crate is adopted for another reason. Because of the newtypes, any of these is a change to one module plus a superseding ADR.
* Answers the open question on rounding, zero, and negative amounts in [requirements §10](../requirements.md). Duplicate `tx` IDs and the reporting of invalid rows are decided by D5 (LYA-43). The output layout is decided by D2 (LYA-40). Dependency and lint policy are decided by D7 (LYA-45).
* Tracked by issue LYA-39 (D1). The `Amount` implementation (E2) builds on these rules.
