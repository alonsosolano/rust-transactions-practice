---
status: accepted
date: 2026-11-12
decision-makers: Alonso Solano
---

# Write diagnostics as prefixed key=value lines and always end with a summary

## Context and Problem Statement

[ADR-0006](0006-dispute-lifecycle.md) R10 bounds the diagnostics for skipped rows: full detail for the first 100, then a single notice that further detail is suppressed, then a summary with one count per reason. A detailed entry names the row type, client, `tx`, the owning client when known, the reason, and the input line number. [ADR-0010](0010-input-validation-and-errors.md) V11 applies the same budget to invalid rows and requires any echoed input to be escaped and truncated, and its condition table names the seventeen reason codes. [ADR-0016](0016-dispute-row-reasons-and-check-order.md) fixes the six reasons for dispute-related rows.

None of them fixes the bytes. The sample cases of [ADR-0014](0014-testing-strategy.md) compare stderr byte for byte (T4), so the exact text of a detailed line, the notice, and the summary is part of the contract, and so is when the summary appears.

What does each diagnostic line look like, what does an invalid row show, when is the summary written, and what does it contain?

## Decision Drivers

* An operator reading a terminal must see at once which row was skipped, where, and why.
* A script must be able to select and count lines with `grep`, `cut`, or `awk`, without a parser.
* No input may forge a line or reach the terminal unescaped (ADR-0010 V11, CWE-117).
* The volume is bounded by a constant, whatever the input (R10).
* The output is deterministic: the same input always gives the same stderr bytes.
* Fatal error lines keep their current form ([ADR-0010](0010-input-validation-and-errors.md) V11), so a caller still finds the cause of a failed run on the last line.
* Follow established log and command-line conventions (see [More Information](#more-information)).

## Considered Options

The choices below were decided one by one. Each lists the options considered and the chosen one.

1. **Line style:** a class prefix and `line N:` location followed by `key=value` fields; pure logfmt; prose sentences.
2. **What an invalid row shows:** the whole raw line, escaped and truncated; only the field that failed; nothing beyond the reason.
3. **Amount on `deposit` and `withdrawal` lines:** shown, or left out as in the R10 field list.
4. **Suppression notice:** written when the first suppressed row arrives, or at the end of the run.
5. **Summary when no row was skipped:** always written, or omitted.
6. **Summary content:** a total line and one line per reason that occurred; or every reason, including zeros.
7. **Summary on a fatal run:** written before the fatal line, or omitted.
8. **Where to record the format:** a new ADR, or edits to the accepted ADR-0006 and ADR-0010.

## Decision Outcome

Chosen options: a class prefix and `line N:` location followed by `key=value` fields; the whole raw line for invalid rows; the amount shown; the notice written when the first suppressed row arrives; the summary always written, including on a fatal run, where it comes before the fatal line; a total line plus one line per reason that occurred; and this new ADR.

### Rules

| # | Rule | One-line justification |
|---|------|------------------------|
| D1 | Every diagnostic is one line of printable ASCII ending in `\n`, written with a single `write_all` of a line that is fully formatted first. The result of every stderr write is dropped (R10, [ADR-0012](0012-dependencies-and-safety-policy.md) S8). | A line formatted in one buffer is never interleaved field by field, and dropping the result keeps a failing stderr from changing stdout, state, or the exit code. |
| D2 | A **detailed line** is `<class>: line <N>: <reason> <fields>`. `<class>` is `rejected` or `ignored`, as in the ADR-0010 outcome classes. `<reason>` is the reason code of the ADR-0010 condition table. `<fields>` are space-separated `key=value` pairs. | The class prefix and the GNU-style `line N:` location match the existing fatal lines (`error: line 7: …`) and let `grep '^ignored:'` select a class. `key=value` pairs are the logfmt convention, easy to read and to cut. |
| D3 | For a valid row that the engine rejects or ignores, `<fields>` are `type=<type> client=<client> tx=<tx>`, then `amount=<amount>` for a `deposit` or `withdrawal`, then `owner=<client>` when the reason carries the owning client (`duplicate_tx`, `client_mismatch`). The amount uses the output layout of [ADR-0009](0009-output-format.md) O5 (`1.5000`). | These are the fields R10 lists, in the order of the input columns. The amount is the one value an operator needs to judge an `insufficient_funds` or `overflow` line, and it is not sensitive data under OWASP's list. Every value is a number or a fixed word, so nothing in these fields is echoed input. |
| D4 | For an invalid row, `<fields>` are `detail=<cause>` when the reason is `invalid_amount` (one of `empty`, `syntax`, `too_precise`, `zero`, `too_large`), then `input=` followed by the raw line, without its line ending, shown with the `Escaped` rules of ADR-0010 V11: quoted, escaped, and cut at 256 bytes with ` (truncated)` after the closing quote. | An invalid row has no trustworthy client or `tx`, so the raw line is the only complete record of what arrived. Escaping keeps it on one line (A32). The amount cause stays a detail: it is counted under the single `invalid_amount` reason, as ADR-0010 requires. |
| D5 | The first 100 skipped rows (`MAX_DETAILED_ROWS`) get a detailed line. When the 101st skipped row arrives, one **notice** is written in its place: `notice: line <N>: more than 100 skipped rows; further skipped rows are counted in the summary only`. No later row writes anything until the summary. | R10's single notice, written where suppression starts so that the operator knows from which line the detail is missing. clang's error limit and the Linux kernel's printk rate limit both report suppression at the point it begins. |
| D6 | Every run that opens its input ends its diagnostics with the **summary**, whether or not a row was skipped. The first line is `summary: skipped=<n> rejected=<r> ignored=<i>`, followed by one line `summary: reason=<code> count=<n>` for each reason with a non-zero count, in the order of the ADR-0010 condition table. | A summary on every run states the count explicitly, so a reader can tell "nothing skipped" from "diagnostics lost". Listing only reasons that occurred keeps the common case short, as clang's "N warnings and M errors generated" does; every reason still has its own counter. |
| D7 | The summary is written once the input is no longer being read, **before** the output CSV is written and before any fatal line. On a fatal error raised while the input is read (a missing or wrong header, an over-long line, an unterminated quote, a read error), the summary covers the rows read so far and the fatal line follows it as the last line. A failed stdout write also comes after the summary. A usage error and an input that cannot be opened stop before the input is read and write only their single fatal line. | The last line is always the outcome: the fatal line on a failed run, the summary on a completed one. Counts for a run that failed still show what was skipped before it stopped. |
| D8 | Counters are `u64` and use `saturating_add`. The count of each reason is selected by an exhaustive `match`, so a new `Rejection` or `InvalidRow` variant fails to compile until it is mapped to a reason ([ADR-0011](0011-architecture-and-memory.md) S6). | A count cannot reach `u64::MAX` (each row takes at least one byte of input), so saturation is harmless, as ADR-0012 S7 allows. |

### Examples

A run with a rejected withdrawal and two ignored rows:

```text
rejected: line 4: insufficient_funds type=withdrawal client=2 tx=5 amount=7.2500
ignored: line 6: client_mismatch type=dispute client=2 tx=1 owner=1
ignored: line 7: invalid_amount detail=too_precise input="deposit,1,7,1.00005"
summary: skipped=3 rejected=1 ignored=2
summary: reason=invalid_amount count=1
summary: reason=client_mismatch count=1
summary: reason=insufficient_funds count=1
```

A run with nothing skipped writes one line: `summary: skipped=0 rejected=0 ignored=0`. A run whose header is wrong writes that same line and then `error: wrong header: …`.

### Reasons, classes, and summary order

| Order | Reason | Class |
|-------|--------|-------|
| 1 | `malformed_quote` | ignored |
| 2 | `field_count` | ignored |
| 3 | `unknown_type` | ignored |
| 4 | `invalid_client` | ignored |
| 5 | `invalid_tx` | ignored |
| 6 | `missing_amount` | ignored |
| 7 | `invalid_amount` | ignored |
| 8 | `duplicate_tx` | ignored |
| 9 | `unknown_tx` | ignored |
| 10 | `client_mismatch` | ignored |
| 11 | `not_a_deposit` | ignored |
| 12 | `already_disputed` | ignored |
| 13 | `already_charged_back` | ignored |
| 14 | `not_disputed` | ignored |
| 15 | `overflow` | ignored |
| 16 | `insufficient_funds` | rejected |
| 17 | `account_locked` | rejected |

### Bound on volume

stderr holds at most 100 detailed lines, one notice, one summary total line, and 17 reason lines, plus one fatal line. A detailed line is at most a few hundred bytes: the echoed input is cut at 256 input bytes, each of which takes at most four output bytes. So the whole stderr of a run stays below about 110 KiB, whatever the input.

### Consequences

* Good, because every skipped row is visible either as a line or as a count, and the counts are on every run.
* Good, because the lines can be filtered by class, reason, or field with standard text tools.
* Good, because no input byte reaches stderr unescaped: echoed input goes through `Escaped`, and every other value is a number or a fixed word.
* Good, because the last line of stderr always states the outcome of the run.
* Bad, because a completed run that skipped nothing still writes one stderr line, which departs from the Unix habit of silence on success. Accepted: the explicit zero count is the confirmation the operator asked for, and stdout is unaffected.
* Bad, because a fatal run now writes the summary before its error line, so stderr of a failed run has more than one line. Accepted: the fatal line is still exactly one line and still the last one, and the summary shows what was skipped before the failure.
* Bad, because exact stderr comparison (ADR-0014 T4) means every sample case shows the summary in its `expected.stderr`. Accepted: that is the contract.
* Neutral, because writes are still blocking: a stderr reader that stops consuming can stall the run, as R10 already accepts.

### Confirmation

* Unit tests in `src/diagnostics.rs` produce every reason and check its detailed line and its summary line, the budget and the single notice, and that a failing writer changes nothing.
* Integration tests in `tests/run.rs` cover A15 (1,000,000 ignored rows give 100 detailed lines, one notice, and a full count), A16 (a failing stderr leaves stdout unchanged), A32 (escaped input), and the summary before a fatal line.
* The `expected.stderr` of every sample case under `tests/cases/` shows the exact lines.
* Review check: no diagnostic writes anything other than a fully formatted line, and no write result is used.

## Pros and Cons of the Options

### Line style

* **Class prefix, `line N:`, then `key=value` fields (chosen).** Good, because it reads like the fatal lines the CLI already writes and like GNU-style `file:line: message` diagnostics, and the fields are as easy to cut as logfmt. Bad, because it is a hybrid that no single tool parses natively.
* **Pure logfmt (`class=ignored line=5 reason=…`).** Good, because logfmt parsers read it directly. Bad, because it looks different from the fatal lines, and a human has to find the reason among the keys.
* **Prose sentences.** Good, because they are the easiest to read once. Bad, because scripts must parse free text, and the wording becomes a contract.

### What an invalid row shows

* **The whole raw line, escaped and truncated (chosen).** Good, because it is the only complete record of a row that could not be parsed, and it is what A32 needs. Bad, because up to 256 bytes per line are echoed.
* **Only the field that failed.** Good, because it is shorter. Bad, because the input adapter would need to carry field positions through every reason, and the other fields still matter to find the row upstream.
* **Nothing beyond the reason.** Bad, because the operator cannot see what arrived, and A32 cannot be checked.

### Amount on `deposit` and `withdrawal` lines

* **Shown (chosen).** Good, because `insufficient_funds` and `overflow` lines cannot be judged without it. Bad, because each line is a few bytes longer.
* **Left out.** Good, because it is exactly the R10 list. Bad, because the operator has to open the input to see the amount.

### Suppression notice

* **When the first suppressed row arrives (chosen).** Good, because it marks the line where detail stops, as clang and the kernel rate limiter do. Bad, because nothing.
* **At the end of the run.** Bad, because the reader cannot tell where detail stopped, and the information is already in the summary.

### Summary when no row was skipped

* **Always written (chosen).** Good, because the zero count is an explicit confirmation, and the output of every run has the same shape. Bad, because a clean run is not silent.
* **Omitted.** Good, because it follows the Unix rule of silence and clang's behavior. Bad, because "nothing skipped" and "diagnostics lost" look the same.

### Summary content

* **Total line plus reasons that occurred (chosen).** Good, because the common case is short and the order is fixed. Bad, because a script looking for a reason must treat a missing line as zero.
* **Every reason, including zeros.** Good, because the schema is fixed. Bad, because every run that skips a row writes 18 summary lines.

### Summary on a fatal run

* **Written before the fatal line (chosen).** Good, because the counts of the rows read before the failure are kept, and the fatal line stays last. Bad, because a failed run writes more than one stderr line.
* **Omitted.** Good, because a failed run keeps exactly one stderr line. Bad, because the counts for the rows already read are lost.

### Where to record the format

* **A new ADR (chosen).** Good, because ADR-0006 and ADR-0010 stay as accepted and this ADR only adds what they left open. Bad, because the reader follows one more link.
* **Edits to ADR-0006 and ADR-0010.** Bad, because accepted ADRs are not rewritten; a change is recorded in a new ADR (`AGENTS.md`).

## More Information

* Refines [ADR-0006](0006-dispute-lifecycle.md) R10 and [ADR-0010](0010-input-validation-and-errors.md) V11 without changing them: the budget, the notice, the per-reason counts, the escaping, and the single fatal line are unchanged. The fatal line is no longer the only stderr line of a failed run that read its input (D7), and [requirements §1](../requirements.md) says so.
* Industry practice consulted:
  * [OWASP Logging Cheat Sheet](https://cheatsheetseries.owasp.org/cheatsheets/Logging_Cheat_Sheet.html): log validation failures, prefer structured formats, encode CR, LF, and delimiters, and limit the size of logged input.
  * [logfmt](https://brandur.org/logfmt): flat `key=value` lines that are readable by humans and easy for machines to parse.
  * [GNU Coding Standards, Formatting Error Messages](https://ftp.gwdg.de/pub/gnu/www/prep/standards/html_node/Errors.html): `program:sourcefile:lineno: message` for non-interactive programs.
  * [Command Line Interface Guidelines](https://clig.dev/): messages go to stderr, keep the signal-to-noise ratio high, and group many similar errors.
  * Suppression: clang's [`-ferror-limit` test](https://github.com/llvm/llvm-project/blob/main/clang/test/Misc/error-limit.c) shows the errors up to the limit and then "too many errors emitted, stopping now"; the Linux kernel's [printk rate limiting](https://access.redhat.com/node/1315523) reports "N callbacks suppressed" when suppression ends.
  * Summary: clang ends a run with "N warnings and M errors generated." and writes nothing when there are none (see [Gentoo bug 875656](https://bugs.gentoo.org/show_bug.cgi?id=875656) for an example).
* Decided with the human while planning E10 (LYA-58).
