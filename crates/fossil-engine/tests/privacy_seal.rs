//! The bound, end to end: a real program, a real policy, a real corpus on disk.
//!
//! The unit tests in `fossil-df` measure the arithmetic against hand-built
//! `RecordBatch`es. This one asks the question those cannot: does
//! `fossil run` actually refuse, and **is the disk empty afterwards?** The
//! design's whole claim is that protection is total at write time because the
//! offending bytes are never written, and a refusal that leaves a half-written
//! tree behind would falsify it without failing anything else.
//!
//! Read back with plain SQL and a line scan, never through `fossil_sinks`'s
//! structs — the same rule `conformance.rs` states at length. A recipient has a
//! Parquet reader and a text editor.

#![cfg(not(target_arch = "wasm32"))]

use std::fmt::Write as _;
use std::path::Path;

use duckdb::Connection;

/// Enough people for classes worth counting, and few enough to run fast.
const PEOPLE: u32 = 600;

/// One vertex type with two quasi-identifiers and one sensitive column.
///
/// `postcode` takes 10 values and `birth_year` 6, and the classes are the
/// distinct PAIRS — of which there are `lcm(6, 10) = 30` rather than 60, because
/// `i % 6` and `i % 10` move together on the shared factor of 2. So 600 people
/// fall into 30 classes of 20, which is comfortably over a `k` of 5 and
/// comfortably under one of 25.
///
/// The first draft of this comment said 60 and 10, which is what multiplying the
/// two counts gives and is not what correlated keys do. The test caught it,
/// which is the argument for asserting the reached number rather than only the
/// pass.
const PROGRAM: &str = "\
type { Person } := io.shex(\"person.shex\")

Users := io.csv(\"users.csv\")

People : Person from Users
    @subject = \"https://example.org/person/{Users.id}\"
    birthYear = Users.birth_year
    postcode = Users.postcode
    diagnosis = Users.diagnosis
";

const SHAPE: &str = r#"{
  "@context": "http://www.w3.org/ns/shex.jsonld",
  "type": "Schema",
  "shapes": [
    {
      "type": "ShapeDecl",
      "id": "https://example.org/Person",
      "shapeExpr": {
        "type": "Shape",
        "expression": {
          "type": "EachOf",
          "expressions": [
            { "type": "TripleConstraint", "predicate": "https://example.org/birthYear",
              "valueExpr": { "type": "NodeConstraint",
                             "datatype": "http://www.w3.org/2001/XMLSchema#integer" } },
            { "type": "TripleConstraint", "predicate": "https://example.org/postcode",
              "valueExpr": { "type": "NodeConstraint",
                             "datatype": "http://www.w3.org/2001/XMLSchema#string" } },
            { "type": "TripleConstraint", "predicate": "https://example.org/diagnosis",
              "valueExpr": { "type": "NodeConstraint",
                             "datatype": "http://www.w3.org/2001/XMLSchema#string" } }
          ]
        }
      }
    }
  ]
}"#;

/// The policy, in the shape the documentation shows.
///
/// It classifies `diagnosis` as sensitive and never puts it in the
/// quasi-identifier set, which is the asymmetry the whole design turns on: the
/// bound is checked over `birthYear` and `postcode` and the sensitive column is
/// never read.
fn policy(k: u64) -> String {
    format!(
        r#"{{
  "@context": ["http://www.w3.org/ns/odrl.jsonld",
               {{"fossil": "https://fossil-lang.org/ns/privacy#",
                 "dpv": "https://w3id.org/dpv#"}}],
  "@type": "Set",
  "uid": "https://example.org/policies/people-v1",
  "profile": "https://fossil-lang.org/ns/privacy/v1",
  "permission": [{{
    "target": "Person",
    "action": "use",
    "constraint": [
      {{"leftOperand": "fossil:anonymityK", "operator": "gteq", "rightOperand": {k}}},
      {{"leftOperand": "fossil:absentQuasiIdentifier", "operator": "eq", "rightOperand": "value"}},
      {{"and": [
        {{"leftOperand": "fossil:attribute", "operator": "eq",
          "rightOperand": "https://example.org/birthYear"}},
        {{"leftOperand": "fossil:classification", "operator": "eq",
          "rightOperand": "fossil:QuasiIdentifier"}}]}},
      {{"and": [
        {{"leftOperand": "fossil:attribute", "operator": "eq",
          "rightOperand": "https://example.org/postcode"}},
        {{"leftOperand": "fossil:classification", "operator": "eq",
          "rightOperand": "fossil:QuasiIdentifier"}}]}},
      {{"and": [
        {{"leftOperand": "fossil:attribute", "operator": "eq",
          "rightOperand": "https://example.org/diagnosis"}},
        {{"leftOperand": "fossil:classification", "operator": "eq",
          "rightOperand": "dpv:SensitivePersonalData"}}]}}
    ]
  }}]
}}"#
    )
}

fn write_fixture(dir: &Path) {
    let mut users = String::from("id,birth_year,postcode,diagnosis\n");
    for i in 0..PEOPLE {
        let _ = writeln!(users, "{i},{},PC{},d{}", 1950 + (i % 6), i % 10, i % 3);
    }
    std::fs::write(dir.join("users.csv"), users).expect("write users.csv");
    std::fs::write(dir.join("person.shex"), SHAPE).expect("write shape");
    std::fs::write(dir.join("mapping.fossil"), PROGRAM).expect("write mapping");
}

fn introspect(path: &Path) {
    let system = fossil_engine::host_system(path);
    let _ = fossil_introspect::introspect_program(
        &*system,
        path,
        &std::collections::HashMap::new(),
        &fossil_introspect::RunCreds::default(),
    );
}

fn run(
    dir: &Path,
    dest: &Path,
    policy: Option<&fossil_policy::PrivacyPolicy>,
) -> miette::Result<()> {
    introspect(&dir.join("mapping.fossil"));
    fossil_engine::run(
        &dir.join("mapping.fossil"),
        &format!("file://{}", dest.display()),
        &std::collections::HashMap::new(),
        None,
        policy,
    )
    .map(|_| ())
}

/// The `privacy:` block of `graph.graph.yml`, by line scan — the way a
/// recipient reads it, and not through the struct that wrote it.
fn privacy_block(dest: &Path) -> Vec<String> {
    let text = std::fs::read_to_string(dest.join("graph.graph.yml")).expect("read graph.graph.yml");
    let mut lines = text.lines().skip_while(|l| *l != "privacy:");
    lines.next().expect("a privacy: key");
    lines
        .take_while(|l| l.starts_with("  "))
        .map(|l| l.trim().to_string())
        .collect()
}

#[test]
fn a_run_with_no_policy_says_so_on_the_artifact() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_fixture(dir.path());
    let dest = dir.path().join("out");
    run(dir.path(), &dest, None).expect("a run with no policy is still a run");

    // Not an omission. `undeclared` is a claim a recipient can read, and it is
    // what forgetting the policy produces — which is the only reason forgetting
    // is survivable while the binding is a host argument.
    assert_eq!(privacy_block(&dest), vec!["bound: undeclared".to_string()]);
}

#[test]
fn a_release_that_clears_the_bound_is_sealed_with_numbers_a_stranger_can_recompute() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_fixture(dir.path());
    let dest = dir.path().join("out");
    let policy = fossil_policy::parse(&policy(5)).expect("a valid policy");
    run(dir.path(), &dest, Some(&policy)).expect("the bound holds");

    let block = privacy_block(&dest);
    let has = |s: &str| block.iter().any(|l| l == s);
    assert!(has("bound: k-anonymity"), "{block:?}");
    assert!(has("k: 5"), "{block:?}");
    assert!(has("reached: 20"), "{block:?}");
    assert!(has("population: 600"), "{block:?}");
    assert!(has("suppressed: 0"), "{block:?}");
    assert!(has("absent_quasi_identifier: value"), "{block:?}");
    assert!(
        has("quasi_identifiers: Person.birthYear Person.postcode"),
        "{block:?}"
    );
    assert!(
        has("policy: https://example.org/policies/people-v1"),
        "{block:?}"
    );

    // And the number is re-derivable from the bytes by somebody who has none of
    // this — which is what `apps/corpus`'s `declared-privacy` does in
    // JavaScript and what this does in SQL. Two implementations, one artefact.
    let conn = Connection::open_in_memory().expect("duckdb");
    let tiles = dest.join("vertex/Person/tiles.parquet");
    let smallest: i64 = conn
        .query_row(
            &format!(
                "SELECT min(n) FROM (SELECT count(*) AS n FROM read_parquet('{}') \
                 GROUP BY \"birthYear\", postcode)",
                tiles.display()
            ),
            [],
            |row| row.get(0),
        )
        .expect("recompute k");
    assert_eq!(smallest, 20, "the manifest and the bytes must agree");

    // The sensitive column is in the corpus and was never part of the check.
    // That is not a gap: verifying k needs the quasi-identifiers and not the
    // sensitive attribute, which is what lets an auditor check compliance
    // without entitlement to the data.
    let columns: i64 = conn
        .query_row(
            &format!(
                "SELECT count(*) FROM (DESCRIBE SELECT * FROM read_parquet('{}')) \
                 WHERE column_name = 'diagnosis'",
                tiles.display()
            ),
            [],
            |row| row.get(0),
        )
        .expect("describe");
    assert_eq!(columns, 1);
}

/// **The assertion the whole design rests on.**
///
/// A release that misses the bound is refused, and the refusal leaves nothing
/// on disk. If a half-written tree survived here, "the bytes that would violate
/// the bound are never written" would be false and every downstream guarantee
/// would go with it — a recipient handed a partial corpus reads every column of
/// it exactly as easily as a whole one.
#[test]
fn a_release_that_misses_the_bound_is_refused_and_writes_nothing() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_fixture(dir.path());
    let dest = dir.path().join("out");
    // 20 to a class, and 25 asked for.
    let policy = fossil_policy::parse(&policy(25)).expect("a valid policy");
    let err = run(dir.path(), &dest, Some(&policy)).expect_err("the bound does not hold");

    let message = format!("{err:?}");
    assert!(message.contains("k=20"), "{message}");
    assert!(message.contains("k=25"), "{message}");
    // The diagnostic names counts and columns and never a value. A refusal that
    // printed the offending row would have moved the disclosure into the log.
    assert!(!message.contains("PC1"), "{message}");
    assert!(!message.contains("d0"), "{message}");

    assert!(
        !dest.join("graph.graph.yml").exists(),
        "a refused release must leave no manifest"
    );
    assert!(
        !dest.join("vertex").exists(),
        "a refused release must leave no payload"
    );
}
