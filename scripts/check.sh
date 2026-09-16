#!/usr/bin/env bash
# The full local gate: everything CI used to be asked to do, run where the
# evidence actually is.
#
# The compatibility fixtures under `test captures/` are the oracle for most of
# this suite -- they are what proves a decode table matches the tool it was
# ported from -- and they are far too large to check in or ship to a runner.
# A green GitHub job could therefore only ever mean "the tests that do not
# need evidence passed", which is the weaker half. So the gate is local, and
# it runs the capture-gated tests by default.
#
# Usage:
#   scripts/check.sh            # everything, including capture-gated tests
#   scripts/check.sh --no-captures   # skip those (a checkout without evidence)
#   scripts/check.sh --fast     # fmt + clippy + allow check, no tests
set -uo pipefail

cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

captures=1
tests=1
for arg in "$@"; do
  case "$arg" in
    --no-captures) captures=0 ;;
    --fast) tests=0 ;;
    -h|--help) sed -n '2,16p' "$0"; exit 0 ;;
    *) echo "unknown option: $arg" >&2; exit 2 ;;
  esac
done

if [ "$captures" -eq 1 ] && [ ! -d "test captures" ]; then
  echo "note: 'test captures/' is absent -- running as --no-captures."
  echo "      The capture-gated assertions will SKIP, so a green run here is"
  echo "      weaker than a green run in the development copy."
  captures=0
fi

failed=()
step() {
  local name="$1"; shift
  echo
  echo "=== $name"
  if "$@"; then
    echo "--- $name: ok"
  else
    echo "--- $name: FAILED"
    failed+=("$name")
  fi
}

step "cargo fmt" cargo fmt --all -- --check
step "cargo clippy" cargo clippy --workspace --all-targets -- -D warnings
step "allow justifications" scripts/check-allow-justifications.sh

if [ "$tests" -eq 1 ]; then
  if [ "$captures" -eq 1 ]; then
    step "cargo test --workspace (with captures)" cargo test --workspace
  else
    step "cargo test --workspace (captures skipped)" \
      env TRIAGE_ALLOW_COMPAT_SKIP=1 cargo test --workspace
  fi
fi

echo
if [ ${#failed[@]} -eq 0 ]; then
  if [ "$tests" -eq 1 ] && [ "$captures" -eq 1 ]; then
    echo "All checks passed, capture-gated assertions included."
  else
    echo "All checks passed (reduced run: $([ "$tests" -eq 0 ] && echo 'no tests' || echo 'captures skipped'))."
  fi
  exit 0
fi

echo "FAILED: ${failed[*]}"
echo "Read the output above before drawing any conclusion about the cause;"
echo "do not gate anything on a pipeline whose exit status comes from grep."
exit 1
