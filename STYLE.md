# Code style

Rules set by the project owner, how they are applied in this Rust codebase, and what is
enforced by `cargo test` versus reviewed by hand. Stated with a grain of salt: they were given before
the code had been read, so expect them to be refined.

## The rules

1. **Dependencies:** keep them minimal. The current set is fine, and the count can be ignored for now
   to move faster.
2. **Allocation (Rust):**
   - Not a fan of RAII, but don't go crazy.
   - Dislikes dealing with lifetimes.
   - Don't default to pointer indirection, vtables or boxed types.
   - Generally prefer an arena to individual allocations.
   - For graph-like data, if that does not solve it, use intrusive arrays and "handles are the
     better pointers".
3. **Errors:** take the good ideas from `anyhow` in regular code without adding the dependency. An LSP
   server must never crash in normal use, so avoid `unwrap`.
4. **Names:** no inscrutable single-letter variable names.
5. **Comments:** never at the end of a line of code. Put them on the line above.

## How these are applied here

**Allocation and lifetimes**

- Refer to things by handle, not by reference: `FileId`, `NameId` and friends are small `Copy` values
  resolved through the owning structure. Handles carry a generation so a stale one is detected, not
  followed. This removes lifetimes from public types and from the server struct.
- Strings (class names, at-rule chains, import specifiers) live in one pool owned by the workspace and
  are referred to by `(start, length)` or an interned id. No `Box<str>` or `Vec<Box<str>>` per item.
- Graph edges are flat arrays linked by index (an intrusive list: each edge stores the index of the
  next edge from the same file; each file stores the index of its first edge). Rebuilding a file's
  edges on re-index is a splice, with a free list for reuse.
- No `dyn`. Language dispatch is an enum with a `match` in one module (the language seam), not trait objects.
- No `Drop`-driven designs. Rely on std types and let them drop.

**Errors**

- One crate error type with a context chain (outermost message first), a `Result` alias, and a
  `context(...)` extension on `Result` and `Option`, as `anyhow` does. Written in the crate, not imported.
- Request handlers return `Result`; the message loop turns an error into an LSP error response and
  carries on. The loop exits only on shutdown or a closed transport.
- Last-resort panic boundary around each message (`catch_unwind`), answering with an internal-error
  response and logging, so one bad input cannot take the editor integration down.
- Startup problems (for example the CSS grammar failing to load) are reported on stderr and the server
  keeps running in a degraded state instead of panicking.
- Indexing that can go out of bounds uses `get(..)`, never direct slicing on externally supplied offsets.

**Names and comments**

- Descriptive names. The one allowed single letter is `i` for a plain index loop. Standard
  abbreviations like `uri` are fine; `ws` for workspace is borderline, prefer `workspace`.
- Comments go on their own line above the code they describe. Annotation markers inside fixture files
  (`/*~ a, b */`, `<!--~ a -->`) are test data in non-Rust files and are exempt.

## What `cargo test` enforces (`tests/style.rs`)

| Rule | Scope | Enforced |
|---|---|---|
| No trailing comments | all Rust files | yes |
| No `unwrap` / `expect` / `panic!` family | `src/`, outside inline test modules | yes |
| No single-letter bindings (`let`, `for`, closure and `fn` parameters) | `src/`, outside inline test modules | yes, with a ratchet list |
| Handles, arenas, no `dyn`/`Box`, error type, panic boundary | everywhere | reviewed by hand |

The ratchet list `PENDING_NAME_CLEANUP` names files still to be converted. A test fails if a listed file
is already clean, so the list can only shrink. The style tests are themselves tested: a probe file with one
violation of each rule fails all three checks.

## Exemptions

- `tests/` may use `unwrap`/`expect` (a panic there is a test failure, which is the point).
- `examples/` (developer CLIs) may `expect` on bad command-line usage.
- Existing tests contain many single-letter names (mostly `c` for the client and `u` for a URI); the
  rule applies to new tests and they will be converted when touched.

## Current conformance

Done (stage A):

- Trailing comments: none anywhere in Rust code.
- Panic sites in library code: none. A panic boundary around each message turns an unexpected panic into an
  internal-error response; startup problems (a grammar that fails to load) are reported on stderr.
- Names: no single-letter bindings in `src/`; the pending list in `tests/style.rs` is empty.
- Lifetimes: none on public types or on the server. Long-lived values are handles (`FileId`,
  `OccurrenceId`, `NameId`) resolved through `Workspace`; borrowed views exist only inside a function.
- Allocation: class names, at-rule preludes and other strings live in one pool (`Strings`) and are
  referred to by `NameId`; at-rule chains are stored once per run of identical neighbours; per-name
  occurrence lists are linked through the occurrences themselves instead of per-name vectors.
- No `dyn`, no `Box` per item. Language dispatch is an enum with `match` in `languages/mod.rs`.
- Errors: crate `Error` with a context chain and a `Context` extension, as `anyhow` does, no dependency.
- Bounds: externally supplied offsets are read with `get(..)`.

Remaining:

- Test code has single-letter names (mostly `c` for the client and `u` for a URI); the rule applies to new
  tests and they will be converted when touched.
- The string pool never reclaims names; compaction would renumber every id.
- Not machine-checked: handles-not-references and no-`Box` (reviewed by hand).
- Stage A was last compiled and unit-tested before the integration tests were adapted; see ROADMAP.md.
