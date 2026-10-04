#!/usr/bin/env python3
"""Fetch pinned vendor PDFs, verify them offline, and search extracted pages.

No Python packages required. Fetch requires curl; extraction requires Poppler's
pdfinfo/pdftotext. These tools are unrelated to the cargo test/runtime dependency.
"""
import argparse
import hashlib
import json
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1] / "specs"


def manifest():
    return json.loads((ROOT / "sources.json").read_text(encoding="utf-8"))["documents"]


def verify_file(path, doc):
    content = path.read_bytes()
    if not content.startswith(b"%PDF-"):
        raise ValueError(f"{path}: not a PDF")
    if len(content) != doc["bytes"]:
        raise ValueError(f"{path}: size changed ({len(content)} != {doc['bytes']})")
    actual = hashlib.sha256(content).hexdigest()
    if actual != doc["sha256"]:
        raise ValueError(f"{path}: SHA-256 changed; expected {doc['sha256']}, got {actual}")


def pages(path):
    result = path.read_text(encoding="utf-8").split("\f")
    if result and not result[-1].strip():
        result.pop()
    return result


def extract(doc):
    for tool in ("pdfinfo", "pdftotext"):
        if not shutil.which(tool):
            raise RuntimeError(f"{tool} not found; install Poppler (macOS: brew install poppler)")
    pdf = ROOT / doc["pdf"]
    verify_file(pdf, doc)
    info = subprocess.check_output(["pdfinfo", str(pdf)], text=True)
    match = re.search(r"^Pages:\s+(\d+)$", info, re.MULTILINE)
    if not match or int(match.group(1)) != doc["pages"]:
        raise ValueError(f"{pdf}: unexpected PDF page count")
    text = ROOT / doc["text"]
    text.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(dir=text.parent, suffix=".txt", delete=False) as tmp:
        temp = Path(tmp.name)
    try:
        subprocess.run(["pdftotext", "-layout", "-enc", "UTF-8", str(pdf), str(temp)], check=True)
        extracted = pages(temp)
        if len(extracted) != doc["pages"] or not any(doc["revision_marker"] in page for page in extracted):
            raise ValueError(f"{pdf}: text extraction failed page/revision validation")
        temp.replace(text)
    finally:
        temp.unlink(missing_ok=True)


def fetch(doc):
    if not shutil.which("curl"):
        raise RuntimeError("curl not found")
    destination = ROOT / doc["pdf"]
    destination.parent.mkdir(parents=True, exist_ok=True)
    if destination.exists():
        verify_file(destination, doc)  # Never silently replace a changed document.
        print(f"Verified existing {doc['id']}")
        return
    with tempfile.NamedTemporaryFile(dir=destination.parent, suffix=".pdf", delete=False) as tmp:
        temp = Path(tmp.name)
    try:
        subprocess.run([
            "curl", "--fail", "--silent", "--show-error", "--location",
            "--proto", "=https", "--proto-redir", "=https",
            "--connect-timeout", "15", "--max-time", "120",
            "--max-filesize", str(doc["bytes"]),
            "--output", str(temp), doc["url"],
        ], check=True)
        verify_file(temp, doc)
        temp.replace(destination)
        print(f"Downloaded and verified {doc['id']}")
    finally:
        temp.unlink(missing_ok=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["fetch", "check", "extract", "find", "page"])
    parser.add_argument("--document", help="document id (omit for all, except page)")
    parser.add_argument("--extract", action="store_true", help="extract searchable text after fetch")
    parser.add_argument("--pattern", help="regex for find, case-insensitive")
    parser.add_argument("--number", type=int, help="1-based PDF page number for page")
    args = parser.parse_args()
    documents = manifest()
    if args.document:
        documents = [doc for doc in documents if doc["id"] == args.document]
        if not documents:
            parser.error(f"unknown document {args.document}")
    if args.action == "page" and (len(documents) != 1 or not args.number):
        parser.error("page requires --document and --number")
    if args.action == "find" and not args.pattern:
        parser.error("find requires --pattern")
    if args.extract and args.action != "fetch":
        parser.error("--extract is only valid for fetch; use the extract action otherwise")
    for doc in documents:
        if args.action == "fetch":
            fetch(doc)
            if args.extract:
                extract(doc)
        elif args.action == "extract":
            extract(doc)
            print(f"Extracted {doc['id']} ({doc['pages']} pages)")
        else:
            verify_file(ROOT / doc["pdf"], doc)
            if args.action == "check":
                text = ROOT / doc["text"]
                if text.exists():
                    extracted = pages(text)
                    if len(extracted) != doc["pages"] or not any(doc["revision_marker"] in page for page in extracted):
                        raise ValueError(f"{text}: text page/revision mismatch")
                print(f"Verified {doc['id']} ({doc['bytes']} bytes, {doc['pages']} pages)")
            else:
                extracted = pages(ROOT / doc["text"])
                if len(extracted) != doc["pages"]:
                    raise ValueError(f"{doc['id']}: text page count mismatch; run extract")
                if args.action == "page":
                    if not 1 <= args.number <= len(extracted):
                        parser.error(f"page out of range: 1..{len(extracted)}")
                    print(extracted[args.number - 1])
                else:
                    pattern = re.compile(args.pattern, re.IGNORECASE)
                    for number, page in enumerate(extracted, 1):
                        for line in page.splitlines():
                            if pattern.search(line):
                                print(f"{doc['id']}:PDF-page-{number}: {line.strip()}")


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, RuntimeError, subprocess.CalledProcessError) as error:
        print(f"specs: {error}", file=sys.stderr)
        sys.exit(1)
