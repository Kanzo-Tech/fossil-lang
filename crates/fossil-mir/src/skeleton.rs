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

/// The IRI-template skeleton of a mapping's `iri = ...` subject property, or
/// `None` when there is no subject or it is not a backtick template.
#[allow(clippy::elidable_lifetime_names)]
pub fn subject_template_skeleton<'db>(
    db: &'db dyn fossil_base::Db,
    mapping: MappingLoc<'db>,
) -> Option<String> {
    for prop in body(db, mapping).properties(db) {
        if matches!(prop.key, PropertyKey::Iri) {
            return match &prop.value {
                HirExpr::Template(t) => Some(template_skeleton(t.as_str())),
                _ => None,
            };
        }
    }
    None
}

/// Replace dynamic field placeholders (`${.field}`) in a backtick-template's raw
/// text with a uniform marker, keeping static prefix expansions (`${pfx:}`) and
/// literal segments verbatim. Two templates that interpolate different columns at
/// the same positions therefore share a skeleton — the basis for resolving an
/// IRI-template property to its target vertex type (the `${...}` inner of a field
/// reference begins with `.`; a prefix expansion does not).
#[must_use]
pub fn template_skeleton(text: &str) -> String {
    const FIELD_MARKER: char = '\u{1}';
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find("${") {
        out.push_str(&rest[..open]);
        let after = &rest[open + 2..];
        let Some(close) = after.find('}') else {
            out.push_str(&rest[open..]); // unterminated — keep verbatim
            return out;
        };
        let inner = &after[..close];
        if inner.trim_start().starts_with('.') {
            out.push(FIELD_MARKER); // dynamic per-row field → wildcard
        } else {
            out.push_str("${"); // static prefix expansion → keep verbatim
            out.push_str(inner);
            out.push('}');
        }
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    out
}

#[cfg(all(test, not(target_arch = "wasm32")))]
// The `${.id}` / `${ex:}` template fixtures are skeleton syntax, not Rust format args.
#[allow(clippy::literal_string_with_formatting_args)]
mod tests {
    use super::template_skeleton;

    #[test]
    fn templates_with_same_shape_share_skeleton() {
        // Different field, same prefix + positions → same skeleton (so an FK
        // template resolves to the matching subject's vertex type).
        let a = template_skeleton("${ex:}person/${.id}");
        let b = template_skeleton("${ex:}person/${.user_id}");
        assert_eq!(a, b);
    }

    #[test]
    fn different_prefix_paths_differ() {
        let a = template_skeleton("${ex:}person/${.id}");
        let b = template_skeleton("${ex:}order/${.id}");
        assert_ne!(a, b);
    }
}
