//! Salsa accumulator for diagnostics.
//!
//! Any `#[salsa::tracked]` query can call `Diagnostic { ... }.accumulate(db)`
//! and the host (CLI / LSP / WASM) collects them via
//! `query::accumulated::<Diagnostic>(db, input)`.

#[salsa::accumulator]
#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub severity: Severity,
    pub message: String,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

/// Byte-offset span into the source text.
///
/// Phase 1 carries `(start, end)` only. Phase 2 may extend with file id
/// once cross-file spans are needed, via a `FileSpan` newtype layered above.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}

impl Span {
    #[must_use]
    pub const fn new(start: u32, end: u32) -> Self {
        Self { start, end }
    }
}
