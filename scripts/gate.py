#!/usr/bin/env python3
"""Run Mush's merge-confidence checks with summary or verbose rendering."""

from __future__ import annotations

import subprocess
import sys
from pathlib import Path
from typing import NamedTuple

if sys.version_info < (3, 11):
    raise SystemExit("gate.py requires Python 3.11 or newer")

ROOT = Path(__file__).resolve().parent.parent


class Step(NamedTuple):
    name: str
    argv: tuple[str, ...]


STEPS = (
    Step("repository checks", ("just", "check")),
    Step("coverage", ("scripts/coverage-gate.sh",)),
    Step("dependency policy", ("scripts/dependency-gate.sh",)),
    Step("CodeScene", ("scripts/codescene-gate.sh",)),
)


def run_step(step: Step) -> subprocess.CompletedProcess[str]:
    try:
        return subprocess.run(
            step.argv,
            cwd=ROOT,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            check=False,
        )
    except OSError as error:
        command = step.argv[0]
        return subprocess.CompletedProcess(
            step.argv,
            127,
            f"gate: cannot run {command!r}: {error}\n",
            "",
        )


def run(steps: tuple[Step, ...], verbose: bool) -> int:
    failures = 0
    for step in steps:
        result = run_step(step)
        if verbose or result.returncode != 0:
            print(f"--- {step.name} ---")
            print(result.stdout, end="" if result.stdout.endswith("\n") else "\n")
        verdict = "PASS" if result.returncode == 0 else "FAIL"
        print(f"{verdict}  {step.name}")
        failures += result.returncode != 0
    if failures:
        print(f"gate: {failures} required check(s) failed", file=sys.stderr)
        return 1
    print("gate: all required local merge-confidence checks passed")
    return 0


def main() -> int:
    if sys.argv[1:] not in ([], ["--verbose"]):
        print("usage: gate.py [--verbose]", file=sys.stderr)
        return 2
    return run(STEPS, verbose=sys.argv[1:] == ["--verbose"])


if __name__ == "__main__":
    raise SystemExit(main())
