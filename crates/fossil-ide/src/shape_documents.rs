//! The shape documents a program names — the editor host's half of getting
//! them into the database.
//!
//! A program brings a shape document in with
//! `type { Person } = io.shex("shapes/person.shex")`. The checker decodes that
//! document through [`fossil_hir::shape_documents::shape_document`], which is keyed by a
//! [`SourceFile`] **input**, so the document has to be registered before any
//! query goes looking for it — and registering takes `&mut dyn Db`, which no
//! query body can have. That is why this is a host job and lives here rather
//! than in the compiler.
//!
//! # Why the editor needs this more than the CLI does
//!
//! The checker used to read the document with `System::read_file`, which
//! registers no Salsa dependency and goes to disk. In a batch compile that is
//! merely wasteful. In an editor it is two bugs: editing the `.shex` re-ran
//! nothing, so a diagnostic derived from it never cleared; and the disk is not
//! the truth for a document the user has open and has not saved.
//!
//! `fossil-cli` carries the same loop for the native hosts, because it may
//! not depend on the editor surface. What it does NOT carry any more is a
//! second answer to «which documents» and «under what key»: both are
//! [`fossil_hir::documents`], one function each, called from here and from
//! there. This module used to hold a copy of `documents_named` (byte-identical
//! to the engine's) and a hand-rolled `registry_key` that disagreed with the
//! checker's on a `://` scheme and on a `@conn` alias — silently, because a key
//! that stops matching reads exactly like a document nobody registered.
//!
//! Both are the same fix — the document is an input — and both halves are the
//! host's: [`register_missing_documents`] puts the ones nobody has opened in,
//! and a host that opens a buffer registers it under its own path, which
//! REPLACES whatever was read from disk. From then on the user's keystrokes in
//! the `.shex` are `set_text` on the input the checker reads, and the
//! diagnostics move with them.

use fossil_base::{Db, SourceFile, file_at, register_file};
use fossil_hir::documents::{documents_named, registry_key};

/// Register every document `file` names that the database does not already
/// hold, taking each one's text from `read`.
///
/// `read` is handed the [`registry_key`] and answers `None` for anything it
/// cannot produce — a host with no filesystem (the playground) answers `None`
/// to everything, and that is a real answer: the document simply is not there,
/// `file_at` says so, and registering it later invalidates the readers that
/// missed it. That last part is the whole reason the registry is a Salsa input.
///
/// **Already-registered documents are left alone.** The user's open buffer is
/// the truth; re-reading the disk under it on every keystroke would replace
/// their unsaved text with the saved copy, which is the editor bug in the
/// opposite direction.
///
/// Returns how many documents were registered — zero on every call after the
/// first, which is what makes this safe to call from `didChange`.
pub fn register_missing_documents(
    db: &mut dyn Db,
    file: SourceFile,
    read: &dyn Fn(&str) -> Option<String>,
) -> usize {
    // Resolve and read first, register second: `register_file` takes the
    // database exclusively.
    let pending: Vec<(String, String)> = documents_named(&*db, file)
        .into_iter()
        .filter_map(|document| {
            let key = registry_key(&*db, file, &document);
            if file_at(&*db, &key).is_some() {
                return None;
            }
            read(&key).map(|text| (key, text))
        })
        .collect();

    let registered = pending.len();
    for (key, text) in pending {
        let document = SourceFile::new(&*db, text, key.clone());
        register_file(db, key, document);
    }
    registered
}

// The `.fossil` source below carries a `{users.id}` interpolation hole and
// `type { … }` braces — LITERAL Fossil source, not Rust format-string args.
#[allow(clippy::literal_string_with_formatting_args)]
#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use fossil_base::test_support::{PERSON_DOCUMENT, new_db};

    use super::*;

    /// A program naming its shape document with `document`, and writing the one
    /// property [`PERSON_DOCUMENT`] declares. `Person` binds POSITIONALLY to
    /// the first shape the document declares.
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

    /// The shape the checker resolves for `file`'s one mapping, or `None` when
    /// the document it named did not reach it — which is what a key mismatch
    /// looks like from here, and is indistinguishable from having registered
    /// nothing.
    fn resolves_target_shape(db: &fossil_base::FossilDb, file: SourceFile) -> bool {
        let mapping = *fossil_hir::def_map::def_map(db, file)
            .mappings(db)
            .first()
            .expect("the program has one mapping");
        matches!(
            fossil_hir::shapes::resolve_target_shape(db, mapping),
            Ok(Some(_))
        )
    }

    #[test]
    fn registration_reaches_the_key_the_compiler_resolves() {
        let mut db = new_db();
        let file = program(&db, "a/prog.fossil", "shapes/person.shex");
        assert!(file_at(&db, "a/shapes/person.shex").is_none());

        let registered =
            register_missing_documents(&mut db, file, &|_| Some(PERSON_DOCUMENT.to_string()));

        assert_eq!(registered, 1);
        let document = file_at(&db, "a/shapes/person.shex").expect("registered under the key");
        assert_eq!(document.text(&db), PERSON_DOCUMENT);
        assert!(resolves_target_shape(&db, file));
    }

    /// **The defect this module's docblock used to deny.** `registry_key` was a
    /// hand-rolled `parent().join()` here, so a document named by URL was
    /// registered under `a/s3://bucket/person.shex` while the checker asked for
    /// `s3://bucket/person.shex`. Nothing failed: the checker reported the
    /// document as unregistered, and target-side hover and shape-property
    /// completion went empty for every program naming its document this way.
    #[test]
    fn a_document_named_by_url_is_registered_where_the_checker_looks() {
        let mut db = new_db();
        let file = program(&db, "a/prog.fossil", "s3://bucket/person.shex");

        let registered =
            register_missing_documents(&mut db, file, &|_| Some(PERSON_DOCUMENT.to_string()));

        assert_eq!(registered, 1);
        assert!(
            file_at(&db, "s3://bucket/person.shex").is_some(),
            "a reference carrying a scheme is already a locator and is anchored \
             to nothing"
        );
        assert!(resolves_target_shape(&db, file));
    }

    /// The same, for the other reference the anchoring never touches: a `@conn`
    /// alias this host cannot expand passes through verbatim.
    #[test]
    fn a_document_named_through_a_connection_alias_is_registered_where_the_checker_looks() {
        let mut db = new_db();
        let file = program(&db, "a/prog.fossil", "@warehouse/person.shex");

        let registered =
            register_missing_documents(&mut db, file, &|_| Some(PERSON_DOCUMENT.to_string()));

        assert_eq!(registered, 1);
        assert!(file_at(&db, "@warehouse/person.shex").is_some());
        assert!(resolves_target_shape(&db, file));
    }

    /// A host with nothing to read registers nothing, and says so rather than
    /// registering an empty document — an empty `.shex` and a missing one are
    /// different answers.
    #[test]
    fn a_host_that_cannot_read_registers_nothing() {
        let mut db = new_db();
        let file = program(&db, "prog.fossil", "shapes/person.shex");
        assert_eq!(register_missing_documents(&mut db, file, &|_| None), 0);
        assert!(file_at(&db, "shapes/person.shex").is_none());
    }

    /// The second call is a no-op, which is what makes it safe on `didChange`.
    /// The buffer already registered wins over anything `read` would produce.
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
            register_missing_documents(&mut db, file, &|_| Some(PERSON_DOCUMENT.to_string())),
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
