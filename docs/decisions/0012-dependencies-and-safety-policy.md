---
status: accepted
date: 2026-11-10
decision-makers: Alonso Solano
---

# Depend on three vetted crates and enforce a no-panic, no-unsafe lint policy

## Context and Problem Statement

Dependencies must be common, well-maintained crates that we consider secure, and each one needs a recorded reason ([requirements §9](../requirements.md)). Reviewers also ask whether anything risky is done and why: `unsafe`, `unwrap`, lossy numeric conversions ([requirements §8](../requirements.md)). [ADR-0007](0007-money-representation.md) removed the need for a decimal crate and left open where its numeric lints apply. `Cargo.toml` sets `rust-version = "1.85"` as a placeholder until the dependencies are known.

Which crates does the engine depend on and why are they trusted, which compiler and clippy rules make the safety claims checkable, and how is the dependency graph kept under control over time?

## Decision Drivers

* Every crate in the graph is code we ship or run without writing it. Fewer crates, from maintainers with a long track record, is better.
* No input may cause a panic ([requirements §8](../requirements.md)). The rule must be enforced by the compiler, not only by review.
* Every exception to a safety rule must carry a written reason next to the code.
* Stdout must contain only the output CSV, and a failure to write stderr must never affect the run ([requirements §1, §5](../requirements.md)).
* Follow what the Rust ecosystem does: crates and tools that are the de facto standard for the job, configured the way their maintainers document.
* The checks must run locally, because CI is manual-only ([ADR-0005](0005-toolchain-and-manual-ci.md)).
* The policy must be reversible: adding a crate or relaxing a lint later is a small, recorded change.

## Considered Options

The choices below were decided one by one. Each lists the options considered and the chosen one.

1. **CSV parsing:** the `csv` crate, or a hand-written parser.
2. **Row decoding:** `serde` derive on a row struct, or manual field access on `csv::StringRecord`.
3. **Error types:** `thiserror` for the error enums, hand-written `Display` and `Error` impls, or `anyhow` in `main`.
4. **CLI tests:** `std::process::Command` with Cargo's `CARGO_BIN_EXE_<name>`, or `assert_cmd` with `predicates` (and `tempfile`).
5. **Property-based tests:** `proptest` with only the `std` feature, `proptest` with default features, `quickcheck`, or none.
6. **Supply-chain checks:** `cargo-deny`, `cargo-audit`, both, or none.
7. **Lint policy:** a targeted list of denied lints in `Cargo.toml`, the whole `clippy::restriction` group, or crate-level `#![deny]` attributes.
8. **`rust-version`:** keep the edition 2024 minimum (1.85), or track the latest stable.

## Decision Outcome

Chosen options: `csv`, `serde` derive, and `thiserror` as the only runtime dependencies; plain `std::process::Command` for CLI tests; `proptest` with only `std` for property tests; `cargo-deny` for supply-chain checks; a targeted lint list in `Cargo.toml` that forbids `unsafe` and denies every panicking or lossy construct outside tests; and `rust-version = "1.85"`.

### Dependencies

Versions are the latest that build on Rust 1.85, checked on 2026-11-10. Download counts are all-time totals from crates.io.

| Crate | Version | Kind | Features | Purpose | Trust reasoning |
|-------|---------|------|----------|---------|-----------------|
| `csv` | 1.4.0 | runtime | defaults (none) | Read the input CSV with header handling, whitespace trimming (`Trim::All`), and missing trailing fields (`flexible`); write the output CSV. | By Andrew Gallant (BurntSushi), author of `regex` and ripgrep. 289M downloads. Named by the brief. Its only `unsafe` is 4 `from_utf8_unchecked` calls on bytes it has already validated as UTF-8. The parser itself lives in `csv-core`, which has no `unsafe` and no allocation. |
| `serde` | 1.0.229 | runtime | defaults (`std`) + `derive` | Deserialize each record into a row struct by header name, so the code never indexes fields by position and `Option` covers an empty or missing `amount`. | By David Tolnay (dtolnay) under the `serde-rs` organization. 1.68B downloads; the standard serialization framework of the ecosystem. Named by the brief. `csv` already depends on `serde_core`, so the facade only adds `derive`. |
| `thiserror` | 2.0.21 | runtime | defaults (`std`) | Derive `Display` and `std::error::Error` for the error enums that D5 defines, so each variant's message sits next to its definition. | By dtolnay. 1.82B downloads. No `unsafe`. It generates the same trait impls we would write by hand and adds nothing to the public API. Its macro crate reuses the `syn`/`quote`/`proc-macro2` stack that `serde_derive` already brings in, so it adds two crates. |
| `proptest` | 1.11.0 | dev only | `default-features = false`, `std` | Property-based tests required by [ADR-0007](0007-money-representation.md) (format-then-parse round trip) and for the invariants in [requirements §6](../requirements.md). | Maintained by the `proptest-rs` organization. 241M downloads. Never linked into the shipped binary. Turning off the default `fork`, `timeout`, and `bit-set` features removes `rusty-fork`, `tempfile`, `rustix`, `wait-timeout`, and `bit-set` from the graph. Version 1.12.0 needs Rust 1.88, so the MSRV-aware resolver keeps 1.11.0 (see S5). |

Transitive crates, as resolved on 2026-11-10. `unsafe` counts are the lines in each crate's published source that open an `unsafe` block, function, or impl. They show where audit effort concentrates, not a defect.

| Crate | Pulled in by | Role | `unsafe` |
|-------|--------------|------|----------|
| `csv-core` 0.1.13 | `csv` | The CSV state machine, `no_std`, no allocation | 0 |
| `memchr` 2.8.3 | `csv-core` | SIMD byte search. BurntSushi; 1.7B downloads; also under `regex` and ripgrep | 333 (SIMD intrinsics) |
| `itoa` 1.0.19, `ryu` 1.0.23 | `csv` | Integer and float formatting for `csv`'s serializer. dtolnay. `ryu` is compiled in but unused, because our output never formats a float (ADR-0007 M8) | 16, 29 |
| `serde_core` 1.0.229 | `csv`, `serde` | The serde traits | 2 |
| `serde_derive`, `thiserror-impl` | `serde`, `thiserror` | Procedural macros; run at compile time only, not linked into the binary | 0 |
| `syn` 3.0.10, `quote` 1.0.47, `proc-macro2` 1.0.107, `unicode-ident` 1.0.27 | the two macro crates | Compile-time Rust parsing. dtolnay; `syn` has 3.0B downloads, the most of any crate | 44, 0, 6, 2 |
| `rand`, `rand_core`, `rand_chacha`, `rand_xorshift`, `ppv-lite86`, `zerocopy`, `getrandom`, `libc`, `cfg-if`, `regex-syntax`, `num-traits`, `autocfg`, `bitflags`, `unarray` | `proptest` | Random generation and strategies for property tests; dev only | not audited (dev only) |

In total, 8 crates are linked into the binary, 6 run only at compile time, and 15 are dev-only. Licenses in the graph: MIT, Apache-2.0, Unlicense, BSL-1.0 (`ryu`), and Unicode-3.0 (`unicode-ident`), all permissive.

### Rules

| # | Rule | One-line justification |
|---|------|------------------------|
| S1 | The dependencies are exactly the four in the table above. Adding, replacing, or removing one needs a new ADR with the same table columns that links back here. | §9 asks for a recorded reason per crate, and an unrecorded crate is unreviewed code. |
| S2 | Version requirements are caret requirements on the full tested version (`"1.4.0"`, not `"1"` or `"=1.4.0"`). `Cargo.lock` is committed, and CI runs every cargo command with `--locked`. Features are listed explicitly, and default features are turned off where they are not needed. | The lockfile pins what was tested. Exact `=` pins would block compatible security fixes. Unneeded features are unneeded code. |
| S3 | `cargo deny check` passes, using `deny.toml`: no RustSec advisory (vulnerable, unsound, or unmaintained), no yanked version, only the allowed licenses, no duplicate versions of a crate, no wildcard requirements, and crates.io as the only source. | One tool covers advisories, licenses, duplicates, and sources. It uses the same RustSec database as `cargo-audit`. |
| S4 | No `unsafe` code in this crate: `unsafe_code = "forbid"`. Non-ASCII identifiers are forbidden too. | `forbid` cannot be overridden by an inner `allow` or `expect`. ASCII identifiers rule out look-alike names in review. |
| S5 | `rust-version = "1.85"`, the minimum for edition 2024. The edition 2024 resolver is MSRV-aware, so it only picks dependency versions that build on 1.85. CI checks the build with Rust 1.85.0. Raising `rust-version` is a deliberate change with its own commit and reason. | The widest compatibility at no cost, since every dependency above supports 1.85. Without the CI check the declared value would be a claim, not a fact. |
| S6 | Outside tests, no construct that panics on bad data: `unwrap`, `expect`, `panic!`, `todo!`, `unimplemented!`, `unreachable!`, slice or map indexing (`v[i]`), and string slicing (`&s[a..b]`) are denied. Fallible lookups use `get`, and errors propagate with `?`. | The no-panic rule of §8 becomes a compile-time check instead of a promise. |
| S7 | Outside tests, numbers follow [ADR-0007](0007-money-representation.md) crate-wide: `arithmetic_side_effects`, `float_arithmetic`, and `as_conversions` are denied, and the types `f32` and `f64` are disallowed. Division or remainder by a non-zero literal (`/ 10_000`) is allowed by the lint. Counters outside the money path (such as diagnostic counts) may use `saturating_add` where saturation is harmless, with a comment saying why. | Overflow and lossy casts are the silent numeric bugs. Applying the lints crate-wide means no module can opt out by accident. |
| S8 | All output goes through explicit writers: `print!`, `println!`, `eprint!`, `eprintln!`, and `dbg!` are denied. Stdout is written through one writer that carries the CSV. Stderr writes use `writeln!` and drop the result. `main` returns `std::process::ExitCode` and never calls `std::process::exit`. | `println!` and `eprintln!` panic when the write fails (a closed pipe, scenario A16). A stray `println!` would corrupt the output CSV. `exit` skips destructors, so a buffered stdout would not be flushed. |
| S9 | `#[allow]` is denied. A lint exception uses `#[expect(lint, reason = "...")]` at the narrowest scope (one expression, function, or test module). | Every exception carries its justification, as §8 requires. `expect` also fails the build once the exception is no longer needed, so stale exceptions cannot pile up. |
| S10 | Tests may use `unwrap`, `expect`, `panic!`, indexing, and printing (via `clippy.toml`), and plain arithmetic. `as` conversions stay denied in tests. Integration test functions return `Result<(), Box<dyn Error>>` and use `?`, because helper functions outside `#[test]` are not covered by the test exemption. | A failing assertion in a test is the desired outcome, not a defect. |
| S11 | The release profile keeps overflow checks on (`overflow-checks = true`). | Defense in depth behind S7: if an unchecked operation ever slips in, it panics instead of wrapping silently. A crash is visible; a wrong balance is not. |
| S12 | The lints in S6 to S8 flag specific constructs; they do not prove the absence of panics. A function from the standard library or a dependency can still panic on input it is given, as its documentation lists under "Panics" (for example `str::split_at` on an index that is out of bounds or not on a character boundary). Code that handles input data uses the non-panicking variant where one exists (`split_at_checked`, `get`, `checked_*`). Review checks the "Panics" section of every such API called on input data, and the input-handling functions get property tests on arbitrary input that assert they return an error instead of panicking. | Closes the gap the lints leave, so the §8 no-panic claim rests on lints, review, and tests together. |

### Lint and build configuration

E1 copies these blocks into `Cargo.toml` and a new `clippy.toml`. They were checked with clippy on Rust 1.96.1 and the full dependency set on Rust 1.85.0.

```toml
# Cargo.toml (additions)
[package]
rust-version = "1.85"
publish = false

[dependencies]
csv = "1.4.0"
serde = { version = "1.0.229", features = ["derive"] }
thiserror = "2.0.21"

[dev-dependencies]
proptest = { version = "1.11.0", default-features = false, features = ["std"] }

[lints.rust]
unsafe_code = "forbid"         # S4
non_ascii_idents = "forbid"    # S4

[lints.clippy]
# S6: no panics on bad data
unwrap_used = "deny"
expect_used = "deny"
panic = "deny"
todo = "deny"
unimplemented = "deny"
unreachable = "deny"
indexing_slicing = "deny"
string_slice = "deny"
# S7: numbers (ADR-0007)
arithmetic_side_effects = "deny"
float_arithmetic = "deny"
as_conversions = "deny"
disallowed_types = "deny"
# S8: output only through explicit writers
print_stdout = "deny"
print_stderr = "deny"
dbg_macro = "deny"
exit = "deny"
# S9: every exception has a reason
allow_attributes = "deny"
allow_attributes_without_reason = "deny"

[profile.release]
overflow-checks = true         # S11
```

```toml
# clippy.toml
# S10: tests may panic, index, and print.
allow-unwrap-in-tests = true
allow-expect-in-tests = true
allow-panic-in-tests = true
allow-indexing-slicing-in-tests = true
allow-print-in-tests = true
allow-dbg-in-tests = true

# S7: no floating point anywhere (ADR-0007 M8).
disallowed-types = [
  { path = "f32", reason = "no floating point (ADR-0007 M8)" },
  { path = "f64", reason = "no floating point (ADR-0007 M8)" },
]
```

The lints are `deny` rather than `warn`, so a plain `cargo clippy` already fails on a violation, without relying on `-D warnings`.

`deny.toml` (S3) is added by this decision, together with the `cargo deny check` command in [AGENTS.md](../../AGENTS.md) and [CONTRIBUTING.md](../../CONTRIBUTING.md) and the deny and MSRV steps in the manual CI workflow. `publish = false` is also set now, so `cargo-deny` does not treat this unpublished crate as an unlicensed package.

### Consequences

* Good, because the binary links 8 crates, all by two maintainers with a long public track record (BurntSushi and dtolnay), and the shipped `unsafe` is concentrated in `memchr`'s SIMD code.
* Good, because the constructs that most often panic or lose data (`unwrap`, indexing, `as`, unchecked arithmetic, floats, `unsafe`) are rejected by every `cargo clippy`, so review can focus on what the lints cannot see: panics inside called APIs (S12).
* Bad, because the no-panic claim still depends on review and tests for those API calls; the lints narrow the search but do not settle it.
* Good, because every remaining exception is a `#[expect(..., reason = "...")]` that a reviewer can find with one search.
* Good, because `cargo deny check` catches new advisories against the committed lockfile even when no code changes.
* Bad, because the strict lints make some code longer: `get(..)` with error handling instead of `v[i]`, `checked_add` instead of `+`, `u64::from` instead of `as`.
* Bad, because `serde_derive` and `thiserror-impl` run procedural macros on the build machine. This is the normal cost of the ecosystem's standard crates, limited here to crates by the author of `syn` itself.
* Bad, because `cargo deny check` needs network access to update the advisory database and is one more tool to install.
* Neutral, because `ryu` is compiled in through `csv` without being used. Removing it would mean replacing `csv`.
* Neutral, because pinning `rust-version` to 1.85 holds `proptest` at 1.11. Raising it later is a one-line change under S5.

### Confirmation

* `cargo clippy --all-targets -- -D warnings` enforces S4 and S6 to S10. A violation fails the build.
* `cargo deny check` enforces S3. It runs locally whenever `Cargo.toml` or `Cargo.lock` changes, and in the manual CI workflow.
* The manual CI workflow runs `cargo +1.85.0 check --all-targets --locked` for S5 and uses `--locked` on every cargo command for S2.
* Review checks: the dependency list in `Cargo.toml` matches the table above (S1); `main` returns `ExitCode` and never calls `std::process::exit`, which clippy's `exit` lint allows inside `main` (S8); every `#[expect]` has a reason that would convince a reviewer (S9); every standard-library or dependency function called on input data has been checked for documented panics, and a non-panicking variant is used where one exists (S12).
* Property tests feed arbitrary strings and records to the input-handling functions (row decoding, amount parsing) and assert that each call returns a value or an error, never a panic (S12).
* Scenario A16 in [requirements §6](../requirements.md) covers the S8 behavior: a failing stderr never panics and never changes stdout or the exit code.

## Pros and Cons of the Options

### CSV parsing

* **`csv` crate (chosen).** Good, because it handles quoting, trimming, flexible field counts, and headers, and its core has been fuzzed and used in production for years. Good, because the brief names it. Bad, because it brings `itoa`, `ryu`, and `memchr` along.
* **Hand-written parser.** Good, because it has no dependencies. Bad, because quoting and edge cases are exactly the kind of parsing code that hides bugs, and it would need its own test suite for a solved problem.

### Row decoding

* **`serde` derive (chosen).** Good, because fields are matched by header name and an empty or missing `amount` maps to `Option`, with no positional indexing for S6 to forbid. Good, because it is the decoding style the `csv` documentation teaches. Good, because the row struct can borrow `&str` fields from a reused record, so decoding allocates nothing per row. Neutral, because values are validated in a second step (strict amount parsing under ADR-0007 M3) whichever option is chosen. Bad, because it adds the `syn` macro stack at compile time.
* **Manual `StringRecord` access.** Good, because it adds no crate beyond `csv`. Bad, because every field access is a positional `get(i)` with its own error branch, and column handling is code we maintain instead of a derive.

### Error types

* **`thiserror` (chosen).** Good, because each variant's message is written next to the variant, and the generated code is the standard `Error` impl. Good, because it costs two crates, both by the author of the macro stack already in the graph. Bad, because it is one more direct dependency.
* **Hand-written impls.** Good, because no dependency. Bad, because each enum needs a `Display` match and an `Error` impl kept in sync by hand.
* **`anyhow` in `main`.** Good, because adding context to errors is easy. Bad, because it erases the error type, so choosing an exit code per error class (D5) would need downcasting. Bad, because it relies on a hand-built vtable with substantial `unsafe`, and a small binary with one error enum gains little from it.

### CLI tests

* **`std::process::Command` with `CARGO_BIN_EXE_<name>` (chosen).** Good, because Cargo builds the binary and passes its path to integration tests, so no crate is needed. Good, because the tests compare stdout, stderr, and the exit code exactly, which is enough when the output is deterministic. Input files are committed sample CSVs under `tests/`. Bad, because assertion failures show raw byte strings instead of a formatted diff.
* **`assert_cmd` + `predicates` (+ `tempfile`).** Good, because of fluent assertions and readable diffs. Bad, because they add about 25 crates, including `regex`, `libc`, and `rustix`, to replace a few lines of standard library code.

### Property-based tests

* **`proptest` with only `std` (chosen).** Good, because it is the most used property-testing crate, with composable strategies and automatic shrinking to a minimal failing case. Good, because dropping the default features removes the process-forking support we do not need. Bad, because its random-generation stack is still 14 crates (dev only).
* **`proptest` with default features.** Bad, because it adds `rusty-fork`, `tempfile`, `rustix`, and `wait-timeout` for test isolation we do not use.
* **`quickcheck`.** Good, because it has fewer dependencies. Bad, because shrinking is per type rather than per strategy, which makes constrained inputs such as valid amount strings harder to generate and shrink. Less adoption (79M downloads).
* **None.** Bad, because ADR-0007 already requires a property-based round-trip test.

### Supply-chain checks

* **`cargo-deny` (chosen).** Good, because it checks RustSec advisories, licenses, duplicate versions, and sources with one config file. It is widely used in the ecosystem (originally by Embark Studios). Bad, because it is a tool to install.
* **`cargo-audit`.** Good, because it is the RustSec project's own tool. Bad, because it only checks advisories, which `cargo-deny` already does with the same database.
* **Both.** Bad, because the advisory check would run twice.
* **None.** Bad, because a vulnerability published after a crate was added would go unnoticed.

### Lint policy

* **Targeted deny list in `Cargo.toml` (chosen).** Good, because each lint maps to a rule with a reason, and the `[lints]` table applies to every target (binary, library, tests) from one place. Good, because clippy's own guidance is to pick restriction lints one by one.
* **Whole `clippy::restriction` group.** Bad, because the group contains lints that contradict each other and is not meant to be enabled as a whole; clippy warns about it (`blanket_clippy_restriction_lints`).
* **`#![deny(...)]` attributes in the crate roots.** Bad, because each target root (library, binary, every integration test file) needs its own copy, and a missing copy goes unnoticed.
* `clippy::pedantic` was considered and left out. It targets style rather than safety, and can be adopted later without touching this policy.

### `rust-version`

* **1.85 (chosen).** Good, because it is the edition 2024 floor and every chosen dependency supports it, so it costs nothing. Good, because Cargo then keeps dependency versions within it. Neutral, because the pinned stable toolchain ([ADR-0005](0005-toolchain-and-manual-ci.md)) is used for development anyway.
* **Latest stable.** Good, because the newest dependency versions are always allowed. Bad, because a reviewer with a slightly older compiler could not build, for no benefit to this crate.

## More Information

* Data sources: crate versions, features, licenses, and `rust-version` values from the published crate manifests; download counts and owners from the crates.io API; `unsafe` counts from a search of each crate's published source. All checked on 2026-11-10 by resolving the full set in a scratch crate, running `cargo deny check` on it with the `deny.toml` added here, and building and testing it on Rust 1.85.0 and 1.96.1.
* References:
  * [Cargo `[lints]` table](https://doc.rust-lang.org/cargo/reference/manifest.html#the-lints-section) and the [MSRV-aware resolver](https://doc.rust-lang.org/cargo/reference/resolver.html#rust-version).
  * [Cargo environment variables for integration tests](https://doc.rust-lang.org/cargo/reference/environment-variables.html#environment-variables-cargo-sets-for-crates) (`CARGO_BIN_EXE_<name>`).
  * [Clippy lint list](https://rust-lang.github.io/rust-clippy/master/index.html) and [Clippy configuration](https://doc.rust-lang.org/clippy/lint_configuration.html).
  * [`println!` panics if writing to stdout fails](https://doc.rust-lang.org/std/macro.println.html#panics).
  * [cargo-deny](https://embarkstudios.github.io/cargo-deny/) and the [RustSec advisory database](https://rustsec.org/).
  * [`csv` crate tutorial](https://docs.rs/csv/latest/csv/tutorial/index.html) on decoding with serde.
* Coordination: D5 (LYA-43) decides which errors exist, how each is classified, and its exit code; this ADR only decides that the error enums use `thiserror` and that `main` maps them to an `ExitCode` without `anyhow`. Revisit S1 if D5 needs something `thiserror` cannot express.
* Settles the open question in [ADR-0007](0007-money-representation.md) on where its numeric lints apply: crate-wide, outside tests (S7).
* Revisit this decision if a dependency gets a RustSec advisory or loses its maintainer, if E1 to E-n find a lint that blocks reasonable code without a safety benefit, or if a newer `rust-version` is needed.
* Tracked by issue LYA-45 (D7). The crate skeleton (E1, LYA-49) applies the configuration above.
