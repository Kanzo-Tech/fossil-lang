//! The stdlib catalog, rendered for `DataFusion` — the engine half of a call.
//!
//! `fossil_hir::stdlib` says what a function IS: its name, its signature, and
//! its classification (a builtin of the engine, an inline SQL form, or a UDF).
//! This module says what each becomes HERE, which is a materializer's business
//! and not the language's — the same split `primitive_to_graphar` keeps for the
//! datatype lattice.
//!
//! # Two vocabularies, one catalog
//!
//! `LoweringKind::Builtin` carries the `DuckDB` spelling because the `DuckDB`
//! path is what the catalog was written for. Most of those names are also
//! `DataFusion`'s; the three that are not are listed in [`datafusion_name`],
//! which is the only place the two engines' vocabularies are reconciled.
//!
//! # The UDFs
//!
//! `LoweringKind::Udf` names a function no engine has (`fossil_slug`). The
//! logic is pure Rust and lives here as a `ScalarUDF`, so the compiler ring can
//! run it without a database at all. `WasmClass::NativeUdfOnly` is about
//! `DuckDB`-WASM, which has no runtime registration API — it says nothing about
//! this path.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

use datafusion::arrow::array::{Array, ArrayRef, StringArray};
use datafusion::arrow::datatypes::DataType;
use datafusion::error::DataFusionError;
use datafusion::logical_expr::{ColumnarValue, ScalarUDF, Volatility, create_udf};

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
        // The three that differ.
        "regexp_matches" => "regexp_like",
        "string_split" => "string_to_array",
        "strptime" => "to_timestamp",
        // Aggregates are not scalar expressions: `math.sum` in a property
        // position is a different feature (a pipeline with a group-by), and F5
        // is where it lands. Saying so beats emitting a call that plans wrong.
        "avg" | "max" | "min" | "sum" => return None,
        _ => return None,
    })
}

/// Every `ScalarUDF` the stdlib needs and `DataFusion` does not ship, keyed by
/// the `udf_name` its catalog entry declares.
pub static UDFS: LazyLock<HashMap<&'static str, Arc<ScalarUDF>>> = LazyLock::new(|| {
    let mut m: HashMap<&'static str, Arc<ScalarUDF>> = HashMap::new();
    m.insert(
        "fossil_slug",
        Arc::new(unary_string_udf("fossil_slug", |s| Ok(slug::slugify(s)))),
    );
    m.insert(
        "fossil_unicode_norm",
        Arc::new(binary_string_udf("fossil_unicode_norm", |s, form| {
            use unicode_normalization::UnicodeNormalization;
            Ok(match form.to_ascii_uppercase().as_str() {
                "NFC" => s.nfc().collect::<String>(),
                "NFD" => s.nfd().collect::<String>(),
                "NFKC" => s.nfkc().collect::<String>(),
                "NFKD" => s.nfkd().collect::<String>(),
                other => {
                    return Err(DataFusionError::Execution(format!(
                        "fossil_unicode_norm: unknown normalization form `{other}` \
                         (expected NFC/NFD/NFKC/NFKD)"
                    )));
                }
            })
        })),
    );
    m
});

/// Build a `(VARCHAR, VARCHAR) -> VARCHAR` UDF from a pure per-row function.
fn binary_string_udf(
    name: &'static str,
    f: impl Fn(&str, &str) -> Result<String, DataFusionError> + Send + Sync + 'static,
) -> ScalarUDF {
    create_udf(
        name,
        vec![DataType::Utf8, DataType::Utf8],
        DataType::Utf8,
        Volatility::Immutable,
        Arc::new(move |args: &[ColumnarValue]| {
            let arrays = ColumnarValue::values_to_arrays(args)?;
            let as_str = |i: usize| -> Result<&StringArray, DataFusionError> {
                arrays[i]
                    .as_any()
                    .downcast_ref::<StringArray>()
                    .ok_or_else(|| {
                        DataFusionError::Execution(format!("{name} expects string arguments"))
                    })
            };
            let (a, b) = (as_str(0)?, as_str(1)?);
            let mut out: Vec<Option<String>> = Vec::with_capacity(a.len());
            for i in 0..a.len() {
                if a.is_null(i) || b.is_null(i) {
                    out.push(None);
                } else {
                    out.push(Some(f(a.value(i), b.value(i))?));
                }
            }
            let array: ArrayRef = Arc::new(StringArray::from(out));
            Ok(ColumnarValue::Array(array))
        }),
    )
}

/// Build a `VARCHAR -> VARCHAR` UDF from a pure per-row function.
///
/// A row-level `Err` is a query error, not a null: `validate.*` exists to stop
/// a run, and a silent null is the failure mode this whole phase is about.
fn unary_string_udf(
    name: &'static str,
    f: impl Fn(&str) -> Result<String, DataFusionError> + Send + Sync + 'static,
) -> ScalarUDF {
    create_udf(
        name,
        vec![DataType::Utf8],
        DataType::Utf8,
        Volatility::Immutable,
        Arc::new(move |args: &[ColumnarValue]| {
            let arrays = ColumnarValue::values_to_arrays(args)?;
            let input = arrays[0]
                .as_any()
                .downcast_ref::<StringArray>()
                .ok_or_else(|| {
                    DataFusionError::Execution(format!("{name} expects a string argument"))
                })?;
            let mut out: Vec<Option<String>> = Vec::with_capacity(input.len());
            for i in 0..input.len() {
                if input.is_null(i) {
                    out.push(None);
                } else {
                    out.push(Some(f(input.value(i))?));
                }
            }
            let array: ArrayRef = Arc::new(StringArray::from(out));
            Ok(ColumnarValue::Array(array))
        }),
    )
}

/// The catalogued functions this engine cannot run yet, pinned so that making
/// one work — or breaking one — is a diff in this list and not a surprise in a
/// user's program.
#[cfg(test)]
const UNREACHABLE_ON_DATAFUSION: &[&str] = &[
    // Two reasons, and they are different.
    //
    // The aggregates are not scalar calls at all: `math.sum` in a property
    // position is a pipeline with a group-by, which is F5.
    "math.avg",
    "math.max",
    "math.min",
    "math.sum",
    // These six are UDFs whose pure logic exists ONCE, inside
    // `fossil-runtime`'s DuckDB trampolines, and `fossil-runtime` may not
    // depend on this crate (nor this on it — the two engine halves are
    // deliberately unaware of each other). Copying the logic here would make
    // two implementations of `validate.email`, which is the mistake F1 spent a
    // day undoing. They move here when the DuckDB half goes, which is F5 — and
    // that half is already dead: `fossil_runtime::execute` has no caller but
    // its own test, and the codegen that emitted its SQL no longer exists.
    "anon.hmac",
    "clean.strip_html",
    "validate.email",
    "validate.iso_date",
    "validate.url",
    "validate.uuid",
];

#[cfg(test)]
mod tests {
    use super::*;
    use fossil_hir::stdlib::{LoweringKind, stdlib};

    /// Every catalogued function reaches an implementation on this engine, or
    /// is named here as one that does not. The list is the honest half: an
    /// aggregate is not a scalar call, and saying which functions those are
    /// beats discovering it when a program uses one.
    #[test]
    fn every_catalogued_function_is_reachable_or_declared_unreachable() {
        let mut unreachable: Vec<&str> = Vec::new();
        for entry in stdlib().iter() {
            match &entry.lowering {
                LoweringKind::Builtin { duckdb_name } => {
                    if datafusion_name(duckdb_name.as_str()).is_none() {
                        unreachable.push(entry.name.as_str());
                    }
                }
                LoweringKind::Udf { udf_name } => {
                    if !UDFS.contains_key(udf_name.as_str()) {
                        unreachable.push(entry.name.as_str());
                    }
                }
                // Inline forms render structurally; plan ops are not scalars.
                LoweringKind::Inline(_) | LoweringKind::Plan(_) => {}
            }
        }
        unreachable.sort_unstable();
        let mut declared: Vec<&str> = UNREACHABLE_ON_DATAFUSION.to_vec();
        declared.sort_unstable();
        assert_eq!(
            unreachable, declared,
            "a function became reachable or unreachable without this list moving"
        );
    }

    #[test]
    fn slug_is_a_real_udf_on_this_engine() {
        assert!(UDFS.contains_key("fossil_slug"));
    }
}
