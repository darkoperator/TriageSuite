# Agent and contributor notes

## Running the tests

Two environment variables control test behaviour. Neither affects an ordinary
run of any tool.

- **`TRIAGE_ALLOW_COMPAT_SKIP=1`** — lets capture-gated tests skip instead of
  panicking when the evidence tree they need is absent.
  `triage_testkit::skip_if_missing` panics by default *on purpose*, so that a
  green run always means the gated assertions really executed rather than
  quietly doing nothing. The evidence captures and the Zimmerman-comparison
  fixtures are not part of this repository, so a fresh clone needs this
  variable or the suite fails on missing fixtures rather than on a real defect.
- **`TRIAGE_RUN_STAMP`** — pins the `yyyyMMddHHmmss` run stamp that
  `triage_core::output::router::run_stamp` puts into default output filenames.
  Set it in any test where two runs must land on the *same* filename, such as
  one asserting the `--overwrite` guard. Without it, those runs only collide if
  they start inside the same wall-clock second, which is a race that loses
  under a loaded `cargo test --workspace`. Values are restricted to a short
  alphanumeric token; anything else is ignored and the clock is used, because
  the stamp becomes part of a path.

A full check:

```
TRIAGE_ALLOW_COMPAT_SKIP=1 cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

Clippy is expected to pass with no warnings.

## Before changing a parser

Each tool's contract lives in `docs/tools/<Tool>.md`; read that page before
changing how its artifact is read. Two conventions are worth stating up front
because breaking either is silent:

- **Evidence is never modified.** SQLite-backed tools go through
  `triage-sqlite`, which opens the original `immutable=1` when there is no
  write-ahead log, and otherwise copies the `{db,-wal,-shm}` set to a temporary
  directory and checkpoints the copy. A plain read-write open would checkpoint
  an attached log on close and rewrite the original.
- **Enum decode tables are transcribed from upstream sources and pinned by a
  test that cites the source.** A test that asserts a table against the literal
  written directly above it passes for whatever values the table happens to
  contain, right or wrong, which is how four tables in one crate shipped
  misnamed with a green suite.
