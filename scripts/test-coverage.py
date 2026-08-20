#!/usr/bin/env python3
"""Focused tests for coverage floor, ratchet, and branch enforcement."""

from __future__ import annotations

import importlib.util
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("check-coverage.py")
SPEC = importlib.util.spec_from_file_location("mush_coverage", SCRIPT)
assert SPEC and SPEC.loader
coverage = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(coverage)


def report(line: float, branch: float, branch_count: int = 10) -> dict:
    return {
        "data": [
            {
                "totals": {
                    "lines": {"percent": line},
                    "branches": {"percent": branch, "count": branch_count},
                },
                "files": [
                    {
                        "filename": str(coverage.ROOT / path),
                        "summary": {
                            "lines": {"count": 10_000, "covered": int(line * 100)},
                            "branches": {
                                "count": 10_000 if branch_count else 0,
                                "covered": int(branch * 100) if branch_count else 0,
                            },
                        },
                    }
                    for path in sorted(
                        item.relative_to(coverage.ROOT).as_posix()
                        for item in (coverage.ROOT / "src").rglob("*.rs")
                        if item.relative_to(coverage.ROOT).as_posix() != "src/store.rs"
                    )
                ],
            }
        ]
    }


POLICY = {
    "scope": {"production_paths": ["src/"], "non_executable": ["src/store.rs"]},
    "floor": {"line": 90.0, "branch": 80.0},
    "baseline": {"line": 90.92, "branch": 83.73},
}


class CoverageTests(unittest.TestCase):
    def test_measured_baseline_passes(self):
        self.assertEqual(coverage.violations(report(90.92, 83.73), POLICY), [])

    def test_ratchets_are_enforced_even_above_floors(self):
        policy = {
            **POLICY,
            "baseline": {"line": 95.0, "branch": 90.0},
        }
        problems = coverage.violations(report(94.0, 89.0), policy)
        self.assertEqual(len(problems), 2)
        self.assertTrue(all("baseline" in problem for problem in problems))

    def test_zero_branches_cannot_pass(self):
        problems = coverage.violations(report(100.0, 100.0, branch_count=0), POLICY)
        self.assertIn("branch instrumentation measured zero branches", problems)

    def test_missing_production_file_fails(self):
        measured = report(100.0, 100.0)
        measured["data"][0]["files"] = []
        problems = coverage.violations(measured, POLICY)
        self.assertIn("coverage report measured no production files", problems)

    def test_report_wide_totals_do_not_change_production_result(self):
        measured = report(90.92, 83.73)
        measured["data"][0]["totals"]["lines"]["percent"] = 1.0
        measured["data"][0]["totals"]["branches"]["percent"] = 1.0
        self.assertEqual(coverage.violations(measured, POLICY), [])


if __name__ == "__main__":
    unittest.main()
