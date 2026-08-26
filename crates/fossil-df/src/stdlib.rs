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
        "starts_with" => "starts_with",
        "substring" => "substr",
        "trim" => "btrim",
        "upper" => "upper",
        // Shared, and reached only through a template: these are the names the
        // nine `InlineForm` variants used to hide in a doc-comment, so they
        // could never appear here before ruling 15.
        "regexp_replace" => "regexp_replace",
        // The two that differ.
        //
        // `string_to_array` lives in the NESTED function package, not the
        // scalar one, so `str.split` needs both the `nested_expressions` feature
        // and a lookup that searches both — see `render_expr_template`. It was
        // filed as unrenderable for want of those, beside rows that cannot be
        // done at all.
        "string_split" => "string_to_array",
        "strptime" => "to_timestamp",
        // Aggregates are not scalar expressions: `math.sum` in a property
        // position is a pipeline with a group-by, which is a different feature.
        // Saying so beats emitting a call that plans wrong.
        //
        // These four are the WHOLE of `UNREACHABLE_ON_DATAFUSION` now, and they
        // are not a capability gap: `math.sum` is how a `group_by`'s
        // aggregations are SPELLED, and `RegistryEntry::agg_fn` turns the row
        // into the `AggFn` that `fossil_mir::lower` reads. Only the scalar
        // rendering is missing, and only a scalar rendering should be.
        "avg" | "max" | "min" | "sum" => return None,
        // `error`, `json_extract_string`, `split_part` and `regexp_matches`
        // stood here and are gone with the rows that named them
        // (`core.require` + `validate.*`, `parse.json`, `parse.csv_row`, and
        // `validate.regex` respectively). `no_mapping_is_dead` below is what
        // keeps this list from outliving its templates again — the arms used to
        // be an append-only record of names once seen.
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
///
/// **It is down to one cause from four, and that is the change worth reading.**
/// Three of the four were not capability gaps at all:
///
/// - `str.split` and `anon.hash` recorded a Cargo feature and a lookup that
///   searched one function package. `string_to_array` and `sha256` are real
///   DataFusion functions living in the nested and crypto packages, absent from
///   `all_default_functions()`. `str.split` renders now — `nested_expressions`
///   is on and `render_expr_template` searches both packages — and `anon.hash`
///   left the language, which is why `crypto_expressions` stays off.
/// - `core.require` and the five `validate.*` rows needed `error()`, which has
///   no DataFusion spelling: there is no way to raise from an expression. They
///   are deleted from the language rather than carried as a permanent gap.
/// - `parse.json` needed a JSON extraction DataFusion does not ship. Also
///   deleted.
///
/// What is left is the four aggregates, and they are here for a reason that is
/// not a gap either: an aggregate in a SCALAR position is a pipeline with a
/// group-by, and only the scalar rendering is missing. They remain the spelling
/// of a `group_by`'s aggregations — see `datafusion_name`.
#[cfg(test)]
const UNREACHABLE_ON_DATAFUSION: &[&str] = &["math.avg", "math.max", "math.min", "math.sum"];

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

    /// **Every arm of `datafusion_name` is named by some catalogue template.**
    ///
    /// The reconciliation table is append-only by nature: a row leaves the
    /// language and its function names stay behind, spelling out a mapping for
    /// text nothing writes. Four arms had already outlived their rows —
    /// `error`, `json_extract_string`, `split_part` and `regexp_matches` — and
    /// nothing was red, because a mapping nobody reaches cannot be wrong.
    ///
    /// It reads this file as text rather than taking a hand-written list of the
    /// arms, which is the shape `xtask`'s `registry_is_shared` and
    /// `packages/introspect`'s parity test both use: derive the guard from the
    /// original instead of repeating it. A second list here would need the same
    /// guard one level up.
    ///
    /// **What it cannot prove:** that a mapping is CORRECT. That `substring`
    /// should become `substr` and not `substring` is a fact about DataFusion,
    /// and the test above — which plans every template through the real
    /// renderer — is what would catch it being wrong.
    #[test]
    fn no_mapping_is_dead() {
        // The names every surviving template actually calls. A template is SQL,
        // so a call is an identifier followed by `(`; `CAST` and `CASE` are
        // keywords the reader handles itself and are excluded by being
        // upper-case, which no catalogued function name is.
        let mut called: Vec<String> = Vec::new();
        for entry in stdlib().iter() {
            let LoweringKind::Expr(t) = &entry.lowering else {
                continue;
            };
            let chars: Vec<char> = t.chars().collect();
            let mut i = 0;
            while i < chars.len() {
                if !(chars[i].is_ascii_alphabetic() || chars[i] == '_') {
                    i += 1;
                    continue;
                }
                let start = i;
                while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                let word: String = chars[start..i].iter().collect();
                if chars.get(i) == Some(&'(') && word.chars().any(|c| c.is_ascii_lowercase()) {
                    called.push(word);
                }
            }
        }

        // The arms, read off this file. Each is `"a" => …` or `"a" | "b" => …`
        // inside `datafusion_name`, and nothing else in the function is a string
        // literal on a line with a `=>`.
        let source = include_str!("stdlib.rs");
        let body = source
            .split_once("pub fn datafusion_name")
            .expect("the function is in this file")
            .1;
        let body = body.split_once("\n}\n").expect("the function ends").0;

        let mut dead: Vec<String> = Vec::new();
        for line in body.lines() {
            let line = line.trim();
            if line.starts_with("//") || !line.contains("=>") {
                continue;
            }
            let arm = line.split_once("=>").expect("checked").0;
            for name in arm.split('"').skip(1).step_by(2) {
                if !called.iter().any(|c| c == name) {
                    dead.push(name.to_owned());
                }
            }
        }

        assert!(
            dead.is_empty(),
            "`datafusion_name` maps {dead:?}, which no catalogue template calls. \
             A row left the language and its reconciliation stayed behind; delete \
             the arm."
        );
        // The guard is worthless if the scan found nothing to scan.
        assert!(
            called.len() > 10,
            "only {} template calls were found; a mapping check over an empty \
             corpus passes vacuously",
            called.len()
        );
    }
}
