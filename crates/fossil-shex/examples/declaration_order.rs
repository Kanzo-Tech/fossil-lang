//! Does declaration order survive the parse, and is `ShExDescriptor::shapes()` stable?
//!
//! `type { A, B } = io.shex(...)` binds **by order**, so the order has to exist
//! and be the file's. Two things are measured here, both unverified until this
//! ran:
//!
//! 1. Whether rudof's `Schema::shapes()` returns the shapes in the order the
//!    document declares them, for both surface syntaxes (`ShExC` and `ShExJ`).
//! 2. Whether `ShExDescriptor::shapes()` — a `HashMap::values()` — is usable for
//!    the same purpose. Run the example twice: Rust seeds its hasher per process,
//!    so a differing second line is the finding, not a flake.
//!
//! The shapes are named so that declaration order, alphabetical order and IRI
//! order all differ; otherwise a wrong order could pass by coincidence.

use fossil_shex::ShExDescriptor;

const SHEXC: &str = r"PREFIX ex: <https://example.org/>

ex:Zeta  { ex:a . }
ex:Alpha { ex:b . }
ex:Mu    { ex:c . }
ex:Beta  { ex:d . }
ex:Omega { ex:e . }
";

const SHEXJ: &str = r#"{
  "@context": "http://www.w3.org/ns/shex.jsonld",
  "type": "Schema",
  "shapes": [
    { "type": "ShapeDecl", "id": "https://example.org/Zeta",
      "shapeExpr": { "type": "Shape" } },
    { "type": "ShapeDecl", "id": "https://example.org/Alpha",
      "shapeExpr": { "type": "Shape" } },
    { "type": "ShapeDecl", "id": "https://example.org/Mu",
      "shapeExpr": { "type": "Shape" } },
    { "type": "ShapeDecl", "id": "https://example.org/Beta",
      "shapeExpr": { "type": "Shape" } },
    { "type": "ShapeDecl", "id": "https://example.org/Omega",
      "shapeExpr": { "type": "Shape" } }
  ]
}"#;

const DECLARED: [&str; 5] = ["Zeta", "Alpha", "Mu", "Beta", "Omega"];

fn local(s: &str) -> String {
    s.rsplit(['#', '/'])
        .next()
        .unwrap_or(s)
        .trim_end_matches('>')
        .to_string()
}

fn report(label: &str, src: &str) {
    let desc = match ShExDescriptor::from_shex_source(src) {
        Ok(d) => d,
        Err(e) => {
            println!("{label}: PARSE FAILED — {e:?}");
            return;
        }
    };

    let ast: Vec<String> = desc
        .schema()
        .shapes()
        .into_iter()
        .flatten()
        .map(|d| local(&format!("{}", d.id)))
        .collect();

    let table: Vec<String> = desc.shapes().map(|b| local(&b.iri.to_string())).collect();

    println!("{label}");
    println!("  declared in file : {DECLARED:?}");
    println!("  rudof AST order  : {ast:?}  {}", verdict(&ast));
    println!("  descriptor order : {table:?}  {}", verdict(&table));
}

fn verdict(got: &[String]) -> &'static str {
    if got.len() != DECLARED.len() {
        return "<- WRONG LENGTH";
    }
    if got.iter().zip(DECLARED).all(|(g, d)| g == d) {
        "<- matches declaration order"
    } else {
        "<- DOES NOT match declaration order"
    }
}

fn main() {
    report("ShExC (compact, human-authored)", SHEXC);
    println!();
    report("ShExJ (JSON interchange)", SHEXJ);
    println!(
        "\nRun twice. `descriptor order` differing between runs is the HashMap \
         finding of ADR-0057's tenth amendment, not a flake."
    );
}
