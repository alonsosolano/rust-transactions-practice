---
status: accepted
date: 2026-11-10
decision-makers: Alonso Solano
---

# Keep the exercise statement and its issuer out of the repository

## Context and Problem Statement

The project brief was received as a document with two explicit submission constraints: the document itself, or anything derived from it, must not be committed, and the solution must not mention the issuing organization, its parent company, products, brands, or web domains. At the same time, every agent and engineer needs the requirements in the repo to work without access to the original conversation. How do we give them the requirements without breaking those constraints?

## Decision Drivers

* Violating the constraints can disqualify the submission. This is a hard block.
* All project context must live in the repo.

## Considered Options

* Rewrite the requirements in original wording, with no reference to the issuer
* Keep the requirements outside the repo
* Commit the original document

## Decision Outcome

Chosen option: "Rewrite the requirements in original wording, with no reference to the issuer", because it is the only option that keeps the repo self-contained without committing the document. Committing the document is ruled out by the constraints.

The rewrite lives in `docs/requirements.md`. It states the behavior contract in this repo's own words and structure and contains no names, brands, or domains of the issuer.

### Consequences

* Good, because agents and reviewers have a complete behavior contract in the repo.
* Bad, because `docs/requirements.md` may drift from the brief. When in doubt, the human checks the brief and updates the requirements.

### Confirmation

* `*.pdf` is ignored by `.gitignore`.
* `AGENTS.md` lists this as hard rule 1.
* The PR template includes a checklist item for it.
* Before submission, the human searches the full history (`git log -p`) for the issuer's names and domains.
