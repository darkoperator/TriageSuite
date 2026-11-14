You are doing the final whole-branch review of an OxideLens build step in this repository. Every task already passed
its own task-scoped review; your job is what those gates cannot see: cross-task integration, the plan and spec as a
whole, and triage of deferred findings. Review only: do not edit files, do not change git state (no checkout, no stash,
no reset), and do not run npm or cargo. Do all of the review yourself; if the diff is too large for one pass, review it
in passes and say so.

## Requirements
- Constraints binding every task: `{{CONSTRAINTS}}`
- The review standards for this workspace: `{{STANDARDS}}`. Apply every rule in it across the whole diff.
{{EXTRA}}

## The diff
`{{PACKAGE}}`: commit list, stat and full diff for {{BASE}}..{{HEAD}}. Use the stat to plan passes and read the parts you
need; you may also read files in the repository directly.

## Focus (what per-task gates miss)
- Contract integrity end to end: every Rust command in `crates/oxl-app`, the generated TS
  (`app/src/contract/generated`), `api.ts`, the mock backend and the recorded fixtures; field names and optionality
  consistent; `COMMANDS` order.
- The constraints across the whole tree: evidence and run output never written; no network in the app; the front end
  holds no rules; untrusted text through `displaySafe`; one writer connection; query slots and cancellation consistent
  across features; no real host, user or path names in committed files.
- State that two tasks both touch (settings, tabs, saved views, export, cached pages) and the late-answer rules of the
  review standards applied across them.
- Error paths a reasonable analyst meets: a locked or deleted case, damaged settings, huge and wide relations.
- Docs (`docs/APP.md`, `README.md`, `CODING_STANDARDS.md`) accurate against the code.

For behaviour the plan and spec are silent on, judge by what a reasonable analyst would expect, and grade by the effect
on that person.

## Output
First line: `READY`, `WITH FIXES` or `NOT READY`.
### Strengths
### Issues: Critical / Important / Minor; each with file:line, what is wrong, why it matters, how to fix
### Deferred and parked triage: for each deferred or parked finding listed in the notes, FIX BEFORE MERGE or CAN STAY, with one line why
### Declined to judge: each behaviour you set aside as outside the plan or spec, with one line why
