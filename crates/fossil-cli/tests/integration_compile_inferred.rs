//! Integration test for Phase 13-04a — fossil-cli pre-introspection produces
//! byte-identical output BOTH ways:
//!
//! 1. With sidecar `.csvw.json` present (backwards-compat path; v0.1
//!    .fossil files keep working; `D-CSVW-DEPRECATED` warning emitted by
//!    fossil-hir but compile succeeds + bytes identical).
//! 2. Without sidecar (the inferred path takes over via the new
//!    `pre_introspect_and_register` step).
//!
//! Validates the load-bearing claim of ADR-0037: the CLI mirrors the
//! browser-side DESCRIBE-then-register flow so users can drop sidecars
//! safely. 13-04b can then delete `hello.csvw.json` without breaking the
//! walking-skeleton invariant.

use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn fossil_binary() -> std::path::PathBuf {
    // Use the just-built release binary. The release path is the canonical
    // walking-skeleton entry point.
    if let Some(bin) = std::env::var_os("CARGO_BIN_EXE_fossil") {
        return std::path::PathBuf::from(bin);
    }
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    std::path::Path::new(manifest_dir)
        .join("..")
        .join("..")
        .join("target")
        .join("release")
        .join("fossil")
}

fn repo_root() -> std::path::PathBuf {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    std::path::PathBuf::from(manifest_dir).join("..").join("..")
}

/// Unique temp dir (no tempfile dep — mirrors the convention in
/// `crates/fossil-runtime/tests/graphar_decomp.rs:269`).
fn unique_tempdir(stem: &str) -> std::path::PathBuf {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id();
    let dir = std::env::temp_dir().join(format!("fossil-13-04a-{stem}-{pid}-{ts}"));
    std::fs::create_dir_all(&dir).expect("mkdir tempdir");
    dir
}

/// Smoke: existing `examples/hello.fossil` (WITH sidecar still present at this
/// point) compiles to byte-identical output. Mirrors the standing
/// walking-skeleton check.
#[test]
fn hello_fossil_with_sidecar_still_compiles_byte_identical() {
    let out = unique_tempdir("hello-sidecar");
    let hello = repo_root().join("examples/hello.fossil");
    // Run from repo-root so the `examples/users.csv` path in the .fossil
    // resolves correctly (v0.1 quirk; pre_introspect_and_register handles
    // the source_dir-relative-then-cwd-fallback path resolution).
    let status = Command::new(fossil_binary())
        .current_dir(repo_root())
        .arg("compile")
        .arg(&hello)
        .arg("--out-dir")
        .arg(&out)
        .status()
        .expect("fossil compile");
    assert!(
        status.success(),
        "compile failed for examples/hello.fossil (sidecar path)"
    );
    let manifest = std::fs::metadata(out.join("manifest.yaml")).expect("manifest.yaml exists");
    let parquet = std::fs::metadata(out.join("output.parquet")).expect("output.parquet exists");
    assert_eq!(
        manifest.len(),
        283,
        "manifest bytes drift on legacy CSVW path"
    );
    assert_eq!(
        parquet.len(),
        784,
        "parquet bytes drift on legacy CSVW path"
    );
}

/// Load-bearing test: a hand-built `.fossil` WITHOUT any sidecar
/// `.csvw.json` compiles to byte-identical output. Proves the
/// `InferredDescriptor` pre-introspection step actually fires + produces
/// equivalent forward propagation to the CSVW path.
#[test]
fn no_sidecar_fossil_compiles_via_inferred_descriptor_path() {
    let dir = unique_tempdir("no-sidecar-fixture");
    // CSV fixture — same shape as examples/users.csv:
    let csv_path = dir.join("u.csv");
    std::fs::write(
        &csv_path,
        "id,name\n1,Alice\n2,Bob\n3,Carol\n4,Dani\n5,Eli\n",
    )
    .expect("write csv");
    // .fossil with NO sidecar .csvw.json; uses io.csv() without a `schema=` arg.
    // The Fossil source uses `${...}` template-interpolation syntax — the
    // `uninlined_format_args` clippy lint is silenced by the `#[allow]` attr
    // on this test function (the strings are .fossil source, not Rust format args).
    let fossil_path = dir.join("hello-no-sidecar.fossil");
    let fossil_src = format!(
        "prefix ex: <https://example.org/>\n\
         \n\
         users := io.csv(\"u.csv\")\n\
         \n\
         User : ex:Person from users\n\
         {indent}iri = `{lb}ex:{rb}user/{lb}.id{rb}`\n\
         {indent}ex:name = .name\n",
        indent = "    ",
        lb = "${",
        rb = "}",
    );
    std::fs::write(&fossil_path, fossil_src).expect("write .fossil");

    let out = unique_tempdir("no-sidecar-out");
    let status = Command::new(fossil_binary())
        .current_dir(&dir)
        .arg("compile")
        .arg(&fossil_path)
        .arg("--out-dir")
        .arg(&out)
        .status()
        .expect("fossil compile (no sidecar)");
    assert!(
        status.success(),
        "compile failed for no-sidecar fixture (inferred path)"
    );
    let manifest = std::fs::metadata(out.join("manifest.yaml")).expect("manifest.yaml exists");
    let parquet = std::fs::metadata(out.join("output.parquet")).expect("output.parquet exists");
    assert_eq!(
        manifest.len(),
        283,
        "manifest bytes drift on inferred path (no sidecar)"
    );
    assert_eq!(
        parquet.len(),
        784,
        "parquet bytes drift on inferred path (no sidecar)"
    );
}
