//! Native Rust UDFs for the `native_udf_only` stdlib functions (STDL-03/05).
//!
//! These implement the `fossil-hir` entries whose `LoweringKind` is
//! [`Udf`](https://docs.rs/fossil-hir) — i.e. functions classified
//! [`WasmClass::NativeUdfOnly`]. They are registered on a native `DuckDB`
//! [`Connection`] via [`register_stdlib_udfs`] so that generated SQL of the
//! form `fossil_slug(x)` resolves and runs natively.
//!
//! # Why native-only (Pitfall 3)
//!
//! `DuckDB`-WASM has no runtime scalar-function registration API, and the impl
//! crates here (`unicode-normalization`, `slug`, `voca_rs`, `hmac`/`sha2`,
//! `uuid`) are native dependencies. That is *exactly* why these functions are
//! `native_udf_only`: the WASM playground reads the classification manifest
//! (`fossil-wasm`) and disables them in-browser. This module lives in
//! `fossil-runtime`, which carries a `wasm32` `compile_error!` cfg-tripwire, so
//! none of these crates can leak into a WASM-gated crate's dependency tree.
//!
//! # UDF name ⇔ registry mapping
//!
//! The registered names match the `udf_name` spellings in
//! `fossil_hir::stdlib::LoweringKind::Udf` exactly (the codegen `render_expr` Call
//! arm renders `udf_name(args)`):
//!
//! | registry function       | UDF name                  |
//! | ----------------------- | ------------------------- |
//! | `clean.slug`            | `fossil_slug`             |
//! | `clean.normalize_unicode` | `fossil_unicode_norm`   |
//! | `clean.strip_html`      | `fossil_strip_html`       |
//! | `validate.email`        | `fossil_validate_email`   |
//! | `validate.url`          | `fossil_validate_url`     |
//! | `validate.uuid`         | `fossil_validate_uuid`    |
//! | `validate.iso_date`     | `fossil_validate_iso_date`|
//! | `anon.hmac`             | `fossil_hmac`             |
//!
//! `validate.*` returns the input unchanged when valid and raises a `DuckDB`
//! runtime error when invalid (stdlib.md §validate). `anon.hmac` returns a
//! hex-encoded HMAC-SHA256 (stdlib.md §anon/hmac, default algo `sha256`).

use std::error::Error;

use duckdb::Connection;
use duckdb::core::{DataChunkHandle, Inserter, LogicalTypeId};
use duckdb::ffi::duckdb_string_t;
use duckdb::types::DuckString;
use duckdb::vscalar::{ScalarFunctionSignature, VScalar};
use duckdb::vtab::arrow::WritableVector;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use unicode_normalization::UnicodeNormalization;

/// Read column `col` of `input` as an owned `Vec<String>`, one entry per row.
///
/// # Safety
///
/// Same contract as [`VScalar::invoke`]: `input` must be the live chunk
/// `DuckDB` handed the trampoline and `col` must be a `VARCHAR` column in range.
// SAFETY: third-party-trait integration boundary (ADR-0004). The unsafe reads
// borrowed DuckDB storage (`as_slice_with_len`) for exactly `input.len()` rows
// of the given VARCHAR column and copies each into an owned String before
// return — nothing borrowed is retained. No safe alternative: the DuckDB
// vector accessor is `unsafe fn` by design.
#[allow(unsafe_code)]
unsafe fn read_varchar_col(input: &mut DataChunkHandle, col: usize) -> Vec<String> {
    let len = input.len();
    let vector = input.flat_vector(col);
    let raw = vector.as_slice_with_len::<duckdb_string_t>(len);
    raw.iter()
        .map(|ptr| DuckString::new(&mut { *ptr }).as_str().to_string())
        .collect()
}

/// Generate a single-`VARCHAR`-arg `VScalar` impl from a pure `Fn(&str) ->
/// Result<String, Box<dyn Error>>`. The closure body owns the per-row logic;
/// returning `Err` surfaces as a `DuckDB` runtime error (used by `validate.*`).
macro_rules! varchar_unary_udf {
    ($name:ident, $body:expr) => {
        struct $name;

        impl VScalar for $name {
            type State = ();

            // SAFETY: third-party-trait integration boundary (ADR-0004). The
            // unsafe is the DuckDB scalar-function ABI: we only read column 0
            // for rows 0..input.len() and write the same range to `output`, and
            // retain nothing past return. No safe alternative — the trait
            // method is `unsafe fn` by DuckDB's design.
            #[allow(unsafe_code)]
            unsafe fn invoke(
                _: &Self::State,
                input: &mut DataChunkHandle,
                output: &mut dyn WritableVector,
            ) -> Result<(), Box<dyn Error>> {
                let inputs = unsafe { read_varchar_col(input, 0) };
                let out = output.flat_vector();
                let f: fn(&str) -> Result<String, Box<dyn Error>> = $body;
                for (row, s) in inputs.iter().enumerate() {
                    let v = f(s)?;
                    out.insert(row, v.as_str());
                }
                Ok(())
            }

            fn signatures() -> Vec<ScalarFunctionSignature> {
                vec![ScalarFunctionSignature::exact(
                    vec![LogicalTypeId::Varchar.into()],
                    LogicalTypeId::Varchar.into(),
                )]
            }
        }
    };
}

/// Generate a two-`VARCHAR`-arg `VScalar` impl from a pure `Fn(&str, &str) ->
/// Result<String, Box<dyn Error>>`.
macro_rules! varchar_binary_udf {
    ($name:ident, $body:expr) => {
        struct $name;

        impl VScalar for $name {
            type State = ();

            // SAFETY: see `varchar_unary_udf` — identical DuckDB-ABI boundary,
            // reading columns 0 and 1 for rows 0..input.len() (ADR-0004).
            #[allow(unsafe_code)]
            unsafe fn invoke(
                _: &Self::State,
                input: &mut DataChunkHandle,
                output: &mut dyn WritableVector,
            ) -> Result<(), Box<dyn Error>> {
                let a = unsafe { read_varchar_col(input, 0) };
                let b = unsafe { read_varchar_col(input, 1) };
                let out = output.flat_vector();
                let f: fn(&str, &str) -> Result<String, Box<dyn Error>> = $body;
                for (row, (x, y)) in a.iter().zip(b.iter()).enumerate() {
                    let v = f(x, y)?;
                    out.insert(row, v.as_str());
                }
                Ok(())
            }

            fn signatures() -> Vec<ScalarFunctionSignature> {
                vec![ScalarFunctionSignature::exact(
                    vec![LogicalTypeId::Varchar.into(), LogicalTypeId::Varchar.into()],
                    LogicalTypeId::Varchar.into(),
                )]
            }
        }
    };
}

// ── clean/ ──────────────────────────────────────────────────────────────────

varchar_unary_udf!(FossilSlug, |s| Ok(slug::slugify(s)));

varchar_unary_udf!(FossilStripHtml, |s| Ok(voca_rs::strip::strip_tags(s)));

varchar_binary_udf!(FossilUnicodeNorm, |s, form| {
    let normalized = match form.to_ascii_uppercase().as_str() {
        "NFC" => s.nfc().collect::<String>(),
        "NFD" => s.nfd().collect::<String>(),
        "NFKC" => s.nfkc().collect::<String>(),
        "NFKD" => s.nfkd().collect::<String>(),
        other => {
            return Err(
                format!("fossil_unicode_norm: unknown normalization form '{other}' (expected NFC/NFD/NFKC/NFKD)")
                    .into(),
            );
        }
    };
    Ok(normalized)
});

// ── validate/ — return input unchanged when valid, else runtime error ─────────

varchar_unary_udf!(FossilValidateEmail, |s| {
    if is_valid_email(s) {
        Ok(s.to_string())
    } else {
        Err(format!("fossil_validate_email: '{s}' is not a well-formed email").into())
    }
});

varchar_unary_udf!(FossilValidateUrl, |s| {
    if is_valid_url(s) {
        Ok(s.to_string())
    } else {
        Err(format!("fossil_validate_url: '{s}' is not a well-formed URL").into())
    }
});

varchar_unary_udf!(FossilValidateUuid, |s| {
    if uuid::Uuid::parse_str(s).is_ok() {
        Ok(s.to_string())
    } else {
        Err(format!("fossil_validate_uuid: '{s}' is not a valid UUID").into())
    }
});

varchar_unary_udf!(FossilValidateIsoDate, |s| {
    if is_valid_iso_date(s) {
        Ok(s.to_string())
    } else {
        Err(format!("fossil_validate_iso_date: '{s}' is not an ISO 8601 date").into())
    }
});

// ── anon/ — hex-encoded HMAC-SHA256 (default algo) ────────────────────────────

varchar_binary_udf!(FossilHmac, |message, key| {
    let mut mac = Hmac::<Sha256>::new_from_slice(key.as_bytes())
        .map_err(|e| -> Box<dyn Error> { format!("fossil_hmac: invalid key: {e}").into() })?;
    mac.update(message.as_bytes());
    let bytes = mac.finalize().into_bytes();
    let mut hex = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write as _;
        let _ = write!(hex, "{b:02x}");
    }
    Ok(hex)
});

/// RFC 5322 *simplified* email well-formedness: exactly one `@`, non-empty
/// local part, and a dotted domain with non-empty labels (stdlib.md §validate).
fn is_valid_email(s: &str) -> bool {
    let mut parts = s.splitn(2, '@');
    let (Some(local), Some(domain)) = (parts.next(), parts.next()) else {
        return false;
    };
    if local.is_empty() || domain.contains('@') {
        return false;
    }
    let mut labels = domain.split('.');
    domain.contains('.')
        && labels.all(|label| {
            !label.is_empty() && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
}

/// RFC 3986 syntax sniff: an absolute URL with a scheme followed by `://` and a
/// non-empty authority (stdlib.md §validate/url). Intentionally permissive —
/// the goal is "well-formed", not reachability.
fn is_valid_url(s: &str) -> bool {
    let Some((scheme, rest)) = s.split_once("://") else {
        return false;
    };
    !scheme.is_empty()
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        && scheme
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic())
        && !rest.is_empty()
}

/// ISO 8601 calendar date `YYYY-MM-DD` with range-checked month/day
/// (stdlib.md §`validate/iso_date`).
fn is_valid_iso_date(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return false;
    }
    let digits = |slice: &str| slice.chars().all(|c| c.is_ascii_digit());
    if !(digits(&s[0..4]) && digits(&s[5..7]) && digits(&s[8..10])) {
        return false;
    }
    let month: u8 = s[5..7].parse().unwrap_or(0);
    let day: u8 = s[8..10].parse().unwrap_or(0);
    (1..=12).contains(&month) && (1..=31).contains(&day)
}

/// Register every `native_udf_only` stdlib function on a `DuckDB` connection.
///
/// Called once by [`crate::execute`] after the in-memory connection is opened
/// and before the generated SQL batch runs, so any `fossil_slug(x)` /
/// `fossil_validate_email(x)` / `fossil_hmac(x, k)` call in the plan resolves.
///
/// # Errors
///
/// Returns the underlying [`duckdb::Error`] if any scalar-function registration
/// fails (e.g. a name collision in the catalog).
pub fn register_stdlib_udfs(conn: &Connection) -> duckdb::Result<()> {
    conn.register_scalar_function::<FossilSlug>("fossil_slug")?;
    conn.register_scalar_function::<FossilUnicodeNorm>("fossil_unicode_norm")?;
    conn.register_scalar_function::<FossilStripHtml>("fossil_strip_html")?;
    conn.register_scalar_function::<FossilValidateEmail>("fossil_validate_email")?;
    conn.register_scalar_function::<FossilValidateUrl>("fossil_validate_url")?;
    conn.register_scalar_function::<FossilValidateUuid>("fossil_validate_uuid")?;
    conn.register_scalar_function::<FossilValidateIsoDate>("fossil_validate_iso_date")?;
    conn.register_scalar_function::<FossilHmac>("fossil_hmac")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Open an in-memory connection with the stdlib UDFs registered, run
    /// `SELECT <expr>`, and return the single `String` result.
    fn eval(expr: &str) -> duckdb::Result<String> {
        let conn = Connection::open_in_memory()?;
        register_stdlib_udfs(&conn)?;
        let mut stmt = conn.prepare(&format!("SELECT {expr}"))?;
        stmt.query_row([], |row| row.get::<_, String>(0))
    }

    #[test]
    fn slug_lowercases_and_dashes() {
        assert_eq!(eval("fossil_slug('Hello World')").unwrap(), "hello-world");
        assert_eq!(
            eval("fossil_slug('Crème Brûlée!')").unwrap(),
            "creme-brulee"
        );
    }

    #[test]
    fn strip_html_removes_tags() {
        assert_eq!(
            eval("fossil_strip_html('<p>Hello <b>there</b></p>')").unwrap(),
            "Hello there"
        );
    }

    #[test]
    fn unicode_norm_nfc_composes() {
        // "e" + combining acute (U+0301) → NFC "é" (U+00E9, 2 UTF-8 bytes).
        let composed = eval("fossil_unicode_norm('e\u{0301}', 'NFC')").unwrap();
        assert_eq!(composed, "\u{00e9}");
    }

    #[test]
    fn validate_email_passes_valid_errors_invalid() {
        assert_eq!(eval("fossil_validate_email('a@b.com')").unwrap(), "a@b.com");
        let err = eval("fossil_validate_email('not-an-email')").unwrap_err();
        assert!(err.to_string().contains("fossil_validate_email"), "{err}");
    }

    #[test]
    fn validate_url_uuid_iso_date() {
        assert_eq!(
            eval("fossil_validate_url('https://example.org/x')").unwrap(),
            "https://example.org/x"
        );
        assert!(eval("fossil_validate_url('nope')").is_err());
        assert_eq!(
            eval("fossil_validate_uuid('550e8400-e29b-41d4-a716-446655440000')").unwrap(),
            "550e8400-e29b-41d4-a716-446655440000"
        );
        assert!(eval("fossil_validate_uuid('xyz')").is_err());
        assert_eq!(
            eval("fossil_validate_iso_date('2026-05-21')").unwrap(),
            "2026-05-21"
        );
        assert!(eval("fossil_validate_iso_date('2026-13-99')").is_err());
    }

    #[test]
    fn hmac_is_stable_hex_sha256() {
        // RFC-style fixed vector: deterministic hex digest, 64 chars.
        let out = eval("fossil_hmac('message', 'secret')").unwrap();
        assert_eq!(out.len(), 64);
        assert!(out.chars().all(|c| c.is_ascii_hexdigit()));
        // Stable across calls.
        assert_eq!(out, eval("fossil_hmac('message', 'secret')").unwrap());
    }
}
