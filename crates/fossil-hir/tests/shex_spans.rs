//! Every shape and predicate the conformance corpus declares in `ShExC` has a
//! span, and the span selects its label.
//!
//! The spans are found by `fossil_shex::spans`, a lookup over the document's
//! text with rudof's own prefix spellings. Goto-definition and the second label
//! of a type error both read them, and a `None` costs a silent `0..0` jump or a
//! missing label — nothing fails. This walks `docs/programs` so a corpus
//! document that defeats the lookup is a red test rather than a cursor at the
//! top of a file.
//!
//! It cannot say the corpus is complete: a document no program contains is a
//! document this does not read.

use std::path::{Path, PathBuf};

use fossil_shex::ShExDescriptor;

fn shex_documents(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut paths: Vec<_> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
        .map(|e| e.expect("a directory entry").path())
        .collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            shex_documents(&path, out);
        } else if path.extension().is_some_and(|e| e == "shex") {
            out.push(path);
        }
    }
}

#[test]
fn every_corpus_shape_and_predicate_has_a_span() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/programs");
    let mut documents = Vec::new();
    shex_documents(&root, &mut documents);
    assert!(!documents.is_empty(), "no `.shex` under {}", root.display());

    let mut missing = Vec::new();
    let mut seen = 0usize;
    for path in &documents {
        let src = std::fs::read_to_string(path).expect("read a corpus document");
        // A document the decoder refuses is a fixture for that refusal.
        let Ok(descriptor) = ShExDescriptor::from_shex_source(&src) else {
            continue;
        };
        for shape in descriptor.to_output_shapes().shapes() {
            let spans = std::iter::once((&shape.iri, shape.span))
                .chain(shape.properties.iter().map(|p| (&p.predicate, p.span)));
            for (iri, span) in spans {
                seen += 1;
                if span.and_then(|s| s.slice(&src)).is_none() {
                    missing.push(format!("{}: {iri}", path.display()));
                }
            }
        }
    }
    assert!(
        seen > 0,
        "every corpus document was refused — this asserts nothing"
    );
    assert!(
        missing.is_empty(),
        "declared without a span:\n{}",
        missing.join("\n")
    );
}
