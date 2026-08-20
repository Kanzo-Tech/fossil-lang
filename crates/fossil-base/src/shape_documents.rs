//! Decoding a shape document — the query that runs one row of
//! [`crate::providers`] against one registered file.
//!
//! # What this replaces
//!
//! The compiler used to read a `.shex` by calling
//! `db.system().read_file(&resolved)` from inside a tracked query and handing
//! the bytes to `fossil_shex::ShExDescriptor::from_shex_source`. Two things were
//! wrong with that, and they are different problems:
//!
//! 1. **`fossil-hir` had to link `ShEx`** to name the return type. That is the
//!    dependency [`fossil_graph_schema::shapes`] exists to cut: the compiler now
//!    names [`OutputShapes`], and a *host* supplies the row.
//! 2. **The read registered no Salsa dependency.** `read_file` is a host call,
//!    not an input; `crates/fossil-hir/src/infer.rs` says so in its own module
//!    docs ("Reads from inside a tracked query do not register a Salsa
//!    dependency"), and that read was chosen deliberately, on the grounds that
//!    it keeps `MAX_PER_MAPPING_FAN_OUT` at 1. Editing the `.shex`
//!    therefore re-ran nothing, and in the LSP that is a stale diagnostic that
//!    never clears. [`decode_shape_document`] pays for the fix: it reads the text
//!    from a [`SourceFile`] input instead — see [`crate::files`] for the registry
//!    that turns a path into one.
//!
//! # The row is chosen by NAME, and that is the change
//!
//! `decoder_for(decoders, uri)` lived here and picked a row by the document's
//! **extension**. It is gone. Ruling 13 of `SURFACE-PLAN.md`: the name a program
//! writes after `io.` selects the row, the row checks its own extension, and the
//! two are checked against each other rather than one of them being decorative.
//! See [`crate::providers`] for the whole argument.
//!
//! The consequence for this module is the query key. It was the document alone;
//! it is now the document **and the name**, interned into [`TypeDocument`],
//! because `io.shex("x.ttl")` and `io.shacl("x.ttl")` are two different
//! questions about one file and used to be one memoized answer. The fan-out
//! property that argument rested on survives: the key contains no mapping, so N
//! mappings naming one document under one constructor still share ONE decode,
//! and editing the PROGRAM re-runs none.

use fossil_graph_schema::OutputShapes;

use crate::db::Db;
use crate::files::SourceFile;
use crate::providers::provider;

/// A decode request: **which document, read as which language**.
///
/// Interned rather than passed as two arguments because a tracked query's key
/// has to be a Salsa struct — salsa 0.26 rejects a bare `String` with "the trait
/// bound `String: SalsaStructInDb` is not satisfied", which `crate::files`'s own
/// tests already record.
#[salsa::interned(debug)]
pub struct TypeDocument<'db> {
    /// The registered document.
    pub document: SourceFile,
    /// The provider name the program wrote after `io.` — `"shex"`, `"shacl"`.
    #[returns(ref)]
    pub provider: String,
}

/// Decode a registered shape document with the provider the program named.
///
/// Tracked, and keyed by the [`SourceFile`] input plus the name — so
/// `doc.text(db)` registers a real dependency and **editing the document re-runs
/// this query and everything downstream of it**. That is the whole reason this
/// is a query rather than a call to `System::read_file`.
///
/// Prefer [`shape_document`], which interns the key for you.
///
/// # The `None`s, and the one that is a value instead
///
/// - **The name is not an installed row**, or names a row that does not read
///   types → `None`. The caller checked this before asking (that check is where
///   the diagnostic naming the constructor comes from), so reaching it here is
///   a host that installed a different table than the one the caller consulted.
/// - **The row's extension does not accept the document** → **not checked here**.
///   It belongs to the caller too, for the same reason: the span of the
///   `io.shex("…")` that named it is up there, and a rejection with no span is
///   the silent drop this whole change is against.
/// - **A row ran and failed** → `Some(OutputShapes::rejected(_))`. The failure
///   is carried *in* the value, where [`OutputShapes::rejections`] puts it next
///   to the per-shape rejections a decoder collects on the `Ok` path.
///
/// Collapsing that last case to `None` would delete the only evidence a
/// malformed `.shex` ever produces — the checker could report "no shapes" but
/// never "this shape file is broken, here is why", which is precisely the
/// diagnostic a user needs.
#[salsa::tracked]
pub fn decode_shape_document<'db>(
    db: &'db dyn Db,
    request: TypeDocument<'db>,
) -> Option<OutputShapes> {
    let doc = request.document(db);
    let row = provider(crate::providers::installed(db), request.provider(db))?;
    let decode = row.reads_types?;
    let uri = doc.path(db);
    Some(match decode(uri, doc.text(db)) {
        Ok(shapes) => shapes,
        Err(rejection) => OutputShapes::rejected(rejection),
    })
}

/// [`decode_shape_document`] with the key interned for you — the call every
/// consumer makes.
///
/// A plain function so a caller that is not itself a tracked query (the IDE
/// features call straight in) does not have to hold a `'db` interned handle.
#[must_use]
pub fn shape_document(db: &dyn Db, doc: SourceFile, provider_name: &str) -> Option<OutputShapes> {
    decode_shape_document(db, TypeDocument::new(db, doc, provider_name.to_string()))
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use std::path::Path;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::SystemTime;

    use fossil_graph_schema::Rejection;
    use salsa::Setter as _;

    use crate::providers::Capability;

    /// Does `provider_name` name an installed row that reads types?
    ///
    /// The cheap half of the question [`shape_document`] answers expensively.
    /// It was a `pub fn` beside it, for "a caller that wants to diagnose the
    /// name before spending a decode" — and in the whole workspace no such
    /// caller was ever written. These three assertions were its only callers,
    /// so it lives where its callers do.
    fn reads_types(db: &dyn Db, provider_name: &str) -> bool {
        provider(crate::providers::installed(db), provider_name)
            .is_some_and(|p| p.provides(Capability::ReadTypes))
    }

    use super::*;
    use crate::db::FossilDb;
    use crate::files::{file_at, register_file};
    use crate::providers::{DATA, Provider};
    use crate::system::{FsError, System};
    // The line decoder used to be written out again here, in a THIRD dialect of
    // the same idea (first line a shape IRI, each further line a predicate).
    // There is one now, and it lives one module over — see
    // `crate::test_support`'s header for why it is a feature and not a
    // `#[cfg(test)]` module.
    use crate::test_support::{SHEX, TABLE, decode_lines};

    /// A second type-reading row, to prove selection actually selects — and it
    /// shares `SHEX`'s extensions on purpose, so only the NAME can tell them
    /// apart.
    // The `Result` is not optional here even though this one never fails: the
    // signature has to be `DecodeTypes`'s exactly, or it is not a row.
    #[allow(clippy::unnecessary_wraps)]
    fn decode_nothing(_uri: &str, _text: &str) -> Result<OutputShapes, Rejection> {
        Ok(OutputShapes::new(Vec::new(), Vec::new()))
    }

    static NOTHING: Provider = Provider {
        name: "nothing",
        extensions: &["shex", "ttl"],
        reads_rows: None,
        reads_types: Some(decode_nothing),
    };

    static TWO_ROWS: &[&Provider] = &[&SHEX, &NOTHING];

    #[derive(Debug, Default)]
    struct DecodingHost;

    impl System for DecodingHost {
        fn read_file(&self, path: &Path) -> Result<Vec<u8>, FsError> {
            Err(FsError::NotFound(path.display().to_string()))
        }
        fn now(&self) -> SystemTime {
            SystemTime::UNIX_EPOCH
        }
        fn providers(&self) -> &'static [&'static Provider] {
            TWO_ROWS
        }
    }

    /// A host that installed no type readers — the default.
    #[derive(Debug)]
    struct BareHost;

    impl System for BareHost {
        fn read_file(&self, path: &Path) -> Result<Vec<u8>, FsError> {
            Err(FsError::NotFound(path.display().to_string()))
        }
        fn now(&self) -> SystemTime {
            SystemTime::UNIX_EPOCH
        }
    }

    fn person(db: &FossilDb) -> SourceFile {
        SourceFile::new(
            db,
            "shape https://example.org/Person\nprop https://example.org/name - 1 1\n".to_string(),
            "p.shex".to_string(),
        )
    }

    // -- the query -------------------------------------------------------

    /// **The bug ruling 13 names, as a test.** One document, two constructors,
    /// two answers. Until the name reached the dispatch these were one memoized
    /// value and `io.shex` / `io.shacl` behaved identically.
    #[test]
    fn one_document_and_two_names_are_two_questions() {
        let db = FossilDb::new(Arc::new(DecodingHost));
        let doc = person(&db);
        assert_eq!(
            shape_document(&db, doc, "shex")
                .expect("the shex row ran")
                .shapes()
                .count(),
            1
        );
        assert_eq!(
            shape_document(&db, doc, "nothing")
                .expect("the other row ran")
                .shapes()
                .count(),
            0,
            "the name selects the row; the extension did not change"
        );
    }

    #[test]
    fn a_host_with_no_type_readers_yields_none() {
        let db = FossilDb::new(Arc::new(BareHost));
        assert_eq!(BareHost.providers(), DATA, "the default is the data rows");
        assert!(shape_document(&db, person(&db), "shex").is_none());
        assert!(!reads_types(&db, "shex"));
    }

    /// A name no installed row carries is `None`, and so is a name that carries
    /// a row which reads ROWS.
    #[test]
    fn a_name_that_reads_no_types_yields_none() {
        let db = FossilDb::new(Arc::new(DecodingHost));
        assert!(shape_document(&db, person(&db), "linkml").is_none());
        assert!(!reads_types(&db, "linkml"));
        assert!(!reads_types(&db, "csv"), "`io.csv` reads rows, not types");
    }

    #[test]
    fn a_decoded_document_reaches_the_neutral_vocabulary() {
        let db = FossilDb::new(Arc::new(DecodingHost));
        let shapes = shape_document(&db, person(&db), "shex").expect("the shex row");
        assert_eq!(shapes.shapes().count(), 1);
        let p = shapes
            .lookup("https://example.org/Person")
            .expect("the shape");
        assert_eq!(p.properties[0].predicate, "https://example.org/name");
        assert_eq!(
            shapes
                .to_graph_schema(&fossil_graph_schema::Renames::default())
                .nodes[0]
                .label,
            "Person"
        );
    }

    /// A malformed document is `Some(_)` carrying its rejection — never `None`.
    /// `None` means "no row read this"; losing the difference would delete the
    /// only evidence a broken `.shex` produces.
    #[test]
    fn a_malformed_document_is_carried_not_dropped() {
        let db = FossilDb::new(Arc::new(DecodingHost));
        let doc = SourceFile::new(
            &db,
            "!malformed empty document\n".to_string(),
            "broken.shex".to_string(),
        );
        let shapes = shape_document(&db, doc, "shex").expect("a row ran and failed");
        assert_eq!(shapes.shapes().count(), 0);
        assert_eq!(
            shapes.rejections(),
            [Rejection::Malformed("empty document".into())],
            "the failure is a diagnostic the checker can surface"
        );
    }

    /// The point of the whole change: editing the document's text re-executes
    /// the decode; editing an unrelated file does not.
    #[test]
    fn editing_the_document_reexecutes_the_query_and_an_unrelated_edit_does_not() {
        let executions: Arc<AtomicUsize> = Arc::new(AtomicUsize::new(0));
        let seen = executions.clone();
        let callback: Box<dyn Fn(salsa::Event) + Send + Sync + 'static> = Box::new(move |event| {
            if let salsa::EventKind::WillExecute { database_key } = event.kind
                // Salsa 0.26 debug-renders a key as `query_name(Id(raw))` while
                // the database is attached, which it is inside the callback.
                && format!("{database_key:?}").contains("decode_shape_document")
            {
                seen.fetch_add(1, Ordering::SeqCst);
            }
        });
        let mut db = FossilDb::with_event_callback(Arc::new(DecodingHost), callback);

        let doc = person(&db);
        let other = SourceFile::new(&db, "unrelated".to_string(), "m.fossil".to_string());

        assert!(shape_document(&db, doc, "shex").is_some());
        assert_eq!(executions.load(Ordering::SeqCst), 1, "cold");

        // A second read is a cache hit: no dependency changed.
        assert!(shape_document(&db, doc, "shex").is_some());
        assert_eq!(executions.load(Ordering::SeqCst), 1, "warm");

        // An unrelated file's text changes. The decode never read it, so
        // nothing re-executes.
        other.set_text(&mut db).to("still unrelated".to_string());
        assert!(shape_document(&db, doc, "shex").is_some());
        assert_eq!(
            executions.load(Ordering::SeqCst),
            1,
            "an unrelated edit must not re-execute the decode"
        );

        // The document itself changes. This is what `read_file` could not see.
        doc.set_text(&mut db)
            .to("shape https://example.org/Person\n\
             prop https://example.org/name - 1 1\n\
             prop https://example.org/age - 1 1\n"
                .to_string());
        let shapes = shape_document(&db, doc, "shex").expect("decoded");
        assert_eq!(
            executions.load(Ordering::SeqCst),
            2,
            "editing the document re-executes the decode"
        );
        assert_eq!(
            shapes
                .lookup("https://example.org/Person")
                .expect("the shape")
                .properties
                .len(),
            2,
            "and the new answer is the edited document's"
        );
    }

    /// The registry and the query compose: a host registers the document by
    /// path, the compiler resolves the path and decodes what it finds.
    #[test]
    fn a_registered_document_is_reachable_by_path_and_decodes() {
        let mut db = FossilDb::new(Arc::new(DecodingHost));
        let doc = SourceFile::new(
            &db,
            "shape https://example.org/City\nprop https://example.org/population - 1 1\n"
                .to_string(),
            "shapes/city.shex".to_string(),
        );
        register_file(&mut db, "shapes/city.shex".to_string(), doc);

        let found = file_at(&db, "shapes/city.shex").expect("registered");
        let shapes = shape_document(&db, found, "shex").expect("decoded");
        assert!(shapes.lookup("https://example.org/City").is_some());
    }

    /// The table `test_support` hands out is reachable through the same lookup,
    /// and `decode_lines` is the row's `decode` — the identity the rest of the
    /// workspace's tests rest on.
    #[test]
    fn the_test_support_table_carries_the_line_row_under_the_name_shex() {
        let row = crate::providers::provider(TABLE, "io.shex").expect("installed");
        assert_eq!(row.name, "shex");
        assert!(
            std::ptr::fn_addr_eq(
                row.reads_types.expect("the row reads types"),
                decode_lines as crate::providers::DecodeTypes,
            ),
            "the row's `decode` is `decode_lines`"
        );
    }
}
