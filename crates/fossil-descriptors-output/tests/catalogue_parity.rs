//! `catalogue.bnf` is the source of truth for what names exist. This fails when
//! the Rust statics disagree with it.
//!
//! It lives here and not in `fossil-base` because this is the only crate that
//! can see all six rows: `fossil-base` carries the four that read data, this
//! one carries `SHEX` and `SHACL`, and the dependency runs this way — the
//! `Cargo.toml` next door refuses a `fossil-hir` edge for the same reason.
//!
//! **It derives the table from the file rather than restating it.** That is the
//! pattern from `packages/introspect/tests/rust-parity.test.ts`, and the point
//! of it: a test that repeats the values it checks agrees with itself, which is
//! how nine `///` comments in this tree came to assert invariants nothing held.
//! Nothing below names a row, an extension or a capability — those come out of
//! `catalogue.bnf`, and the assertions are about the two lists MATCHING.
//!
//! What it cannot prove, and says so rather than implying otherwise: that
//! `decodes decode_shex` names the function actually installed. A `fn` pointer
//! has no name at run time. It checks that the row declares the CAPABILITY the
//! file gives it; the identity of the function is what generating the statics
//! from this file would close, and that step is not taken yet.

use std::collections::BTreeMap;

use fossil_base::{Capability, NativeReader, Provider, RowReader};
use fossil_descriptors_output::PROVIDERS;

/// One row as `catalogue.bnf` declares it.
#[derive(Debug, PartialEq, Eq)]
struct Declared {
    extensions: Vec<String>,
    /// `Some(fn_name)` for `reads native <fn>`, `None` for `materialised`.
    native: Option<String>,
    reads_rows: bool,
    reads_types: bool,
}

/// Parse the `row … = … .` lines. Everything else in the file is `(* … *)`
/// commentary, which is where the argument lives and which this ignores.
fn declared() -> BTreeMap<String, Declared> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../catalogue.bnf");
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("catalogue.bnf must be readable at {path}: {e}"));

    let mut rows = BTreeMap::new();
    let mut in_comment = false;
    for line in text.lines() {
        // The file is BNF-shaped, so comments nest around whole blocks.
        if line.contains("(*") {
            in_comment = !line.contains("*)");
            continue;
        }
        if in_comment {
            in_comment = !line.contains("*)");
            continue;
        }
        let Some(rest) = line.trim().strip_prefix("row ") else {
            continue;
        };
        let (name, body) = rest
            .split_once('=')
            .unwrap_or_else(|| panic!("a `row` line needs an `=`: {line}"));

        let extensions: Vec<String> = body
            .split('"')
            .skip(1)
            .step_by(2)
            .map(str::to_owned)
            .collect();
        assert!(
            !extensions.is_empty(),
            "row `{}` declares no extension",
            name.trim()
        );

        let native = body
            .split_whitespace()
            .skip_while(|w| *w != "native")
            .nth(1);
        rows.insert(
            name.trim().to_owned(),
            Declared {
                extensions,
                native: native.map(str::to_owned),
                reads_rows: body.contains("reads "),
                reads_types: body.contains("decodes "),
            },
        );
    }
    assert!(!rows.is_empty(), "catalogue.bnf declares no rows at all");
    rows
}

/// The `DuckDB` table function a [`NativeReader`] names — the string the file
/// writes after `native`. It is spelled out in each variant's own doc comment.
const fn native_fn(reader: NativeReader) -> &'static str {
    match reader {
        NativeReader::CsvAuto => "read_csv_auto",
        NativeReader::JsonAuto => "read_json_auto",
        NativeReader::Parquet => "read_parquet",
    }
}

fn installed() -> BTreeMap<String, &'static Provider> {
    PROVIDERS.iter().map(|p| (p.name.to_owned(), *p)).collect()
}

#[test]
fn every_declared_row_is_installed_and_no_others() {
    let declared: Vec<String> = declared().keys().cloned().collect();
    let installed: Vec<String> = installed().keys().cloned().collect();
    assert_eq!(
        declared, installed,
        "catalogue.bnf and `PROVIDERS` name different rows"
    );
}

#[test]
fn each_row_accepts_the_extensions_the_file_gives_it() {
    let installed = installed();
    for (name, row) in declared() {
        let actual: Vec<String> = installed[&name]
            .extensions
            .iter()
            .map(|e| (*e).to_owned())
            .collect();
        assert_eq!(
            row.extensions, actual,
            "row `{name}`: catalogue.bnf and the static disagree on extensions"
        );
    }
}

#[test]
fn each_row_declares_the_capabilities_the_file_gives_it() {
    let installed = installed();
    for (name, row) in declared() {
        let p = installed[&name];
        assert_eq!(
            row.reads_rows,
            p.provides(Capability::ReadRows),
            "row `{name}`: `reads` in catalogue.bnf, `reads_rows` in the static"
        );
        assert_eq!(
            row.reads_types,
            p.provides(Capability::ReadTypes),
            "row `{name}`: `decodes` in catalogue.bnf, `reads_types` in the static"
        );
    }
}

#[test]
fn a_native_row_names_the_table_function_the_file_names() {
    let installed = installed();
    for (name, row) in declared() {
        let actual = match installed[&name].reads_rows {
            Some(RowReader::Native(r)) => Some(native_fn(r)),
            Some(RowReader::Materialised) | None => None,
        };
        assert_eq!(
            row.native.as_deref(),
            actual,
            "row `{name}`: catalogue.bnf and the static disagree on how bytes become rows"
        );
    }
}

/// The guard has to be able to fail. If the parse silently found nothing, every
/// assertion above would compare two empty lists and pass — which is exactly how
/// `alpha-steps.test.ts` in the sibling repository spent weeks matching a corpus
/// of zero.
#[test]
fn the_parse_actually_read_the_file() {
    let rows = declared();
    assert!(
        rows.len() >= 6,
        "catalogue.bnf parsed to {} rows; the file declares at least six",
        rows.len()
    );
    assert!(
        rows.contains_key("csv") && rows.contains_key("shex"),
        "the parse missed rows it must see: {:?}",
        rows.keys().collect::<Vec<_>>()
    );
}
