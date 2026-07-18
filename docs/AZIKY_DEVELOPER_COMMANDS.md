# Aziky Developer Commands

Aziky's first-class developer commands use deterministic discovery, ordering,
diagnostics, and summaries. They do not require an external test framework,
formatter, linter, runtime, registry, or network service.

## Tests

```text
aziky test [path]
    [--filter <substring>]
    [--list]
    [--timeout-ms <positive integer>]
    [--features <list>]
    [--no-default-features]
    [--package-cache <path>]
```

Each test is a standalone `.azk` program. Exit status zero passes; any nonzero
status, signal, compilation error, emission error, or timeout fails. Output is
captured and printed only for failures.

When `path` is a directory containing `tests/`, discovery starts there.
Otherwise it recursively discovers `.azk` files under `path`; a single source
path runs one test. `.git`, `.aziky`, and `target` directories are excluded and
symbolic links are rejected. Names use `/` separators and are sorted before
filtering and execution. Tests run sequentially, so result order cannot depend
on scheduler timing. Reports deliberately omit elapsed time and temporary paths.

Package tests use the root `Aziky.toml`, checked lockfile, selected features,
offline cache, and ordinary `mod` / `use` resolution. Unlike `compile`, the test
runner permits files under `tests/` to act as package entry programs.

`--list` performs discovery and filtering without compilation or execution.
`--filter` is a case-sensitive substring of the portable test path. The default
timeout is 10 seconds per test; the explicit timeout is a safety boundary and
is not printed for successful tests.

Native execution currently has an explicit `linux-x86_64` capability check.
Discovery and reporting are target-neutral; additional runners will attach to
the future platform-runtime target boundary.

## Formatter

```text
aziky fmt [paths...]
aziky fmt [paths...] --check
aziky fmt <one-file.azk> --stdout
aziky fmt --stdin
```

The baseline formatter makes only semantic-preserving layout changes:

- normalizes CRLF and CR newlines to LF;
- removes leading and trailing whitespace around source lines;
- applies four-space indentation from braces while ignoring braces in strings,
  characters, line comments, and nested block comments;
- preserves blank lines, limiting trailing blank lines;
- writes exactly one final newline.

It intentionally does not reflow expressions or comments yet. Formatting is
idempotent. `--check` never writes and lists files that would change before
returning failure, making it suitable for CI. `--stdout` requires exactly one
file and performs no write. `--stdin` reads one UTF-8 source buffer and writes
only its formatted form to stdout; it cannot be combined with paths or other
formatter modes and is the editor integration contract.

## Linter

```text
aziky lint [paths...]
aziky lint [paths...] --deny-warnings
aziky lint [paths...] --check
aziky lint [paths...] --diagnostic-format=json
```

`lint` parses each discovered source independently and emits sorted diagnostics
in this stable form:

```text
path:line:column: warning[AZK-LNNN]: message
```

The accepted baseline rules are:

| Code | Rule |
|---|---|
| `AZK-L001` | No trailing whitespace |
| `AZK-L002` | No tab indentation |
| `AZK-L003` | Source lines are at most 120 columns |
| `AZK-L004` | Nonempty source ends with a newline |
| `AZK-L005` | Source uses portable LF rather than CR/CRLF newlines |
| `AZK-L100` | Functions, parameters, fields, and modules use `snake_case` |
| `AZK-L101` | Types, traits, and enum variants use `UpperCamelCase` |

Warnings do not fail an interactive lint by default. `--deny-warnings` and its
CI alias `--check` return failure when any warning exists. Parse errors always
fail.

`check <input.azk> --diagnostic-format=json` and lint's matching option emit one
versioned `aziky-diagnostics-v1` JSON object. The object contains a status and a
sorted diagnostic array with severity, optional stable code, message, path,
one-based line, and one-based column. Failed checks retain a nonzero exit code;
the machine payload is not mixed with human-readable error rendering.

## Benchmark status

First-class benchmark discovery and execution are intentionally deferred. The
existing standalone benchmark scripts continue to work, but benchmark tooling
remains planned beyond the accepted developer-command baseline.
