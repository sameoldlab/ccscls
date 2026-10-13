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

Done:

- Trailing comments: none left anywhere (40 moved).
- Panic sites in library code: none (`css.rs`: `expect`s removed, grammar load failure degrades).
- Single-letter names: clean in `css.rs`, `line_index.rs`, `markup.rs`, `uri.rs`.
- Slicing on externally supplied offsets replaced by `get(..)` in those files.

Pending, folded into roadmap stage A (they restructure the same code):

- `server.rs` and `workspace.rs`: single-letter names, `Server<'c>` and `DefRef<'a>`/`UsageRef<'a>`/`Target<'a>`
  lifetimes, `Box<dyn Error>` in `run`'s signature.
- `ClassDef` and `Usage` own `Box<str>` / `Vec<Box<str>>` per item: move to the shared pool with ids.
- Crate error type and the panic boundary.
- Per-file name-map entries `(file slot, index)` become generational handles.
- Test-code names.
