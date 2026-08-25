//! A type-reading provider with no schema language behind it, and the host that
//! installs it — for the tests of every crate above this one.
//!
//! # Why this is here and not in a test module
//!
//! The compiler names no schema language: a host supplies a
//! [`Provider`](crate::Provider) row, the document is a
//! [`SourceFile`](crate::SourceFile) input, and what comes back is
//! [`OutputShapes`]. Every crate that wants to test *against a resolved shape*
//! therefore needs a decoder, and reaching for the real `ShEx` one puts the
//! dependency straight back in — which is what `0e6898d` cut out of
//! `fossil-mir` on purpose, and what `fossil-hir`'s own test decoder was written
//! to avoid.
//!
//! So the line format below was written three times: once in
//! `fossil-base`'s own `shape_documents` tests, once in
//! `fossil-hir/src/test_support.rs` (`#[cfg(test)]`, so no other crate could
//! reach it), and a third was about to be written in `fossil-mir`'s
//! `tests/lower_pg.rs`. A `#[cfg(test)]` module is invisible across a crate
//! boundary, so "put it where the first one lives" was not available until this
//! module was made a feature instead.
//!
//! **This is not compiler logic** — the anti-pattern `CLAUDE.md` names for this
//! crate. It is a reference implementation of the seam `fossil-base` itself
//! defines ([`Provider`](crate::Provider),
//! [`shape_document`](crate::shape_document), [`register_file`]), in a format
//! that exists nowhere else and that no program will ever be written in.
//! Coverage of a real `ShEx` document belongs where the `ShEx` decoder lives.
//!
//! # The row is named `shex`, and that is not a lie about the format
//!
//! Dispatch is by NAME, so a test program writing
//! `type { P } := io.shex("p.shex")` reaches whichever row is called `shex`. A
//! row called `lines` would be unreachable from every program in the workspace's
//! tests. The name is the CONSTRUCTOR, not the syntax; what the row actually
//! parses is the format below, and only these tests ever write it.
//!
//! # Reaching it
//!
//! ```toml
//! [dev-dependencies]
//! fossil-base = { path = "../fossil-base", features = ["test-support"] }
//! ```
//!
//! The module is also compiled under `cfg(test)` so this crate's own tests use
//! it without a self-referential dev-dependency.
//!
//! # The format
//!
//! ```text
//! shape <iri>
//! prop <predicate-iri> <value> <min> <max>
//! ```
//!
//! `<value>` is `-` (the document narrows nothing), an XSD local name
//! (`string`, `integer`, …), or `@<target-iri>` (an edge). `<max>` is a number
//! or `*`. A line beginning `!malformed ` makes the decoder reject the whole
//! document with the rest of the line as its reason.

use std::path::Path;
use std::sync::Arc;
use std::time::SystemTime;

use fossil_graph_schema::{Occurs, OutputShapes, Primitive, PropertyConstraint, Rejection, Shape};

use crate::db::{Db, FossilDb};
use crate::files::{SourceFile, register_file};
use crate::providers::{CSV, JSON, PARQUET, Provider, RDF};
use crate::system::{FsError, System};
use fossil_descriptors_input::DescriptorCache;

/// The cheapest `System` a test can stand up: a real filesystem, a real clock,
/// and whatever provider table the trait defaults to.
///
/// It lived in [`crate::system`] beside the trait, and moved here because it
/// has no production consumer — every use in the workspace is a test, a bench
/// or an example. A host that COMPILES a program installs the rows that read
/// types, and the default table is the data rows alone; `fossil-lsp`'s
/// `LspSystem` says so where it replaced this.
///
/// **It is not enough on its own for a test that resolves a shape document** —
/// see [`DecodingHost`], which is this plus that table.
#[derive(Debug, Default)]
pub struct NativeSystem {
    /// The introspected-schema table this host owns. One field, no methods —
    /// the storage, the locking and the freshness rule all live on
    /// [`DescriptorCache`].
    descriptors: DescriptorCache,
}

impl System for NativeSystem {
    fn read_file(&self, path: &Path) -> Result<Vec<u8>, FsError> {
        std::fs::read(path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => FsError::NotFound(path.display().to_string()),
            _ => FsError::Io(e.to_string()),
        })
    }

    fn now(&self) -> SystemTime {
        SystemTime::now()
    }

    fn descriptors(&self) -> Option<&DescriptorCache> {
        Some(&self.descriptors)
    }
}

/// One shape with one un-narrowed `name` predicate — the document most of the
/// tests name.
pub const PERSON_DOCUMENT: &str = "\
shape http://example.org/Person
prop http://example.org/name - 1 1
";

/// Decode the line format above. Pure: no IO, no clock — it is called from
/// inside a tracked query.
///
/// # Errors
///
/// [`Rejection::Malformed`] for a `!malformed` line, a `prop` before any
/// `shape`, or a line the format does not name.
pub fn decode_lines(_uri: &str, text: &str) -> Result<OutputShapes, Rejection> {
    let mut shapes: Vec<Shape> = Vec::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let mut tokens = line.split_whitespace();
        match tokens.next() {
            Some("!malformed") => {
                return Err(Rejection::Malformed(tokens.collect::<Vec<_>>().join(" ")));
            }
            Some("shape") => shapes.push(Shape {
                iri: tokens.next().unwrap_or_default().to_string(),
                properties: Vec::new(),
            }),
            Some("prop") => {
                let Some(shape) = shapes.last_mut() else {
                    return Err(Rejection::Malformed(
                        "a `prop` before any `shape`".to_string(),
                    ));
                };
                let predicate = tokens.next().unwrap_or_default().to_string();
                let value = tokens.next().unwrap_or("-");
                let min = tokens.next().and_then(|t| t.parse().ok()).unwrap_or(1);
                let max = tokens
                    .next()
                    .map_or(Some(1), |t| (t != "*").then(|| t.parse().unwrap_or(1)));
                let (datatype, targets) = value.strip_prefix('@').map_or_else(
                    || {
                        (
                            Primitive::from_xsd_iri(&format!(
                                "http://www.w3.org/2001/XMLSchema#{value}"
                            )),
                            Vec::new(),
                        )
                    },
                    |target| (None, vec![target.to_string()]),
                );
                shape.properties.push(PropertyConstraint {
                    predicate,
                    datatype,
                    targets,
                    occurs: Occurs { min, max },
                    // This toy `line`-per-predicate syntax is not `ShExC`, so
                    // there is no document a range would point into. `None` is
                    // the same answer a `ShExJ` document gives, and consumers
                    // are required to handle it.
                    span: None,
                });
            }
            _ => return Err(Rejection::Malformed(format!("unknown line `{line}`"))),
        }
    }
    Ok(OutputShapes::new(shapes, Vec::new()))
}

/// The row a test program reaches with `io.shex(…)`, claiming the extensions a
/// `ShEx` document is written under so the document can be named the way a
/// program names one.
pub static SHEX: Provider = Provider {
    name: "shex",
    extensions: &["shex", "shexj", "shexc"],
    reads_rows: None,
    reads_types: Some(decode_lines),
};

/// The table [`DecodingHost`] hands out: the four data rows plus the line row.
///
/// The data rows are here because they are not optional — a test program says
/// `User := io.csv("u.csv")` as often as it says `io.shex`, and a host that
/// installed only the type reader would fail to recognise its own sources.
pub static TABLE: &[&Provider] = &[&CSV, &JSON, &PARQUET, &RDF, &SHEX];

/// The real filesystem for everything except shape documents, plus the provider
/// table. A shape document is read through [`System::read_file`] like any other
/// file, so this delegates rather than stubbing.
///
/// [`NativeSystem`] on its own is NOT enough for any test that resolves a
/// shape: its [`System::providers`] is the trait default (the data rows only),
/// so no row reads types, `shape_document` returns `None`, and every mapping
/// resolves no shape at all.
#[derive(Debug, Default)]
pub struct DecodingHost(NativeSystem);

impl System for DecodingHost {
    fn read_file(&self, path: &Path) -> Result<Vec<u8>, FsError> {
        self.0.read_file(path)
    }
    fn now(&self) -> SystemTime {
        self.0.now()
    }
    fn providers(&self) -> &'static [&'static Provider] {
        TABLE
    }
    /// **The introspected-schema table, delegated** — it was the trait default
    /// `None`, and that is why no test above this crate could take a source row
    /// from introspection.
    ///
    /// `lookup_inferred` reads `db.system().descriptors()`, so with `None` it
    /// missed unconditionally and no fixture above this crate could take a
    /// source row from introspection at all.
    /// [`register_inferred`] is the other half: a host puts the descriptor in
    /// before the compile, exactly as `fossil_engine::pre_introspect_and_register`
    /// and the browser's `registerInferredDescriptor` do.
    fn descriptors(&self) -> Option<&fossil_descriptors_input::DescriptorCache> {
        self.0.descriptors()
    }
}

/// Register an introspected row for `uri` — the HOST's job, done by hand.
///
/// A test that wants a typed source row calls this. The columns are what an
/// introspection of the
/// file would have found; nothing here reads a file, because the point of the
/// inferred path is that the host has already looked.
pub fn register_inferred(db: &dyn Db, uri: &str, columns: &[(&str, Primitive)]) {
    let Some(cache) = db.system().descriptors() else {
        panic!("this host keeps no descriptor table");
    };
    cache.insert(fossil_descriptors_input::InferredDescriptor {
        uri: uri.into(),
        columns: columns
            .iter()
            .map(
                |(name, primitive)| fossil_descriptors_input::InferredColumn {
                    name: (*name).into(),
                    primitive: *primitive,
                },
            )
            .collect(),
        // Empty means "never fresh", which is right for a table nobody re-reads.
        freshness_token: String::new(),
    });
}

/// A database whose host installs the line decoder.
#[must_use]
pub fn new_db() -> FossilDb {
    let system: Arc<dyn System> = Arc::new(DecodingHost::default());
    FossilDb::new(system)
}

/// Put `text` in the database under `path`.
///
/// Registration is the half that is *not* the decoder and is just as easy to
/// forget: `decoded_document` resolves through [`crate::file_at`], which reads
/// the Salsa registry and never the disk, so a `.shex` written next to the
/// program is invisible until this is called.
pub fn register_document(db: &mut dyn Db, path: &str, text: &str) {
    let doc = SourceFile::new(&*db, text.to_string(), path.to_string());
    register_file(db, path.to_string(), doc);
}

/// A database holding `src` as `test.fossil` plus one shape document
/// registered at `document_path`, which is the path a `test.fossil` program
/// resolves to (both are at the root, so the name is the key).
#[must_use]
pub fn db_with_document(src: &str, document_path: &str, document: &str) -> (FossilDb, SourceFile) {
    let mut db = new_db();
    let file = SourceFile::new(&db, src.to_string(), "test.fossil".to_string());
    register_document(&mut db, document_path, document);
    (db, file)
}

/// A database holding `src` at `program_path` plus one shape document at
/// `document_path` — the same as [`db_with_document`] for a program that does
/// not sit at the root, where the registry key is the document joined onto the
/// PROGRAM's directory and not the bare name.
#[must_use]
pub fn db_with_document_at(
    program_path: &str,
    src: &str,
    document_path: &str,
    document: &str,
) -> (FossilDb, SourceFile) {
    let mut db = new_db();
    let file = SourceFile::new(&db, src.to_string(), program_path.to_string());
    register_document(&mut db, document_path, document);
    (db, file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_line_decoder_reads_the_three_value_forms() {
        let doc = decode_lines(
            "x.shex",
            "shape http://example.org/Person\n\
             prop http://example.org/name string 1 1\n\
             prop http://example.org/note - 0 *\n\
             prop http://example.org/lives @http://example.org/City 1 1\n",
        )
        .expect("decodes");
        let person = doc.lookup("http://example.org/Person").expect("the shape");
        assert_eq!(person.properties[0].datatype, Some(Primitive::String));
        assert_eq!(person.properties[1].datatype, None, "`-` narrows nothing");
        assert_eq!(person.properties[1].occurs, Occurs { min: 0, max: None });
        assert_eq!(person.properties[2].targets, ["http://example.org/City"]);
    }

    #[test]
    fn a_malformed_line_rejects_the_document() {
        assert_eq!(
            decode_lines("x.shex", "!malformed no shapes here\n"),
            Err(Rejection::Malformed("no shapes here".into()))
        );
    }

    /// The registration half: a document written to the registry is reachable
    /// through the same `file_at` the compiler resolves with.
    #[test]
    fn a_registered_document_decodes_through_the_query() {
        let (db, _file) = db_with_document("", "person.shex", PERSON_DOCUMENT);
        let doc = crate::files::file_at(&db, "person.shex").expect("registered");
        let shapes =
            crate::shape_documents::shape_document(&db, doc, "shex").expect("the `shex` row");
        assert!(shapes.lookup("http://example.org/Person").is_some());
    }
}
