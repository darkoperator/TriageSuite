# OxideLens review standards

What a reviewer checks in an OxideLens diff beyond what the tools already
enforce. `cargo clippy -- -D warnings`, `cargo fmt --check` and
`npm --prefix app run lint` cover the mechanical rules: no HTML sinks, no date
arithmetic in the front end, no mouse-only handlers. These are the judgement
calls. Each one comes from a defect that reached review more than once.

Report a breach as Important unless it says otherwise. Cite the file and line,
and say what input or event sequence breaks it.

## Every change

- **Each new test names the change that would turn it red.** A test that
  passes against a reverted or mutated implementation asserts nothing. Look
  for these:
  - a test that cannot tell two columns apart, because they hold the same
    values;
  - a rollback test that fails before the step it claims to undo;
  - a check that reads only the first element of a list;
  - a "not drawn" test that never holds an answer back.
- **The test proves what its name says.** If the name says "across cases",
  the test opens two different cases, not the same one twice.
- **Evidence is never written.** A file the app or CLI writes is new
  (`create_new` for a `.partial`, renamed into place). Look at what happens
  when the path, or the `.partial` beside it, already exists as a file, a
  symlink or a junction.
- **Untrusted input is bounded before it is parsed.** That covers a file
  read, a JSON document, a directory listing, a list of windows. The bound
  holds for what a later step re-reads: a document saved under a cap must
  still import under that cap.
- **SQL values are bound; identifiers come from the catalog through
  `quote_ident`.** Every `ox` table and column has a `COMMENT`. Host names in
  code, tests and docs are synthetic (`WS01`, `SRV01`, `acme.local`).

## Front end (`app/`)

The patterns are described in [docs/APP.md](docs/APP.md#writing-front-end-code).

- **A late answer is never drawn.** For each `await` on a backend call, check
  three things:
  - the version or key is compared after the `await`, and in the rejection
    path too;
  - ids still in flight are cancelled when the state moves on;
  - `superseded`, `interrupted` and `no_case` are not reported as errors.

  Look for state captured when a component mounts and never re-read: rules,
  layouts, zone offsets, a case's identity. Also look for two pieces of
  state that change together but are swapped in different effects.
- **One write path for settings.** A screen that writes settings outside the
  shell's queue can persist an older object last. A refused write drops only
  its own change.
- **Focus moves with the analyst.** A dialog takes focus when it opens and
  returns it to its opener when it closes. A shortcut is inactive while a
  text field has focus. Arrow keys handled by an inner control do not also
  move the grid's cell. (Lint catches a missing key handler; it does not
  catch focus that goes to `body`.)
- **The front end holds no rules.** It does no trimming, capping or
  validating that the backend already does, no parsing of times or of stored
  text with a regex, and no glob matching. When the screen needs a fact, the
  engine or the contract gains a field. Names the analyst types are checked by
  the backend, whose refusal the screen shows.
- **Untrusted text stays text.** A string from the case is rendered through
  `displaySafe`. The exception is an `<input>`'s `value`, which holds the raw
  text. Nothing from the data becomes a class name or a style value.
