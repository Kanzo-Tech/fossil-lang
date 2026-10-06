//! The shape documents a program names are Salsa INPUTS, registered before the
//! checker runs — and what that buys, measured in query executions.
//!
//! The checker decodes a document through
//! [`fossil_hir::shape_documents::shape_document`], which is keyed by a
//! [`SourceFile`] input, so the document has to be IN the database before any
//! query goes looking for it; putting it there is a host job
//! ([`fossil_base::register_file`] takes `&mut dyn Db`). The checker used to
//! read the document with `System::read_file`, which registers no dependency:
//! editing the `.shex` re-ran nothing, and a document missing when first asked
//! stayed missing forever. The host here is the check half of
//! `tests/support/native.rs`; the executor registers the same way through
//! `Executor::register_document`.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use fossil_base::{Db as _, Diagnostic, FossilDb, SourceFile, System, file_at};
use fossil_hir::documents::{documents_named, registry_key};
use fossil_hir::shape_documents::shape_document;
use salsa::Setter as _;

#[path = "support/native.rs"]
mod native;
mod support;

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

/// A database over the check host, counting the `WillExecute` events whose
/// key mentions `query`.
fn counting_db(query: &'static str) -> (FossilDb, Arc<AtomicUsize>) {
    let executions: Arc<AtomicUsize> = Arc::new(AtomicUsize::new(0));
    let seen = executions.clone();
    let callback: Box<dyn Fn(salsa::Event) + Send + Sync + 'static> = Box::new(move |event| {
        if let salsa::EventKind::WillExecute { database_key } = event.kind
            && format!("{database_key:?}").contains(query)
        {
            seen.fetch_add(1, Ordering::SeqCst);
        }
    });
    let system: Arc<dyn System> = Arc::new(native::check_host());
    (FossilDb::with_event_callback(system, callback), executions)
}

/// Intern the program AND introspect its CSV, which is what gives `.name` a
/// type — without it there is nothing for the shape's constraint to
/// disagree with. The check host does the same, in the same order.
fn intern(db: &FossilDb, dir: &Path) -> SourceFile {
    let file = SourceFile::new(
        db,
        PROGRAM.to_string(),
        dir.join("prog.fossil").to_string_lossy().into_owned(),
    );
    fossil_introspect::pre_introspect_and_register(
        db.system(),
        &fossil_lineage::program_sources(db, file, &std::collections::HashMap::new()),
        &std::collections::HashMap::new(),
        fossil_introspect::Reach::Anywhere,
    );
    file
}

fn expects_integer(p: &fossil_base::Problem) -> bool {
    matches!(p, fossil_base::Problem::PropertyMismatch { expected, .. } if expected == "Integer")
}

/// Every mapping's diagnostics, drained the way `check` drains them.
fn diagnostics(db: &FossilDb, file: SourceFile) -> Vec<fossil_base::Problem> {
    let def_map = fossil_hir::def_map::def_map(db, file);
    let mut out = Vec::new();
    for mapping in def_map.mappings(db) {
        let _ = fossil_mir::lower_to_mir_pg(db, *mapping);
        out.extend(
            fossil_mir::lower_to_mir_pg::accumulated::<Diagnostic>(db, *mapping)
                .into_iter()
                .map(|d| d.problem.clone()),
        );
    }
    out
}

/// The registration itself: the document the program names lands under the
/// key the compiler resolves, and decodes.
#[test]
fn the_document_the_program_names_is_registered_and_decodes() {
    let dir = program_dir(DEMANDS_INTEGER);
    let (mut db, _) = counting_db("shape_document");
    let file = intern(&db, dir.path());
    native::register_shape_documents(&mut db, file);

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
/// This loop's whole contract: it registers what the program NAMES, and
/// passes no judgment on a program that names none. Naming a shape document
/// is mandatory, but the refusal for omitting one is `lower_to_hir`'s, at the
/// mapping's header — not here.
#[test]
fn a_program_that_names_no_document_registers_nothing() {
    let dir = program_dir(DEMANDS_INTEGER);
    let (db, _) = counting_db("shape_document");
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
    let (mut db, decodes) = counting_db("shape_document");
    let file = intern(&db, dir.path());
    native::register_shape_documents(&mut db, file);
    let key = registry_key(&db, file, "person.shex");
    let document = file_at(&db, &key).expect("registered");

    let before = diagnostics(&db, file);
    assert!(
        before.iter().any(expects_integer),
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
        !after.iter().any(expects_integer),
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
    let (mut db, decodes) = counting_db("shape_document");
    let file = intern(&db, dir.path());
    native::register_shape_documents(&mut db, file);

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
