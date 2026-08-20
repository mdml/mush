#!/usr/bin/env bash
set -euo pipefail

required_files=(
  AGENTS.md
  README.md
  docs/AGENTS.md
  docs/ABSTRACTIONS.md
  docs/ARCHITECTURE.md
  docs/CONTRIBUTING.md
  docs/ROADMAP.md
  docs/VERIFICATION.md
)

for required_file in "${required_files[@]}"; do
  if [[ ! -f "$required_file" ]]; then
    echo "repository policy: required file is missing: $required_file" >&2
    exit 1
  fi
done

if [[ ! -L docs/CLAUDE.md || "$(readlink docs/CLAUDE.md)" != "AGENTS.md" ]]; then
  echo "repository policy: docs/CLAUDE.md must be a relative symlink to AGENTS.md" >&2
  exit 1
fi

mapfile -t markdown_files < <(rg --files -g '*.md' | sort)
if ((${#markdown_files[@]} == 0)); then
  echo "repository policy: no Markdown files found" >&2
  exit 1
fi

if rg --line-number --with-filename $'\r$|[ \t]+$' "${markdown_files[@]}"; then
  echo "repository policy: Markdown contains CRLF or trailing whitespace" >&2
  exit 1
fi

if rg --line-number --with-filename '/(home|Users)/[^/[:space:]`]+' "${markdown_files[@]}"; then
  echo "repository policy: Markdown contains a machine-specific absolute path" >&2
  exit 1
fi

if rg --ignore-case --line-number --with-filename 'personal-toolchain|reviewed unit|investment (cap|limit)|review budget|one block per week' "${markdown_files[@]}"; then
  echo "repository policy: Markdown contains private planning or integration language" >&2
  exit 1
fi

mapfile -t active_milestones < <(rg '^\| M[0-9]+ .*\| Active \|$' docs/ROADMAP.md)
if ((${#active_milestones[@]} != 1)); then
  echo "repository policy: roadmap must contain exactly one active milestone" >&2
  exit 1
fi

git diff --check HEAD --
