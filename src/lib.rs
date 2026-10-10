//! A language server that indexes CSS class names.
//!
//! Layering (each layer is testable without the ones above it):
//!
//! * [`line_index`] / [`uri`]: pure coordinate and path helpers.
//! * [`css`]: source text -> `Vec<ClassDef>`. No I/O, no LSP types.
//! * [`workspace`]: the set of indexed files and a name -> definitions map.
//! * [`server`]: the LSP message loop and request handlers.

pub mod css;
pub mod line_index;
pub mod server;
pub mod uri;
pub mod workspace;
