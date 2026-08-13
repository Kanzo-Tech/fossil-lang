//! The shape documents a program names — the editor host's half of getting
//! them into the database.
//!
//! A program brings a shape document in with
//! `type { Person } = io.shex("shapes/person.shex")`. The checker decodes that
//! document through [`fossil_base::shape_document`], which is keyed by a
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
//! `fossil-engine` carries a twin of this module for the native hosts, because
//! it may not depend on the editor surface. The half that must
//! agree with `fossil-hir` is [`registry_key`], and the tests here and there
//! are what hold it: a key that stops matching reads exactly like a document
//! nobody registered.
//!
//! Both are the same fix — the document is an input — and both halves are the
//! host's: [`register_missing_documents`] puts the ones nobody has opened in,
//! and a host that opens a buffer registers it under its own path, which
//! REPLACES whatever was read from disk. From then on the user's keystrokes in
//! the `.shex` are `set_text` on the input the checker reads, and the
//! diagnostics move with them.

use fossil_base::{Db, SourceFile, file_at, register_file};
use smol_str::SmolStr;

/// Every distinct document `file` names, in the order it names them.
///
/// Two places name one: [`fossil_hir::def_map::TypeEntry::document`] — the
/// string inside `type { … } = io.shex("…")` — and
/// [`fossil_hir::def_map::SourceEntry::schema_arg`], the `schema = "…"` of a
/// source. Both reach the compiler through [`fossil_base::file_at`] now
/// (`def_map`'s positional type binding and `infer`'s RDF member row), so a
/// loop that registered only the first would leave
/// `{ A, B } := io.rdf(…, schema = "x.shex")` resolving nothing.
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
    let def_map = fossil_hir::def_map::def_map(db, file);
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

/// The key the compiler looks a document up under: the path the program writes,
/// resolved against the path of the program that writes it.
///
/// **It must equal what the reader passes to [`fossil_base::file_at`]**, which
/// is `fossil_hir::def_map::resolve_relative` rendered as a string. That
/// function is `pub(crate)` to `fossil-hir`, so this is a mirror of it, not a
/// call to it; the tests below and `fossil-engine`'s twin are what stop the two
/// drifting, because a mismatched key reads exactly like a document that was
/// never registered.
///
/// It is a KEY and not a locator. The LSP's file paths are `file://` URIs, so
/// the key for `prog.fossil`'s `person.shex` is `file:///…/person.shex` — a
/// string no filesystem will open, and the right thing to key by anyway, since
/// the editor's own `didOpen` for that document arrives under the same URI.
/// Reading the bytes is the caller's business.
#[must_use]
pub fn registry_key(db: &dyn Db, file: SourceFile, document: &str) -> String {
    let program = std::path::PathBuf::from(file.path(db));
    program
        .parent()
        .map_or_else(
            || std::path::PathBuf::from(document),
            |dir| dir.join(document),
        )
        .to_string_lossy()
        .into_owned()
}

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

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use std::sync::Arc;

    use fossil_base::{FossilDb, NativeSystem, System};

    use super::*;

    const NAMES_A_DOCUMENT: &str = "\
prefix ex: <http://example.org/>
type { Person } = io.shex(\"shapes/person.shex\")
users := io.csv(\"users.csv\")
User : ex:Person from users
    iri = `${ex:}u/${.id}`
    ex:name = .name
";

    fn db() -> FossilDb {
        let system: Arc<dyn System> = Arc::new(NativeSystem::default());
        FossilDb::new(system)
    }

    fn program(db: &FossilDb, path: &str) -> SourceFile {
        SourceFile::new(db, NAMES_A_DOCUMENT.to_string(), path.to_string())
    }

    #[test]
    fn a_program_names_the_document_in_its_type_binding() {
        let db = db();
        let file = program(&db, "prog.fossil");
        assert_eq!(documents_named(&db, file), ["shapes/person.shex"]);
    }

    #[test]
    fn a_program_that_names_none_yields_none() {
        let db = db();
        let file = SourceFile::new(
            &db,
            "users := io.csv(\"users.csv\")\n".to_string(),
            "prog.fossil".to_string(),
        );
        assert!(documents_named(&db, file).is_empty());
    }

    /// The key is resolved against the PROGRAM's path, whatever shape that path
    /// has — a bare name, a directory, or the `file://` URI the LSP keys by.
    #[test]
    fn the_key_is_the_document_resolved_against_the_program() {
        let db = db();
        assert_eq!(
            registry_key(&db, program(&db, "prog.fossil"), "shapes/person.shex"),
            "shapes/person.shex",
            "a program with no directory resolves to the document as written"
        );
        assert_eq!(
            registry_key(&db, program(&db, "a/b/prog.fossil"), "person.shex"),
            "a/b/person.shex"
        );
        assert_eq!(
            registry_key(
                &db,
                program(&db, "file:///w/prog.fossil"),
                "shapes/person.shex"
            ),
            "file:///w/shapes/person.shex",
            "the URI the editor opened the program under keys the document too"
        );
    }

    #[test]
    fn registration_reaches_the_key_the_compiler_resolves() {
        let mut db = db();
        let file = program(&db, "a/prog.fossil");
        assert!(file_at(&db, "a/shapes/person.shex").is_none());

        let registered =
            register_missing_documents(&mut db, file, &|_| Some("ex:Person {}".to_string()));

        assert_eq!(registered, 1);
        let document = file_at(&db, "a/shapes/person.shex").expect("registered under the key");
        assert_eq!(document.text(&db), "ex:Person {}");
    }

    /// A host with nothing to read registers nothing, and says so rather than
    /// registering an empty document — an empty `.shex` and a missing one are
    /// different answers.
    #[test]
    fn a_host_that_cannot_read_registers_nothing() {
        let mut db = db();
        let file = program(&db, "prog.fossil");
        assert_eq!(register_missing_documents(&mut db, file, &|_| None), 0);
        assert!(file_at(&db, "shapes/person.shex").is_none());
    }

    /// The second call is a no-op, which is what makes it safe on `didChange`.
    /// The buffer already registered wins over anything `read` would produce.
    #[test]
    fn an_already_registered_document_is_not_replaced() {
        let mut db = db();
        let file = program(&db, "prog.fossil");
        let buffer = SourceFile::new(
            &db,
            "the buffer the user is editing".to_string(),
            "shapes/person.shex".to_string(),
        );
        register_file(&mut db, "shapes/person.shex".to_string(), buffer);

        assert_eq!(
            register_missing_documents(&mut db, file, &|_| Some("the saved copy".to_string())),
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
