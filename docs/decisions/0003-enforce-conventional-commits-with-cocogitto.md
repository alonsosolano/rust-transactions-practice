---
status: accepted
date: 2026-11-10
decision-makers: Alonso Solano
---

# Enforce Conventional Commits with cocogitto

## Context and Problem Statement

Commits are written by both the human and AI agents. Agents reliably follow rules that are enforced by tooling and tend to drift on rules that are only documented. Which convention do we use, and how is it enforced in a Rust-only repo?

## Decision Drivers

* A widely recognized, machine-parseable convention.
* Enforcement that does not add a Node.js or Python toolchain.
* Hook configuration versioned in the repo so every clone gets the same check.

## Considered Options

* cocogitto with a versioned `commit-msg` hook
* committed (crate-ci) with a manually installed hook
* commitlint with husky
* Documentation only, no enforcement

## Decision Outcome

Chosen option: "cocogitto with a versioned `commit-msg` hook", because it is a single Rust tool that both validates messages and installs hooks defined in the committed `cog.toml`.

Convention: Conventional Commits 1.0.0. Repo-specific rules (types, description style, header length) are in `CONTRIBUTING.md`.

The hook only checks the commit message. Formatting, lints, and tests are run manually before pushing (see `AGENTS.md`).

### Consequences

* Good, because invalid messages are rejected at commit time for humans and agents alike.
* Good, because `cog verify` can also validate PR titles before a squash merge.
* Bad, because each clone must run `cog install-hook --all` once.
* Bad, because without CI, nothing re-checks commits made with hooks bypassed. Mitigated by the rule against `--no-verify`.

### Confirmation

`cog check` validates the full history. Run it before submission.

## Pros and Cons of the Options

### committed (crate-ci)

* Good, because it is a focused Rust linter.
* Bad, because hook installation relies on the Python pre-commit framework or a hand-written script.

### commitlint with husky

* Good, because it is the most common setup.
* Bad, because it adds Node.js and `node_modules` to a Rust project.

## More Information

* https://www.conventionalcommits.org/en/v1.0.0/
* https://docs.cocogitto.io/guide/git_hooks.html
