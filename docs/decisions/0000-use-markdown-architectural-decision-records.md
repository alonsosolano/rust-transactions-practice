---
status: accepted
date: 2026-11-10
decision-makers: Alonso Solano
---

# Use Markdown Architectural Decision Records

## Context and Problem Statement

This repo is worked on by a human and several AI agents across sessions and tools. Decisions and the reasoning behind them must live in the repo, not in chat history. Which format should those records use?

## Considered Options

* MADR 4.0.0
* Michael Nygard's original ADR template
* Free-form notes in the README

## Decision Outcome

Chosen option: "MADR 4.0.0", because it is plain Markdown that diffs well, has optional sections that keep small decisions short, and records considered options, which reviewers and agents both need to understand why an alternative was rejected.

### Consequences

* Good, because any agent or reviewer can recover the reasoning without the author present.
* Good, because the `NNNN-title-with-dashes.md` convention keeps records sorted and easy to reference.
* Bad, because it adds a small writing overhead to each decision.

### Confirmation

Records live in `docs/decisions/`, are created from `adr-template.md`, and are reviewed in the PR that introduces the decision.

## More Information

* https://adr.github.io/madr/
