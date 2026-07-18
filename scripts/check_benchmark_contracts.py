#!/usr/bin/env python3
"""Check that every timed benchmark has one matching, declared source triplet."""

from __future__ import annotations

import re
import sys
from pathlib import Path


CONTRACT = re.compile(r"^// benchmark-contract: (.+)$")
EXTENSIONS = ("azk", "rs", "c")


def fail(message: str) -> None:
    print(f"benchmark_contracts=FAIL: {message}", file=sys.stderr)
    raise SystemExit(1)


def main() -> None:
    if len(sys.argv) < 2:
        fail("expected at least one scenario name")

    bench_dir = Path("bench")
    requested = sys.argv[1:]
    if len(requested) != len(set(requested)):
        fail("the harness scenario list contains duplicates")

    # Rust-only stress tools are allowed beside the timed cross-language suite.
    # Any Aziky or C source, however, declares intent to be a timed triplet.
    discovered = {
        path.stem
        for extension in ("azk", "c")
        for path in bench_dir.glob(f"*.{extension}")
    }
    expected = set(requested)
    missing = sorted(expected - discovered)
    extra = sorted(discovered - expected)
    if missing or extra:
        fail(f"harness/source mismatch missing={missing} extra={extra}")

    for scenario in requested:
        contracts: dict[str, str] = {}
        for extension in EXTENSIONS:
            path = bench_dir / f"{scenario}.{extension}"
            if not path.is_file():
                fail(f"missing source: {path}")
            first_line = path.read_text(encoding="utf-8").splitlines()[0]
            match = CONTRACT.fullmatch(first_line)
            if match is None:
                fail(f"missing first-line workload contract: {path}")
            contracts[extension] = match.group(1)
        if len(set(contracts.values())) != 1:
            fail(f"contract mismatch for {scenario}: {contracts}")

    print(f"benchmark_contracts=PASS scenarios={len(requested)}")


if __name__ == "__main__":
    main()
