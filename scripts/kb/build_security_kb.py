#!/usr/bin/env python3
"""Build a large security knowledge base for Xode from public, freely-licensed sources.

Each item becomes a Markdown note with front matter (tags, severity, refs) and
`[[CVE-YYYY-NNNNN]]` wiki links, so advisories, PoCs and exploits all connect to
their CVE in Xode's graph. Writeup collections are cloned as-is and indexed directly.

Sources (all public):
  cve        CVEProject/cvelistV5            — official CVE records (CC0)
  ghsa       github/advisory-database        — GitHub Security Advisories (OSV, CC-BY 4.0)
  poc        nomi-sec/PoC-in-GitHub          — index of public CVE PoC repos
  exploitdb  Exploit-DB archive              — exploit metadata (+ optional source)
  bugbounty  disclosed bug-bounty writeups   — cloned writeup collections
  ctf        CTF writeups                    — cloned writeup collections

Typical use:
  # smoke: recent slice
  python3 build_security_kb.py --out ~/xode-kb/security --since 2024 --limit 400
  # full pull (large, slow)
  python3 build_security_kb.py --out ~/xode-kb/security --since 2015 --sources all

Then add ~/xode-kb/security as a Library source in Xode (or: xode, then /kb add library <path>).
Only depends on Python 3 and git.
"""
from __future__ import annotations

import argparse
import csv
import json
import re
import subprocess
import sys
from pathlib import Path

CVE_RE = re.compile(r"CVE-\d{4}-\d{4,7}", re.I)

REPOS = {
    "cve": "https://github.com/CVEProject/cvelistV5",
    "ghsa": "https://github.com/github/advisory-database",
    "poc": "https://github.com/nomi-sec/PoC-in-GitHub",
    "exploitdb": "https://gitlab.com/exploit-database/exploitdb",
}
# Writeup collections cloned and indexed as-is (each is already Markdown).
WRITEUP_REPOS = {
    "bugbounty": [
        "https://github.com/ngalongc/bug-bounty-reference",
        "https://github.com/devanshbatham/Awesome-Bugbounty-Writeups",
        "https://github.com/reddelexc/hackerone-reports",
    ],
    "ctf": [
        "https://github.com/sajjadium/ctf-archives",
        "https://github.com/Hacker0x01/awesome-ctf-writeups",
    ],
}

ALL_SOURCES = ["cve", "ghsa", "poc", "exploitdb", "bugbounty", "ctf"]


def log(msg: str) -> None:
    print(msg, file=sys.stderr, flush=True)


def run(cmd: list[str], cwd: Path | None = None, check: bool = True) -> int:
    return subprocess.run(cmd, cwd=cwd, check=check).returncode


def clone_or_pull(url: str, dest: Path, depth: int | None = 1) -> bool:
    """Shallow-clone a repo, or fast-forward it if already present. Returns False on failure."""
    try:
        if (dest / ".git").exists():
            log(f"  updating {dest.name}")
            run(["git", "-C", str(dest), "pull", "--ff-only", "--depth", str(depth or 1)], check=False)
        else:
            dest.parent.mkdir(parents=True, exist_ok=True)
            log(f"  cloning {url} -> {dest.name}")
            cmd = ["git", "clone", "--filter=blob:none"]
            if depth:
                cmd += ["--depth", str(depth)]
            cmd += [url, str(dest)]
            run(cmd)
        return True
    except subprocess.CalledProcessError as e:
        log(f"  ! clone/pull failed for {url}: {e}")
        return False


def slug(s: str, maxlen: int = 80) -> str:
    s = re.sub(r"[^\w.-]+", "-", s.strip().lower()).strip("-")
    return (s[:maxlen] or "note").rstrip("-")


def fm(front: dict) -> str:
    """Minimal YAML front matter."""
    out = ["---"]
    for k, v in front.items():
        if v is None or v == "" or v == []:
            continue
        if isinstance(v, list):
            items = ", ".join(str(x).replace(",", " ").strip() for x in v if str(x).strip())
            if not items:
                continue
            out.append(f"{k}: [{items}]")
        else:
            val = str(v).replace("\n", " ").strip()
            out.append(f"{k}: {json.dumps(val) if ':' in val or '#' in val else val}")
    out.append("---\n")
    return "\n".join(out)


def write_note(root: Path, rel: str, text: str) -> None:
    p = root / rel
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_text(text, encoding="utf-8")


def cve_links(ids: list[str]) -> str:
    seen, out = set(), []
    for cid in ids:
        cid = cid.upper()
        if cid not in seen and CVE_RE.fullmatch(cid):
            seen.add(cid)
            out.append(f"[[{cid}]]")
    return " ".join(out)


def cwe_tags(cwes: list[str]) -> list[str]:
    return [f"cwe-{re.sub(r'[^0-9]', '', c)}" for c in cwes if re.search(r"\d", c)]


def year_of(cid: str) -> str:
    m = re.search(r"CVE-(\d{4})-", cid.upper())
    return m.group(1) if m else "misc"


# ----------------------------------------------------------------- CVE (cvelistV5)

def norm_cve(src: Path, out: Path, since: int, limit: int) -> int:
    base = src / "cves"
    if not base.exists():
        log("  ! cvelistV5 layout unexpected; skipping")
        return 0
    n = 0
    years = sorted([d for d in base.iterdir() if d.is_dir() and d.name.isdigit() and int(d.name) >= since])
    for ydir in years:
        for jf in sorted(ydir.rglob("CVE-*.json")):
            if limit and n >= limit:
                return n
            try:
                data = json.loads(jf.read_text(encoding="utf-8"))
            except Exception:
                continue
            cid = data.get("cveMetadata", {}).get("cveId") or jf.stem
            state = data.get("cveMetadata", {}).get("state", "")
            cna = data.get("containers", {}).get("cna", {})
            title = cna.get("title") or ""
            descs = cna.get("descriptions", []) or []
            desc = next((d.get("value", "") for d in descs if d.get("lang", "").startswith("en")), "")
            desc = desc or (descs[0].get("value", "") if descs else "")
            if state == "REJECTED" or not desc:
                continue
            # severity from CVSS
            sev, score = "", ""
            for m in cna.get("metrics", []) or []:
                for key in ("cvssV4_0", "cvssV3_1", "cvssV3_0", "cvssV2_0"):
                    if key in m:
                        sev = (m[key].get("baseSeverity") or "").lower()
                        score = m[key].get("baseScore", "")
                        break
                if sev:
                    break
            cwes = []
            for pt in cna.get("problemTypes", []) or []:
                for d in pt.get("descriptions", []) or []:
                    if d.get("cweId"):
                        cwes.append(d["cweId"])
            refs = [r.get("url", "") for r in cna.get("references", []) or [] if r.get("url")]
            published = data.get("cveMetadata", {}).get("datePublished", "")[:10]
            vendors = []
            for a in cna.get("affected", []) or []:
                v = a.get("vendor")
                p = a.get("product")
                if v and v not in ("n/a", "unspecified"):
                    vendors.append(v)
                if p and p not in ("n/a", "unspecified"):
                    vendors.append(p)
            tags = ["cve"] + cwe_tags(cwes) + ([sev] if sev else [])
            front = {
                "title": cid,
                "tags": sorted(set(tags)),
                "source": "cvelistV5",
                "id": cid,
                "severity": f"{sev} {score}".strip(),
                "published": published,
            }
            body = [fm(front), f"# {cid}"]
            if title:
                body.append(f"\n**{title}**")
            body.append(f"\n{desc.strip()}\n")
            if vendors:
                body.append(f"**Affected:** {', '.join(sorted(set(vendors))[:20])}")
            if cwes:
                body.append(f"**Weakness:** {', '.join(sorted(set(cwes)))}")
            if refs:
                body.append("\n## References\n" + "\n".join(f"- {u}" for u in refs[:30]))
            write_note(out, f"cve/{ydir.name}/{cid}.md", "\n".join(body) + "\n")
            n += 1
    return n


# ----------------------------------------------------------------- GHSA (OSV)

def norm_ghsa(src: Path, out: Path, since: int, limit: int) -> int:
    base = src / "advisories" / "github-reviewed"
    if not base.exists():
        base = src / "advisories"
    n = 0
    for jf in sorted(base.rglob("GHSA-*.json")):
        if limit and n >= limit:
            return n
        try:
            d = json.loads(jf.read_text(encoding="utf-8"))
        except Exception:
            continue
        gid = d.get("id", jf.stem)
        published = (d.get("published") or "")[:10]
        if since and published[:4].isdigit() and int(published[:4]) < since:
            continue
        summary = d.get("summary", "") or gid
        details = d.get("details", "") or ""
        aliases = [a for a in d.get("aliases", []) if CVE_RE.fullmatch(a or "")]
        sev = (d.get("database_specific", {}) or {}).get("severity", "").lower()
        cwes = (d.get("database_specific", {}) or {}).get("cwe_ids", []) or []
        pkgs = []
        for a in d.get("affected", []) or []:
            pk = a.get("package", {}) or {}
            if pk.get("name"):
                pkgs.append(f"{pk.get('ecosystem','')}:{pk['name']}".strip(":"))
        refs = [r.get("url", "") for r in d.get("references", []) or [] if r.get("url")]
        tags = ["advisory", "ghsa"] + cwe_tags(cwes) + ([sev] if sev else [])
        front = {"title": summary[:120], "tags": sorted(set(tags)), "source": "ghsa",
                 "id": gid, "severity": sev, "published": published, "aliases": aliases}
        body = [fm(front), f"# {summary}", f"\n`{gid}`" + (f" · {cve_links(aliases)}" if aliases else "")]
        if details:
            body.append(f"\n{details.strip()[:8000]}\n")
        if pkgs:
            body.append(f"**Packages:** {', '.join(sorted(set(pkgs))[:20])}")
        if refs:
            body.append("\n## References\n" + "\n".join(f"- {u}" for u in refs[:30]))
        write_note(out, f"advisories/{(published[:4] or 'misc')}/{gid}.md", "\n".join(body) + "\n")
        n += 1
    return n


# ----------------------------------------------------------------- PoC-in-GitHub

def norm_poc(src: Path, out: Path, since: int, limit: int) -> int:
    n = 0
    years = sorted([d for d in src.iterdir() if d.is_dir() and d.name.isdigit() and int(d.name) >= since])
    for ydir in years:
        for jf in sorted(ydir.glob("CVE-*.json")):
            if limit and n >= limit:
                return n
            try:
                repos = json.loads(jf.read_text(encoding="utf-8"))
            except Exception:
                continue
            if not repos:
                continue
            cid = jf.stem.upper()
            repos = sorted(repos, key=lambda r: r.get("stargazers_count", 0), reverse=True)
            tags = ["poc", "exploit"]
            front = {"title": f"PoC — {cid}", "tags": tags, "source": "poc-in-github", "id": cid}
            body = [fm(front), f"# Public PoCs for {cid}", f"\n{cve_links([cid])}\n",
                    f"{len(repos)} public proof-of-concept repositor{'y' if len(repos)==1 else 'ies'}:\n"]
            for r in repos[:40]:
                stars = r.get("stargazers_count", 0)
                desc = (r.get("description") or "").strip().replace("\n", " ")
                body.append(f"- [{r.get('full_name','?')}]({r.get('html_url','')}) · ★{stars}" + (f" — {desc[:160]}" if desc else ""))
            write_note(out, f"poc/{ydir.name}/{cid}-poc.md", "\n".join(body) + "\n")
            n += 1
    return n


# ----------------------------------------------------------------- Exploit-DB

def norm_exploitdb(src: Path, out: Path, limit: int, full: bool) -> int:
    csvf = src / "files_exploits.csv"
    if not csvf.exists():
        log("  ! files_exploits.csv not found; skipping exploitdb")
        return 0
    n = 0
    with csvf.open(encoding="utf-8", errors="replace", newline="") as fh:
        for row in csv.DictReader(fh):
            if limit and n >= limit:
                break
            eid = row.get("id", "").strip()
            desc = (row.get("description") or "").strip()
            if not eid or not desc:
                continue
            codes = [c for c in re.split(r"[;\s]+", row.get("codes", "") or "") if CVE_RE.fullmatch(c)]
            typ = (row.get("type") or "").strip()
            plat = (row.get("platform") or "").strip()
            date = (row.get("date_published") or row.get("date") or "")[:10]
            author = (row.get("author") or "").strip()
            fpath = (row.get("file") or "").strip()
            tags = ["exploit", "exploitdb"] + ([typ.lower()] if typ else []) + ([plat.lower()] if plat else [])
            front = {"title": desc[:120], "tags": sorted(set(tags)), "source": "exploit-db",
                     "id": f"EDB-{eid}", "published": date, "aliases": codes}
            body = [fm(front), f"# {desc}", f"\nExploit-DB **{eid}** · type: {typ or '?'} · platform: {plat or '?'}"
                    + (f" · by {author}" if author else "")]
            if codes:
                body.append(f"\n{cve_links(codes)}")
            body.append(f"\n**Source file:** `{fpath}` (in the Exploit-DB archive)")
            if full and fpath:
                ep = src / fpath
                if ep.exists() and ep.stat().st_size < 200_000:
                    try:
                        code = ep.read_text(encoding="utf-8", errors="replace")
                        body.append("\n```\n" + code[:40000] + "\n```")
                    except Exception:
                        pass
            yr = date[:4] if date[:4].isdigit() else "misc"
            write_note(out, f"exploitdb/{yr}/EDB-{eid}-{slug(desc,40)}.md", "\n".join(body) + "\n")
            n += 1
    return n


# ----------------------------------------------------------------- writeup repos (raw md)

def clone_writeups(kind: str, out: Path, work: Path, depth: int) -> int:
    dest_root = out / kind
    total = 0
    for url in WRITEUP_REPOS.get(kind, []):
        name = url.rstrip("/").split("/")[-1]
        dest = dest_root / name
        if clone_or_pull(url, dest, depth=depth):
            # Drop the .git to keep the Library folder clean (indexed as plain files).
            git_dir = dest / ".git"
            if git_dir.exists():
                subprocess.run(["rm", "-rf", str(git_dir)], check=False)
            total += sum(1 for _ in dest.rglob("*.md"))
    return total


# ----------------------------------------------------------------- index note

def write_index(out: Path, counts: dict) -> None:
    lines = [
        "---\ntags: [index, security]\n---",
        "# Security knowledge base\n",
        "Public security research indexed for Xode: CVEs, advisories, public PoCs, exploit metadata and writeups. "
        "Notes cross-link to their `[[CVE-YYYY-NNNNN]]`.\n",
        "## Contents\n",
    ]
    labels = {"cve": "CVE records", "ghsa": "GitHub advisories", "poc": "Public PoCs",
              "exploitdb": "Exploit-DB entries", "bugbounty": "Bug-bounty writeups", "ctf": "CTF writeups"}
    for k in ALL_SOURCES:
        if counts.get(k):
            lines.append(f"- **{labels[k]}** — {counts[k]:,} notes (`{k}/`)")
    lines.append("\n_Generated by `scripts/kb/build_security_kb.py`. Re-run to update._")
    write_note(out, "README.md", "\n".join(lines) + "\n")


def main() -> int:
    ap = argparse.ArgumentParser(description="Build a security knowledge base for Xode.")
    ap.add_argument("--out", required=True, type=Path, help="output Library folder")
    ap.add_argument("--work", type=Path, default=Path.home() / ".cache" / "xode-kb-src", help="clone cache dir")
    ap.add_argument("--sources", default="cve,ghsa,poc", help="comma list or 'all'")
    ap.add_argument("--since", type=int, default=2018, help="earliest year (cve/ghsa/poc)")
    ap.add_argument("--limit", type=int, default=0, help="max notes per source (0 = all); use for smoke runs")
    ap.add_argument("--depth", type=int, default=1, help="git clone depth")
    ap.add_argument("--full-exploits", action="store_true", help="embed exploit source into exploitdb notes")
    args = ap.parse_args()

    sources = ALL_SOURCES if args.sources.strip() == "all" else [s.strip() for s in args.sources.split(",") if s.strip()]
    bad = [s for s in sources if s not in ALL_SOURCES]
    if bad:
        ap.error(f"unknown sources: {bad}; choose from {ALL_SOURCES}")

    out: Path = args.out.expanduser()
    work: Path = args.work.expanduser()
    out.mkdir(parents=True, exist_ok=True)
    work.mkdir(parents=True, exist_ok=True)
    counts: dict[str, int] = {}

    for s in sources:
        log(f"[{s}]")
        if s in WRITEUP_REPOS:
            counts[s] = clone_writeups(s, out, work, args.depth)
        else:
            dest = work / s
            if not clone_or_pull(REPOS[s], dest, depth=args.depth):
                continue
            if s == "cve":
                counts[s] = norm_cve(dest, out, args.since, args.limit)
            elif s == "ghsa":
                counts[s] = norm_ghsa(dest, out, args.since, args.limit)
            elif s == "poc":
                counts[s] = norm_poc(dest, out, args.since, args.limit)
            elif s == "exploitdb":
                counts[s] = norm_exploitdb(dest, out, args.limit, args.full_exploits)
        log(f"  -> {counts.get(s, 0)} notes")

    write_index(out, counts)
    total = sum(counts.values())
    log(f"\nDone: {total:,} notes in {out}")
    log(f"Add it in Xode: Library → add folder → {out}   (or  /kb add library {out})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
