You are re-reviewing a fix round for one task in the OxideLens workspace (`oxidelens/`). Review only: do not edit
files, do not change git state, and do not run cargo or npm.

The earlier review of this task found the findings in `{{FINDINGS}}`. The implementer has fixed them; its fix report is
at the end of `{{REPORT}}` (it carries the tests run and their output; do not re-run them). The task brief is
`{{BRIEF}}`, the constraints are `{{CONSTRAINTS}}`, and the review standards are `{{STANDARDS}}`.
{{EXTRA}}

The fix diff alone ({{BASE}}..{{HEAD}}) is in `{{PACKAGE}}`. Read it once.

For each finding: ADDRESSED or NOT ADDRESSED, with file:line evidence. A finding answered by a ruling in the extra notes
above is ADDRESSED if the code matches the ruling. Then flag any new breakage the fix diff itself introduces
(Critical/Important/Minor), including a breach of the review standards. Do not review code the fix did not touch;
observations outside the fix go under "Out of scope" and do not change the verdict.

## Output
First line: `APPROVED` (every finding addressed, no new Critical/Important breakage) or `REVISE`.
### Findings: id, ADDRESSED or NOT ADDRESSED, evidence
### New breakage in the fix diff
### Out of scope
