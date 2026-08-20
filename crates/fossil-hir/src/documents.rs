//! The shape documents a program names, and the key each one is looked up
//! under — **one function per question, for the checker and every host at
//! once**.
//!
//! A program brings a shape document in with
//! `type { Person } := io.shex("shapes/person.shex")`. The document is a Salsa
//! **input** ([`fossil_base::shape_document`] is keyed by a
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
//! [`fossil_engine::documents`] and [`fossil_ide::shape_documents`] each
//! carried a copy of [`documents_named`], and `fossil-ide`'s [`registry_key`]
//! was a hand-rolled `parent().join()` that disagreed with this one on two live
//! cases: a document named through a scheme (`s3://bucket/x.shex` came back as
//! `/programs/s3://bucket/x.shex`) and one named through a `@conn` alias. Both
//! files carried a docblock asserting the drift was closed. It was not, and no
//! test held it — which is why there is one function now and not a claim.
//!
//! Both hosts already depend on this crate, so nothing forced the copies.

use fossil_base::{Db, SourceFile};
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
/// the compiler's business — a `.csvw.json` named here is a file the compiler
/// may read and no shape decoder will, and that is the correct division: this
/// function parses nothing to decide.
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
/// program wrote, put through [`fossil_base::SourceAnchor`] — the one
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
    let dir = fossil_base::program_dir(file.path(db));
    fossil_base::SourceAnchor::beside(&dir).locator(document)
}

// The `.fossil` sources below carry a `{users.id}` interpolation hole and
// `type { … }` braces — LITERAL Fossil source, not Rust format-string args.
#[allow(clippy::literal_string_with_formatting_args)]
#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    use fossil_base::test_support::{PERSON_DOCUMENT, new_db, register_document};

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

    /// And the whole point of the two above: a document registered under
    /// [`registry_key`] is a document the CHECKER resolves. This is the
    /// end-to-end statement the two docblocks used to make in prose — a key
    /// mismatch is indistinguishable from a document nobody registered, so
    /// only resolving the target shape can tell them apart.
    #[test]
    fn a_document_registered_by_url_resolves_the_target_shape() {
        let mut db = new_db();
        let file = program(&db, "/programs/prog.fossil", "s3://bucket/person.shex");
        let key = registry_key(&db, file, "s3://bucket/person.shex");
        register_document(&mut db, &key, PERSON_DOCUMENT);

        let mapping = *crate::def_map::def_map(&db, file)
            .mappings(&db)
            .first()
            .expect("the program has one mapping");
        let resolved = crate::shapes::resolve_target_shape(&db, mapping)
            .expect("the document declares the mapping's target shape")
            .expect("the program names a document, so there is something to check against");
        assert!(
            resolved.constraint_for("http://example.org/name").is_some(),
            "the shape the checker resolved is the document that was registered"
        );
    }
}
