# rust-transactions-practice

A command-line transaction engine. It reads account transactions from a CSV file, applies deposits, withdrawals, disputes, resolves, and chargebacks to client accounts, and writes the final account balances to stdout as CSV.

> Status: project setup. The engine is not implemented yet.

## Usage

```sh
cargo run -- transactions.csv > accounts.csv
```

## Documentation

- [docs/requirements.md](docs/requirements.md): behavior contract.
- [docs/decisions/](docs/decisions/): architecture decision records.
- [CONTRIBUTING.md](CONTRIBUTING.md): setup, commit, branch, and PR conventions.
- [AGENTS.md](AGENTS.md): instructions for AI coding agents.

## Design, assumptions, and correctness

To be written alongside the implementation.

## AI assistance

This project was developed with AI coding assistants. The full prompts and outputs are shared with reviewers separately.
