#!/usr/bin/env bash
set -euo pipefail

required=(
    .gitattributes
    README.md
    CONTRIBUTING.md
    CODE_OF_CONDUCT.md
    SECURITY.md
    CONTRIBUTORS.md
    LICENSE.md
    LICENSE-MIT
    LICENSE-APACHE
)
for path in "${required[@]}"; do
    if [[ ! -f "$path" ]]; then
        echo "repository_hygiene=FAILED reason=missing_file path=$path"
        exit 2
    fi
done

while IFS= read -r path; do
    attribute="$(git check-attr eol -- "$path")"
    if [[ "$attribute" != "$path: eol: lf" ]]; then
        echo "repository_hygiene=FAILED reason=package_fixture_line_endings path=$path"
        exit 2
    fi
done < <(git ls-files examples/package_app)

if git ls-files | rg -q '(^|/)(target|node_modules|dist)/|\.vsix$|\.out$|^--test-threads=1$|^examples/ex$|^simd_benchmarks$'; then
    echo "repository_hygiene=FAILED reason=tracked_generated_artifact"
    git ls-files | rg '(^|/)(target|node_modules|dist)/|\.vsix$|\.out$|^--test-threads=1$|^examples/ex$|^simd_benchmarks$'
    exit 2
fi

if rg -n -i 'aziky-local|super-compiler|super_compiler|UNLICENSED' \
    --hidden \
    --glob '!.git/**' \
    --glob '!Cargo.lock' \
    --glob '!editors/vscode/package-lock.json' \
    --glob '!scripts/check_repository_hygiene.sh' \
    .; then
    echo "repository_hygiene=FAILED reason=stale_public_identity"
    exit 2
fi

rg -q '^license = "MIT AND Apache-2\.0"$' Cargo.toml
rg -q '^MIT AND Apache-2\.0$' LICENSE.md
rg -q '^authors = \["Yassine Azily"\]$' Cargo.toml
rg -q '^- Creator and original author: \*\*Yassine Azily\*\*$' README.md
rg -q '^- License: \*\*MIT AND Apache-2\.0\*\*' README.md

license_files=(
    LICENSE.md
    LICENSE-MIT
    LICENSE-APACHE
    editors/vscode/LICENSE
    editors/vscode/LICENSE-MIT
    editors/vscode/LICENSE-APACHE
)
for path in "${license_files[@]}"; do
    head -n 1 "$path" | rg -q \
        '^Copyright \(c\) 2026 Yassine Azily and Contributors .*CONTRIBUTORS\.md.*$'
done

echo "repository_hygiene=PASS"
