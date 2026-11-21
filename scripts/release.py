#!/usr/bin/env python3
"""Validate release metadata, print the version, and extract changelog notes."""
import argparse
from datetime import date
from pathlib import Path
import re
import sys
import tomllib

ROOT = Path(__file__).resolve().parents[1]
VERSION_PATTERN = re.compile(
    r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)"
    r"(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?"
)


def validate_version(value):
    match = VERSION_PATTERN.fullmatch(value)
    if not match:
        raise ValueError("Expected MAJOR.MINOR.PATCH[-PRERELEASE], without leading zeroes")
    if match[4] and any(part.isdigit() and len(part) > 1 and part.startswith("0") for part in match[4].split(".")):
        raise ValueError("Numeric prerelease identifiers cannot have leading zeroes")
    return value


def workspace(root=ROOT):
    return tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))["workspace"]


def version(root=ROOT):
    return validate_version(workspace(root)["package"]["version"])


def release_notes(root=ROOT, release_version=None):
    release_version = release_version or version(root)
    changelog = (root / "CHANGELOG.md").read_text(encoding="utf-8")
    pattern = rf"^## \[{re.escape(release_version)}\] - (\d{{4}}-\d{{2}}-\d{{2}})\s*$"
    matches = list(re.finditer(pattern, changelog, re.MULTILINE))
    if len(matches) != 1:
        raise ValueError(f"CHANGELOG.md must contain exactly one dated entry for {release_version}")
    entry = matches[0]
    date.fromisoformat(entry[1])
    notes = re.split(r"^## ", changelog[entry.end():], maxsplit=1, flags=re.MULTILINE)[0].strip()
    if not notes:
        raise ValueError(f"Changelog entry for {release_version} is empty")
    return f"# Transbox {release_version}\n\n{notes}\n"


def check(root=ROOT, tag=None):
    release_version = version(root)
    if tag is not None and tag != f"v{release_version}":
        raise ValueError(f"Tag {tag!r} must match workspace version v{release_version}")
    lock = tomllib.loads((root / "Cargo.lock").read_text(encoding="utf-8"))
    for member in workspace(root)["members"]:
        manifest = tomllib.loads((root / member / "Cargo.toml").read_text(encoding="utf-8"))["package"]
        if manifest.get("version") != {"workspace": True}:
            raise ValueError(f"{member} must inherit the workspace version")
        versions = [p["version"] for p in lock["package"] if p["name"] == manifest["name"] and "source" not in p]
        if versions != [release_version]:
            raise ValueError(f"Cargo.lock version for {manifest['name']} is stale; run cargo update --workspace --offline")
    release_notes(root, release_version)
    return release_version


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("version")
    validator = commands.add_parser("check")
    validator.add_argument("--tag", help="Require this tag to match the workspace version")
    notes = commands.add_parser("notes")
    notes.add_argument("--output", type=Path)
    args = parser.parse_args()
    if args.command == "version":
        print(version())
    elif args.command == "check":
        print(f"Release metadata OK: v{check(tag=args.tag)}")
    else:
        text = release_notes()
        if args.output:
            args.output.parent.mkdir(parents=True, exist_ok=True)
            args.output.write_text(text, encoding="utf-8")
        else:
            print(text, end="")


if __name__ == "__main__":
    try:
        main()
    except (ValueError, OSError, KeyError) as error:
        sys.exit(str(error))
