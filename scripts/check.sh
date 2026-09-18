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
#   scripts/check.sh --no-duckdb     # skip the DuckDB view assertions
#   scripts/check.sh --fast     # fmt + clippy + allow check, no tests
set -uo pipefail

cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

captures=1
tests=1
duckdb=1
for arg in "$@"; do
  case "$arg" in
    --no-captures) captures=0 ;;
    --no-duckdb) duckdb=0 ;;
    --fast) tests=0 ;;
    -h|--help) sed -n '2,17p' "$0"; exit 0 ;;
    *) echo "unknown option: $arg" >&2; exit 2 ;;
  esac
done

if [ "$captures" -eq 1 ] && [ ! -d "test captures" ]; then
  echo "note: 'test captures/' is absent -- running as --no-captures."
  echo "      The capture-gated assertions will SKIP, so a green run here is"
  echo "      weaker than a green run in the development copy."
  captures=0
fi

if [ "$duckdb" -eq 1 ] && ! command -v duckdb >/dev/null 2>&1; then
  echo "note: duckdb is not on PATH -- running as --no-duckdb."
  echo "      The generated DuckDB views will NOT be proven to load."
  duckdb=0
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
  test_env=()
  test_label="cargo test --workspace"
  if [ "$captures" -eq 1 ]; then
    test_label="$test_label (with captures)"
  else
    test_env+=(TRIAGE_ALLOW_COMPAT_SKIP=1)
    test_label="$test_label (captures skipped)"
  fi
  if [ "$duckdb" -eq 1 ]; then
    test_env+=(TRIAGE_REQUIRE_DUCKDB=1)
  fi
  # `${test_env[@]+"${test_env[@]}"}` and not the obvious `"${test_env[@]}"`:
  # bash 3.2 -- which is macOS's /bin/bash, and what this script runs under on
  # a stock Mac -- treats an EMPTY array's `"${arr[@]}"` as an unset variable
  # and aborts under `set -u`. The array really is empty in one supported
  # case: captures present and duckdb skipped, i.e. `--no-duckdb`. The `+`
  # form expands to nothing at all when the array is unset or empty, and
  # expands each element quoted otherwise.
  step "$test_label" env ${test_env[@]+"${test_env[@]}"} cargo test --workspace
fi

echo
if [ ${#failed[@]} -eq 0 ]; then
  if [ "$tests" -eq 1 ] && [ "$captures" -eq 1 ] && [ "$duckdb" -eq 1 ]; then
    echo "All checks passed, capture-gated and DuckDB view assertions included."
  elif [ "$tests" -eq 0 ]; then
    echo "All checks passed (reduced run: no tests)."
  else
    reasons=()
    [ "$captures" -eq 0 ] && reasons+=("captures skipped")
    [ "$duckdb" -eq 0 ] && reasons+=("DuckDB skipped")
    reasons_joined="$(printf '%s, ' "${reasons[@]}")"
    reasons_joined="${reasons_joined%, }"
    echo "All checks passed (reduced run: ${reasons_joined})."
  fi
  exit 0
fi

echo "FAILED: ${failed[*]}"
echo "Read the output above before drawing any conclusion about the cause;"
echo "do not gate anything on a pipeline whose exit status comes from grep."
exit 1
