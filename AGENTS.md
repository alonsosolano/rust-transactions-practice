# AGENTS.md

Instructions for AI coding agents working in this repository. Humans should start with [README.md](README.md) and [CONTRIBUTING.md](CONTRIBUTING.md).

## Project

A command-line transaction engine in Rust. It reads a CSV of account transactions, applies them to client accounts (deposits, withdrawals, disputes, resolves, chargebacks), and prints final account balances as CSV to stdout.

- Behavior contract: [docs/requirements.md](docs/requirements.md). Read it before changing engine logic.
- Decisions: [docs/decisions/](docs/decisions/). Read the relevant ADRs before changing anything they cover.

## Hard rules

These are non-negotiable. Stop and ask the human if a task would break one.

1. **Never commit the original exercise statement or anything derived from it**, and never mention the organization that issued it, its parent company, products, brands, or web domains, anywhere in the repo: code, comments, docs, test data, commit messages, branch names, or PR text. See [ADR-0001](docs/decisions/0001-keep-exercise-statement-out-of-repository.md).
2. **The CLI contract is fixed**: `cargo run -- <input.csv> > <output.csv>`. One positional argument, output on stdout only. Diagnostics go to stderr. Do not add required flags or change output columns.
3. **Language is English (US)** for everything: code, comments, docs, commits, PRs, and conversation with the human.
4. **Do not change the git identity** and do not add `Co-Authored-By` or other AI attribution trailers to commits.
5. **Do not commit, push, or open PRs unless the human asks.**
6. **Record significant decisions as ADRs** in `docs/decisions/` (MADR 4.0.0, template in `adr-template.md`). If a change contradicts an accepted ADR, write a new ADR that supersedes it instead of silently diverging.

## Commands

```sh
cargo build                                  # build
cargo run -- transactions.csv > accounts.csv # run
cargo test                                   # all tests
cargo fmt --all --check                      # formatting
cargo clippy --all-targets -- -D warnings    # lints, warnings are errors
cargo deny check                             # advisories, licenses, sources
cog verify "<commit message>"                # validate a commit message
```

Before declaring a task done, run `cargo fmt --all --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo test`, and fix any failure. If `Cargo.toml` or `Cargo.lock` changed, also run `cargo deny check`. CI exists but is manual-only ([ADR-0005](docs/decisions/0005-toolchain-and-manual-ci.md)), so local checks are the gate.

## Toolchain

Rust stable, edition 2024. `rust-toolchain.toml` selects the `stable` channel, which tracks the latest stable release rather than a fixed version ([ADR-0005](docs/decisions/0005-toolchain-and-manual-ci.md)), and includes `rustfmt` and `clippy`. `rust-version` is 1.85; do not raise it without a reason ([ADR-0012](docs/decisions/0012-dependencies-and-safety-policy.md)).

## Dependencies and safety

Follow [ADR-0012](docs/decisions/0012-dependencies-and-safety-policy.md):

- Do not add, replace, or remove a dependency without a new ADR. Prefer the standard library.
- No `unsafe`, no `unwrap`/`expect`/`panic!`/indexing outside tests, no floats, no unchecked arithmetic, no `as` casts, no `println!`/`eprintln!`. Clippy enforces all of these.
- Clippy does not see panics inside called functions. Before calling a standard-library or dependency function on input data, check its "Panics" section and use the non-panicking variant (`split_at_checked`, `get`, `checked_*`).
- Never use `#[allow]`. If an exception is truly needed, use `#[expect(lint, reason = "...")]` at the narrowest scope.

## Commits, branches, PRs

- Commits follow [Conventional Commits 1.0.0](https://www.conventionalcommits.org/en/v1.0.0/), enforced by a cocogitto `commit-msg` hook. Never bypass it with `--no-verify`; if a message is rejected, fix it and retry.
- Work on a branch named `<type>/<short-kebab-description>`; PRs are squash-merged, so the PR title becomes the commit on `main` and must itself be a valid Conventional Commit.
- PR descriptions follow Google's CL description guidance and the template in `.github/pull_request_template.md`.

Full rules and examples: [CONTRIBUTING.md](CONTRIBUTING.md).

## Repository layout

```
src/                 crate source, with unit and property tests in each module
tests/               integration (run.rs) and CLI (cli.rs) tests
tests/cases/         sample cases: input.csv and expected stdout, stderr, exit status
examples/            large-input generator (generate.rs) and latency probe (row_latency.rs)
proptest-regressions/ saved failing seeds that proptest replays first
docs/requirements.md behavior contract
docs/decisions/      architecture decision records (MADR)
docs/performance.md  time and memory measurements
.github/             PR template, manual-only CI workflow
clippy.toml          test exemptions and the float ban (lint levels are in Cargo.toml)
cog.toml             commit convention and git hooks (cocogitto)
deny.toml            supply-chain policy (cargo-deny)
rust-toolchain.toml  toolchain channel and components
```
