//! `TyKind` pretty-printing — the single source of truth for rendering Fossil
//! types as user-facing strings.
//!
//! [`render_ty_kind`] lives here, and not in `fossil-ide::hover` where it was
//! written, because two layers render the same types and one of them is below
//! the IDE. It is consumed by:
//!   - [`crate::check::compatible`]'s mismatch diagnostic messages,
//!   - the LSP hover (which re-exports it from `fossil-ide::hover`).
//!
//! # `Unknown(InferenceId)` must never reach the surface
//!
//! The internal `TyKind::Unknown(InferenceId)`
//! synthesis-state placeholder is normalised to `"?"` here — it never appears
//! verbatim in a surface diagnostic or hover.

use crate::ty::TyKind;

/// Render a [`TyKind`] as a Fossil-style type string.
///
/// `Unknown(InferenceId)` renders as `"?"` (internal inference state must never
/// reach the surface). `Error` renders as `"Error"`.
#[must_use]
pub fn render_ty_kind<'db>(db: &'db dyn fossil_base::Db, kind: &TyKind<'db>) -> String {
    match kind {
        TyKind::Primitive(p) => format!("{p:?}"),
        TyKind::Iri => "Iri".to_string(),
        TyKind::IriTemplate => "IriTemplate".to_string(),
        TyKind::Seq(inner) => format!("Seq<{}>", render_ty_kind(db, inner.kind(db))),
        TyKind::Record(_) => "Record { ... }".to_string(),
        TyKind::Error(_) => "Error".to_string(),
        TyKind::Unknown(_) => "?".to_string(),
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::render_ty_kind;
    use crate::ty::{Ty, TyKind};
    use fossil_graph_schema::Primitive;
    use std::sync::Arc;

    fn db() -> fossil_base::FossilDb {
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem::default());
        fossil_base::FossilDb::new(system)
    }

    #[test]
    fn renders_primitive() {
        let db = db();
        let s = render_ty_kind(&db, &TyKind::Primitive(Primitive::String));
        assert_eq!(s, "String");
    }

    #[test]
    fn renders_nested_seq() {
        let db = db();
        let int = Ty::new(&db, TyKind::Primitive(Primitive::Integer));
        let seq = Ty::new(&db, TyKind::Seq(int));
        let seq_seq = Ty::new(&db, TyKind::Seq(seq));
        assert_eq!(render_ty_kind(&db, seq_seq.kind(&db)), "Seq<Seq<Integer>>");
    }

    #[test]
    fn unknown_never_leaks_to_surface() {
        let db = db();
        let s = render_ty_kind(&db, &TyKind::Unknown(crate::ty::InferenceId(7)));
        assert_eq!(s, "?", "internal inference state must render as ?");
    }
}
