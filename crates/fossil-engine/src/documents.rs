//! The shape documents a program names, registered as Salsa inputs before the
//! checker runs.
//!
//! A program brings a shape document in with
//! `type { Person } = io.shex("shapes/person.shex")`. The checker decodes that
//! document through [`fossil_base::shape_document`], which is keyed by a
//! [`SourceFile`] **input** — so the document has to be IN the database before
//! any query goes looking for it, and putting it there is a host job:
//! [`fossil_base::register_file`] takes `&mut dyn Db`, which no query body can
//! have.
//!
//! That fixes the bug the input exists for. The checker used to read the
//! document with `System::read_file`, which registers no Salsa dependency:
//! editing the `.shex` re-ran nothing, and a document that was missing when the
//! query first asked stayed missing forever, because a cached miss had nothing
//! to invalidate it.
//!
//! # The order is forced
//!
//! parse → ask the def-map what the program names → read → register → check.
//! You cannot know which documents to register without parsing the program, and
//! registering bumps the registry's revision, so the `def_map` computed here is
//! re-derived once on the way to the check. That is one structural pass per
//! compile — the engine builds a fresh database per `check`/`run` anyway (see
//! [`crate::system::open_db`]) — and not per keystroke, which is the editor
//! hosts' concern and where the same loop is spelled once in `fossil-ide`.
//!
//! # Why there are two of these
//!
//! `fossil_ide::shape_documents` is this file's twin, and it is deliberate, not
//! an oversight: `fossil-lsp` and `fossil-wasm` share it, and the engine
//! cannot, because `fossil-engine` depending on the editor surface is the
//! arrangement that was undone deliberately — `fossil-lineage`'s module docs
//! name it as the mistake that crate exists to prevent.
//!
//! What the two twins no longer duplicate is the two questions the compiler
//! answers: WHICH documents a program names and under WHAT key each is looked
//! up. Both are [`fossil_hir::documents`], and this file's copies of them are
//! gone. `documents_named` was byte-identical in both; `registry_key` was not,
//! and the drift was live — the editor's half hand-rolled `parent().join()`,
//! which anchors a `s3://` URL and a `@conn` alias that the checker leaves
//! alone. The claim that stood here — «both are now the same call, so there is
//! nothing left to drift» — was true of this file and false of the pair, which
//! is exactly the kind of thing a docblock cannot hold. What holds it now is
//! that there is one function, and `fossil_hir::documents`' tests resolve a
//! target shape through it.

use std::path::Path;

use fossil_base::{Db, FossilDb, SourceFile, file_at, register_file};
use fossil_hir::documents::{documents_named, registry_key};

/// Read and register every shape document `file` names that the database does
/// not already hold.
///
/// A document already in the registry is left alone: registration is
/// idempotent by key, and re-reading the disk under a document somebody else
/// registered would replace it with a staler copy.
///
/// A document that cannot be read is skipped, not diagnosed. The compiler is
/// the side that knows a missing document is worth complaining about — it has
/// the span of the `io.shex("…")` that named it, and this loop has a path and
/// an `io::Error`. Skipping leaves `file_at` returning `None`, which is exactly
/// the state the checker's own diagnostic reads.
// `pub(crate)` and not `pub`: this module is private, so clippy calls the
// restriction redundant and rustc's `unreachable_pub` calls the unrestricted
// form unreachable. Only one of the two can be satisfied, and the rustc lint is
// the one this workspace opted into by name.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn register_shape_documents(db: &mut FossilDb, file: SourceFile) {
    // Read first, register second: `register_file` takes the database
    // exclusively and `System::read_file` borrows it shared.
    let pending: Vec<(String, String)> = documents_named(db, file)
        .into_iter()
        .filter_map(|document| {
            let key = registry_key(db, file, &document);
            if file_at(db, &key).is_some() {
                return None;
            }
            match db.system().read_file(Path::new(&key)) {
                Ok(bytes) => match String::from_utf8(bytes) {
                    Ok(text) => Some((key, text)),
                    Err(e) => {
                        tracing::debug!("shape document `{key}` is not UTF-8: {e}");
                        None
                    }
                },
                Err(e) => {
                    tracing::debug!("shape document `{key}` was not read: {e}");
                    None
                }
            }
        })
        .collect();

    for (key, text) in pending {
        let document = SourceFile::new(&*db, text, key.clone());
        register_file(db, key, document);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use fossil_base::{Diagnostic, System, shape_document};
    use salsa::Setter as _;

    use super::*;

    /// A program whose one mapping writes `name` from a CSV column, and which
    /// names its output shape document.
    ///
    /// `Person` is a local label bound positionally to the first shape the
    /// document declares — there is no CURIE and no prefix to expand. The
    /// property key is the bare name: `name`, the last segment of
    /// `http://example.org/name`, which is the predicate the shape below
    /// declares. The identity is the language slot `@subject`, first line of the
    /// body, and there is exactly one per type; its holes are ordinary
    /// expressions in a quoted string, and every column reference names the row
    /// it is drawn `from`.
    const PROGRAM: &str = "\
type { Person } := io.shex(\"person.shex\")
users := io.csv(\"users.csv\")
User : Person from users
    @subject = \"http://example.org/u/{users.id}\"
    name = users.name
";

    /// The same program with one character of one mapping body changed — an
    /// edit the checker must see and the decode must not.
    const PROGRAM_EDITED: &str = "\
type { Person } := io.shex(\"person.shex\")
users := io.csv(\"users.csv\")
User : Person from users
    @subject = \"http://example.org/v/{users.id}\"
    name = users.name
";

    /// `http://example.org/name` constrained to an integer — which the mapping
    /// above does not
    /// write, so the backward check has something to say.
    const DEMANDS_INTEGER: &str = r#"{
  "@context": "http://www.w3.org/ns/shex.jsonld",
  "type": "Schema",
  "shapes": [
    {
      "type": "ShapeDecl",
      "id": "http://example.org/Person",
      "shapeExpr": {
        "type": "Shape",
        "expression": {
          "type": "TripleConstraint",
          "predicate": "http://example.org/name",
          "valueExpr": {
            "type": "NodeConstraint",
            "datatype": "http://www.w3.org/2001/XMLSchema#integer"
          }
        }
      }
    }
  ]
}"#;

    /// The same shape with the constraint the mapping does satisfy.
    const DEMANDS_STRING: &str = r#"{
  "@context": "http://www.w3.org/ns/shex.jsonld",
  "type": "Schema",
  "shapes": [
    {
      "type": "ShapeDecl",
      "id": "http://example.org/Person",
      "shapeExpr": {
        "type": "Shape",
        "expression": {
          "type": "TripleConstraint",
          "predicate": "http://example.org/name",
          "valueExpr": {
            "type": "NodeConstraint",
            "datatype": "http://www.w3.org/2001/XMLSchema#string"
          }
        }
      }
    }
  ]
}"#;

    /// A program directory with the program, its shape document and the CSV.
    fn program_dir(shex: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("prog.fossil"), PROGRAM).expect("write program");
        std::fs::write(dir.path().join("person.shex"), shex).expect("write document");
        std::fs::write(dir.path().join("users.csv"), "id,name\n1,ada\n").expect("write csv");
        dir
    }

    /// A database over the engine host, counting the `WillExecute` events whose
    /// key mentions `query`.
    fn counting_db(dir: &Path, query: &'static str) -> (FossilDb, Arc<AtomicUsize>) {
        let executions: Arc<AtomicUsize> = Arc::new(AtomicUsize::new(0));
        let seen = executions.clone();
        let callback: Box<dyn Fn(salsa::Event) + Send + Sync + 'static> = Box::new(move |event| {
            if let salsa::EventKind::WillExecute { database_key } = event.kind
                && format!("{database_key:?}").contains(query)
            {
                seen.fetch_add(1, Ordering::SeqCst);
            }
        });
        let system: Arc<dyn System> = Arc::new(crate::system::EngineSystem::for_program_dir(dir));
        (FossilDb::with_event_callback(system, callback), executions)
    }

    /// Intern the program AND introspect its CSV, which is what gives `.name` a
    /// type — without it there is nothing for the shape's constraint to
    /// disagree with. `check` and `run` do the same, in the same order.
    fn intern(db: &FossilDb, dir: &Path) -> SourceFile {
        crate::pre_introspect_and_register(
            db.system(),
            PROGRAM,
            fossil_base::SourceAnchor::beside(dir),
            &std::collections::HashMap::new(),
        );
        SourceFile::new(
            db,
            PROGRAM.to_string(),
            dir.join("prog.fossil").to_string_lossy().into_owned(),
        )
    }

    /// Every mapping's diagnostics, drained the way `check` drains them.
    fn diagnostics(db: &FossilDb, file: SourceFile) -> Vec<String> {
        let def_map = fossil_hir::def_map::def_map(db, file);
        let mut out = Vec::new();
        for mapping in def_map.mappings(db) {
            let _ = fossil_mir::lower_to_mir_pg(db, *mapping);
            out.extend(
                fossil_mir::lower_to_mir_pg::accumulated::<Diagnostic>(db, *mapping)
                    .into_iter()
                    .map(|d| d.message.clone()),
            );
        }
        out
    }

    /// The registration itself: the document the program names lands under the
    /// key the compiler resolves, and decodes.
    #[test]
    fn the_document_the_program_names_is_registered_and_decodes() {
        let dir = program_dir(DEMANDS_INTEGER);
        let (mut db, _) = counting_db(dir.path(), "shape_document");
        let file = intern(&db, dir.path());
        register_shape_documents(&mut db, file);

        let key = registry_key(&db, file, "person.shex");
        let document = file_at(&db, &key).expect("the named document is registered");
        let shapes = shape_document(&db, document, "shex").expect("the `io.shex` row");
        assert!(
            shapes.lookup("http://example.org/Person").is_some(),
            "the decoded document carries the shape the program targets"
        );
    }

    /// A program that names nothing registers nothing.
    ///
    /// This used to read «the empty answer is a program with no output
    /// contract, not a failure», which is the rule the ruling of 2026-08-11
    /// replaced: naming a shape document is mandatory, and the refusal is
    /// [`fossil_hir::shapes::TargetShapeError::NoDocument`], raised where the
    /// mapping is checked. What is asserted here is unchanged and is this
    /// loop's whole contract — it registers what the program NAMES, and passes
    /// no judgment on a program that names none.
    #[test]
    fn a_program_that_names_no_document_registers_nothing() {
        let dir = program_dir(DEMANDS_INTEGER);
        let (db, _) = counting_db(dir.path(), "shape_document");
        let file = SourceFile::new(
            &db,
            "users := io.csv(\"users.csv\")\n".to_string(),
            dir.path()
                .join("prog.fossil")
                .to_string_lossy()
                .into_owned(),
        );
        assert!(documents_named(&db, file).is_empty());
    }

    /// **This is what the fan-out cost buys.** Editing the document's text —
    /// through the input, not the disk — re-runs the decode and the checker,
    /// and the diagnostic changes. With `System::read_file` the edit was
    /// invisible: nothing depended on it, so nothing re-ran.
    #[test]
    fn editing_the_document_rechecks_the_program_and_the_diagnostic_changes() {
        let dir = program_dir(DEMANDS_INTEGER);
        let (mut db, decodes) = counting_db(dir.path(), "shape_document");
        let file = intern(&db, dir.path());
        register_shape_documents(&mut db, file);
        let key = registry_key(&db, file, "person.shex");
        let document = file_at(&db, &key).expect("registered");

        let before = diagnostics(&db, file);
        assert!(
            before.iter().any(|m| m.contains("expects Integer")),
            "the shape demands an integer and the mapping writes a string, so \
             the backward check must say so; got {before:?}"
        );
        let decodes_when_warm = decodes.load(Ordering::SeqCst);
        assert!(decodes_when_warm >= 1, "the document was decoded");

        // The document changes to one the program satisfies.
        document.set_text(&mut db).to(DEMANDS_STRING.to_string());

        let after = diagnostics(&db, file);
        assert!(
            decodes.load(Ordering::SeqCst) > decodes_when_warm,
            "editing the document must re-execute the decode"
        );
        assert!(
            !after.iter().any(|m| m.contains("expects Integer")),
            "and the checker must have re-run against the edited document; \
             got {after:?}"
        );
    }

    /// The converse, and the one worth paying for: editing the PROGRAM re-runs
    /// the checker and does NOT re-run the decode. A dependency that fires on
    /// everything is not a dependency, and re-decoding every `.shex` on every
    /// keystroke is the cost this design would have if the document were read
    /// from the program's own query.
    #[test]
    fn editing_the_program_does_not_re_run_the_decode() {
        let dir = program_dir(DEMANDS_INTEGER);
        let (mut db, decodes) = counting_db(dir.path(), "shape_document");
        let file = intern(&db, dir.path());
        register_shape_documents(&mut db, file);

        let before = diagnostics(&db, file);
        let warm = decodes.load(Ordering::SeqCst);
        assert!(warm >= 1, "the document was decoded");

        file.set_text(&mut db).to(PROGRAM_EDITED.to_string());

        let after = diagnostics(&db, file);
        assert_eq!(
            decodes.load(Ordering::SeqCst),
            warm,
            "editing the program must not re-execute the decode"
        );
        assert_eq!(
            before, after,
            "and the shape it is checked against has not moved"
        );
    }
}
