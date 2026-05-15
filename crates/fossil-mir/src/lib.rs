//! Phase 0 stub — replaced incrementally starting in Phase 1.
//!
//! See `.planning/ROADMAP.md` for the walking-skeleton plan.

#![allow(unused)]

/// Phase 0 placeholder. Returns the crate name so the stub is non-empty
/// and the linker actually emits a symbol on every target.
#[must_use]
pub fn phase_zero_marker() -> &'static str {
    env!("CARGO_PKG_NAME")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_returns_crate_name() {
        assert_eq!(phase_zero_marker(), env!("CARGO_PKG_NAME"));
    }
}
