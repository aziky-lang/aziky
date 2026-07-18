# Aziky Packages and Reproducible Resolution

Aziky packages are deterministic, offline compilation units. The compiler has
no registry client and never downloads a missing dependency. A package build is
defined by its manifest, selected features, checked-in lockfile, explicit
target options, and checksum-verified cache contents.

## Manifest

The root file is `Aziky.toml`. Aziky accepts the documented, intentionally
small TOML subset: one-line quoted strings, booleans, string arrays, and inline
dependency tables. Unknown sections and keys are errors.

```toml
[package]
name = "example"
version = "0.1.0"
entry = "src/main.azk"

[features]
default = ["dep:formatting"]
extended = ["formatting/color"]

[dependencies]
math = { version = "1.2.3", checksum = "sha256:<64 lowercase hex digits>" }
formatting = { version = "2.0.0", checksum = "sha256:<64 lowercase hex digits>", optional = true, default-features = false, features = ["compact"] }
```

Versions are exact semantic versions. Version ranges and implicit upgrades are
not accepted. Dependency aliases must be valid Aziky module identifiers. Use a
different package name behind an alias with `package = "actual-name"`.

Entry paths are relative, UTF-8 paths using `/`. Absolute paths, `.` and `..`
components, platform prefixes, and backslashes are rejected so manifests have
the same meaning on every host.

## Source cache and checksums

The default cache is `.aziky/cache` beside the root manifest:

```text
.aziky/cache/<package-name>/<exact-version>/Aziky.toml
.aziky/cache/<package-name>/<exact-version>/<package sources>
```

`--package-cache <path>` selects another cache. Relative cache paths are
resolved from the root package, never from the process working directory.

Package checksums cover the package manifest and every `.azk` file recursively.
Paths are sorted and normalized to `/`, and path and content lengths are framed
before SHA-256 hashing. Symlinks and non-UTF-8 paths are rejected. Other files
do not affect compilation and are not included.

Generate a checksum while authoring a cache entry with:

```text
aziky package checksum .aziky/cache/math/1.2.3
```

A missing cache entry is always an error. There is deliberately no online
fallback, making an offline build the ordinary build rather than a special
mode.

## Features

`default` is enabled unless `--no-default-features` is supplied. Additional
root features are selected with `--features name,other`.

Feature entries have three forms:

- `feature-name` enables another feature in the same package.
- `dep:alias` enables an optional dependency.
- `alias/feature-name` enables an optional dependency and forwards a feature.

Unknown features, dependencies, and forwarded feature names are errors. Feature
sets are unions and are written in sorted order to the lockfile.

## Lockfile and graph rules

`aziky package lock [path]` resolves the complete graph and writes
`Aziky.lock`. The lock records:

- lock format version;
- root identity and manifest checksum;
- selected root features and dependency aliases;
- every exact package identity and content checksum;
- selected package features and resolved edges.

No absolute cache or host path is recorded. Package entries are ordered by
logical identity, with features and edges sorted, so repeated resolution emits
identical bytes.

`compile`, `check`, and `package verify` require the lockfile to exactly match
the manifest, feature selection, dependency graph, and cache contents. They do
not silently rewrite it. Run `package lock` deliberately after a dependency or
feature change.

Aziky currently enforces one exact version for each package name throughout a
graph. Conflicting versions, conflicting checksums, dependency cycles, missing
cache entries, identity mismatches, stale locks, and content tampering all
produce deterministic diagnostics.

## Source integration

A manifest dependency is imported through the existing module syntax:

```aziky
mod math;
use math::answer;
```

Local files retain the existing `math.azk` / `math/mod.azk` discovery rules. A
local module and a dependency with the same alias are an ambiguity error rather
than a host-order-dependent shadowing rule. Dependency packages receive stable
internal namespaces derived from their logical identity, allowing a shared
transitive package to be loaded once across a diamond graph.

Package resolution is target-neutral. It does not inspect Linux ABIs, native
path separators, executable formats, or syscall availability.
