#!/usr/bin/env python3
"""Focused tests for gate result and rendering semantics."""

from __future__ import annotations

import contextlib
import importlib.util
import io
import subprocess
import unittest
from pathlib import Path
from unittest.mock import patch

SCRIPT = Path(__file__).with_name("gate.py")
SPEC = importlib.util.spec_from_file_location("mush_gate", SCRIPT)
assert SPEC and SPEC.loader
gate = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(gate)


class GateTests(unittest.TestCase):
    def execute(self, verbose: bool, returncodes: list[int]) -> tuple[int, str, list[tuple[str, ...]]]:
        commands = []

        def fake_run(step):
            commands.append(step.argv)
            code = returncodes[len(commands) - 1]
            return subprocess.CompletedProcess(step.argv, code, f"output for {step.name}\n", "")

        stream = io.StringIO()
        with patch.object(gate, "run_step", side_effect=fake_run), contextlib.redirect_stdout(stream), contextlib.redirect_stderr(stream):
            result = gate.run(gate.STEPS, verbose)
        return result, stream.getvalue(), commands

    def test_verbose_changes_only_rendering(self):
        quiet = self.execute(False, [0, 0, 0, 0])
        verbose = self.execute(True, [0, 0, 0, 0])
        self.assertEqual(quiet[0], verbose[0])
        self.assertEqual(quiet[2], verbose[2])
        self.assertNotIn("output for coverage", quiet[1])
        self.assertIn("output for coverage", verbose[1])

    def test_every_step_runs_and_any_failure_fails_the_gate(self):
        result, output, commands = self.execute(False, [0, 1, 0, 1])
        self.assertEqual(result, 1)
        self.assertEqual(commands, [step.argv for step in gate.STEPS])
        self.assertIn("FAIL  coverage", output)
        self.assertIn("FAIL  CodeScene", output)
        self.assertIn("output for coverage", output)


if __name__ == "__main__":
    unittest.main()
