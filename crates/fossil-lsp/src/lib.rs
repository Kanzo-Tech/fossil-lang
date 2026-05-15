//! Phase 0 stub — native-only crate. WASM build is intentionally NOT gated
//! in CI for this crate; see `decisions/0002-fifteen-crate-workspace-layout.md`
//! (written in Plan 05) for rationale.

#![allow(unused)]

#[cfg(not(target_arch = "wasm32"))]
#[must_use]
pub fn phase_zero_marker() -> &'static str {
    env!("CARGO_PKG_NAME")
}

#[cfg(target_arch = "wasm32")]
compile_error!(
    "fossil-runtime / fossil-cli / fossil-lsp are native-only; \
     do not add them to the WASM CI gate"
);

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    #[test]
    fn marker_returns_crate_name() {
        assert_eq!(phase_zero_marker(), env!("CARGO_PKG_NAME"));
    }
}
