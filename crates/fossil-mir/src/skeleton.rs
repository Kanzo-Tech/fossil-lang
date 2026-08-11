//! IRI-template skeleton matching — resolves an IRI-template property to its
//! target vertex type (the basis for classifying a mapping property as an EDGE
//! vs a literal vertex prop).
//!
//! This is language semantics ("what output does the mapping declare"), so it
//! lives in `fossil-mir` (next to the lowering) and is reused by BOTH
//! [`crate::lower::lower_to_mir_pg`] and the codegen's `synthesize_sink_plan`
//! (`fossil-codegen` depends on `fossil-mir`) — one source of truth, no
//! duplicated skeleton logic across crates.

use fossil_hir::body::body;
use fossil_hir::def_map::def_map;
use fossil_hir::lower::InterpolationPart;
use fossil_hir::lower::lower_to_hir;
use fossil_hir::{HirExpr, MappingLoc, PropertyKey};
use smol_str::SmolStr;

/// Every mapping's subject skeleton in a file, paired with the vertex type it
/// declares — the table a property's IRI template is matched against to decide
/// whether it is an edge.
///
/// **File-keyed on purpose.** Each mapping needs the whole table, so computing
/// it per mapping made the lowering quadratic in mappings per file: measured
/// 2026-08-07 with `cargo run --release --example query_time -p fossil-mir`,
/// `lower_to_mir_pg` cost 1.16 ms at 100 mappings and **87 ms at 1000** — 94%
/// of the whole compile, 78× for 10× the input. Salsa was the only thing
/// keeping it from being worse: the n² calls were n² memo hits, and a memo hit
/// is ~72 ns, so memoising harder would still have left 72 ms at 1000. The fix
/// is to stop asking n times for one answer.
///
/// It does NOT widen the pinned per-mapping fan-out: that invariant is about
/// `body` / `typecheck_mapping` / `expr_types`, and this is read by
/// `lower_to_mir_pg`, which already reads `def_map(db, file)`. Editing an
/// `iri = ...` template does invalidate this for the whole file — correctly,
/// because that edit changes which properties of every OTHER mapping are edges
/// to this type.
#[salsa::tracked]
pub fn subject_skeletons(
    db: &dyn fossil_base::Db,
    file: fossil_base::SourceFile,
) -> Vec<(String, SmolStr)> {
    let dm = def_map(db, file);
    let hir = lower_to_hir(db, file);
    dm.mappings(db)
        .iter()
        .enumerate()
        .filter_map(|(i, loc)| {
            let ty = SmolStr::new(crate::lower::local_name(
                hir.mappings(db).get(i)?.shape_iri.as_str(),
            ));
            Some((subject_template_skeleton(db, *loc)?, ty))
        })
        .collect()
}

/// The IRI-template skeleton of a mapping's subject property, or `None` when
/// there is no subject.
#[allow(clippy::elidable_lifetime_names)]
pub fn subject_template_skeleton<'db>(
    db: &'db dyn fossil_base::Db,
    mapping: MappingLoc<'db>,
) -> Option<String> {
    for prop in body(db, mapping).properties(db) {
        if matches!(prop.key, PropertyKey::Iri) {
            return match &prop.value {
                HirExpr::Interpolation(parts) => Some(template_skeleton(parts)),
                _ => None,
            };
        }
    }
    None
}

/// Replace every per-row hole with one marker, keeping literal text and
/// constant holes verbatim. Two subject IRIs that interpolate different columns
/// at the same positions therefore share a skeleton — the basis for resolving
/// an IRI property to its target vertex type.
///
/// It used to take the template's RAW TEXT and scan it for `${`, calling a hole
/// dynamic when its inner text began with `.`. Now the parts arrive parsed, so
/// "is this hole per-row?" is a question about the expression, which is the
/// only place that question has an answer: a `Call` over a column is per-row
/// too, and the text scan called it constant.
#[must_use]
pub fn template_skeleton(parts: &[InterpolationPart]) -> String {
    const HOLE_MARKER: char = '\u{1}';
    let mut out = String::new();
    for part in parts {
        match part {
            InterpolationPart::Text(t) => out.push_str(t),
            // A constant hole contributes its value; it is the same for every
            // row, so two mappings only match if it matches.
            InterpolationPart::Hole(HirExpr::PrefixedName { iri }) => out.push_str(iri),
            InterpolationPart::Hole(HirExpr::StringLit(s)) => out.push_str(s),
            InterpolationPart::Hole(_) => out.push(HOLE_MARKER),
        }
    }
    out
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::{HirExpr, InterpolationPart, template_skeleton};

    /// `"https://example.org/{path}/{<column>}"` — the shape every subject IRI
    /// in the corpus has, built from parts rather than from text.
    fn subject(path: &str, column: &str) -> Vec<InterpolationPart> {
        vec![
            InterpolationPart::Hole(HirExpr::PrefixedName {
                iri: "https://example.org/".into(),
            }),
            InterpolationPart::Text(format!("{path}/").into()),
            InterpolationPart::Hole(HirExpr::FieldRef(column.into())),
        ]
    }

    #[test]
    fn templates_with_same_shape_share_skeleton() {
        // Different column, same prefix + positions → same skeleton (so an FK
        // template resolves to the matching subject's vertex type).
        assert_eq!(
            template_skeleton(&subject("person", "id")),
            template_skeleton(&subject("person", "user_id"))
        );
    }

    #[test]
    fn different_prefix_paths_differ() {
        assert_ne!(
            template_skeleton(&subject("person", "id")),
            template_skeleton(&subject("order", "id"))
        );
    }

    /// The qualified spelling is the same hole as the anonymous one: a subject
    /// written `{users.id}` must match one written `{.id}`, or the ninth
    /// amendment's fixture rewrite would silently drop every edge in the file.
    #[test]
    fn a_qualified_hole_matches_an_anonymous_one() {
        let anonymous = subject("person", "id");
        let qualified = vec![
            InterpolationPart::Hole(HirExpr::PrefixedName {
                iri: "https://example.org/".into(),
            }),
            InterpolationPart::Text("person/".into()),
            InterpolationPart::Hole(HirExpr::ColumnRef {
                binding: "users".into(),
                column: "id".into(),
            }),
        ];
        assert_eq!(template_skeleton(&anonymous), template_skeleton(&qualified));
    }
}
