//! The shape-document decoders: a `ShEx` or SHACL document read into the
//! vocabulary a declared type lowers to (`/docs/design/declared`).
//!
//! A document is one more way to write a type. [`shex`] reads `ShExC` and
//! `ShExJ` through `shex_ast`, [`shacl`] reads Turtle through rudof's
//! `ShaclParser`, and both produce [`fossil_graph_schema::OutputShapes`]. The
//! compiler never names either language: `fossil_hir::providers` installs these
//! two as the decoders of `io.shex` and `io.shacl`.

pub mod shacl;
pub mod shex;
