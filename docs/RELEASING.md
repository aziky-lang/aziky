# Maintainer Release Process

Aziky is not published automatically. CI builds artifacts for inspection but
has no package-registry, Marketplace, release, or deployment credentials.

Canonical repository: <https://github.com/aziky-lang/aziky>

## Repository configuration

Protect `main`, require the Linux, Windows, macOS, and VS Code CI jobs, prevent
force-pushes and branch deletion, and enable GitHub's vulnerability-reporting
facility. CI must remain read-only and must not hold publishing credentials.

## Versioned release

1. Confirm the checklist and release-status documents reflect reality.
2. Move user-visible entries from `Unreleased` into a dated version section in
   `CHANGELOG.md`.
3. Update `Cargo.toml`, `Cargo.lock`, and the VS Code package versions as
   applicable.
4. Run formatting, repository hygiene, Markdown links, extension tests, and the
   complete release gate from a clean tree.
5. Review target limitations and generated ABI compatibility explicitly.
6. Push a signed annotated tag only after required public CI passes.
7. Build release assets from the tagged tree; do not commit generated binaries
   or VSIX archives.

Aziky is currently pre-1.0. A tag does not imply long-term ABI, language, or
standard-library stability unless the release notes say so explicitly.
