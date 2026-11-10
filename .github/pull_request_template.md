<!--
Title: a valid Conventional Commit header, imperative, specific.
  e.g. "feat(engine): lock account on chargeback"
  Check it: cog verify "<title>"
The title and this description become the squash commit on main.
-->

## What

<!-- One or two sentences on what this PR changes. -->

## Why

<!-- The problem or requirement this addresses. Reference docs/requirements.md sections or ADRs. -->

## How

<!-- Approach taken and why it was chosen over alternatives. Link a new ADR if a decision was made. -->

## Limitations

<!-- Known shortcomings, follow-ups, or "None". -->

## Testing

<!-- How correctness was verified: unit tests, sample CSVs, manual runs. -->

## Checklist

- [ ] `cargo fmt --all --check` passes
- [ ] `cargo clippy --all-targets -- -D warnings` passes
- [ ] `cargo test` passes
- [ ] CLI contract unchanged (`cargo run -- input.csv > output.csv`)
- [ ] ADR added or updated if a decision was made
- [ ] No reference to the exercise issuer or the original statement (ADR-0001)
