//! A language server that indexes CSS class names.
//!
//! Layering (each layer is testable without the ones above it):
//!
//! * [`error`], [`config`], [`strings`], [`line_index`], [`uri`]: small foundations.
//! * [`analysis`]: language-neutral results (occurrences of class names).
//! * [`languages`]: the seam. One module per file type, behind one enum.
//! * [`workspace`]: the set of indexed files and a name -> occurrences index.
//! * [`server`]: the LSP message loop and request handlers.

pub mod analysis;
pub mod config;
pub mod error;
pub mod languages;
pub mod line_index;
pub mod server;
pub mod strings;
pub mod uri;
pub mod workspace;
