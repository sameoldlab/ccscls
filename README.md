# css-class-lsp

A language server that indexes CSS class names and understands where classes are used in markup,
like Tailwind's server but for plain CSS. Rust, `lsp-server` (sync message loop), tree-sitter-css.

## Status

The stage 1 core loop (HTML + CSS folder: index, completion, hover, definition, references, live
buffers, watching, ignores) is done and tested. See **[ROADMAP.md](ROADMAP.md)** for the full status,
the verified/ambient/undefined model that drives the design, the planned stages (seam refactor, graph and
verdicts, frameworks, rename, cascade/colour) and the libraries evaluated.

Unbuilt work is specified as ignored tests; `cargo test -- --ignored` lists everything outstanding
(all red until built). Spec files `tests/stage2..5_*.rs` map to roadmap stages in ROADMAP.md section 4.

## Interactive feedback loop

1. **Ask the real server from the shell** (same message loop the editor uses, in-process):

       cargo run -q --example query -- playground classes
       cargo run -q --example query -- playground playground/index.html 10:30 hover
       cargo run -q --example query -- playground playground/about.html 3:15 definition
       cargo run -q --example query -- playground playground/css/components.css 1:3 references
       cargo run -q --example query -- playground playground/index.html 12:30 completion

   (`line:col` are 1-based.) `playground/` is a small site with a deliberate typo class to hover.
2. **Use it in your editor.** `scripts/dev-lsp` rebuilds then runs the debug binary, logging to
   `/tmp/css-class-lsp.log` (`tail -f` it in a split). Kakoune + kak-lsp, in `kak-lsp.toml`
   (format from memory, check against your kak-lsp version):

       [language_server.css-class-lsp]
       filetypes = ["html", "css"]
       roots = [".git"]
       command = "/absolute/path/to/css-class-lsp/scripts/dev-lsp"

   Restart the server (`lsp-stop; lsp-start`) after code changes; open `playground/` as the project.
3. **Re-run tests on save:** `cargo watch -x test` (or `bacon test`), if you have them installed.
4. `CSS_CLASS_LSP_LOG=1` makes the server log every message and its handling time to stderr.

## Testing strategy

Each level uses the one below it as a trusted base.

* **Unit tests** (`src/`): line index, UTF-16, URI round-trips, escapes, workspace bookkeeping.
* **Level 1, fixtures with known answers** (no LSP): `tests/fixtures/css/*.css` and `html/*.html`
  carry their expected output inline, so input and answer sit side by side:

      .a, .b { }           /*~ a, b */
      .sm\:flex { }        /*~ sm\:flex=>sm:flex */          raw token => unescaped name
      .x { }               /*~ x */ /*ctx: @layer base | @media (min-width: 1px) */
      <div class="a b">    <!--~ a, b -->

  Every class on an unmarked line must NOT be found, so false positives fail too. Expectations
  are written from CSS/HTML semantics, never copied from output. `$0` marks a cursor in inline
  tests (`tests/html_context.rs`). `tests/fixture_format.rs` tests this tooling itself.
  `tests/css_robustness.rs` is a cheap typing simulation (every prefix/suffix of every fixture);
  it is deliberately not adversarial.
* **Level 2, through the LSP machinery** (`tests/stage*.rs`): an in-process client
  (`tests/common`, over `Connection::memory()`) drives the real server loop with real messages
  against `tests/fixtures/site` (copied to a temp dir). Expected locations are computed from the
  fixture annotations, independently of the server.
* **Scenarios** (`tests/scenarios.rs`): each directory under `tests/fixtures/scenarios` is a tiny project.
  Every class usage in its HTML carries `<!--v: btn=verified card=ambient ghost=undefined -->` and each
  file says `<!--world: open -->` or `closed`. Verified means a chain of links/imports reaches a definition,
  ambient means it is defined somewhere unlinked, undefined means nowhere.
* **Level 3, process smoke** (`tests/stdio_smoke.rs`): spawns the binary, Content-Length framing,
  clean shutdown. One test.

## Style

Rules and their enforcement are in **[STYLE.md](STYLE.md)**; `tests/style.rs` checks the mechanical ones
(comments above code, no panics in library code, no single-letter names) as part of `cargo test`.

## Layout

    src/languages/    the seam: one module per file type behind the `Language` enum
      mod.rs            detect, analyze, completion context, render name, watch globs
      css.rs            selectors -> definitions (tree-sitter-css), CSS escaping
      html.rs           class attributes -> usages (lexical scanner), cursor context
      svelte.rs         class attributes, class: directives and class={...} expressions, <style> blocks
    src/analysis.rs   language-neutral results: Occurrence, Role, Scope, Import, Span, FileAnalysis
    src/config.rs     settings from initializationOptions (diagnostic severities)
    src/workspace.rs  files, generational handles, per-name occurrence lists
      workspace/visibility.rs   import graph, reach, verified/ambient/undefined verdicts
    src/strings.rs    interned string pool (NameId)
    src/server.rs     LSP loop and handlers, panic boundary
    src/error.rs      Error with a context chain
    src/line_index.rs, src/uri.rs   coordinates and paths
    examples/query.rs, examples/dump.rs   dev tools (dump = print the tree-sitter tree)

## Decisions and gotchas worth knowing

* tree-sitter-css rejects non-Latin identifiers (`.привет`, `.日本語`, `.😀`) although the CSS
  spec allows them; non-ASCII bytes are masked to `_` before parsing (offsets unchanged).
* Every class token in a selector is indexed (including `:not(.x)`), because rename must touch all.
* A space after a hex escape (`.x\1F600 {}`) belongs to the name token; `render_name` re-adds it on rename.
* `initialize_finish` swallows the `initialized` notification, so the initial scan runs right
  after the handshake (a stage 1 test caught this).
* The initial scan blocks the message loop; requests queue until it finishes.
* Watcher events for gitignored files are indexed unless under `node_modules` (known gap).
* HTML values containing `{{`, `{%`, `<%`, `<?` are opaque (no usages, no completion).
