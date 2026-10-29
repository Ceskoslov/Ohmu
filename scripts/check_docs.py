#!/usr/bin/env python3
"""Check repository-local Markdown links and the README language switch."""
from pathlib import Path
import re
import sys
from urllib.parse import unquote, urlsplit

ROOT = Path(__file__).resolve().parents[1]
errors = []
count = 0
for document in sorted(ROOT.rglob("*.md")):
    if any(part in {"target", ".git", ".local"} for part in document.relative_to(ROOT).parts):
        continue
    count += 1
    body = document.read_text(encoding="utf-8")
    # Ignore fenced examples; links in the actual prose must resolve.
    prose = re.sub(r"```.*?```", "", body, flags=re.S)
    for destination in re.findall(r"\[[^\]]*\]\(([^)]+)\)", prose):
        destination = destination.strip().split(" ", 1)[0].strip("<>")
        link = urlsplit(destination)
        if link.scheme or link.netloc or not link.path:
            continue
        target = (document.parent / unquote(link.path)).resolve()
        if not target.is_relative_to(ROOT) or not target.exists():
            errors.append(f"{document.relative_to(ROOT)}: broken/local-escape link: {destination}")

for filename, other in [("README.md", "README.en.md"), ("README.en.md", "README.md")]:
    if f"]({other})" not in (ROOT / filename).read_text(encoding="utf-8"):
        errors.append(f"{filename}: missing language switch to {other}")

if errors:
    print("\n".join(errors), file=sys.stderr)
    sys.exit(1)
print(f"Checked local links in {count} Markdown files and both README language switches.")
