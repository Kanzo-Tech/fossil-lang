//! `FossilProgram.mapping()`, the Rust half: the RML-Core mapping a program's
//! types describe, with no run and no description of its sources.
//!
//! The mapping's content is `fossil_mir::describe`'s, and `fossil-df`'s
//! `tests/mapping.rs` holds it to the document a run writes; this holds that the
//! workspace feeds it the types the program declares and imports.

use fossil_wasm::FossilWorkspace;

const PROGRAM: &str = r#"
@iri("https://schema.org/Person")
type Laureate
    @iri("https://schema.org/name") name: String
    @iri("https://nobel.example/vocab/awarded") awarded: Seq<Ref<Prize>>

@iri("https://nobel.example/vocab/Prize")
type Prize
    @iri("https://nobel.example/vocab/category") category: String
"#;

fn mapping(program: &str) -> Result<String, fossil_graph_schema::Failure> {
    let mut ws = FossilWorkspace::new();
    let handle = ws.open_file_native("declared.fossil".into(), program.into());
    ws.mapping_native(handle).expect("the handle is open")
}

#[test]
fn the_declared_types_are_described_with_no_source_read() {
    let turtle = mapping(PROGRAM).expect("the types describe");
    for want in [
        "https://schema.org/Person",
        "https://schema.org/name",
        "https://nobel.example/vocab/category",
        "Person_awarded_Prize",
    ] {
        assert!(turtle.contains(want), "`{want}` is not in:\n{turtle}");
    }
}

#[test]
fn a_program_with_no_type_describes_nothing() {
    let turtle = mapping("").expect("nothing to describe");
    assert!(!turtle.contains("Person"), "{turtle}");
}

#[test]
fn two_columns_that_share_an_iri_are_refused_as_a_run_would() {
    let clash = PROGRAM.replace(
        "https://nobel.example/vocab/awarded",
        "https://schema.org/name",
    );
    let failure = mapping(&clash).expect_err("A2 refuses it");
    assert_eq!(failure.problem.code(), "write/ambiguous-iri");
}

#[test]
fn an_unknown_handle_is_none() {
    let mut ws = FossilWorkspace::new();
    let handle = ws.open_file_native("a.fossil".into(), String::new());
    ws.close_file_native(handle).expect("closes");
    assert!(ws.mapping_native(handle).is_none());
}
