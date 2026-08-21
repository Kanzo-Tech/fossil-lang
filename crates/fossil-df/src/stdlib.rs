//! The stdlib catalog, rendered for `DataFusion` — the engine half of a call.
//!
//! `fossil_hir::stdlib` says what a function IS: its receiver, its name, its
//! signature, and its lowering (a scalar SQL expression template, or an
//! operator of the algebra).
//! This module says what each becomes HERE, which is a materializer's business
//! and not the language's — the same split `primitive_to_graphar` keeps for the
//! datatype lattice.
//!
//! # Two vocabularies, one catalog
//!
//! A `LoweringKind::Expr` template is written in `DuckDB` SQL, because `DuckDB`
//! is the engine every one of them was MEASURED against. Most of those names
//! are also `DataFusion`'s; the ones that are not are listed in
//! `datafusion_name`, the only place the two vocabularies are reconciled.
//!
//! # There are no UDFs any more
//!
//! A `UDFS` map lived here: `ScalarUDF`s for `fossil_slug` and
//! `fossil_unicode_norm`, the two functions no engine ships, reimplemented in
//! Rust so the compiler ring could run them without a database. Ruling 15 of
//! `SURFACE-PLAN.md` deleted the `Udf` lowering kind, so no row names one.
//! `str.slug` is a template both engines render; `clean.normalize_unicode`
//! left the language.

/// The `DataFusion` spelling of a catalogued builtin.
///
/// Identity for every name the two engines share; the exceptions are the whole
/// content of this function. A name with no `DataFusion` equivalent returns
/// `None` and the caller reports it rather than emitting a call that would fail
/// at plan time with an engine-level message.
#[must_use]
pub fn datafusion_name(duckdb_name: &str) -> Option<&'static str> {
    Some(match duckdb_name {
        // Shared spellings.
        "abs" => "abs",
        "concat" => "concat",
        "contains" => "contains",
        "ends_with" => "ends_with",
        "length" => "length",
        "lower" => "lower",
        "replace" => "replace",
        "round" => "round",
        "sha256" => "sha256",
        "starts_with" => "starts_with",
        "substring" => "substr",
        "trim" => "btrim",
        "upper" => "upper",
        // Shared, and reached only through a template: these are the names the
        // nine `InlineForm` variants used to hide in a doc-comment, so they
        // could never appear here before ruling 15.
        "regexp_replace" => "regexp_replace",
        "split_part" => "split_part",
        // The four that differ.
        "regexp_matches" => "regexp_like",
        "string_split" => "string_to_array",
        "strptime" => "to_timestamp",
        // `error()` has NO DataFusion equivalent, and it is the single reason
        // the four validators do not render on this engine. Naming it here as a
        // `None` rather than leaving it to fall through the catch-all is the
        // difference between a gap that is declared and a gap that is a typo.
        "error" => return None,
        // `json_extract` is DuckDB's; DataFusion ships no JSON extraction in
        // its default function set.
        "json_extract" => return None,
        // Aggregates are not scalar expressions: `math.sum` in a property
        // position is a different feature (a pipeline with a group-by), and F5
        // is where it lands. Saying so beats emitting a call that plans wrong.
        "avg" | "max" | "min" | "sum" => return None,
        _ => return None,
    })
}

// `template_to_datafusion` lived here: a textual pass over a whole template,
// replacing `regexp_matches(` with `regexp_like(` and so on before the text was
// handed to a SQL parser. It went with the SQL parser. `render_expr_template`
// reads the template itself and reconciles each function name through
// `datafusion_name` as it meets it, which is one mechanism instead of two and
// cannot rewrite a name that appears inside a string literal.

/// The catalogued functions this engine cannot render, pinned so that making
/// one work — or breaking one — is a diff in this list and not a surprise in a
/// user's program.
#[cfg(test)]
const UNREACHABLE_ON_DATAFUSION: &[&str] = &[
    // Four causes, and they are different. Every one of them was MEASURED by
    // the test below planning the real template, not guessed from a name.
    //
    // 1 — `sha256` and `string_to_array` are real DataFusion functions that are
    //     not in `all_default_functions()`: they live in optional function
    //     packages this crate does not register. A `SessionContext` that
    //     registered them would make these two render with no other change.
    "anon.hash",
    "str.split",
    // 2 — `error()`. There is no way to raise from a DataFusion expression, so
    //     "return this value or stop the run" has no spelling. It is the sole
    //     cause for all five.
    "core.require",
    "validate.email",
    "validate.iso_date",
    "validate.url",
    "validate.uuid",
    // 3 — the aggregates are not scalar calls at all: `math.sum` in a property
    //     position is a pipeline with a group-by, which is F5.
    "math.avg",
    "math.max",
    "math.min",
    "math.sum",
    // 4 — `json_extract` is DuckDB's; DataFusion ships no JSON extraction.
    "parse.json",
    // WHAT IS NOT HERE ANY MORE, and it is the point of ruling 15 on this
    // engine: `str.slug` and `str.strip_html`. They were native Rust UDFs whose
    // logic lived inside `fossil-runtime`'s DuckDB trampolines, unreachable
    // from this crate by construction. As templates they render here, because a
    // template is portable in a way a UDF is not. `clean.strip_html` was the
    // name in this list; the row is `str.strip_html` now and it is renderable.
];

#[cfg(test)]
mod tests {
    use super::*;
    use fossil_hir::stdlib::{LoweringKind, stdlib};

    /// Every catalogued function reaches an implementation on this engine, or
    /// is named above as one that does not.
    ///
    /// **This test got teeth.** It used to check that a `Builtin`'s name had a
    /// `DataFusion` spelling and that a `Udf`'s name was in a map — a question
    /// about two lookup tables. It now PLANS each row's template through the
    /// real renderer, so it fails on a template that is not valid SQL, not just
    /// on a name that is not in a list. The nine `InlineForm` variants it could
    /// not see at all are included for the first time.
    #[test]
    fn every_catalogued_function_is_reachable_or_declared_unreachable() {
        use datafusion::logical_expr::lit;

        let mut unreachable: Vec<&str> = Vec::new();
        for entry in stdlib().iter() {
            match &entry.lowering {
                LoweringKind::Expr(template) => {
                    let args: Vec<datafusion::logical_expr::Expr> =
                        entry.sig.params.iter().map(|_| lit("x")).collect();
                    if crate::render_expr_template(template.as_str(), &args).is_err() {
                        unreachable.push(entry.name.as_str());
                    }
                }
                // An operator is not a scalar expression; there is nothing to
                // render and nothing to declare.
                LoweringKind::Op(_) => {}
            }
        }
        unreachable.sort_unstable();
        let mut declared: Vec<&str> = UNREACHABLE_ON_DATAFUSION.to_vec();
        declared.sort_unstable();
        assert_eq!(
            unreachable, declared,
            "a function became renderable or unrenderable without this list moving"
        );
    }

    /// The two rows that were native-only Rust and are portable SQL now. This is
    /// the concrete gain of ruling 15 on this engine, so it is pinned rather
    /// than left as a claim in a comment.
    #[test]
    fn slug_and_strip_html_render_on_this_engine() {
        use datafusion::logical_expr::lit;
        for name in ["str.slug", "str.strip_html"] {
            let entry = stdlib().lookup(name).expect("catalogued");
            let LoweringKind::Expr(t) = &entry.lowering else {
                panic!("`{name}` must be an Expr row");
            };
            assert!(
                crate::render_expr_template(t.as_str(), &[lit("x")]).is_ok(),
                "`{name}` must render on DataFusion"
            );
        }
    }
}
