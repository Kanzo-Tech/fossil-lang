//! **One meaning, two sinks** (decision S1/G1 of the engine split): for every
//! conformance program that runs, the triples the N-Triples sink writes are the
//! triples `rml::sql` makes of `mapping.ttl` over the `GraphAr` tables the same
//! program writes, and both runs report the same dropped edges.
//!
//! The N-Triples document is read back by `oxttl`, so a line the sink writes is
//! held to a parser and not to the printer that wrote it.
//!
//! **A literal is compared by its value where the two engines spell one value
//! two ways.** Arrow renders a double, a decimal or a timestamp as text in its
//! own form and `DuckDB` in another (`1e20` and `100000000000000000000.0`, a
//! fraction of seconds with or without its zeros); RDF compares the two as one
//! value of the datatype, and so does this test. Every other literal, and every
//! IRI, is compared by its text.
//!
//! # What it does NOT prove
//!
//! - **The programs under `errors/`**, which compile to nothing to run.
//! - **A refusal.** `GraphAr`'s writer refuses data no graph can hold (a value
//!   it has no type for, a subject twice in a type) that a set of triples can;
//!   the comparison is over the programs that both sinks write.

#[path = "support/native.rs"]
mod native;
mod support;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use fossil_exec::Sink;
use native::Triple;
use oxrdf::{NamedOrBlankNode, Term};

const XSD: &str = "http://www.w3.org/2001/XMLSchema#";

/// Every program directory under `docs/programs/` that is meant to run.
fn programs() -> Vec<(String, PathBuf, String)> {
    fn walk(dir: &Path, root: &Path, out: &mut Vec<(String, PathBuf, String)>) {
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
            .map(|e| e.expect("dir entry").path())
            .collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                if path.file_name().is_some_and(|n| n == "errors") {
                    continue;
                }
                walk(&path, root, out);
            } else if path.extension().is_some_and(|e| e == "fossil") {
                let name = dir
                    .strip_prefix(root)
                    .expect("under the root")
                    .display()
                    .to_string();
                let file = path
                    .file_name()
                    .expect("a file")
                    .to_string_lossy()
                    .into_owned();
                out.push((name, dir.to_path_buf(), file));
            }
        }
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/programs")
        .canonicalize()
        .expect("docs/programs is on disk");
    let mut out = Vec::new();
    walk(&root, &root, &mut out);
    out
}

/// The triples of the written `graph.nt`, in the shape `rml::sql` answers.
fn parsed(ran: &native::Written) -> BTreeSet<Triple> {
    let text = ran.text(fossil_sinks::ntriples::FILE);
    oxttl::NTriplesParser::new()
        .for_slice(&text)
        .map(|triple| {
            let triple = triple.unwrap_or_else(|e| panic!("graph.nt does not parse: {e}"));
            let (sk, s) = match triple.subject {
                NamedOrBlankNode::NamedNode(n) => ("I", n.into_string()),
                NamedOrBlankNode::BlankNode(b) => ("B", b.into_string()),
            };
            let p = triple.predicate.into_string();
            let [ok, o, dt, lang] = match triple.object {
                Term::NamedNode(n) => ["I", n.as_str(), "", ""].map(str::to_string),
                Term::BlankNode(b) => ["B", b.as_str(), "", ""].map(str::to_string),
                Term::Literal(l) => [
                    "L",
                    l.value(),
                    l.datatype().as_str(),
                    l.language().unwrap_or(""),
                ]
                .map(str::to_string),
                other @ Term::Triple(_) => panic!("an object fossil does not write: {other}"),
            };
            [sk.to_string(), s, p, ok, o, dt, lang]
        })
        .collect()
}

/// A literal's text in one spelling per value, for the datatypes whose value
/// the two engines print differently.
fn canonical(mut triple: Triple) -> Triple {
    let datatype = triple[5].strip_prefix(XSD).unwrap_or_default().to_string();
    let value = &mut triple[4];
    match datatype.as_str() {
        "double" | "float" | "decimal" => {
            if let Ok(v) = value.parse::<f64>() {
                *value = format!("{v:?}");
            }
        }
        "integer" => {
            if let Ok(v) = value.parse::<i128>() {
                *value = v.to_string();
            }
        }
        "dateTime" | "time" => {
            if let Some((whole, fraction)) = value.split_once('.') {
                let fraction = fraction.trim_end_matches('0');
                *value = if fraction.is_empty() {
                    whole.to_string()
                } else {
                    format!("{whole}.{fraction}")
                };
            }
        }
        _ => {}
    }
    triple
}

fn canonical_set(triples: BTreeSet<Triple>) -> BTreeSet<Triple> {
    triples.into_iter().map(canonical).collect()
}

#[test]
fn every_program_means_the_same_triples_in_both_sinks() {
    let mut findings = Vec::new();
    let mut compared = 0;
    for (name, dir, file) in programs() {
        let Ok(graphar) = native::run_dir(&dir, &file, &[]) else {
            // A program that does not run is `programs.rs`'s to report.
            continue;
        };
        let ntriples = match native::run_dir_as(&dir, &file, &[], Sink::NTriples) {
            Ok(ran) => ran,
            Err(e) => {
                findings.push(format!(
                    "{name}: GraphAr wrote it and N-Triples refused: {e}"
                ));
                continue;
            }
        };
        compared += 1;
        if graphar.report.dropped != ntriples.report.dropped {
            findings.push(format!(
                "{name}: dropped {:?} in GraphAr and {:?} in N-Triples",
                graphar.report.dropped, ntriples.report.dropped
            ));
        }
        let (rml, nt) = (
            canonical_set(graphar.triples()),
            canonical_set(parsed(&ntriples)),
        );
        for only in rml.difference(&nt) {
            findings.push(format!("{name}: only in GraphAr's mapping: {only:?}"));
        }
        for only in nt.difference(&rml) {
            findings.push(format!("{name}: only in N-Triples: {only:?}"));
        }
    }
    assert!(compared > 0, "no program ran");
    assert!(findings.is_empty(), "{}", findings.join("\n"));
}

/// The document is one line per triple, sorted, with no line twice: one graph
/// is always the same bytes.
#[test]
fn the_document_is_sorted_and_has_no_line_twice() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    let ran =
        native::run_dir_as(&dir, "hello.fossil", &[], Sink::NTriples).expect("hello.fossil runs");
    let text = ran.text(fossil_sinks::ntriples::FILE);
    let lines: Vec<&str> = text.lines().collect();
    let mut sorted = lines.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(lines, sorted);
    // 5 vertices, each typed and named.
    assert_eq!(lines.len(), 10);
}
