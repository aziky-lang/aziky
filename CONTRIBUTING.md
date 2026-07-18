# Contributing to Aziky

Thank you for helping improve Aziky. This project values correctness,
determinism, explicit safety boundaries, clear diagnostics, and evidence-backed
performance work.

## Before starting

For substantial language, ABI, ownership, package-format, target, or governance
changes, open an issue before implementation. Describe the problem, proposed
contract, compatibility impact, alternatives, and how the change will be tested.

Small bug fixes, tests, documentation corrections, and focused cleanup can go
directly to a pull request.

## Development setup

Required:

- Rust 1.88 or newer with `rustfmt`;
- Linux x86-64 for native generated-code execution tests;
- `bash` and `rg` for repository scripts.

```text
cargo build --locked
cargo test --locked
cargo fmt --all -- --check
```

The compiler must continue to build offline:

```text
cargo build --locked --offline
cargo test --locked --offline
```

For VS Code extension changes:

```text
cd editors/vscode
npm ci
npm test
npm run package
```

Before submitting, also run the repository-level static checks:

```text
scripts/check_repository_hygiene.sh
python3 scripts/check_markdown_links.py
```

## Change requirements

- Add positive and negative tests for new behavior.
- Preserve source locations and deterministic diagnostic ordering.
- Update the language reference before declaring syntax stable.
- Document target-specific behavior and reject unsupported capabilities early.
- Keep output-independent maps and iteration orders from affecting artifacts.
- Do not add a compiler dependency without prior design discussion.
- Do not commit generated binaries, `target`, `node_modules`, VSIX files, logs,
  profiles, or local package caches.
- Keep package-cache material only when it is an explicit fixture under
  `examples` and covered by a test.

Performance patches must include a semantic equivalence argument, regression
tests, cross-language result parity where applicable, and measurements that do
not weaken the workload.

## Pull requests

Keep each pull request focused. Include:

- the problem and user-visible outcome;
- important design decisions and limitations;
- tests run and relevant target environment;
- documentation and checklist updates;
- benchmark methodology when making performance claims.

All required CI checks must pass. Maintainers may request a smaller change,
additional negative tests, or a written design before accepting broad work.

## Licensing contributions

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in Aziky is licensed under the project’s conjunctive
MIT AND Apache-2.0 terms. By contributing, you confirm that you have the right
to submit the work under both licenses.
