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
/// Multiplying the two counts gives 60 and is not what correlated keys do,
/// which is the argument for asserting the reached `k` rather than only the pass.
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

/// The same policy, with a generalisation hierarchy on each quasi-identifier.
///
/// Both are pasted from `crates/fossil-kanon/hierarchies/` in shape — a
/// `numeric` with declared buckets and `enclosing_bucket` presentation, and a
/// `prefix` over ascending lengths. The buckets are in this fixture's units
/// (`1950 + i % 6`) rather than the shipped age file's, because the column is a
/// birth year and not an age; the shape of the declaration is the thing being
/// exercised.
fn generalising_policy(k: u64) -> String {
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
          "rightOperand": "fossil:QuasiIdentifier"}},
        {{"leftOperand": "fossil:generalization", "operator": "eq",
          "rightOperand": {{"kind": "numeric", "buckets": [1950, 1953, 1956],
                            "presentation": "enclosing_bucket"}}}}]}},
      {{"and": [
        {{"leftOperand": "fossil:attribute", "operator": "eq",
          "rightOperand": "https://example.org/postcode"}},
        {{"leftOperand": "fossil:classification", "operator": "eq",
          "rightOperand": "fossil:QuasiIdentifier"}},
        {{"leftOperand": "fossil:generalization", "operator": "eq",
          "rightOperand": {{"kind": "prefix", "lengths": [1, 2, 3]}}}}]}},
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

/// The same program, binding its own policy document (grammar.bnf, `PolicyDef`).
///
/// One line, and it is the whole of the difference between an obligation an
/// operator has to remember and one that lives in the file under review.
const PROGRAM_BINDING_A_POLICY: &str = "\
policy := \"people.jsonld\"

type { Person } := io.shex(\"person.shex\")

Users := io.csv(\"users.csv\")

People : Person from Users
    @subject = \"https://example.org/person/{Users.id}\"
    birthYear = Users.birth_year
    postcode = Users.postcode
    diagnosis = Users.diagnosis
";

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
    let system = fossil_cli::host_system(path);
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
    fossil_cli::run(
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
    // what naming no policy produces — by either of the two ways of naming one.
    // A program that binds none and a run with no `--policy` is a release whose
    // producer said nothing, and saying nothing has to be legible.
    assert_eq!(privacy_block(&dest), vec!["bound: undeclared".to_string()]);
}

// ─── The binding, against the flag ──────────────────────────────────────────
//
// `--policy` was the only way to name a policy, and a flag is a thing you
// forget. These four are what the production bought.

/// **The claim, stated as a test.** `run` is handed NO policy — the same call
/// that produced `bound: undeclared` above — and the release is verified anyway,
/// because the program binds one. Nothing on the command line, nothing for an
/// operator to remember, and the seal is the same seal the flag produces.
#[test]
fn a_policy_the_program_binds_is_verified_with_nothing_on_the_command_line() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_fixture(dir.path());
    std::fs::write(dir.path().join("mapping.fossil"), PROGRAM_BINDING_A_POLICY)
        .expect("write the binding program");
    std::fs::write(dir.path().join("people.jsonld"), policy(5)).expect("write the policy");
    let dest = dir.path().join("out");

    run(dir.path(), &dest, None).expect("the bound holds");

    let block = privacy_block(&dest);
    assert!(
        block.contains(&"bound: k-anonymity".to_string()),
        "a bound policy is a checked bound: {block:?}",
    );
    assert!(block.contains(&"k: 5".to_string()), "{block:?}");
    assert!(
        block.contains(&"policy: https://example.org/policies/people-v1".to_string()),
        "and the manifest names the document that was read: {block:?}",
    );
}

/// The refusal half, from the binding. A producer who binds a `k` their data
/// misses gets the same refusal and the same empty destination — the binding is
/// not a weaker statement of the bound, it is the same statement in a place that
/// cannot be skipped.
#[test]
fn a_bound_policy_the_release_misses_refuses_and_writes_nothing() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_fixture(dir.path());
    std::fs::write(dir.path().join("mapping.fossil"), PROGRAM_BINDING_A_POLICY)
        .expect("write the binding program");
    // 20 to a class, and 25 asked for.
    std::fs::write(dir.path().join("people.jsonld"), policy(25)).expect("write the policy");
    let dest = dir.path().join("out");

    let err = run(dir.path(), &dest, None).expect_err("the bound does not hold");
    let message = format!("{err:?}");
    assert!(message.contains("k=20"), "{message}");
    assert!(message.contains("k=25"), "{message}");
    assert!(
        !dest.join("graph.graph.yml").exists(),
        "a refused release must leave no manifest"
    );
}

/// **Both given is an ERROR, and the decision is written down.** Letting the
/// flag win lets an operator weaken a bound the program declares; letting the
/// program win makes the flag silently inert, which is worse still because the
/// operator has evidence they were checked against a document nobody opened.
/// One message, repaired by deleting one of the two.
#[test]
fn binding_a_policy_and_passing_the_flag_as_well_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_fixture(dir.path());
    std::fs::write(dir.path().join("mapping.fossil"), PROGRAM_BINDING_A_POLICY)
        .expect("write the binding program");
    std::fs::write(dir.path().join("people.jsonld"), policy(5)).expect("write the policy");
    let dest = dir.path().join("out");
    let flag = fossil_policy::parse(&policy(5)).expect("a valid policy");

    let err = run(dir.path(), &dest, Some(&flag)).expect_err("two policies is not a merge");
    let message = format!("{err:?}");
    assert!(message.contains("--policy"), "{message}");
    assert!(
        !dest.join("graph.graph.yml").exists(),
        "and it refuses BEFORE anything is written"
    );
}

/// A bound document that cannot be read is a message about the policy, not a
/// run that gets most of the way and then cannot say what it was checking
/// against. The message names the reference as WRITTEN as well as the locator it
/// resolved to, because those differ under `@conn` and under a program run from
/// another directory, and only one of the two is in the source.
#[test]
fn a_bound_policy_that_cannot_be_read_refuses_naming_what_the_program_wrote() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_fixture(dir.path());
    std::fs::write(dir.path().join("mapping.fossil"), PROGRAM_BINDING_A_POLICY)
        .expect("write the binding program");
    // and no `people.jsonld` beside it.
    let dest = dir.path().join("out");

    let err = run(dir.path(), &dest, None).expect_err("the policy is not there");
    let message = format!("{err:?}");
    assert!(message.contains("people.jsonld"), "{message}");
    assert!(
        !dest.join("graph.graph.yml").exists(),
        "and nothing was written"
    );
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

/// **The bound becomes reachable, end to end.**
///
/// [`a_release_that_misses_the_bound_is_refused_and_writes_nothing`] is the same
/// `k` over the same 600 people, and it refuses: 30 classes of 20 do not reach
/// 25 and no amount of asking changes that. The only difference here is that the
/// policy declares a hierarchy per quasi-identifier — so the writer *derives*
/// the generalisation instead of measuring what it was handed, and the release
/// happens.
///
/// Before this, that was not a thing the system could do. A declared bound could
/// only ever refuse, because nothing anywhere could produce a generalised column
/// and a corpus passed exactly when its source data was already k-anonymous.
///
/// Everything asserted below is read back from the bytes: the manifest by line
/// scan, the payload by SQL. The number the manifest claims is recomputed from
/// the Parquet by a `DuckDB` aggregate that has never heard of the deriver, which
/// is the same independence the write path itself relies on.
#[test]
fn a_release_that_would_have_been_refused_is_derived_and_released() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_fixture(dir.path());
    let dest = dir.path().join("out");
    let policy = fossil_policy::parse(&generalising_policy(25)).expect("a valid policy");
    run(dir.path(), &dest, Some(&policy)).expect("the derivation reaches the bound");

    let block = privacy_block(&dest);
    let has = |s: &str| block.iter().any(|l| l == s);
    assert!(has("bound: k-anonymity"), "{block:?}");
    assert!(has("k: 25"), "{block:?}");
    assert!(has("population: 600"), "{block:?}");
    // Not one row was dropped to get there. Generalisation is what reached the
    // bound; suppression would have been a different claim and a charged budget.
    assert!(has("suppressed: 0"), "{block:?}");

    // The manifest says which columns were generalised, so a recipient reads it
    // rather than inferring it from the values. `@bucket` because a numeric
    // column publishes a declared bucket and never an observed range.
    let generalization = block
        .iter()
        .find(|l| l.starts_with("generalization: "))
        .unwrap_or_else(|| panic!("{block:?}"))
        .trim_start_matches("generalization: ");
    assert!(
        generalization.contains("Person.birthYear@bucket"),
        "{generalization}"
    );
    assert!(
        generalization.contains("Person.postcode@"),
        "{generalization}"
    );

    let reached: u64 = block
        .iter()
        .find_map(|l| l.strip_prefix("reached: "))
        .unwrap_or_else(|| panic!("{block:?}"))
        .parse()
        .expect("a number");
    assert!(reached >= 25, "{block:?}");

    let conn = Connection::open_in_memory().expect("duckdb");
    let tiles = dest.join("vertex/Person/tiles.parquet");

    // The manifest and the bytes agree, recomputed by a third implementation.
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
    assert_eq!(
        u64::try_from(smallest).expect("a class size is not negative"),
        reached,
        "the manifest and the bytes must agree"
    );

    // And the corpus really is generalised: no cell holds a source postcode any
    // more. `PC0` is what the fixture wrote and a prefix hierarchy is what
    // replaced it.
    let raw: i64 = conn
        .query_row(
            &format!(
                "SELECT count(*) FROM read_parquet('{}') WHERE postcode = 'PC0'",
                tiles.display()
            ),
            [],
            |row| row.get(0),
        )
        .expect("count raw postcodes");
    assert_eq!(
        raw, 0,
        "a generalised column may not publish a source value"
    );

    // The sensitive column is untouched and was never part of any of it —
    // neither the derivation nor the check ever read `diagnosis`.
    let diagnoses: i64 = conn
        .query_row(
            &format!(
                "SELECT count(DISTINCT diagnosis) FROM read_parquet('{}')",
                tiles.display()
            ),
            [],
            |row| row.get(0),
        )
        .expect("count diagnoses");
    assert_eq!(diagnoses, 3);
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
