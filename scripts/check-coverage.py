#!/usr/bin/env python3
"""Enforce Mush's measured line and branch coverage ratchets."""

from __future__ import annotations

import json
import sys
from pathlib import Path

if sys.version_info < (3, 11):
    raise SystemExit("check-coverage.py requires Python 3.11 or newer")

import tomllib

ROOT = Path(__file__).resolve().parent.parent


def relative(filename: str) -> str:
    path = Path(filename)
    try:
        return path.resolve().relative_to(ROOT).as_posix()
    except ValueError:
        return path.as_posix()


def scoped_totals(report: dict, policy: dict) -> dict[str, dict[str, float | int]]:
    prefixes = policy["scope"]["production_paths"]
    files = [
        item
        for item in report["data"][0]["files"]
        if any(relative(item["filename"]).startswith(prefix) for prefix in prefixes)
    ]
    totals = {}
    for metric in ("lines", "branches"):
        count = sum(item["summary"][metric]["count"] for item in files)
        covered = sum(item["summary"][metric]["covered"] for item in files)
        totals[metric] = {
            "count": count,
            "covered": covered,
            "percent": covered * 100 / count if count else 0.0,
        }
    return totals


def production_scope_violations(report: dict, policy: dict) -> list[str]:
    prefixes = policy["scope"]["production_paths"]
    non_executable = set(policy["scope"].get("non_executable", []))
    measured = {
        relative(item["filename"])
        for item in report["data"][0]["files"]
        if any(relative(item["filename"]).startswith(prefix) for prefix in prefixes)
    }
    expected = {
        path.relative_to(ROOT).as_posix()
        for prefix in prefixes
        for path in (ROOT / prefix).rglob("*.rs")
    }
    missing = sorted(expected - measured - non_executable)
    problems = [f"production file was not measured: {path}" for path in missing]
    for path in sorted(non_executable - expected):
        problems.append(f"declared non-executable production file does not exist: {path}")
    for path in sorted(non_executable & measured):
        problems.append(
            f"declared non-executable production file is now measured and must leave the exception: {path}"
        )
    if not measured:
        problems.append("coverage report measured no production files")
    return problems


def violations(report: dict, policy: dict) -> list[str]:
    totals = scoped_totals(report, policy)
    measured = {
        "line": totals["lines"]["percent"],
        "branch": totals["branches"]["percent"],
    }
    found = production_scope_violations(report, policy)
    if totals["branches"]["count"] == 0:
        found.append("branch instrumentation measured zero branches")
    for metric in ("line", "branch"):
        for rule in ("floor", "baseline"):
            required = policy[rule][metric]
            if measured[metric] < required:
                found.append(
                    f"{metric} coverage {measured[metric]:.2f}% is below the "
                    f"{rule} of {required:.2f}%"
                )
    return found


def main() -> int:
    if len(sys.argv) != 3:
        print("usage: check-coverage.py <coverage.json> <coverage-baseline.toml>", file=sys.stderr)
        return 2
    report_path, policy_path = map(Path, sys.argv[1:])
    report = json.loads(report_path.read_text())
    with policy_path.open("rb") as stream:
        policy = tomllib.load(stream)
    totals = scoped_totals(report, policy)
    problems = violations(report, policy)
    print(
        "coverage: "
        f"line {totals['lines']['percent']:.2f}%, "
        f"branch {totals['branches']['percent']:.2f}%"
    )
    if problems:
        for problem in problems:
            print(f"coverage: {problem}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
