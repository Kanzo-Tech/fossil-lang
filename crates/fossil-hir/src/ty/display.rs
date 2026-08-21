//! `TyKind` pretty-printing — the single source of truth for rendering Fossil
//! types as user-facing strings.
//!
//! `render_ty_kind` lives here, and not in `fossil-ide::hover` where it was
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
use fossil_graph_schema::local_name;

/// Render a [`TyKind`] as a Fossil-style type string.
///
/// `Error` renders as `"Error"`. There is no checker-state kind to hide: «no
/// type» is `None` before it ever reaches a renderer.
#[must_use]
pub fn render_ty_kind<'db>(db: &'db dyn fossil_base::Db, kind: &TyKind<'db>) -> String {
    match kind {
        TyKind::Primitive(p) => format!("{p:?}"),
        // The shapes, not the word «Iri»: what a reader needs to know about a
        // reference is what it reaches.
        TyKind::Ref(shapes) if shapes.is_empty() => "Ref<?>".to_string(),
        TyKind::Ref(shapes) => format!(
            "Ref<{}>",
            shapes
                .iter()
                .map(|s| local_name(s).to_string())
                .collect::<Vec<_>>()
                .join(" | ")
        ),
        TyKind::Seq(inner) => format!("Seq<{}>", render_ty_kind(db, inner.kind(db))),
        TyKind::Record(_) => "Record { ... }".to_string(),
        // The bindings it makes addressable, which is what a reader needs of a
        // relation — the columns are one hover away, on the binding.
        TyKind::Relation(rows) => format!(
            "Relation<{}>",
            rows.bindings().cloned().collect::<Vec<_>>().join(", ")
        ),
        TyKind::Null => "Null".to_string(),
        TyKind::Error(_) => "Error".to_string(),
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
}
