//! **The query means what the engine means.** Every conformance program is
//! run two ways — [`fossil_sql::artefact`]'s query on a native `DuckDB`, and
//! the engine (`fossil-exec`) through its N-Triples sink — and the two sets of
//! triples are compared, a numeric or temporal literal by its value.
//!
//! The engine is the oracle until it is deleted (decision of 29 October), and
//! this test goes with it: the conformance suites of R2RML and RML-Core take
//! its place.
//!
//! The check side is the editor's: each source profiled before the compile,
//! so the query reads the columns typed as the program types them.

#![allow(clippy::future_not_send, clippy::redundant_pub_crate, dead_code)]

#[path = "../../fossil-exec/tests/support/native.rs"]
mod native;
#[path = "../../fossil-exec/tests/support/mod.rs"]
mod support;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use fossil_exec::Sink;
use oxrdf::{NamedOrBlankNode, Term};

type Triple = [String; 5];

const XSD: &str = "http://www.w3.org/2001/XMLSchema#";

/// Every program under `docs/programs/` meant to run, and `examples/hello.fossil`.
fn programs() -> Vec<(String, PathBuf, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/programs");
    let mut out = vec![(
        "hello".to_string(),
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples"),
        "hello.fossil".to_string(),
    )];
    let mut dirs: Vec<_> = std::fs::read_dir(&root)
        .expect("docs/programs")
        .map(|e| e.expect("entry").path())
        .filter(|p| p.is_dir() && !p.ends_with("errors"))
        .collect();
    dirs.sort();
    for dir in dirs {
        for e in std::fs::read_dir(&dir).expect("a program dir") {
            let path = e.expect("entry").path();
            if path.extension().is_some_and(|x| x == "fossil") {
                let name = format!(
                    "{}/{}",
                    dir.file_name().unwrap().to_string_lossy(),
                    path.file_name().unwrap().to_string_lossy()
                );
                out.push((
                    name,
                    dir.clone(),
                    path.file_name().unwrap().to_string_lossy().into_owned(),
                ));
            }
        }
    }
    out
}

/// `[s, p, o, datatype, lang]`; an IRI object has an empty datatype.
fn engine(dir: &Path, file: &str) -> Result<BTreeSet<Triple>, String> {
    let ran = native::run_dir_as(dir, file, &[], Sink::NTriples)?;
    let text = ran.text(fossil_sinks::ntriples::FILE);
    Ok(oxttl::NTriplesParser::new()
        .for_slice(&text)
        .map(|t| {
            let t = t.expect("graph.nt parses");
            let s = match t.subject {
                NamedOrBlankNode::NamedNode(n) => n.into_string(),
                NamedOrBlankNode::BlankNode(b) => format!("_:{}", b.into_string()),
            };
            let [o, dt, lang] = match t.object {
                Term::NamedNode(n) => [n.into_string(), String::new(), String::new()],
                Term::Literal(l) => [
                    l.value().to_string(),
                    l.datatype().as_str().to_string(),
                    l.language().unwrap_or("").to_string(),
                ],
                other => panic!("an object fossil does not write: {other}"),
            };
            canonical([s, t.predicate.into_string(), o, dt, lang])
        })
        .collect())
}

fn query(dir: &Path, file: &str) -> Result<(String, BTreeSet<Triple>), String> {
    let (db, f) = native::open_db(&dir.join(file));
    let artefact = fossil_sql::artefact(&db, f).map_err(|e| format!("artefact: {e}"))?;
    let conn = duckdb::Connection::open_in_memory().expect("duckdb");
    let mut stmt = conn
        .prepare(&artefact.sql)
        .map_err(|e| format!("prepare: {e}"))?;
    let rows = stmt
        .query_map([], |r| {
            Ok([
                r.get::<_, String>(0)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<String>>(4)?.unwrap_or_default(),
                r.get::<_, Option<String>>(5)?.unwrap_or_default(),
            ])
        })
        .map_err(|e| format!("run: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("run: {e}"))?;
    Ok((artefact.sql, rows.into_iter().map(canonical).collect()))
}

/// A literal in one spelling per value, where the two engines print one value
/// two ways.
fn canonical(mut t: Triple) -> Triple {
    let datatype = t[3].strip_prefix(XSD).unwrap_or_default().to_string();
    let v = &mut t[2];
    match datatype.as_str() {
        "double" | "float" | "decimal" => {
            if let Ok(x) = v.parse::<f64>() {
                *v = format!("{x:?}");
            }
        }
        "integer" => {
            if let Ok(x) = v.parse::<i128>() {
                *v = x.to_string();
            }
        }
        "dateTime" | "time" => {
            if let Some((whole, fraction)) = v.split_once('.') {
                let fraction = fraction.trim_end_matches('0');
                *v = if fraction.is_empty() {
                    whole.to_string()
                } else {
                    format!("{whole}.{fraction}")
                };
            }
        }
        _ => {}
    }
    t
}

#[test]
fn the_query_means_what_the_engine_means() {
    let mut report = Vec::new();
    let mut agree = 0;
    let mut hello = false;
    for (name, dir, file) in programs() {
        let Ok(expected) = engine(&dir, &file) else {
            report.push(format!("SKIP {name}: the engine does not run it"));
            continue;
        };
        match query(&dir, &file) {
            Err(e) => report.push(format!("FAIL {name}: {e}")),
            Ok((sql, got)) => {
                if got == expected {
                    agree += 1;
                    hello |= name == "hello";
                    report.push(format!("PASS {name} ({} triples)", got.len()));
                } else {
                    let mut lines = vec![format!("FAIL {name}: the triples differ")];
                    for t in expected.difference(&got).take(4) {
                        lines.push(format!("    only the engine: {t:?}"));
                    }
                    for t in got.difference(&expected).take(4) {
                        lines.push(format!("    only the query:  {t:?}"));
                    }
                    if std::env::var_os("FOSSIL_SQL_SHOW").is_some() {
                        lines.push(sql);
                    }
                    report.push(lines.join("\n"));
                }
            }
        }
    }
    println!("{}", report.join("\n"));
    assert!(hello, "hello.fossil disagrees:\n{}", report.join("\n"));
    assert!(agree > 0);
    assert!(
        !report.iter().any(|l| l.starts_with("FAIL")),
        "{}",
        report.join("\n")
    );
}
