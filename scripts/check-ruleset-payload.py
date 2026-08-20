#!/usr/bin/env python3
"""Check the protected-main payload and its required CI context names."""

from __future__ import annotations

import json
import sys
from pathlib import Path

if sys.version_info < (3, 11):
    raise SystemExit("check-ruleset-payload.py requires Python 3.11 or newer")

ROOT = Path(__file__).resolve().parent.parent
EXPECTED_CONTEXTS = ["Repository checks", "Coverage", "Dependency policy"]
CONTEXT_WORKFLOWS = {
    "Repository checks": ".github/workflows/ci.yml",
    "Coverage": ".github/workflows/ci.yml",
    "Dependency policy": ".github/workflows/security.yml",
}


def rule(payload: dict, kind: str) -> dict | None:
    return next((item for item in payload["rules"] if item["type"] == kind), None)


def check(payload: dict) -> list[str]:
    problems = []
    if payload.get("enforcement") != "active":
        problems.append("ruleset enforcement must be active")
    if payload.get("bypass_actors") != []:
        problems.append("ruleset must not have bypass actors")
    conditions = payload.get("conditions", {}).get("ref_name", {})
    if conditions.get("include") != ["refs/heads/main"] or conditions.get("exclude") != []:
        problems.append("ruleset must target only refs/heads/main")
    pull_request = rule(payload, "pull_request") or {}
    parameters = pull_request.get("parameters", {})
    if parameters.get("required_approving_review_count") != 0:
        problems.append("solo-maintainer ruleset must require zero approving reviews")
    if parameters.get("require_last_push_approval") is not False:
        problems.append("solo-maintainer ruleset must not require last-push approval")
    if parameters.get("allowed_merge_methods") != ["rebase", "squash"]:
        problems.append("ruleset must allow only the accepted linear merge methods")
    statuses = (rule(payload, "required_status_checks") or {}).get("parameters", {})
    if statuses.get("strict_required_status_checks_policy") is not True:
        problems.append("required checks must run against an up-to-date branch")
    contexts = [item["context"] for item in statuses.get("required_status_checks", [])]
    if contexts != EXPECTED_CONTEXTS:
        problems.append(f"required contexts are {contexts!r}; expected {EXPECTED_CONTEXTS!r}")
    for context, workflow in CONTEXT_WORKFLOWS.items():
        marker = f"    name: {context}\n"
        if marker not in (ROOT / workflow).read_text():
            problems.append(f"required context {context!r} is not a job name in {workflow}")
    for kind in ("required_linear_history", "non_fast_forward", "deletion"):
        if rule(payload, kind) is None:
            problems.append(f"ruleset lacks {kind}")
    return problems


def main() -> int:
    path = ROOT / ".github/rulesets/main-branch.json"
    problems = check(json.loads(path.read_text()))
    if problems:
        for problem in problems:
            print(f"ruleset: {problem}", file=sys.stderr)
        return 1
    print(f"ruleset: protected main requires {', '.join(EXPECTED_CONTEXTS)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
