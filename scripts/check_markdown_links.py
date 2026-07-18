#!/usr/bin/env python3
"""Validate repository-local links in Markdown files without network access."""

from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path
from urllib.parse import unquote


ROOT = Path(__file__).resolve().parent.parent
LINK = re.compile(r"!?\[[^\]]*\]\(([^)]+)\)")
SCHEME = re.compile(r"^[A-Za-z][A-Za-z0-9+.-]*:")


def tracked_markdown() -> list[Path]:
    output = subprocess.check_output(
        ["rg", "--files", "-g", "*.md"], cwd=ROOT, text=True
    )
    return [ROOT / line for line in output.splitlines() if line]


def destination(raw: str) -> str:
    value = raw.strip()
    if value.startswith("<") and ">" in value:
        return value[1 : value.index(">")]
    return value.split(maxsplit=1)[0]


def main() -> int:
    failures: list[str] = []
    for document in tracked_markdown():
        text = document.read_text(encoding="utf-8")
        for line_number, line in enumerate(text.splitlines(), 1):
            for match in LINK.finditer(line):
                target = destination(match.group(1))
                if not target or target.startswith("#") or SCHEME.match(target):
                    continue
                path_text = unquote(target.split("#", 1)[0])
                resolved = (document.parent / path_text).resolve()
                try:
                    resolved.relative_to(ROOT)
                except ValueError:
                    failures.append(
                        f"{document.relative_to(ROOT)}:{line_number}: "
                        f"link escapes repository: {target}"
                    )
                    continue
                if not resolved.exists():
                    failures.append(
                        f"{document.relative_to(ROOT)}:{line_number}: "
                        f"missing link target: {target}"
                    )

    if failures:
        print("\n".join(failures))
        print("markdown_links=FAILED")
        return 2
    print("markdown_links=PASS")
    return 0


if __name__ == "__main__":
    sys.exit(main())
