//! **`mapping.ttl`**: the RML-Core mapping a run writes beside its graph, held
//! three ways.
//!
//! - **Golden**: the document for `examples/hello.fossil`, and for
//!   `docs/programs/labels` — a relation, a language-tagged set, a single
//!   language-tagged value, a date and the annotations.
//! - **Statement R** (`/docs/format/round-trip`): the triples that rudof's
//!   `rml::sql` makes of the mapping over the *written* Parquet, read by native
//!   `DuckDB`, are exactly the triples the program defines — written out by hand
//!   from the CSV in the program's directory, never from the mapping.
//! - **The document reads back**: `parse(write(m))` is `m`. A2, which refuses a
//!   graph whose image would be ambiguous, is held by `describe`'s own tests:
//!   a type's label is the local name of its class and a column's the local name
//!   of its predicate, so no program reaches an ambiguity a hand-built schema
//!   can.
//!
//! # What it does NOT prove
//!
//! - **That every RML processor reads the document the same way.** Only `rml`
//!   and its SQL rendering run here; the KG-Construct cases are `rml`'s own.
//! - **R2 on arbitrary graphs.** Two programs are read back, not a property
//!   over all admissible graphs.

use std::collections::BTreeSet;
use std::path::Path;

use expect_test::expect;
use graphar_parquet::paths;

#[path = "support/native.rs"]
mod native;
mod support;

const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const LANG_STRING: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#langString";
const XSD: &str = "http://www.w3.org/2001/XMLSchema#";

fn hello() -> native::Corpus {
    let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    native::run_dir(&examples, "hello.fossil", &[]).expect("hello.fossil runs")
}

fn labels() -> native::Corpus {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/programs/labels");
    native::run_dir(&dir, "labels.fossil", &[]).expect("labels.fossil runs")
}

fn mapping_text(corpus: &native::Corpus) -> String {
    String::from_utf8(
        corpus
            .files
            .get("mapping.ttl")
            .expect("the run wrote mapping.ttl")
            .clone(),
    )
    .expect("Turtle is UTF-8")
}

// ───────────────────────────────────────────────────────────────────── golden

/// The mapping of a written `mapping.ttl` in a form that does not depend on the
/// serialiser: the document is parsed, put in `rml`'s canonical order (an RDF
/// graph has none) and printed one line per fact. rudof's Turtle writer numbers
/// blank nodes and orders subjects by the iteration order of its in-memory
/// store, so two runs write the same graph in different bytes; the parsed
/// mapping is the same value.
fn render(text: &str) -> String {
    use std::fmt::Write as _;

    use rml::{
        Constant, Expression, Iterable, LanguageMap, LiteralKind, ObjectMap, TermMap, TermType,
    };

    fn constant(c: &Constant) -> String {
        match c {
            Constant::Iri(iri) => format!("<{}>", iri.as_str()),
            Constant::Literal(l) => match &l.kind {
                LiteralKind::Language(tag) => format!("{:?}@{tag}", l.lexical_form),
                LiteralKind::Datatype(dt) => format!("{:?}^^<{}>", l.lexical_form, dt.as_str()),
            },
        }
    }
    fn term(t: &TermMap) -> String {
        let mut out = match &t.expression {
            Expression::Constant(c) => constant(c),
            Expression::Reference(r) => format!("ref({r})"),
            Expression::Template(t) => format!("template({t})"),
        };
        match t.term_type {
            Some(TermType::Iri) => out.push_str(" as IRI"),
            Some(other) => write!(out, " as {other:?}").unwrap(),
            None => {}
        }
        if let Some(dt) = &t.datatype {
            write!(out, " ^^<{}>", dt.as_str()).unwrap();
        }
        match &t.language {
            Some(LanguageMap::Reference(r)) => write!(out, " @ref({r})").unwrap(),
            Some(LanguageMap::Constant(tag)) => write!(out, " @{tag}").unwrap(),
            None => {}
        }
        out
    }

    let mapping = rml::rdf::parse_turtle(text, None).expect("the mapping parses");
    let mut out = String::new();
    for m in &mapping.triples_maps {
        // The ids are fragments of the document; the parser resolves them
        // against a base that is not ours to print.
        writeln!(out, "{}", m.id.to_string().replace("string://", "")).unwrap();
        match &m.logical_source.iterable {
            Iterable::Sql2008Table(t) => writeln!(out, "  table {t}").unwrap(),
            Iterable::Sql2008Query(q) => writeln!(out, "  query {q}").unwrap(),
        }
        let classes: Vec<_> = m
            .subject_map
            .classes
            .iter()
            .map(|c| format!("<{}>", c.as_str()))
            .collect();
        writeln!(
            out,
            "  subject {}{}",
            term(&m.subject_map.term),
            if classes.is_empty() {
                String::new()
            } else {
                format!(" a {}", classes.join(", "))
            }
        )
        .unwrap();
        for pom in &m.predicate_object_maps {
            let predicates: Vec<_> = pom.predicates.iter().map(term).collect();
            for object in &pom.objects {
                let ObjectMap::Term(object) = object else {
                    panic!("fossil writes no referencing object map");
                };
                writeln!(out, "  {} -> {}", predicates.join(", "), term(object)).unwrap();
            }
        }
    }
    for a in &mapping.annotations {
        writeln!(
            out,
            "<{}> <{}> {}",
            a.subject.as_str(),
            a.predicate.as_str(),
            constant(&a.object)
        )
        .unwrap();
    }
    out
}

#[test]
fn hello_is_one_triples_map_over_its_table() {
    expect![[r"
        <#Person>
          table Person
          subject ref(subject) as IRI a <https://example.org/Person>
          <https://example.org/name> -> ref(name) ^^<http://www.w3.org/2001/XMLSchema#string>
    "]]
    .assert_eq(&render(&mapping_text(&hello())));
}

#[test]
fn labels_has_a_relation_a_tagged_set_a_tagged_value_and_its_annotations() {
    expect![[r#"
        <#Person>
          table Person
          subject ref(subject) as IRI a <https://schema.org/Person>
          <https://schema.org/birthDate> -> ref(birthDate) ^^<http://www.w3.org/2001/XMLSchema#date>
        <#Person_awarded_Prize>
          query SELECT s.subject AS source, d.subject AS target FROM "Person_awarded_Prize" AS e JOIN "Person" AS s ON e._graphArSrcIndex = s._graphArVertexIndex JOIN "Prize" AS d ON e._graphArDstIndex = d._graphArVertexIndex
          subject ref(source) as IRI
          <https://nobel.example/vocab/awarded> -> ref(target) as IRI
        <#Person_name>
          query SELECT subject, n.value AS value, n.lang AS lang FROM (SELECT subject, unnest("name") AS n FROM "Person")
          subject ref(subject) as IRI
          <https://schema.org/name> -> ref(value) @ref(lang)
        <#Prize>
          table Prize
          subject ref(subject) as IRI a <https://nobel.example/vocab/Prize>
          <https://nobel.example/vocab/category> -> ref(category) ^^<http://www.w3.org/2001/XMLSchema#string>
        <#Prize_motivation>
          query SELECT subject, n.value AS value, n.lang AS lang FROM (SELECT subject, unnest("motivation") AS n FROM "Prize")
          subject ref(subject) as IRI
          <https://nobel.example/vocab/motivation> -> ref(value) @ref(lang)
        <https://nobel.example/vocab/awarded> <http://www.w3.org/2000/01/rdf-schema#label> "awarded"@en
        <https://schema.org/Person> <http://www.w3.org/2000/01/rdf-schema#label> "Nobel laureate"@en
        <https://schema.org/Person> <http://www.w3.org/2000/01/rdf-schema#label> "galardonado con el Nobel"@es
        <https://schema.org/birthDate> <http://www.w3.org/2000/01/rdf-schema#label> "birth date"@en
        <https://schema.org/birthDate> <http://www.w3.org/2000/01/rdf-schema#label> "fecha de nacimiento"@es
    "#]].assert_eq(&render(&mapping_text(&labels())));
}

// ───────────────────────────────────────────────────────────── Statement R

type Triple = [String; 7];

/// A triple whose object is an IRI.
fn iri(s: &str, p: &str, o: &str) -> Triple {
    ["I", s, p, "I", o, "", ""].map(str::to_string)
}

/// A triple whose object is a literal of `datatype`.
fn literal(s: &str, p: &str, o: &str, datatype: &str) -> Triple {
    ["I", s, p, "L", o, datatype, ""].map(str::to_string)
}

/// A triple whose object is a language-tagged string.
fn tagged(s: &str, p: &str, o: &str, tag: &str) -> Triple {
    ["I", s, p, "L", o, LANG_STRING, tag].map(str::to_string)
}

/// The triples of the written `mapping.ttl` over the written Parquet: the
/// mapping parsed from its text, rendered to a view by `rml::sql` and read by
/// `DuckDB`, which resolves each table name the mapping uses to the chunk the
/// information files name.
fn triples(corpus: &native::Corpus) -> BTreeSet<Triple> {
    let root = corpus.materialise();
    let mapping = rml::rdf::parse_turtle(&mapping_text(corpus), None).expect("the mapping parses");
    let conn = duckdb::Connection::open_in_memory().expect("duckdb");
    let graph = corpus.graph();
    let view = |name: &str, relative: String| {
        conn.execute_batch(&format!(
            "CREATE VIEW \"{name}\" AS SELECT * FROM read_parquet({})",
            native::at(root.path(), &relative)
        ))
        .unwrap_or_else(|e| panic!("view {name}: {e}"));
    };
    for vertex in &graph.vertices {
        view(
            &vertex.r#type,
            paths::vertex_chunk(vertex, &vertex.property_groups[0], 0),
        );
    }
    for edge in &graph.edges {
        view(&edge.triple(), corpus.adjacency(&edge.triple()));
    }
    let create = rml::sql::create_view(&mapping, None, "triples", |t| Ok(format!("\"{t}\"")))
        .expect("the mapping renders");
    conn.execute_batch(&create)
        .unwrap_or_else(|e| panic!("{create}: {e}"));
    let mut stmt = conn.prepare("SELECT * FROM triples").expect("prepare");
    stmt.query_map([], |row| {
        let mut cells: Vec<String> = Vec::new();
        for i in 0..7 {
            cells.push(row.get(i)?);
        }
        Ok(<[String; 7]>::try_from(cells).expect("seven columns"))
    })
    .expect("query")
    .collect::<Result<_, _>>()
    .expect("rows")
}

#[test]
fn the_triples_of_hello_are_the_triples_of_its_program() {
    let person = "https://example.org/Person";
    let name = "https://example.org/name";
    let mut expected = BTreeSet::new();
    for (id, who) in [
        (1, "Alice"),
        (2, "Bob"),
        (3, "Carol"),
        (4, "Dave"),
        (5, "Eve"),
    ] {
        let s = format!("https://example.org/user/{id}");
        expected.insert(iri(&s, RDF_TYPE, person));
        expected.insert(literal(&s, name, who, &format!("{XSD}string")));
    }
    assert_eq!(triples(&hello()), expected);
}

#[test]
fn the_triples_of_labels_are_the_triples_of_its_program() {
    let person = "https://schema.org/Person";
    let prize = "https://nobel.example/vocab/Prize";
    let (name, born, awarded) = (
        "https://schema.org/name",
        "https://schema.org/birthDate",
        "https://nobel.example/vocab/awarded",
    );
    let (category, motivation) = (
        "https://nobel.example/vocab/category",
        "https://nobel.example/vocab/motivation",
    );
    let who = |id: &str| format!("https://nobel.example/laureate/{id}");
    let win = |id: &str| format!("https://nobel.example/prize/{id}");

    let mut expected = BTreeSet::new();
    for (id, date) in [("6", "1867-11-07"), ("26", "1879-03-14")] {
        expected.insert(iri(&who(id), RDF_TYPE, person));
        expected.insert(literal(&who(id), born, date, &format!("{XSD}date")));
    }
    for (id, value, tag) in [
        ("6", "Marie Curie", "en"),
        ("6", "Maria Skłodowska-Curie", "pl"),
        ("26", "Albert Einstein", "en"),
    ] {
        expected.insert(tagged(&who(id), name, value, tag));
    }
    for (id, laureate, kind, why) in [
        (
            "1903-physics",
            "6",
            "physics",
            "in recognition of the extraordinary services they have rendered by their joint researches",
        ),
        (
            "1911-chemistry",
            "6",
            "chemistry",
            "in recognition of her services to the advancement of chemistry",
        ),
        (
            "1921-physics",
            "26",
            "physics",
            "for his services to Theoretical Physics",
        ),
    ] {
        expected.insert(iri(&win(id), RDF_TYPE, prize));
        expected.insert(literal(&win(id), category, kind, &format!("{XSD}string")));
        expected.insert(tagged(&win(id), motivation, why, "en"));
        expected.insert(iri(&who(laureate), awarded, &win(id)));
    }
    assert_eq!(triples(&labels()), expected);
}

// ──────────────────────────────────────────────────────────── the document

#[test]
fn the_document_parses_and_writes_back_to_itself() {
    for corpus in [hello(), labels()] {
        let mapping =
            rml::rdf::parse_turtle(&mapping_text(&corpus), None).expect("the mapping parses");
        let written = rml::rdf::write_turtle(&mapping).expect("the mapping writes");
        assert_eq!(
            rml::rdf::parse_turtle(&written, None).expect("it parses again"),
            mapping,
            "parse(write(m)) is m"
        );
    }
}
