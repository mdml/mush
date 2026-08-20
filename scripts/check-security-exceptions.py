#!/usr/bin/env python3
"""Validate cargo-deny policy and its structured, expiring exceptions."""

from __future__ import annotations

import datetime
import sys
from pathlib import Path

if sys.version_info < (3, 11):
    raise SystemExit("check-security-exceptions.py requires Python 3.11 or newer")

import tomllib

ROOT = Path(__file__).resolve().parent.parent
REQUIRED_FIELDS = ("id", "tool", "rationale", "owner", "expires", "record")
EXPECTED_SETTINGS = {
    ("advisories", "yanked"): "deny",
    ("advisories", "unmaintained"): "all",
    ("advisories", "unsound"): "all",
    ("bans", "wildcards"): "deny",
    ("sources", "unknown-registry"): "deny",
    ("sources", "unknown-git"): "deny",
}
EXPECTED_LICENSES = {"Apache-2.0", "MIT", "Zlib"}


def load(name: str) -> dict:
    with (ROOT / name).open("rb") as stream:
        return tomllib.load(stream)


def check() -> list[str]:
    deny = load("deny.toml")
    registry = load("security-exceptions.toml")
    problems = []
    for path, expected in EXPECTED_SETTINGS.items():
        actual = deny.get(path[0], {}).get(path[1])
        if actual != expected:
            problems.append(f"deny.toml [{path[0]}].{path[1]} must be {expected!r}")
    if set(deny.get("licenses", {}).get("allow", [])) != EXPECTED_LICENSES:
        problems.append("deny.toml license allowlist differs from the accepted policy")
    if deny.get("licenses", {}).get("exceptions") != [
        {"allow": ["Unicode-3.0"], "name": "unicode-ident"}
    ]:
        problems.append("deny.toml license exceptions differ from the accepted policy")
    if deny.get("bans", {}).get("multiple-versions") != "warn":
        problems.append("deny.toml [bans].multiple-versions must be 'warn'")
    for section, key in (
        ("bans", "skip"),
        ("bans", "skip-tree"),
        ("sources", "allow-git"),
        ("sources", "allow-registry-extra"),
    ):
        if deny.get(section, {}).get(key):
            problems.append(f"deny.toml [{section}].{key} must remain empty")

    ignores = deny.get("advisories", {}).get("ignore", [])
    ignored_ids = set()
    for item in ignores:
        identifier = item.get("id") if isinstance(item, dict) else item
        if not isinstance(identifier, str) or not identifier:
            problems.append(f"deny.toml has malformed advisory ignore {item!r}")
        else:
            ignored_ids.add(identifier)
    entries = registry.get("exception", [])
    registered_ids = set()
    today = datetime.date.today()
    for index, entry in enumerate(entries, 1):
        if not isinstance(entry, dict):
            problems.append(f"security-exceptions.toml entry {index} is not a table")
            continue
        missing = [field for field in REQUIRED_FIELDS if field not in entry]
        if missing:
            problems.append(f"security-exceptions.toml entry {index} lacks {', '.join(missing)}")
            continue
        if entry["tool"] != "cargo-deny":
            problems.append(f"exception {entry['id']} has unsupported tool {entry['tool']!r}")
        for field in ("id", "rationale", "owner", "record"):
            if not isinstance(entry[field], str) or not entry[field].strip():
                problems.append(f"exception entry {index} has invalid {field}")
        expiry = entry["expires"]
        if type(expiry) is not datetime.date or expiry < today:
            problems.append(f"exception {entry['id']} has invalid or expired date {expiry!r}")
        record = str(entry["record"])
        if not record.startswith("docs/decisions/") or not record.endswith(".md"):
            problems.append(f"exception {entry['id']} record must be a decision path")
        elif not (ROOT / record).is_file():
            problems.append(f"exception {entry['id']} record does not exist: {entry['record']}")
        registered_ids.add(entry["id"])

    if len(registered_ids) != len(entries):
        problems.append("security-exceptions.toml contains duplicate exception ids")

    for identifier in sorted(ignored_ids - registered_ids):
        problems.append(f"cargo-deny ignore {identifier!r} has no registered exception")
    for identifier in sorted(registered_ids - ignored_ids):
        problems.append(f"registered exception {identifier!r} has no cargo-deny ignore")
    return problems


def main() -> int:
    problems = check()
    if problems:
        for problem in problems:
            print(f"security exceptions: {problem}", file=sys.stderr)
        return 1
    print("security exceptions: policy settings intact; all suppressions registered and current")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
