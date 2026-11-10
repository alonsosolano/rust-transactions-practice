---
status: accepted
date: 2026-11-10
decision-makers: Alonso Solano
---

# Branch per change, squash merge, Google-style PR descriptions

## Context and Problem Statement

Reviewers will read the history of `main` without the author present. How do we keep that history clean and self-explanatory?

## Considered Options

* Short-lived branches, squash merge, PR title as the final commit
* Short-lived branches, rebase merge
* Commit directly to `main`

## Decision Outcome

Chosen option: "Short-lived branches, squash merge, PR title as the final commit", because each PR becomes one reviewable commit on `main`, and work-in-progress commits on the branch do not leak into the history.

Rules:

* Branch name: `<type>/<short-kebab-description>`.
* PR title: a valid Conventional Commit header, checked with `cog verify`.
* PR description: follows Google's CL description guidance (specific imperative first line; body with the problem, the reasoning, and the limitations; self-contained text). Template in `.github/pull_request_template.md`.

### Consequences

* Good, because `main` reads as a list of complete, described changes.
* Bad, because the per-commit history of a branch is lost after merge. Acceptable for a project of this size.

## More Information

* https://google.github.io/eng-practices/review/developer/cl-descriptions.html
