# Contributing

Rules for humans and AI agents. Agents also follow [AGENTS.md](AGENTS.md).

All written content is in English (US).

## Setup

```sh
# Toolchain: rustup reads rust-toolchain.toml automatically.
rustup show

# Commit checker (pick one)
brew install cocogitto
cargo install --locked cocogitto

# Install the versioned git hooks from cog.toml (once per clone)
cog install-hook --all

# Supply-chain checker (pick one)
brew install cargo-deny
cargo install --locked cargo-deny
```

## Local checks

Run before every push. CI is manual-only, so these are the gate.

```sh
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test
```

When `Cargo.toml` or `Cargo.lock` changes, also run:

```sh
cargo deny check
```

It checks the dependency graph against `deny.toml`: RustSec advisories, licenses, duplicate versions, and sources. It needs network access to update the advisory database.

## Tests

Layout and rules: [ADR-0014](docs/decisions/0014-testing-strategy.md). `cargo test` runs every unit, property, integration, and CLI test.

- **Unit and property tests** live in a `#[cfg(test)] mod tests` at the end of the module they test. Shared helpers such as `assert_invariants` are in `src/test_support.rs`.
- **Integration tests** call the library's `run` in `tests/run.rs`. **CLI tests** run the binary in `tests/cli.rs`.
- **Naming:** a test for a catalog row in `docs/requirements.md` §6 starts with its ID in lowercase (`a1_withdraw_then_chargeback`). A test covering several rows lists them in a `/// Covers: I1, I2.` doc comment.

### Sample cases

Each case is a directory `tests/cases/<case>/` with exactly four files:

| File | Contents |
|------|----------|
| `input.csv` | The input passed to the binary |
| `expected.stdout` | The exact expected stdout |
| `expected.stderr` | The exact expected stderr |
| `expected.status` | The exit code, for example `0` followed by a newline |

Name a case `<id>-<slug>` for a catalog row (`a26-example-output`, `a33a-empty-file`) or `<slug>` otherwise, in lowercase kebab-case. The binary runs from the case directory with the argument `input.csv`, and all three outputs are compared byte for byte. Write the expected files by hand from the rules, never by copying the program's output. Git leaves the bytes of these files untouched (`.gitattributes`), so line endings and byte order marks in a case stay as written.

### Deeper and optional checks

Run the deep property check before a pull request that changes `amount`, `input`, `store`, or `engine`:

```sh
PROPTEST_CASES=10000 cargo test --release
```

If a property test fails, `proptest` saves the case under `proptest-regressions/`. Commit that file with the fix, so every later run replays it.

Generate a large input for manual runs and measurements (never commit the result):

```sh
cargo run --release --example generate -- 1000000 42 mixed > transactions.csv
```

Optional test-quality tools. Neither is a gate and neither has a target percentage:

```sh
cargo install --locked cargo-llvm-cov   # or: brew install cargo-llvm-cov
cargo llvm-cov --html                   # line coverage report in target/llvm-cov/html

cargo install --locked cargo-mutants
cargo mutants                           # lists mutants no test catches; review each by hand
```

## Dependencies and safety

[ADR-0012](docs/decisions/0012-dependencies-and-safety-policy.md) lists every allowed dependency and the lint policy. In short:

- A new dependency needs a new ADR that says why it is needed and why it is trusted.
- Clippy denies `unsafe`, panicking constructs outside tests, floats, unchecked arithmetic, `as` casts, and `print!`-family macros.
- Clippy does not catch a panic inside a called function, such as `str::split_at` on a bad index. For code that handles input, check each API's "Panics" section, prefer the non-panicking variant, and cover it with a property test on arbitrary input.
- `#[allow]` is denied. Use `#[expect(lint, reason = "...")]` at the narrowest scope, so every exception carries its justification.

## Commit messages

Format: [Conventional Commits 1.0.0](https://www.conventionalcommits.org/en/v1.0.0/).

```
<type>[optional scope][!]: <description>

[optional body]

[optional footer(s)]
```

**Types**

| Type       | Use for |
|------------|---------|
| `feat`     | New behavior visible to the user of the binary |
| `fix`      | Bug fix |
| `perf`     | Performance change with no behavior change |
| `refactor` | Code change with no behavior change |
| `test`     | Adding or fixing tests or sample data |
| `docs`     | Documentation and ADRs only |
| `build`    | `Cargo.toml`, dependencies, toolchain |
| `ci`       | `.github/workflows` |
| `style`    | Formatting only |
| `chore`    | Anything else that does not touch `src/` |

**Rules**

- Description: imperative mood ("add", not "added"), lowercase start, no trailing period, header 72 characters max.
- Scope is optional, a single noun naming the area, e.g. `feat(engine):`, `test(cli):`.
- Body explains *why*, not *what*. Wrap at 72 columns. Separate from the header with one blank line.
- Breaking changes: `!` before the colon, or a `BREAKING CHANGE:` footer.
- One logical change per commit.
- No `Co-Authored-By` or other AI attribution trailers.
- Never use `git commit --no-verify`. If the hook rejects a message, fix it.

**Examples**

```
feat(engine): hold funds when a deposit is disputed

The dispute moves the referenced deposit amount from available to held
so that total stays constant until the dispute is resolved or charged
back.
```

```
fix(parser): accept whitespace around csv fields
```

## Branches

`<type>/<short-kebab-description>`, using the same type as the change, e.g. `feat/dispute-handling`, `docs/adr-locked-accounts`.

## Pull requests

PRs are **squash-merged**. The PR title becomes the single commit on `main`, so it must be a valid Conventional Commit header. Check it with:

```sh
cog verify "<PR title>"
```

The squash commit body is the PR description. Write it following [Google's CL description guidance](https://google.github.io/eng-practices/review/developer/cl-descriptions.html):

- **First line** (the title): a short, specific summary of what the PR does, in imperative mood, understandable on its own in `git log`.
- **Body**: what problem it solves, why this approach, and its shortcomings or known limitations. Include enough context that the text stands on its own if links break later.
- Even small PRs get a body that places the change in context. "Fix bug" or "Phase 1" is not a description.
- Before merging, re-read the description and update it if the PR changed during review.

Use the template in [.github/pull_request_template.md](.github/pull_request_template.md).

## Architecture decisions

Significant decisions go in `docs/decisions/` as [MADR 4.0.0](https://adr.github.io/madr/) records.

1. Copy `docs/decisions/adr-template.md` to `docs/decisions/NNNN-title-with-dashes.md` using the next free number.
2. Fill it in. Delete optional sections you do not need.
3. Commit it with `docs(adr): ...`, in the same PR as the change it governs when possible.
4. Never rewrite an accepted ADR's decision. Supersede it with a new ADR and update the old one's status to `superseded by ADR-NNNN`.
