#!/usr/bin/env python3
"""Print the CHANGELOG.md section for a version (Keep a Changelog format).

Usage: python scripts/extract_changelog.py 0.6.0 > release-notes.md
Exits 1 if the section is missing or empty (the release workflow fails rather than ship empty notes).
"""

import re
import sys
from pathlib import Path

version = sys.argv[1].lstrip("v")
text = (Path(__file__).resolve().parent.parent / "CHANGELOG.md").read_text(encoding="utf-8")
m = re.search(
    rf"^## \[{re.escape(version)}\][^\n]*\n(.*?)(?=^## \[|^\[[^\]]+\]: |\Z)", text, re.S | re.M
)
body = m.group(1).strip() if m else ""
if not body:
    sys.exit(f"error: no CHANGELOG.md section for {version}")
print(body)
