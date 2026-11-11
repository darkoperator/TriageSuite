#!/usr/bin/env bash
# Every #[allow(clippy::...)] in the workspace must carry a justification: a
# `//` comment on the line directly above it (a `///` doc line counts too).
#
# Why this exists: the orchestrator's 7-argument build() was flagged by clippy
# and shipped anyway behind a bare `#[allow(clippy::too_many_arguments)]`. A
# silenced lint with no stated reason is an unreviewed decision. Silencing one
# is still allowed -- saying why is not optional.
#
# Usage: scripts/check-allow-justifications.sh [root]   (default: repo root)
set -euo pipefail

root="${1:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
cd "$root"

# oxidelens/ is a cargo workspace of its own, but the same rule binds it.
scan_dirs=(crates)
if [ -d oxidelens/crates ]; then
  scan_dirs+=(oxidelens/crates)
fi

violations=0
while IFS= read -r file; do
  # Read the file once; awk reports 1-based line numbers of offending allows.
  while IFS= read -r lineno; do
    printf '%s:%s: #[allow(...)] with no justification comment above it\n' \
      "$file" "$lineno"
    violations=$((violations + 1))
  done < <(awk '
    /#!?\[allow\(clippy::/ {
      # prev is the previous non-blank line, trimmed of leading whitespace.
      if (prev !~ /^\/\//) print NR
    }
    { line = $0
      sub(/^[[:space:]]+/, "", line)
      if (line != "") prev = line }
  ' "$file")
done < <(find "${scan_dirs[@]}" -name '*.rs' -type f | sort)

if [ "$violations" -gt 0 ]; then
  echo
  echo "$violations un-justified #[allow(clippy::...)] attribute(s)."
  echo "Add a // comment directly above each one saying why the lint is wrong here."
  exit 1
fi

echo "All #[allow(clippy::...)] attributes carry a justification."
