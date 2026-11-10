---
status: accepted
date: 2026-11-10
decision-makers: Alonso Solano
---

# Track stable Rust and keep CI manual-only

## Context and Problem Statement

The crate must build with `cargo build` on a reviewer's machine. GitHub Actions minutes are not available right now. How do we pin the toolchain and handle CI?

## Considered Options

* `rust-toolchain.toml` with `channel = "stable"`, CI workflow with only a manual trigger
* Pin an exact stable version
* No toolchain file, no CI workflow

## Decision Outcome

Chosen option: "`rust-toolchain.toml` with `channel = "stable"`, CI workflow with only a manual trigger".

* Toolchain: latest stable, edition 2024, with `rustfmt` and `clippy` components.
* CI: `.github/workflows/ci.yml` runs fmt, clippy, tests, and a release build, but is triggered only by `workflow_dispatch`, so it never consumes minutes automatically.

### Consequences

* Good, because everyone gets the same components and a current compiler without manual setup.
* Good, because the CI definition is ready; enabling it is a one-line trigger change.
* Bad, because `stable` moves, so a new release could introduce new clippy warnings.
* Bad, because local checks are the only gate until CI is enabled.

### Confirmation

When minutes become available, add `push` and `pull_request` triggers to `ci.yml` and supersede this ADR.
