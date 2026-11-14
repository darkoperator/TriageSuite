You are reviewing one task's implementation in the OxideLens workspace (`oxidelens/`, a separate Cargo workspace
inside this repository; the front end is `oxidelens/app/`). Review only: do not edit files, do not change git state,
and do not run cargo or npm. Read whatever repository files you need.

First whether it matches its requirements, then whether it is well built. This is a task-scoped gate; a whole-branch
review happens after all tasks.

## What was requested
- The task brief: `{{BRIEF}}`
- Constraints binding every task: `{{CONSTRAINTS}}`
- The review standards for this workspace: `{{STANDARDS}}`. Apply every rule in it to the diff.
{{EXTRA}}

## What the implementer claims
`{{REPORT}}`: treat it as unverified claims, including its design rationales. It carries the test evidence (RED/GREEN,
full suite, clippy, fmt, lint); do not re-run tests to confirm it. If the evidence looks truncated or missing, say so as
a gap.

## The diff
`{{PACKAGE}}`: commit list, stat, and the full diff with context for {{BASE}}..{{HEAD}}. Read it once; it is your view
of the change. Inspect code outside the diff only to check a concrete risk you can name (a changed contract's call
sites, for example), and name the risk and what you checked.

## Part 1: spec compliance
- Missing: requirements skipped, or claimed without being implemented. Check the brief's tests one by one: each named
  test exists and asserts what the brief says.
- Extra: unrequested features or over-building.
- Misunderstood: the right feature built the wrong way.
- Cannot verify from the diff: list as a separate item, with what the controller should check.

## Part 2: quality
Correctness and edge cases; error handling (no swallowed errors); tests that verify real behaviour and would fail if the
change were reverted; no verbatim duplication of logic; files with one clear responsibility; the constraints file; the
review standards.

Severity: Critical (wrong results, data loss, security); Important (this task cannot be trusted until fixed: incorrect or
fragile behaviour, a missed requirement, a test that asserts nothing, swallowed errors, duplicated logic, a breach of
the review standards); Minor (polish, broader coverage). If the brief mandates something this rubric calls a defect,
report it as Important labelled plan-mandated.

## Output
First line: `APPROVED` or `REVISE` (`REVISE` if any spec item is missing or any Critical/Important finding).
Then:
### Spec compliance: ✅ or ❌ with file:line; ⚠️ cannot verify from the diff
### Strengths: brief, specific
### Findings: each with id (F1, F2…), severity, file:line, evidence, suggested fix
### Inspected / could not check

Zero findings is a valid answer.
