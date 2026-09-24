#!/usr/bin/env python3
"""Densify the security KB graph: create hub notes that link CVEs by shared CWE and
vendor/product. Each hub is one Markdown note full of [[CVE-…]] links, so in the graph
the CVEs cluster into stars around their weakness type and their vendor.

Usage: python3 link_cves.py <security folder> [--cap 400] [--min 4]
Writes hubs into <folder>/hubs/. Re-run to refresh. Only reads/writes files; the app
picks them up via its watcher.
"""
from __future__ import annotations

import argparse
import re
import sys
from collections import defaultdict
from pathlib import Path

AFFECTED = re.compile(r"^\*\*Affected:\*\* (.+)$", re.M)
WEAKNESS = re.compile(r"CWE-\d+")
TITLE = re.compile(r"^# (CVE-\d{4}-\d+)", re.M)


def slug(s: str, maxlen: int = 70) -> str:
    s = re.sub(r"[^\w.-]+", "-", s.strip().lower()).strip("-")
    return (s[:maxlen] or "x").rstrip("-")


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("folder", type=Path)
    ap.add_argument("--cap", type=int, default=400, help="max CVE links per hub")
    ap.add_argument("--min", type=int, default=4, help="min CVEs for a vendor hub")
    args = ap.parse_args()

    root: Path = args.folder.expanduser()
    cve_dir = root / "cve"
    if not cve_dir.exists():
        print(f"no {cve_dir}", file=sys.stderr)
        return 1

    by_cwe: dict[str, list[str]] = defaultdict(list)
    by_vendor: dict[str, list[str]] = defaultdict(list)
    n = 0
    for f in cve_dir.rglob("CVE-*.md"):
        try:
            txt = f.read_text(encoding="utf-8", errors="replace")
        except Exception:
            continue
        cid = f.stem
        n += 1
        for c in set(WEAKNESS.findall(txt)):
            by_cwe[c].append(cid)
        m = AFFECTED.search(txt)
        if m:
            for v in m.group(1).split(","):
                v = v.strip()
                if v and v.lower() not in ("n/a", "unspecified", "unknown") and len(v) > 1:
                    by_vendor[v].append(cid)
        if n % 50000 == 0:
            print(f"  scanned {n} CVEs...", file=sys.stderr)
    print(f"scanned {n} CVEs; {len(by_cwe)} CWEs, {len(by_vendor)} vendors/products", file=sys.stderr)

    hubs = root / "hubs"
    hubs.mkdir(exist_ok=True)
    for old in hubs.glob("*.md"):
        old.unlink()

    def write_hub(fname: str, title: str, tag: str, ids: list[str]) -> bool:
        ids = sorted(set(ids))
        if len(ids) < 2:
            return False
        shown = ids[: args.cap]
        links = " ".join(f"[[{i}]]" for i in shown)
        extra = f"\n\n(+{len(ids) - len(shown)} more CVEs)" if len(ids) > len(shown) else ""
        body = f"---\ntags: [hub, {tag}]\n---\n\n# {title}\n\n{len(ids)} related CVEs:\n\n{links}{extra}\n"
        (hubs / f"{fname}.md").write_text(body, encoding="utf-8")
        return True

    cwe_hubs = sum(write_hub(c.lower(), f"{c} — weakness cluster", "cwe", ids) for c, ids in by_cwe.items())
    vend = 0
    for v, ids in by_vendor.items():
        if len(set(ids)) >= args.min and write_hub(f"vendor-{slug(v)}", f"{v} — affected products", "vendor", ids):
            vend += 1
    print(f"wrote {cwe_hubs} CWE hubs + {vend} vendor hubs into {hubs}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
