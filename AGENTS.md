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
cog verify "<commit message>"                # validate a commit message
```

Before declaring a task done, run `cargo fmt --all --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo test`, and fix any failure. CI exists but is manual-only ([ADR-0005](docs/decisions/0005-toolchain-and-manual-ci.md)), so local checks are the gate.

## Toolchain

Rust stable, edition 2024, pinned via `rust-toolchain.toml` (includes `rustfmt` and `clippy`).

## Commits, branches, PRs

- Commits follow [Conventional Commits 1.0.0](https://www.conventionalcommits.org/en/v1.0.0/), enforced by a cocogitto `commit-msg` hook. Never bypass it with `--no-verify`; if a message is rejected, fix it and retry.
- Work on a branch named `<type>/<short-kebab-description>`; PRs are squash-merged, so the PR title becomes the commit on `main` and must itself be a valid Conventional Commit.
- PR descriptions follow Google's CL description guidance and the template in `.github/pull_request_template.md`.

Full rules and examples: [CONTRIBUTING.md](CONTRIBUTING.md).

## Repository layout

```
src/                 crate source
docs/requirements.md behavior contract
docs/decisions/      architecture decision records (MADR)
.github/             PR template, manual-only CI workflow
cog.toml             commit convention and git hooks (cocogitto)
rust-toolchain.toml  toolchain pin
```
