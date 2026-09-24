//! The shape documents a program names, and the key each one is looked up
//! under — **one function per question, for the checker and every host at
//! once**.
//!
//! A program brings a shape document in with
//! `type { Person } := io.shex("shapes/person.shex")`. The document is a Salsa
//! **input** ([`crate::shape_documents::shape_document`] is keyed by a
//! [`SourceFile`]), so a host has to register it before any query goes looking
//! — registering takes `&mut dyn Db`, which no query body can have. That splits
//! the work in two: the host asks WHICH documents and under WHAT key, and the
//! checker asks the registry for the same key.
//!
//! # Why the pair lives here
//!
//! Because a key that stops matching reads exactly like a document nobody
//! registered — the least debuggable failure the registry can produce — and
//! there were three answers to «what key».
//! `fossil_cli::documents` and `fossil_ide::shape_documents` each
//! carried a copy of [`documents_named`], and `fossil-ide`'s [`registry_key`]
//! was a hand-rolled `parent().join()` that disagreed with this one on two live
//! cases: a document named through a scheme (`s3://bucket/x.shex` came back as
//! `/programs/s3://bucket/x.shex`) and one named through a `@conn` alias. Both
//! files carried a docblock asserting the drift was closed. It was not, and no
//! test held it — which is why there is one function now and not a claim.
//!
//! Both hosts already depend on this crate, so nothing forced the copies.
//!
//! # Sans-IO
//!
//! [`missing_documents`] reports what is missing and reads nothing; the host
//! fetches each [`MissingDocument::locator`] however it can — a disk, a signed
//! URL — and hands the text back through [`fossil_base::register_document`]
//! under [`MissingDocument::key`]. A host that reads synchronously has
//! [`register_missing_documents`], which is that loop and nothing else.

use std::collections::HashMap;

use fossil_base::{Db, SourceFile, file_at, register_document};
use smol_str::SmolStr;

/// Every distinct document `file` names, in the order it names them.
///
/// Two places name one: [`crate::def_map::TypeEntry::document`] — the string
/// inside `type { … } := io.shex("…")` — and
/// [`crate::def_map::SourceEntry::schema_arg`], the document inside a source's
/// `schema = io.shex("…")`. Both reach the compiler through
/// [`fossil_base::file_at`], so a loop that registered only the first would
/// leave `{ A, B } := io.rdf(…, schema = io.shex("x.shex"))` resolving nothing.
///
/// What is listed is what the program NAMES. Which of those a decoder claims is
/// the compiler's business — a name here is a file the compiler may read, not a
/// promise that a shape decoder will take it, and that is the correct division:
/// this function parses nothing to decide.
///
/// A program that names none yields an empty list. That is a program the
/// checker rejects — naming a shape document is mandatory — but the rejection
/// is the checker's, and this function only reports what is written.
#[must_use]
pub fn documents_named(db: &dyn Db, file: SourceFile) -> Vec<SmolStr> {
    let def_map = crate::def_map::def_map(db, file);
    let mut named: Vec<SmolStr> = Vec::new();
    let mut push = |document: Option<&SmolStr>| {
        if let Some(document) = document
            && !named.contains(document)
        {
            named.push(document.clone());
        }
    };
    for entry in def_map.types(db) {
        push(entry.document.as_ref());
    }
    for entry in def_map.sources(db) {
        push(entry.schema_arg.as_ref());
    }
    named
}

/// The key a document is registered and looked up under: the reference the
/// program wrote, put through [`fossil_locator::SourceAnchor`] — the one
/// resolution rule.
///
/// It is a KEY and not a locator. The LSP's file paths are `file://` URIs, so
/// the key for `prog.fossil`'s `person.shex` is `file:///…/person.shex` — a
/// string no filesystem will open, and the right thing to key by anyway, since
/// the editor's own `didOpen` for that document arrives under the same URI.
/// Reading the bytes is the caller's business, which is why registration takes
/// the text and not the path.
///
/// The three references that are NOT relative paths keep the anchoring out of
/// it — a `@conn` alias, anything carrying a scheme, and an absolute path — and
/// that is the whole of what a hand-rolled `parent().join()` got wrong.
#[must_use]
pub fn registry_key(db: &dyn Db, file: SourceFile, document: &str) -> String {
    let dir = fossil_locator::program_dir(file.path(db));
    fossil_locator::SourceAnchor::beside(&dir).locator(document)
}

/// A document a program names that the database does not hold yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingDocument {
    /// What it is registered under: [`registry_key`], anchored with no
    /// connection map, so repointing a connection never invalidates a query.
    pub key: String,
    /// Where it is fetched from: the same reference through the connection map.
    pub locator: String,
}

/// Every document `file` names that is not registered yet, with the key it
/// goes under and the locator it is fetched from.
///
/// `connections` expands `@conn` aliases in the locator only. The program's
/// directory comes from `file`, so the key and the locator cannot be anchored
/// against two different places.
#[allow(clippy::implicit_hasher)] // `SourceAnchor` takes the std map.
#[must_use]
pub fn missing_documents(
    db: &dyn Db,
    file: SourceFile,
    connections: &HashMap<String, String>,
) -> Vec<MissingDocument> {
    let dir = fossil_locator::program_dir(file.path(db));
    let anchor = fossil_locator::SourceAnchor::new(&dir, connections);
    documents_named(db, file)
        .into_iter()
        .filter_map(|document| {
            let key = registry_key(db, file, &document);
            file_at(db, &key).is_none().then(|| MissingDocument {
                key,
                locator: anchor.locator(&document),
            })
        })
        .collect()
}

/// [`missing_documents`] read synchronously and registered — the loop for a
/// host with no connection map, so each locator is its key.
///
/// `read` answers `None` for a document it cannot produce, and that document
/// stays unregistered: the checker's diagnostic has the span, this loop does
/// not. An already-registered document is never re-read, so an open buffer is
/// not replaced by the saved copy and a second call is a no-op.
///
/// Returns how many documents were registered.
pub fn register_missing_documents(
    db: &mut dyn Db,
    file: SourceFile,
    read: &dyn Fn(&dyn Db, &str) -> Option<String>,
) -> usize {
    // Read first, register second: registering takes the database exclusively.
    let pending: Vec<(String, String)> = missing_documents(&*db, file, &HashMap::new())
        .into_iter()
        .filter_map(|missing| read(&*db, &missing.locator).map(|text| (missing.key, text)))
        .collect();
    for (key, text) in &pending {
        register_document(db, key, text);
    }
    pending.len()
}

// The `.fossil` sources below carry a `{users.id}` interpolation hole and
// `type { … }` braces — LITERAL Fossil source, not Rust format-string args.
#[allow(clippy::literal_string_with_formatting_args)]
#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    use fossil_base::register_file;
    use fossil_base::test_support::{PERSON_DOCUMENT, new_db};

    /// A program naming its shape document with `document`, and writing the one
    /// property [`PERSON_DOCUMENT`] declares.
    fn src_naming(document: &str) -> String {
        format!(
            "type {{ Person }} := io.shex(\"{document}\")\n\
             users := io.csv(\"users.csv\")\n\
             User : Person from users\n\
             \x20   @subject = \"http://example.org/u/{{users.id}}\"\n\
             \x20   name = users.name\n"
        )
    }

    fn program(db: &fossil_base::FossilDb, path: &str, document: &str) -> SourceFile {
        SourceFile::new(db, src_naming(document), path.to_string())
    }

    #[test]
    fn a_program_names_the_document_in_its_type_binding() {
        let db = new_db();
        let file = program(&db, "prog.fossil", "shapes/person.shex");
        assert_eq!(documents_named(&db, file), ["shapes/person.shex"]);
    }

    #[test]
    fn a_relative_document_is_resolved_against_the_program_directory() {
        let db = new_db();
        assert_eq!(
            registry_key(&db, program(&db, "prog.fossil", "p.shex"), "p.shex"),
            "p.shex",
            "a program with no directory resolves to the document as written"
        );
        assert_eq!(
            registry_key(&db, program(&db, "a/b/prog.fossil", "p.shex"), "p.shex"),
            "a/b/p.shex"
        );
        assert_eq!(
            registry_key(
                &db,
                program(&db, "file:///w/prog.fossil", "shapes/p.shex"),
                "shapes/p.shex"
            ),
            "file:///w/shapes/p.shex",
            "the URI the editor opened the program under keys the document too"
        );
    }

    /// **The case the hand-rolled key got wrong.** `parent().join()` produced
    /// `/programs/s3://bucket/person.shex`; a reference that already carries a
    /// scheme is already a locator and is anchored to nothing.
    #[test]
    fn a_document_named_by_url_keys_by_the_url() {
        let db = new_db();
        let file = program(&db, "/programs/prog.fossil", "s3://bucket/person.shex");
        assert_eq!(
            registry_key(&db, file, "s3://bucket/person.shex"),
            "s3://bucket/person.shex"
        );
    }

    /// The other one. An alias this host cannot expand — the checker is never
    /// given a connection map — passes through verbatim, so the "not found"
    /// that follows names what the program wrote.
    #[test]
    fn a_document_named_through_a_connection_alias_is_not_anchored() {
        let db = new_db();
        let file = program(&db, "/programs/prog.fossil", "@warehouse/person.shex");
        assert_eq!(
            registry_key(&db, file, "@warehouse/person.shex"),
            "@warehouse/person.shex"
        );
    }

    fn connections(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(name, base)| ((*name).to_string(), (*base).to_string()))
            .collect()
    }

    fn resolves_target_shape(db: &fossil_base::FossilDb, file: SourceFile) -> bool {
        let mapping = *crate::def_map::def_map(db, file)
            .mappings(db)
            .first()
            .expect("the program has one mapping");
        matches!(
            crate::shapes::resolve_target_shape(db, mapping),
            Ok(Some(_))
        )
    }

    #[test]
    fn without_a_connection_map_the_locator_is_the_key() {
        let db = new_db();
        let file = program(&db, "a/prog.fossil", "shapes/person.shex");
        assert_eq!(
            missing_documents(&db, file, &HashMap::new()),
            [MissingDocument {
                key: "a/shapes/person.shex".to_string(),
                locator: "a/shapes/person.shex".to_string(),
            }]
        );
    }

    /// The map reaches the locator and never the key: the key is what the
    /// program wrote, so repointing `warehouse` invalidates nothing.
    #[test]
    fn a_connection_expands_the_locator_and_leaves_the_key_as_written() {
        let db = new_db();
        let file = program(&db, "a/prog.fossil", "@warehouse/shapes/person.shex");
        assert_eq!(
            missing_documents(
                &db,
                file,
                &connections(&[("warehouse", "s3://bucket/base/")])
            ),
            [MissingDocument {
                key: "@warehouse/shapes/person.shex".to_string(),
                locator: "s3://bucket/base/shapes/person.shex".to_string(),
            }]
        );
    }

    #[test]
    fn an_alias_the_map_does_not_name_passes_through_verbatim() {
        let db = new_db();
        let file = program(&db, "a/prog.fossil", "@warehouse/person.shex");
        let missing = missing_documents(&db, file, &connections(&[("lake", "s3://lake")]));
        assert_eq!(missing[0].locator, "@warehouse/person.shex");
        assert_eq!(missing[0].key, "@warehouse/person.shex");
    }

    #[test]
    fn a_registered_document_is_not_missing() {
        let mut db = new_db();
        let file = program(&db, "a/prog.fossil", "shapes/person.shex");
        register_document(&mut db, "a/shapes/person.shex", PERSON_DOCUMENT);
        assert!(missing_documents(&db, file, &HashMap::new()).is_empty());
    }

    /// Registering under the reported key is what the checker resolves.
    #[test]
    fn registering_what_is_missing_resolves_the_target_shape() {
        let mut db = new_db();
        let file = program(&db, "a/prog.fossil", "@warehouse/person.shex");
        let conns = connections(&[("warehouse", "https://w.example")]);
        for missing in missing_documents(&db, file, &conns) {
            assert_eq!(missing.locator, "https://w.example/person.shex");
            register_document(&mut db, &missing.key, PERSON_DOCUMENT);
        }
        assert!(resolves_target_shape(&db, file));
    }

    #[test]
    fn the_sync_loop_registers_under_the_key_the_checker_resolves() {
        let mut db = new_db();
        let file = program(&db, "a/prog.fossil", "s3://bucket/person.shex");
        assert_eq!(
            register_missing_documents(&mut db, file, &|_, _| Some(PERSON_DOCUMENT.to_string())),
            1
        );
        assert!(file_at(&db, "s3://bucket/person.shex").is_some());
        assert!(resolves_target_shape(&db, file));
    }

    /// An empty `.shex` and a missing one are different answers.
    #[test]
    fn a_host_that_cannot_read_registers_nothing() {
        let mut db = new_db();
        let file = program(&db, "prog.fossil", "shapes/person.shex");
        assert_eq!(register_missing_documents(&mut db, file, &|_, _| None), 0);
        assert!(file_at(&db, "shapes/person.shex").is_none());
    }

    /// The buffer already registered wins over anything `read` would produce,
    /// which is what makes the loop safe on `didChange`.
    #[test]
    fn an_already_registered_document_is_not_replaced() {
        let mut db = new_db();
        let file = program(&db, "prog.fossil", "shapes/person.shex");
        let buffer = SourceFile::new(
            &db,
            "the buffer the user is editing".to_string(),
            "shapes/person.shex".to_string(),
        );
        register_file(&mut db, "shapes/person.shex".to_string(), buffer);

        assert_eq!(
            register_missing_documents(&mut db, file, &|_, _| Some(PERSON_DOCUMENT.to_string())),
            0
        );
        assert_eq!(
            file_at(&db, "shapes/person.shex")
                .expect("still there")
                .text(&db),
            "the buffer the user is editing"
        );
    }
}
