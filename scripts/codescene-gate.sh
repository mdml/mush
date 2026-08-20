#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

command -v cs >/dev/null 2>&1 || {
  echo "codescene: CodeScene CLI 1.0.39 is required; install it and add the 'cs' command to PATH" >&2
  exit 1
}
command -v jq >/dev/null 2>&1 || {
  echo "codescene: jq is required to read CodeScene's JSON result; install jq with the system package manager" >&2
  exit 1
}

actual_version="$(CS_DISABLE_VERSION_CHECK=1 cs version 2>&1 | sed -n 's/^cs version \([^ ]*\).*/\1/p')"
if [[ "$actual_version" != "1.0.39" ]]; then
  echo "codescene: cs 1.0.39 is required (found ${actual_version:-unknown})" >&2
  exit 1
fi

if [[ -z "${CS_ACCESS_TOKEN:-}" ]]; then
  echo "codescene: CS_ACCESS_TOKEN is required; create a PAT at https://codescene.io/users/me/pat and export it" >&2
  exit 1
fi
export CS_DISABLE_VERSION_CHECK=1

mapfile -t files < <(git ls-files --cached --others --exclude-standard 'src/*.rs' | sort)
if ((${#files[@]} == 0)); then
  echo "codescene: no supported production files were found under src/" >&2
  exit 1
fi

failed=0
scored=0
no_code=0
for file in "${files[@]}"; do
  if ! review="$(cs review --output-format json "$file" 2>&1)"; then
    echo "codescene: review failed for $file" >&2
    printf '%s\n' "$review" >&2
    failed=1
    continue
  fi
  score="$(printf '%s' "$review" | jq -r '.score')"
  if [[ "$score" == "null" ]]; then
    printf 'codescene: n/a  %s (no scorable code)\n' "$file"
    no_code=$((no_code + 1))
  elif printf '%s' "$review" | jq -e '.score == 10' >/dev/null; then
    printf 'codescene: 10.0 %s\n' "$file"
    scored=$((scored + 1))
  else
    printf 'codescene: %s %s (required: 10.0)\n' "$score" "$file" >&2
    failed=1
  fi
done

if ((scored == 0)); then
  echo "codescene: no production file received a score; the gate measured nothing" >&2
  exit 1
fi
if ((failed != 0)); then
  echo "codescene: supported production files must score 10.0 under stock rules" >&2
  exit 1
fi
echo "codescene: $scored file(s) at 10.0; $no_code file(s) contained no scorable code"
