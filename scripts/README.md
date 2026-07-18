# Repository Scripts

Run scripts from the repository root. Compiler-building gates use the locked
Cargo graph offline; install Rust 1.88, Bash, `rg`, LLVM tools, and binutils
before running them.

## Release and platform gates

- `run_release_gate.sh`: canonical Linux release acceptance gate.
- `run_portability_gate.sh`: deterministic Linux, Windows, and macOS containers;
  add `--with-wine` to execute the Windows runtime surface.
- `run_macos_x86_64_gate.sh`: native Darwin execution on Intel macOS or Apple
  silicon with Rosetta 2.
- `run_concurrency_gate.sh`: native thread, channel, shutdown, and negative
  ownership contracts.
- `run_package_gate.sh`: offline package graph, cache integrity, and diagnostics.
- `run_developer_tools_gate.sh`: tests, formatter, linter, and stable reporting.
- `run_artifact_gate.sh`: object, archive, shared-library, symbol, and debug
  metadata contracts.

## Correctness checks

- `check_determinism.sh` and `check_reproducible_build.sh`: repeated and
  cross-root artifact identity.
- `differential_encoder_check.sh`: encoder-path parity.
- `fuzz_frontend.sh`: deterministic frontend mutation smoke test.
- `check_repository_hygiene.sh` and `check_markdown_links.py`: public-tree and
  documentation checks used by CI.

## Performance tools

- `run_full_quality_gate.sh`: correctness checks plus the complete benchmark and
  allocator gates; intentionally not run on noisy shared CI hosts.
- `run_bench_suite.sh`, `compare_aziky_rust_bench.sh`, and `time_binary.sh`:
  cross-language benchmark harness.
- `run_allocator_stress.sh`, `check_allocator_stress_gate.sh`, and
  `check_sort_bench_gate.sh`: focused performance gates.
- `collect_profile.sh` and `tune_variants.sh`: explicit PGO/PMU and target-variant
  experiments.

Generated output belongs under `target/` and must not be committed.
