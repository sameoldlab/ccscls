# Roadmap and status

Last updated after the stage 1 core loop and the design discussion about false positives.
Run `cargo test` for the current state; `cargo test -- --ignored` lists unbuilt specs (all red by design).

## 1. Where things stand

**Done and tested** (68 tests pass, 25 specs are `#[ignore]`d on purpose):

- CSS indexer on tree-sitter-css: every class token in a selector, with escapes resolved, enclosing
  rule span, and the at-rule chain (`@media`, `@layer`, ...).
- HTML class scanner (lexical, forgiving): `class` attribute values, cursor context for completion,
  raw-text elements skipped, template syntax treated as opaque.
- Workspace index: name maps for CSS definitions and markup usages, live buffers override disk,
  file watching (dynamic registration), `.gitignore` and `node_modules` respected.
- LSP: completion in `class="..."`, hover, goto definition, references (CSS and HTML, both directions).
- Test tooling: annotated fixtures with known answers, in-process LSP client, stdio smoke test,
  query CLI (`examples/query.rs`), playground, `scripts/dev-lsp` for Kakoune.

**Known gaps in what exists** (none block the roadmap):

- Watcher events for gitignored files are indexed (only `node_modules` is filtered).
- The initial scan blocks the message loop.
- `.svelte` / `.vue` files are not recognised yet; the HTML scanner is not brace-aware.
- The kak-lsp config snippet in the README is from memory and untested.
- A token-trimming fix (`valid_ident_prefix_len` in `src/css.rs`) was found already in the tree and was
  not written in the latest working stretch; it is covered by tests but deserves a read.
- Style: trailing comments, panic sites and single-letter names are cleaned in the stable files and
  enforced by `tests/style.rs`; `server.rs` and `workspace.rs` plus the structural items in STYLE.md are
  pending and belong to stage A.

## 2. The central problem: false positives

Existing class-completion extensions keep every class they have ever seen, so suggestions and "this
class exists" claims are wrong in the large. Location accuracy (import graph, cascade, specificity)
is the hard part of this tool; the LSP plumbing is the easy part.

### Principle: fail open when suggesting, fail closed when asserting

Never hide a class (a wrong graph must not cause a missed completion), but only claim what can be proven.

### Verdicts for a class as seen from a file

| Verdict | Meaning |
|---|---|
| **Verified** | A fully resolved chain of edges connects this file to a rule that defines the class. |
| **Ambient** | Defined somewhere in the workspace, but nothing proves it reaches this file. |
| **Undefined** | No rule anywhere. A typo. The cheap, certain diagnostic. |

Verified must be sound. Ambient may be broad. "Unlinked error" is a severity applied to ambient
(off / hint / warning / error), defaulting low and rising when confidence is high or by config.

### Proven edges vs open worlds

- **Proven:** `<link href>` resolving to a real file; `@import "./x.css"`; inline `<style>` in the same
  file; a Svelte/Vue scoped style used by its own component.
- **Unproven (leave the file open-world):** CDN links, bare package imports, aliased imports (`$lib`),
  any `<script>` that could inject styles, `<base>`, global CSS imported at an app entry point.
- A file is **closed-world** when every mechanism that could add CSS to it has been enumerated and
  resolved. Ambient may only become an error in closed-world files.
- **Declared edges in config** ("`src/app.css` applies to everything under `src/`") count as verified
  and are labeled as declared. This lets framework apps become verified without modelling the bundler.
- Conditional imports (`@import "x.css" print`, `supports()`, `layer()`) are verified-with-condition.

### Explainability

Each verified verdict carries its chain ("index.html links site.css, which imports components.css"),
shown in hover and in the ambient label ("defined in X, no link from here"). Tests re-check chains
independently of the server, which is how soundness is tested.

### Three accuracy axes

1. **Existence**: done (stage 1).
2. **Reachability**: the verdict model above.
3. **Applicability** (does a rule match this element?): `.btn.active` needs both, `.nav .btn` needs an
   ancestor, `:not(.x)` is negation. Static selector roles are cheap and give better labels; real
   cascade and specificity need DOM matching and are a later, partial feature.

## 3. Planned stages

Order changed from the original plan. Mapping to existing spec files is in section 4.

### Stage A: language seam and both-sided file model (behavior-preserving)

Today file-type knowledge leaks into `FileKind`/`detect` (workspace), `upsert`/`target_at` (branching on
kind) and the completion handler (hard-codes HTML).

- One `Language` contract per file type, each in its own module, registered in one list. The core never
  matches on file kind.
  - detect: does this handler own this file (language id / extension)?
  - analyze: text in; definitions, usages, imports, embedded regions out.
  - cursor context: for completion.
  - render name: how to write a class name into this language (CSS escaping incl. hex-escape terminator
    space; Svelte `class:active` shorthand expansion; plain in HTML). Needed by rename.
- Embedded regions (`<style>` in HTML/Svelte/Vue): the handler returns the region; the core runs the CSS
  handler on it and shifts spans. Handlers never call each other.
- A file may hold definitions and usages (today it is one or the other).
- Plain-data boundary (text in, spans out) so a WASM host remains possible later. No dynamic plugins now.
- Shared helpers (HTML tag scanner, JS string extraction) live beside the handlers, not in the core.
- Per-language fixture directories with per-language marker syntax, plus generic contract tests over
  every registered handler (spans in bounds, sorted, name == unescape(raw), prefix sweep).
- Style conformance in the same pass (see STYLE.md), because it rewrites the same code: handles and a
  string pool in place of `Box<str>` and borrowed `DefRef`/`UsageRef`/`Target` views; no lifetime on the
  server struct; crate error type with context chain; per-message panic boundary; enum dispatch, no `dyn`;
  `PENDING_NAME_CLEANUP` in `tests/style.rs` shrinks to empty.
- Exit: all 68 existing tests green with no assertion changes, and the style tests pass with an empty
  pending list.

### Stage B: provenance, graph and verdicts

- Per-file import list (raw specifier plus span) extracted by handlers: `<link>`, `@import`, inline `<style>`.
- Resolution is lazy at query time (join path, look up file ids): nothing to invalidate when files appear
  or disappear. Closure walk with cycle protection; optional memo keyed by a graph version.
  Resolver is behind a trait so JS/alias resolution can slot in later (see libraries).
- Visibility: self plus closure of imports. Verdict computation, chains, closed-world tracking.
- Completion ranks verified first, ambient shown and labeled (never dropped). Hover shows chain or the
  "defined in X, no link from here" explanation, and distinguishes "not visible from here" from "nowhere".
- Diagnostics: undefined = warning; ambient severity configurable (default off or hint).
- Config: ambient severity, declared edges, ignore globs.
- Unresolved or external import => file is open-world => no ambient error, no false claim.
- Tests (replace the current stage 4 specs): scenario workspaces annotated with
  `<!--~ btn:verified, foo:ambient, bar:undefined -->`; cycles, diamonds, missing target later created via
  watcher, unresolved link falls back to open-world; property tests: adding an unrelated CSS file never
  changes existing verdicts, and adding an edge only promotes ambient to verified.

### Stage C: frameworks and JS import edges

- JSX/TSX handler (`className`, `class`, helper calls such as `clsx`/`cn`, template-literal static parts,
  object keys), Svelte (brace-aware tag scanning, `class:name` directive incl. shorthand, `class={...}`
  incl. array/object forms), Vue (`:class` / `v-bind:class`: strings, array items, object keys).
- JS `import "./x.css"` edges via a real resolver (see libraries); tsconfig paths and aliases.
- Svelte/Vue scoped and module styles via the embedded-region mechanism from stage A.
- Specs: `tests/stage3_frameworks.rs` (jsx, svelte, vue) plus the scoped-style specs from stage 4.

### Stage D: rename (global)

- Delayed until after the graph, as decided. Kakoune applies rename edits without a confirmation step, so a
  wrong rename is destructive and worse than a bad completion.
- Rename is global by name: every definition and usage. After stage B it can report skipped or uncertain
  occurrences (ambient ones, dynamic strings); a verified-scoped rename is a filter on the global one.
- Needs `render name` from stage A: CSS escaping, hex-escape terminator space, Svelte shorthand expansion.
- Specs: `tests/stage2_rename.rs` (compatible with global semantics, can stay as written).

### Stage E: applicability, cascade, colour hints

- Selector roles: compound/descendant/negated, shown in hover and used to refine verdict labels.
- "Why doesn't this apply" (specificity and order) for static HTML only; partial for templates.
- Colour hints: see section 5.

### Stage F: polish

Workspace symbols, config file, performance (async scan with a readiness signal), watcher/gitignore gap.

## 4. Mapping to existing spec files

| File | Becomes | Change needed |
|---|---|---|
| `tests/stage1_core.rs` | stays (core loop) | none; must stay green through stage A |
| `tests/stage2_rename.rs` | stage D | none for global rename |
| `tests/stage3_frameworks.rs` | stage C | none |
| `tests/stage4_scoping.rs` | stage B + C | rewrite: assert verified-first ranking and labeled ambient, not exclusion |
| `tests/stage5_polish.rs` | stages B and F | diagnostics: undefined warning + configurable ambient; keep symbols/config |

## 5. Colour previews (research notes)

Two separate LSP mechanisms:

- **Completion swatches** are client-side: clients detect a colour from the item kind `Color` or from
  colour text at the start/end of `documentation`. The server sends nothing extra.
- **`textDocument/documentColor` + `colorPresentation`** decorate colour references in a document and
  drive a picker.
- **kakoune-lsp** (source checked at commit `57d6436`, 2026-10-10): advertises a colour capability and lists
  `Color` as a supported completion kind, but has no `documentColor` handler and draws no swatches. It shows
  completion `detail`/`documentation` in an info box and supports `completionItem/resolve`. It does have
  inlay hints and semantic tokens (neither carries arbitrary RGB). Options for Kakoune: hex text in
  `detail`/`documentation` (works now, lazily via resolve), or a custom Kakoune-side script using `rgb:`
  faces (idea, unverified).
- Implications: the CSS handler must index declarations (property, value span), not only selectors;
  `var(--x)` needs the graph to resolve; modern colour syntax needs a parser; a class with several colour
  declarations needs a policy (swatch only when unambiguous). Extraction lives in the CSS handler's
  output; display is language-neutral core.
- Prior art worth reading: `omena-lsp-server` (Rust CSS-modules LSP implementing documentColor for
  variable references; quality unknown).

## 6. Libraries and bundler pieces evaluated

| Library | What it is | Verdict |
|---|---|---|
| **oxc_resolver** (github.com/oxc-project/oxc_resolver) | Rust port of webpack's enhanced-resolve; used by Rolldown and Rspack. tsconfig `paths`, `extends`, `references`; `alias`, `fallback`, `roots`; package.json `exports` with condition names; extensions. | **Use in stage C** for JS/TS `import "./x.css"` and aliases. Best fit and most maintained. |
| **parcel-resolver** (lib.rs/crates/parcel-resolver) | Parcel's Rust resolver: Node CJS/ESM, tsconfig paths, `alias`/`browser` fields, tilde paths, feature flags. Returns *invalidations* (files whose change invalidates a resolution). | **Maybe.** Its invalidation list is exactly what an incremental graph wants, and tilde paths matter for CSS. Smaller (0.1.x). Try oxc_resolver first, compare. |
| **lightningcss bundler** (docs.rs/lightningcss, bundler module) | Bundles a CSS file and its `@import` closure; pluggable `SourceProvider` with `resolve(specifier, originating_file)`; wraps imported rules in `@media`/`@supports`/`@layer` to preserve behaviour; resolver can mark imports external. | **Do not use for indexing** (it does not keep per-selector spans; see earlier finding). **Copy two ideas:** the resolver-trait shape, and treating `@import` conditions as verified-with-condition. |
| **tailwindcss-oxide** (Tailwind v4 scanner) | State-machine extractor of utility *candidates* from any file (Vue, Svelte, ERB, ... pre-processors), `.gitignore`-aware parallel scan, incremental. | **Skip as a dependency.** It finds candidate strings, not class *positions* or attribute context, and is tuned to Tailwind syntax. Idea worth keeping: a cheap text scan for "possible dynamic usages" to warn during rename. Not verified that it is published as a standalone crate. |
| **lightningcss (values)** | Full CSS value parser. | **Candidate for stage E colour parsing** (parse a declaration value to RGBA). Check wide-gamut coverage then. |
| **tree-sitter-css / html / svelte / vue grammars** | Used today for CSS. Svelte/Vue grammar crates were stale in the first survey. | Keep CSS. For Svelte/Vue prefer the hand scanner with a dialect switch (a stock HTML parser would not fix Svelte braces either). |
| **oxc_parser** | Rust JS/TS/JSX parser with error recovery. | **Likely for stage C** (JSX and `:class` expressions); evaluate against tree-sitter-tsx on half-typed code first. |

Not covered by anything found: bundler-level reachability (which CSS reaches which component). That is
why framework components default to "global CSS plus own scoped styles" with declared edges as the escape
hatch, not a modelled bundle graph. Vite's own CSS `@import` resolution is JS (postcss-import based, from
memory) and not extractable.

## 7. Open decisions

1. How strict visibility is for a framework component with no explicit link to any CSS. Recommendation:
   open-world (sees all global CSS) until a declared edge or closed-world signal says otherwise.
2. Default ambient severity. Recommendation: off for open-world files, hint for closed-world ones.
3. Whether the swatch policy picks `background-color` > `color`, or shows none for multi-colour rules.
4. Whether to ship the language contract as a Rust trait object list or an enum dispatch (recommend the
   enum/registry list first; it is simpler to test and nothing needs dynamic loading).
