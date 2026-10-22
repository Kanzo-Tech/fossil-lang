//! A `Seq` is several values, and a property its shape allows once takes one —
//! whether or not the shape narrows its datatype.
//!
//! A narrowed slot refuses `Seq<String>` by subtyping alone. An un-narrowed one
//! (`-`, `ShEx`'s `.`) takes any value, and before this rule it took a list too:
//! the run then refused it as a type `GraphAr` cannot hold, true and pointing at
//! no line of the program.

use fossil_base::Diagnostic;
use fossil_base::test_support::{db_with_document, register_inferred};
use fossil_graph_schema::Primitive;
use fossil_hir::body::body;
use fossil_hir::check::typecheck_mapping;
use fossil_hir::def_map::def_map;

fn diagnostics(occurs: &str) -> Vec<String> {
    let document = format!(
        "shape http://example.org/Persona\n\
         prop http://example.org/name - 1 1\n\
         prop http://example.org/apodo - {occurs}\n"
    );
    let program = "type { Persona } := io.shex(\"v.shex\")\n\
         personas := io.csv(\"p.csv\")\n\
         P : Persona from personas\n    \
         @subject = \"https://example.org/p/{personas.id}\"\n    \
         name = personas.nombre\n    \
         apodo = str.split(personas.nombre, \" \")\n";
    let (mut db, file) = db_with_document(program, "v.shex", &document);
    register_inferred(
        &mut db,
        "p.csv",
        &[("id", Primitive::Integer), ("nombre", Primitive::STRING)],
    );
    let mapping = *def_map(&db, file)
        .mappings(&db)
        .first()
        .expect("one mapping");
    typecheck_mapping(&db, mapping)
        .diagnostics(&db)
        .iter()
        .chain(body(&db, mapping).diagnostics(&db))
        .map(Diagnostic::message)
        .collect()
}

#[test]
fn a_seq_into_an_unnarrowed_single_valued_property_is_refused() {
    assert_eq!(
        diagnostics("0 1"),
        ["`apodo` expects one value, and this is Seq<String>"]
    );
}

/// The rule is the upper bound's: a property that holds several takes a list,
/// whose elements the writer keeps as a set.
#[test]
fn a_seq_into_an_unnarrowed_multi_valued_property_is_accepted() {
    assert_eq!(diagnostics("0 *"), Vec::<String>::new());
}
