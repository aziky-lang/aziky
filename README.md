<p align="center">
  <img src="editors/vscode/icons/aziky-yaz-dark.svg" width="112" alt="Aziky Yaz mark">
</p>

# Aziky

[![CI](https://github.com/aziky-lang/aziky/actions/workflows/ci.yml/badge.svg)](https://github.com/aziky-lang/aziky/actions/workflows/ci.yml)
[![License: MIT AND Apache-2.0](https://img.shields.io/badge/license-MIT%20AND%20Apache--2.0-blue.svg)](LICENSE.md)

Aziky is an experimental, deterministic programming language and self-contained
compiler toolchain written in Rust. It translates `.azk` programs directly to
native x86-64 machine code and writes executable, object, static-library, and
selected shared-library formats without invoking an external assembler or
linker (no LLVM).

- Project name: **Aziky**
- Repository: **[aziky-lang/aziky](https://github.com/aziky-lang/aziky)**
- License: **MIT AND Apache-2.0** (both licenses apply simultaneously)

Aziky is developed with its [contributors](CONTRIBUTORS.md), and authorship is
preserved through the project history and contributor record.

The project prioritizes auditable compilation, reproducible output, explicit
ownership, offline operation, and predictable performance.

> [!WARNING]
> Aziky is alpha software. The language, standard library, package format, and
> generated-code ABI can change without compatibility guarantees. Do not use it
> for security-critical or production workloads yet.

## What works today

- Hand-written lexer and recursive-descent parser with source diagnostics.
- Structs, enums, traits, modules, functions, collections, control flow, and
  deterministic parallel-loop semantics.
- Runtime-native ownership for current heap objects, files, threads, channels,
  strings, and supported collection families.
- Direct ELF64, Mach-O64, PE32+/COFF, relocatable object, static archive, and
  supported shared-object emission.
- Deterministic offline packages with exact versions, lockfiles, checksums,
  features, and a local cache.
- Built-in `check`, `compile`, `test`, `fmt`, `lint`, package, profile, and target
  commands.
- An installable VS Code extension with highlighting, formatting, diagnostics,
  completion, hover, and definition navigation.
- A zero-external-crate compiler dependency graph.

The precise accepted surface and known limitations are tracked in the
[release status](docs/RELEASE_STATUS.md). Planned application APIs are tracked
separately in the [standard-library roadmap](docs/STANDARD_LIBRARY_ROADMAP.md).

## Supported targets

| Target | Status |
|---|---|
| Linux x86-64 | Primary native development and execution target |
| Windows x86-64 | PE/COFF application runtime tested under Wine |
| macOS x86-64 | Startup, allocation, files, and process services implemented; clocks and threading remain incomplete |
| AArch64 | Planned |

Unsupported target/runtime combinations are rejected before code emission. See
[Aziky targets](docs/AZIKY_TARGETS.md) for the full capability matrix.

## Performance measurement

The repository retains its cross-language workloads and checksum parity gates
as correctness regressions. New performance results will be published soon... See [benchmark methodology](bench/README.md).

## Build from source

Prerequisites:

- Rust 1.88 or newer, including `cargo` and `rustfmt`;
- Linux x86-64 for the complete native execution test suite;
- `bash`, `rg`, and standard binutils for the extended release gates;
- Node.js 24 only when developing or packaging the VS Code extension.

```text
cargo build --locked
cargo test --locked
```

No network access is required after the Rust toolchain and repository are
available. The compiler itself has no third-party crates.

## First program

Create `hello.azk`:

```aziky
fn main() {
    print("Hello from Aziky!\n");
    exit(0u64);
}
```

On Linux x86-64:

```text
cargo run --locked -- check hello.azk
cargo run --locked -- compile hello.azk -o hello
./hello
```

Useful commands:

```text
cargo run --locked -- fmt hello.azk
cargo run --locked -- lint hello.azk
cargo run --locked -- target list
cargo run --locked -- test examples/developer_commands
```

Run `cargo run --locked -- --help` for the complete CLI surface. Developer
command behavior is documented in
[Aziky developer commands](docs/AZIKY_DEVELOPER_COMMANDS.md).

## Packages

Aziky package resolution is deterministic and deliberately offline. Manifests
use `Aziky.toml`, resolved dependency graphs are pinned in `Aziky.lock`, and
dependencies are loaded from a checksum-verified local cache.

The repository includes a complete fixture in `examples/package_app`; See [Aziky packages](docs/AZIKY_PACKAGES.md).


## VS Code extension

```text
cd editors/vscode
npm ci
npm test
npm run package
code --install-extension dist/aziky-language-0.1.1.vsix --force
```

The generated VSIX remains a local build artifact. See the
[extension README](editors/vscode/README.md) and
[editor tooling architecture](docs/AZIKY_EDITOR_TOOLING.md).

## Repository map

- `src/frontend`: lexer, parser, semantic analysis, and frontend optimization;
- `src/backend`: runtime IR, MachineLIR, optimization, allocation, and x86-64 emission;
- `src/object`: executable, object, archive, symbol, and debug metadata writers;
- `stdlib`: embedded Aziky standard-library modules;
- `examples`: runnable programs, negative cases, and deterministic package fixtures;
- `bench`: cross-language Aziky/Rust/C benchmark scenarios;
- `assets`: public Aziky brand assets, including the 1024px Yaz PNG;
- `editors/vscode`: VS Code language extension;
- `scripts`: reproducibility, integration, benchmark, and release gates;
- `docs`: language, toolchain, platform, architecture, and roadmap documentation;
- `research`: implementation research and standalone experiments.

Documentation is indexed in [docs/README.md](docs/README.md).

The geometric Amazigh Yaz mark is available as a
[1024×1024 PNG](assets/aziky-yaz-1024.png) for community and project use.

## Contributing

Contributions are actively encouraged. Read
[CONTRIBUTING.md](CONTRIBUTING.md) before opening a change. Bug reports,
documentation corrections, portability work, diagnostics, tests, and carefully
scoped compiler improvements are welcome. Performance changes must preserve the
workload and pass semantic-equivalence gates before benchmark results count.

Please follow the [Code of Conduct](CODE_OF_CONDUCT.md). Report security issues
using [SECURITY.md](SECURITY.md), not a public issue.

## License

Aziky is dual-licensed under the conjunctive SPDX expression
`MIT AND Apache-2.0`, downstream users must comply with both licenses simultaneously.

- [Apache License, Version 2.0](LICENSE-APACHE)
- [MIT License](LICENSE-MIT)

Unless explicitly stated otherwise, contributions intentionally submitted for
inclusion in Aziky are licensed under the same conjunctive
`MIT AND Apache-2.0` terms. Copyright is held by Yassine Azily and Contributors
as described in [CONTRIBUTORS.md](CONTRIBUTORS.md).
