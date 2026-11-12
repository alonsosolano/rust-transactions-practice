---
status: accepted
date: 2026-11-11
decision-makers: Alonso Solano
---

# Place each test type, compare sample outputs byte for byte, and measure large inputs with a generator

## Context and Problem Statement

Correctness has to be shown, not claimed: sample data committed to the repository, unit tests for the non-trivial logic, and the type system where it helps ([requirements §8](../requirements.md)). [ADR-0013](0013-attack-and-invariant-catalog.md) C2 names five test types (Unit, Integration, CLI, Property, Type) and gives every row of the [requirements §6](../requirements.md) catalog at least one of them, but leaves open where each type lives, how sample files are laid out, how their output is compared, and how large inputs are produced. The issues that write the tests (E5 for sample cases, E6 for the catalog, E7 for large-input measurements) need those answers to start.

The automated gate compares our stdout with an expected output and ignores row order, whitespace, and trailing zeros ([requirements §1, §3](../requirements.md)). [ADR-0009](0009-output-format.md) already fixes one exact output per input, so our own tests can be stricter than the gate.

Where does each test type live, how is a sample case laid out and compared, how are tests named after the catalog, how are property tests run, how are large inputs generated, and which local tools measure test quality?

## Decision Drivers

* Every catalog row must map to a concrete test location, and a search for its ID must find its test ([ADR-0013](0013-attack-and-invariant-catalog.md) C1).
* Our tests must be at least as strict as the automated gate, so nothing the gate rejects can pass locally.
* A test oracle must be independent of the code under test. An expected output produced by the program itself proves only that the program agrees with itself.
* One failing case must not hide the others. A run should report every mismatch.
* Adding a sample case should need only new files, no code (E5 acceptance criterion).
* No new dependency without an ADR, and the lint and safety policy covers tests too ([ADR-0012](0012-dependencies-and-safety-policy.md) S1, S4, S10).
* The default `cargo test` stays fast enough to run before every push, because CI is manual-only ([ADR-0005](0005-toolchain-and-manual-ci.md)).
* Follow the conventions Rust projects already use (see [More Information](#more-information)).

## Considered Options

The choices below were decided one by one. Each lists the options considered and the chosen one.

1. **Output comparison:** byte for byte, by value (parse both CSVs, sort by client, compare numbers, as the gate does), or both.
2. **Sample case layout:** one directory per case with plain files and a standard-library harness, the exact `trycmd` flat layout read by our own harness, or the `trycmd` crate itself.
3. **stderr check:** exact on every case, exact only when an expected file exists, or a substring match.
4. **Expected files:** written by hand only, or an "update expected files" mode that rewrites them from the program's output.
5. **Failure reporting for sample cases:** one test that runs every case and reports all mismatches, stop at the first mismatch, or one generated test per case.
6. **Where unit and property tests live:** in the crate under `#[cfg(test)]`, behind a `test-support` feature, or in a separate crate.
7. **Property test case count:** the `proptest` default of 256 plus a documented deep run, a higher default, or the default only.
8. **Large inputs:** a generator under `examples/`, an `#[ignore]` test, or a shell script.
9. **Test-quality tools:** `cargo llvm-cov` and `cargo mutants` as optional local tools, the same with thresholds, coverage only, or none.
10. **Closed output streams:** reword the catalog to the condition the program can observe (a broken pipe), keep the wording, or open a separate decision.

## Decision Outcome

Chosen options: byte-for-byte comparison; one directory per sample case with plain files and a standard-library harness that reports every mismatch; exact stderr on every case; hand-written expected files only; unit and property tests in the crate; 256 property cases by default plus a documented deep run; a large-input generator under `examples/`; `cargo llvm-cov` and `cargo mutants` as optional local tools without thresholds; and catalog rows that name a broken pipe instead of a "closed" stream.

### Where each test type lives

| Type ([ADR-0013](0013-attack-and-invariant-catalog.md) C2) | Location | Sees | Runs in |
|------|----------|------|---------|
| Unit | A `#[cfg(test)] mod tests` at the end of the module it tests (`src/amount.rs`, `src/engine.rs`, …) | Private items of that module | `cargo test` |
| Property | The same in-crate test modules. Properties of the whole pipeline (I10, I12, I17, I18) live in the test module of `src/lib.rs` and call `run` on in-memory bytes. Shared strategies, `assert_invariants`, and the applied-row ledger of C5 live in `src/test_support.rs`, declared `#[cfg(test)] mod test_support;` | Crate-private items | `cargo test`, 256 cases per property |
| Integration | `tests/run.rs`, calling the public `run` with in-memory input, stdout, and stderr ([ADR-0011](0011-architecture-and-memory.md) S1). Shared helpers in `tests/common/mod.rs` | Public API only | `cargo test` |
| CLI | `tests/cli.rs`: one `sample_cases` test over `tests/cases/*/`, plus hand-written tests for what a file cannot express (T9) | The built binary only | `cargo test` |
| Type | No test of its own. The catalog row also names another type that catches a regression (C2) | — | — |
| Large input | `examples/generate.rs`, a generator run by hand (T12). `cargo test` builds it but does not run it | — | Manual, measured in E7 |

### Rules

| # | Rule | One-line justification |
|---|------|------------------------|
| T1 | Each test type lives where the table above says. Unit and property tests are in the crate under `#[cfg(test)]`; integration and CLI tests are under `tests/`. There is no `test-support` feature, and no test code is compiled into the shipped library. | The Rust Book's split: unit tests next to the code with access to private items, integration tests as an external crate on the public API. `assert_invariants` reads engine internals, which a crate under `tests/` cannot see. |
| T2 | **Naming.** A test that covers a catalog row starts with the row's ID in lowercase: `a1_withdraw_then_chargeback`, `i14_total_matches_applied_rows`. A test that covers several rows names them all in a `/// Covers: I1, I2, I4.` doc comment. A sample case directory is lowercase kebab-case: `<id>[<variant>]-<slug>` for a catalog row (`a26-example-output`, `a33a-empty-file`, where the variant letter matches the row's (a), (b), (c)), or just `<slug>` for a case that is not a catalog row (`interleaved-clients`). | `snake_case` for functions is the Rust API Guidelines convention (C-CASE). A case-insensitive search for an ID such as `A26` then finds both its test functions and its sample case, which is how C1 checks coverage. |
| T3 | **Sample case layout.** Each case is a directory `tests/cases/<case>/` with exactly four files: `input.csv`, `expected.stdout`, `expected.stderr`, and `expected.status` (the exit code as decimal digits followed by `\n`). A missing file or any other file in the directory fails the case. | Cargo's own test suite (via `snapbox`/`trycmd`) and rustc's UI tests both keep expected output in `.stdout` and `.stderr` files next to the case. Requiring all four files means a typo such as `expected.sdout` fails instead of silently skipping a check. |
| T4 | **How a case runs and is compared.** The harness runs the binary from `env!("CARGO_BIN_EXE_rust-transactions-practice")` with the case directory as its working directory, the single argument `input.csv`, and stdin closed (`Stdio::null()`). stdout and stderr are compared with the expected files **byte for byte**, and the exit code with `expected.status`. No normalization of any kind: no line-ending, whitespace, path, or trailing-zero rewriting. | The output has exactly one valid form ([ADR-0009](0009-output-format.md)), so an exact match is stricter than the gate and implies it. Running from the case directory with a relative path mirrors the fixed CLI contract and keeps any echoed path the same on every machine. `trycmd` normalizes newlines by default, which would hide an `\r\n` in our output. |
| T5 | **Every case runs; every mismatch is reported.** `sample_cases` visits the cases in sorted name order, runs all of them, collects every mismatch, and fails once at the end with a report that names each failing case, the stream (`stdout`, `stderr`, or `status`), and the first differing line with expected and actual shown escaped. Finding zero cases is a failure. | One report shows the whole picture after a change. A wrong path or an empty directory would otherwise pass without testing anything. |
| T6 | **Fail-closed cases.** A case whose `expected.status` is not `0` must have an empty `expected.stdout`; the harness checks this before running the case. | This is I12 (a failed run leaves stdout empty) applied to the expected files themselves, so a wrong expected file cannot encode a partial output. |
| T7 | **Expected files are written by hand**, from the expected behavior in the catalog row or worked out by hand from the rules. There is no mode that rewrites them from the program's output. | The oracle stays independent of the implementation. An overwrite mode (rustc's `--bless`, `TRYCMD=overwrite`) makes it one command to accept a bug as the new expected output; rustc's guide itself asks for a manual review after blessing. |
| T8 | **Byte preservation.** `.gitattributes` sets `tests/cases/** -text`, so git never converts line endings in sample files on any platform. The harness also checks that the A43 input still starts with a UTF-8 byte order mark and contains `\r\n`, so a rewrite by an editor or tool fails the test instead of silently weakening it. | Unsetting `text` disables end-of-line conversion on check-in and checkout, and an explicit attribute overrides `core.autocrlf`. `-text` keeps diffs readable in review, which the `binary` macro would hide. |
| T9 | **Hand-written CLI tests** in `tests/cli.rs` cover what a sample case cannot express: no argument and two arguments (A34 a, b), a path that does not exist (A34 c), stdout or stderr connected to a pipe whose reader has closed (A34 d, A16), and running A26's input twice and comparing the outputs (I10). A broken pipe is made by spawning the binary with `Stdio::piped()` and dropping the read end right after `spawn`. | Exit codes and stream failures are only observable through the process. Dropping the reader before the binary writes gives a broken pipe on the write. Nothing orders the parent's drop before the child's write, so this is a race: stdout is written only after the whole input is read ([ADR-0011](0011-architecture-and-memory.md) S12), which makes it very likely, but not certain, that the reader is gone first. If the race is lost, the write succeeds and the test fails with exit `0`, so it can fail spuriously but never pass wrongly. `std::io::pipe` would remove the race but needs Rust 1.87, above our `rust-version` of 1.85 ([ADR-0012](0012-dependencies-and-safety-policy.md) S5). |
| T10 | **A stream closed before the run starts is not an error.** Rust's standard library treats a write to a closed stdout or stderr file descriptor (`EBADF`) as successful. The run then exits `0` and the CSV is discarded, and the program cannot detect it without `libc` or `unsafe` (S1, S4). The catalog therefore names the condition the program can observe: a pipe whose reader has closed, which fails the write with a broken pipe (`EPIPE`). | Verified on the pinned toolchain: with stdout closed (`>&-`) or set to a read-only file the binary exits `0` (200 of 200 runs), and with a closed-reader pipe it exits `74` (200 of 200). The behavior is in `handle_ebadf` in the standard library's `io/stdio.rs`. Accepted limitation; a caller that closes stdout has asked for no output. |
| T11 | **Property tests** use `proptest` with only `std` ([ADR-0012](0012-dependencies-and-safety-policy.md)). Code sets no case count, so each property runs the default 256 cases, which `PROPTEST_CASES` overrides. Before a pull request that changes `amount`, `input`, `store`, or `engine`, run the deep check `PROPTEST_CASES=10000 cargo test --release`. Failing cases are saved under `proptest-regressions/` and committed. Strategies draw client and `tx` IDs from small pools and include invalid rows, so every row type reaches its applied, rejected, and ignored paths, and IDs collide across clients. | 256 keeps the default `cargo test` fast; the environment variable gives more cases without a code change. Committing the regression files means every later run replays past failures first, as the `proptest` book recommends. With uniform IDs over the full `u32` range, almost every dispute would name an unknown transaction and test only the "ignore" path. |
| T12 | **Large inputs** come from `examples/generate.rs`, which uses only the standard library and writes CSV to stdout: `cargo run --release --example generate -- <rows> <seed> <profile>`. The same arguments always give the same bytes. Profiles: `mixed` (a realistic mix of all five types over many clients) and `stateless` (only rows that bind no `tx` ID: disputes on unknown IDs, invalid rows, and duplicates). Its random numbers come from a small seeded generator written in the file with `wrapping_*` arithmetic, not from a crate. Generated files are never committed. Peak memory and time are measured outside the process by E7 (`/usr/bin/time -l` on macOS, `/usr/bin/time -v` on Linux) on a release build. | A separate program scales to any size without slowing `cargo test`, and a fixed seed makes every measurement reproducible. Measuring memory inside a test would need a counting global allocator, and implementing `GlobalAlloc` requires `unsafe`, which S4 forbids in every target. A15's million ignored rows remain an integration test, generated in memory by `tests/run.rs`. |
| T13 | **Test-quality tools are optional and local.** `cargo llvm-cov` reports line coverage and `cargo mutants` reports mutants that no test catches; missed mutants are reviewed by hand. Neither has a threshold or blocks a push. Both install as cargo subcommands on the development toolchain and add no dependency to the crate. `/mutants.out*` is ignored by git. | Coverage shows code no test runs; mutation testing shows code whose behavior no test checks, which is closer to the question "would the tests notice a bug". A percentage target is easy to satisfy without testing the right things, and the brief does not ask for one. |
| T14 | The gate in `CONTRIBUTING.md` stays `cargo fmt`, `cargo clippy --all-targets`, and `cargo test`. `cargo test` runs every unit, property, integration, and CLI test, and builds the generator. The deep property run (T11), the generator (T12), and the tools (T13) are documented commands, not part of the gate. | One command runs every required test, and the slower checks run when the change calls for them. |

### Catalog type to location

Every test type that [requirements §6](../requirements.md) uses has a location, so every row has a place to go:

| Rows (by test type) | Location |
|---------------------|----------|
| Unit: A1 to A14, A23 to A25, A27 to A29, A38 to A41, A46; I3, I6, I7, I11, I16 | `#[cfg(test)]` module of `engine`, `account`, `store`, or `amount`, with `assert_invariants` after every step (C4) |
| Integration: A6, A15 to A22, A30 to A32, A35, A36, A43 to A45; I17 | `tests/run.rs` |
| CLI, sample case: A26, A33 (a, b, c), A37 (a, b), A42 (a, b, c), A43 | `tests/cases/<case>/`, run by `sample_cases` |
| CLI, hand-written: A16, A34 (a to d); I10, I12 | `tests/cli.rs`. I12 is also enforced on every sample case by T6 |
| Property: I1 to I18 | In-crate property tests (T1, T11); I10, I12, I17, I18 at the `run` level in `src/lib.rs` |
| Type: I1, I2, I5 | Covered by the property tests above (C2) |

Rows with more than one type appear under each; the catalog's Test column remains the source of truth, and this table only shows that each type has a home.

### Consequences

* Good, because every catalog row has a concrete file to live in, and E5, E6, and E7 can start from this ADR.
* Good, because sample cases are checked more strictly than the gate checks them, so a formatting regression fails locally before it can fail the gate.
* Good, because a new sample case is four files and no code, and one run reports every failing case.
* Good, because no dependency is added: the harness is `std::process::Command` and `std::fs`, and the generator has its own small random number generator.
* Bad, because exact stderr comparison means any change to a diagnostic's wording touches every expected file that shows it. Accepted: diagnostics are part of the contract ([ADR-0010](0010-input-validation-and-errors.md) V11), and the diff makes the change visible in review.
* Bad, because hand-written expected files take longer to produce than blessed ones. Accepted: an independent oracle is the point of the tests.
* Bad, because the sample-case harness reports all cases as one test, so a filter cannot run a single case. Accepted: the failure report names each case, and adding a per-case harness would need a dependency or `harness = false`.
* Bad, because the broken-pipe test in T9 races the parent's drop of the reader against the child's write. Accepted: deferring stdout until the input is read (S12) makes a lost race unlikely, a lost race shows up as a failing test rather than a false pass, and the race is gone once `rust-version` reaches 1.87 and the test can switch to `std::io::pipe`.
* Neutral, because coverage and mutation results are advisory and depend on someone running them.

### Confirmation

* `tests/cases/` exists with the A26 case before E5 is considered done, and `sample_cases` fails on an empty or missing directory.
* Review check: no test code writes an `expected.*` file, and no harness normalizes output.
* `git check-attr text tests/cases/a43-*/input.csv` prints `text: unset`.
* A search for each catalog ID, ignoring case, finds at least one test function or sample case once E6 is done ([ADR-0013](0013-attack-and-invariant-catalog.md) C1).
* `proptest-regressions/` is committed whenever a property test has failed.
* E7 reports its measurements with the exact `generate` arguments used.

## Pros and Cons of the Options

### Output comparison

* **Byte for byte (chosen).** Good, because ADR-0009 fixes one exact output per input, so any byte difference is a real change. Good, because it implies equality by value and catches what the gate tolerates but the contract forbids (`-0.0000`, `\r\n`, a missing header on an empty run). Bad, because a harmless formatting change touches every expected file. Accepted: ADR-0009 does not allow harmless variants.
* **By value, as the gate does.** Good, because it mirrors the external check. Bad, because it needs a CSV parser and number comparison in the tests, which is a second implementation that could be wrong, and it would let a format regression through.
* **Both.** Bad, because no case could pass byte comparison and fail value comparison, so the second comparator would be dead code.

### Sample case layout

* **One directory per case, standard-library harness (chosen).** Good, because adding a case is creating four files. Good, because the file names follow the `.stdout`/`.stderr` convention of Cargo's test suite and rustc. Good, because no dependency is added. Bad, because the harness is our own code and must be reviewed.
* **Exact `trycmd` flat layout (`<case>.toml`, `<case>.stdout`, `<case>.stderr`, `<case>.in/`).** Good, because the files could later be run by `trycmd` unchanged. Bad, because our harness would need to read TOML, which means a parser dependency or hand parsing.
* **The `trycmd` crate.** Good, because it is the tool Cargo's ecosystem uses (8.1M downloads, `rust-version` 1.85). Bad, because it adds about 50 crates to the dev graph, including `rustix`, `tempfile`, and `wait-timeout`, which ADR-0012 removed from `proptest` on purpose. Bad, because it normalizes newlines unless set to binary mode.

### stderr check

* **Exact on every case (chosen).** Good, because diagnostics are part of the contract (ADR-0010 V11, ADR-0006 R10) and an extra, missing, or reworded line is caught. Bad, because wording changes ripple through expected files.
* **Exact only when the file exists.** Good, because cases about stdout need fewer files. Bad, because a forgotten file silently disables the check.
* **Substring match.** Good, because it tolerates wording changes. Bad, because it cannot catch an extra diagnostic or a summary with a wrong count.

### Expected files

* **Hand-written only (chosen).** Good, because the expected output comes from the rules, not from the code. Bad, because writing them takes longer.
* **Overwrite mode.** Good, because large outputs are cheap to update. Bad, because accepting a bug is one command, and the review of the diff is the only safeguard.

### Failure reporting

* **One test, all cases, one report (chosen).** Good, because a run shows every failing case. Good, because it needs no code per case. Bad, because `cargo test <filter>` cannot pick one case.
* **Stop at the first mismatch.** Bad, because each fix reveals the next failure one run at a time.
* **One generated test per case.** Good, because each case is its own test. Bad, because it needs a build script that generates code, a macro that lists every case (code per case), or a custom harness crate with `harness = false`, which is a dependency.

### Where unit and property tests live

* **In the crate under `#[cfg(test)]` (chosen).** Good, because tests can read the engine's private state, which `assert_invariants` needs. Good, because nothing test-only is compiled into the library.
* **A `test-support` feature.** Good, because tests under `tests/` could use the helpers. Bad, because the helpers become part of the library's API for anyone who enables the feature.
* **A separate test crate.** Bad, because it needs a workspace and still cannot see private items.

### Property test case count

* **256 plus a documented deep run (chosen).** Good, because the default run stays fast and a deeper run needs no code change. Bad, because the deep run depends on discipline.
* **A higher default.** Good, because every run searches more. Bad, because every `cargo test` gets slower, including the many runs during development.
* **Default only.** Bad, because rare interleavings in long row streams get fewer chances to show up.

### Large inputs

* **Generator under `examples/` (chosen).** Good, because it produces any size on demand, is compiled and linted with the crate, and keeps the test suite fast. Good, because a fixed seed makes measurements reproducible. Bad, because it is run by hand.
* **An `#[ignore]` test.** Good, because `cargo test -- --ignored` runs it. Bad, because a test cannot measure its own peak memory without `unsafe`, and it mixes measurement with pass/fail.
* **A shell script.** Bad, because it is a second language to maintain, and random generation in shell is slow and harder to make deterministic across platforms.

### Test-quality tools

* **Both, no threshold (chosen).** Good, because coverage finds unrun code and mutation testing finds unchecked code. Good, because neither adds a crate dependency. Bad, because the results are advisory.
* **Both, with thresholds.** Good, because they would be enforced. Bad, because `cargo mutants` reruns the suite once per mutant, which is slow for a local gate, and a coverage percentage can be met without testing the right things.
* **Coverage only.** Bad, because full line coverage says nothing about whether the assertions would notice a wrong result.
* **None.** Bad, because there would be no evidence beyond the tests themselves.

### Closed output streams

* **Reword the catalog to a broken pipe (chosen).** Good, because the requirement then states what the program can actually detect, and the test reproduces it. Bad, because a caller that closes stdout before the run gets exit `0` and no output. Accepted and recorded in T10.
* **Keep the wording.** Bad, because the requirement would promise exit `74` in a case the standard library hides from the program.
* **Separate decision.** Bad, because detecting a closed descriptor needs `libc` or `unsafe`, which ADR-0012 forbids, for a case where the caller has asked for no output.

## More Information

* Data sources: crate data from the crates.io API (`trycmd` 1.2.1, `cargo-llvm-cov` 0.9.1, `cargo-mutants` 27.1.0) and the dependency graph of `trycmd` from `cargo tree` in a scratch crate, all on 2026-11-11. The `proptest` failure-persistence location (`proptest-regressions/<module>.txt` for a test in `src/<module>.rs`), the `PROPTEST_CASES` override with only the `std` feature, `CARGO_BIN_EXE_<name>` with a hyphenated binary name, the closed-descriptor and broken-pipe exit codes, and the fact that `cargo test` builds examples were all verified by running them in a scratch crate on the pinned toolchain.
* References:
  * The Rust Book, [Test Organization](https://doc.rust-lang.org/book/ch11-03-test-organization.html): unit tests in `#[cfg(test)]` modules, integration tests under `tests/`, shared helpers in `tests/common/mod.rs`.
  * [Cargo environment variables for integration tests](https://doc.rust-lang.org/cargo/reference/environment-variables.html#environment-variables-cargo-sets-for-crates) (`CARGO_BIN_EXE_<name>`, `CARGO_MANIFEST_DIR`).
  * [`trycmd` documentation](https://docs.rs/trycmd/latest/trycmd/): `.stdout` and `.stderr` files next to each case, a missing file skips the check, newline normalization by default, `TRYCMD=overwrite`.
  * [rustc dev guide, UI tests](https://rustc-dev-guide.rust-lang.org/tests/ui.html): `.stdout` and `.stderr` snapshot files, `--bless`, and the instruction to review blessed output by hand.
  * [`proptest` book, failure persistence](https://proptest-rs.github.io/proptest/proptest/failure-persistence.html) and [`proptest::test_runner::Config`](https://docs.rs/proptest/1.11.0/proptest/test_runner/struct.Config.html) (default 256 cases, `PROPTEST_CASES`).
  * [gitattributes](https://git-scm.com/docs/gitattributes): unsetting `text` disables end-of-line conversion; `binary` is `-diff -merge -text`.
  * The Go command's [`testdata` convention](https://pkg.go.dev/cmd/go#hdr-Test_packages), the same idea of committed input files next to the tests.
  * [Rust API Guidelines, naming (C-CASE)](https://rust-lang.github.io/api-guidelines/naming.html).
  * Rust standard library source, `library/std/src/io/stdio.rs`, `handle_ebadf`: a write to a closed stdio descriptor returns success.
  * [`GlobalAlloc`](https://doc.rust-lang.org/std/alloc/trait.GlobalAlloc.html) is an `unsafe trait`.
  * [cargo-llvm-cov](https://github.com/taiki-e/cargo-llvm-cov) (line coverage on stable; branch coverage needs nightly) and [cargo-mutants](https://mutants.rs/) (outcomes caught, missed, unviable, timeout; output in `mutants.out`, which its guide says to ignore in git).
* Changes the wording of A16 and A34 (d) in [requirements §6](../requirements.md) to name a broken pipe (T10). No behavior rule changes, so no catalog row is added (C7).
* Revisit this decision if `rust-version` reaches 1.87 (switch T9 to `std::io::pipe`), if the number of sample cases makes one combined test hard to use, or if CI becomes automatic and the deep property run or the tools should join it.
* Tracked by issue LYA-47 (D9). The sample cases and harness are written in E5 (LYA-53), the catalog tests in E6 (LYA-54), and the generator and measurements in E7 (LYA-55).
