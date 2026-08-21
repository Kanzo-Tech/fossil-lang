//! Byte offsets into a source text.
//!
//! # Why the leaf crate, and not `fossil-base`
//!
//! It lived in `fossil_base::diagnostic` beside [`crate::Shape`]'s consumers,
//! and `fossil-base` carries salsa. `fossil-shex` is deliberately WASM-clean
//! and depends on this crate and not on that one, so the moment a shape
//! document had to say WHERE it declares a predicate — the label
//! `shop:Order declares shop:total as xsd:float`, underlining a line of
//! `shape.shex` — the decoder needed a span type it could not reach.
//!
//! The alternatives were a `(u32, u32)` on the shex side with a conversion at
//! every use, which is what `HirSourcePipe::span` was until it was collapsed,
//! or salsa in a WASM-clean crate. This is the same move F1 made for
//! [`crate::Primitive`]: the shared vocabulary belongs in the leaf, and
//! `fossil-base` re-exports it so no consumer's import changes.

use serde::{Deserialize, Serialize};

/// Byte-offset span into the source text.
///
/// Carries `(start, end)` only. **Which text** is not in here: a span is
/// always relative to one, and what says which is the frame it travels with —
/// `fossil_base::SpanFrame` for a span in the program, and the document field
/// on `fossil_base::SpanLabel` for a span in a shape document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}

impl Span {
    #[must_use]
    pub const fn new(start: u32, end: u32) -> Self {
        Self { start, end }
    }

    /// The text this span selects, or `None` when it does not fall on a
    /// character boundary of `src`.
    ///
    /// Every producer of a span in this workspace reads it off a token, so the
    /// `None` is defensive rather than expected — but a span and a text are
    /// separate values and nothing makes a caller pair them correctly, so
    /// slicing has to be able to fail rather than panic in a compiler.
    #[must_use]
    pub fn slice(self, src: &str) -> Option<&str> {
        src.get(self.start as usize..self.end as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_span_off_a_character_boundary_declines_rather_than_panics() {
        // `é` is two bytes. A span landing between them is not a slice.
        let src = "café";
        assert_eq!(Span::new(0, 3).slice(src), Some("caf"));
        assert_eq!(Span::new(0, 4).slice(src), None);
        assert_eq!(Span::new(0, 5).slice(src), Some("café"));
        assert_eq!(Span::new(0, 99).slice(src), None);
    }
}
