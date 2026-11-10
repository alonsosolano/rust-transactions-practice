---
status: accepted
date: 2026-11-10
decision-makers: Alonso Solano
---

# Use AGENTS.md as the single source of agent instructions

## Context and Problem Statement

Different AI coding tools read different instruction files. Keeping one file per tool leads to duplicated, drifting instructions. How do we give every agent the same instructions from one place?

## Decision Drivers

* One source of truth for agent behavior.
* Works with Claude Code today, and with other agents without extra files.
* Short enough to load on every session; detail lives in linked docs.

## Considered Options

* `AGENTS.md` as the source, `CLAUDE.md` importing it
* `AGENTS.md` as the source, `CLAUDE.md` as a symlink
* Separate per-tool files
* A custom `.ai/` folder with pointer files

## Decision Outcome

Chosen option: "`AGENTS.md` as the source, `CLAUDE.md` importing it", because `AGENTS.md` is the open standard read natively by most coding agents, and a `CLAUDE.md` containing `@AGENTS.md` makes Claude Code load the same content in any version, on any OS, without symlinks.

Additional rules recorded here:

* All repo content and agent conversation is in English (US).
* `AGENTS.md` stays short: commands, hard rules, and pointers. Long-form context goes in `docs/` and `CONTRIBUTING.md`.
* No other tool-specific instruction files for now. Add them as thin pointers to `AGENTS.md` if a new tool is adopted.

### Consequences

* Good, because instructions are edited in one place.
* Good, because any agent that supports the standard works with no extra setup.
* Neutral, because `CLAUDE.md` exists only to import `AGENTS.md`; Claude-specific rules, if ever needed, go below the import.

## More Information

* https://agents.md
