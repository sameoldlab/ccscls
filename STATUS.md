# css-class-lsp: status

Run: `cargo test` (Rust 1.91 used; edition 2024). Debug the grammar: `cargo run --example dump file.css`.

## Done and passing
- `src/`: css indexer (tree-sitter-css), line index (UTF-16), uri helpers, workspace index,
  LSP server (initialize, didOpen/Change/Close, didChangeWatchedFiles, watcher registration,
  hover / definition / references on CSS-side classes).
- Unit tests in src (18 pass).
- Level 1: `tests/css_fixtures.rs` + `tests/fixtures/css/*.css` (10 fixtures, expectations inline
  as `/*~ ... */`), all pass. Mutation-checked: disabling non-ASCII masking and removing the
  class_selector parent check both make it fail.
- `tests/fixture_format.rs` tests the annotation tooling itself (not yet run after last edit).
- `tests/css_robustness.rs::every_prefix_and_suffix_of_every_fixture` passes.

## Known problems (found by the tests)
1. `random_mutations_of_fixtures` FAILS: error recovery in tree-sitter-css can put delimiters
   inside a class_name (e.g. `.w;-1\/2` yields token `w;-1\/2`). Fix: in `css::make_def`, cut the
   token at the first unescaped delimiter (or drop it).
2. `hostile_inputs` does not finish (>60s). One of: 5000 `{`, 2000-deep `.a{`, 20k selectors,
   200k-char identifier. Bisect which; likely tree-sitter error recovery blowing up. Needs a
   size/time guard (e.g. parser timeout via `set_timeout_micros`, or skip indexing huge files).
3. Hex-escape terminator space is part of the name span (documented in a unit test); rename
   must re-insert a space.

## Not written yet
- Level 2: `tests/lsp_flow.rs` (in-process client over `Connection::memory()`; the `TestClient`
  harness in `tests/common/mod.rs` is written but unused/untested), `tests/fixtures/workspace/`.
- Level 3: `tests/stdio_smoke.rs` (spawn the binary, Content-Length framing).
- Markup side (html/jsx/svelte/vue), completion, rename.
