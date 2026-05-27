//! SC#5 (LSP-01 code-actions half) — integration test for the three code
//! actions through the PUBLIC [`fossil_ide::code_actions`] entry.
//!
//! Three cases (Research test-map):
//!   1. **did-you-mean** — a typo'd-identifier diagnostic carrying the
//!      structured [`fossil_base::DidYouMean`] candidate yields a `QuickFix`
//!      whose `WorkspaceEdit` replaces the typo span with the suggestion.
//!   2. **auto-import** — an unknown-prefix diagnostic yields a `QuickFix`
//!      inserting the `prefix xx: <iri>` line.
//!   3. **split-mapping (SECOND-ORDER)** — a real `ShEx` `OneOf` shape produces
//!      a diagnostic with a populated `suggestion_source` (the Phase-3
//!      `generate_split_suggestion` snippet, ADR-0006); the split-mapping
//!      `QuickFix` replaces the offending mapping with that snippet, and the
//!      EDITED document re-compiles cleanly (`parse` + `def_map` + `typecheck`
//!      with no new errors) — the same second-order assertion plan 03-08 used
//!      for the suggestion text itself.
//!
//! The split-mapping case drives the `OneOf` rejection through the REAL
//! `fossil_descriptors_output::ShExDescriptor` (the host-side descriptor a
//! production LSP loads) and the REAL `generate_split_suggestion` — the code
//! action reads `suggestion_source` VERBATIM (never regenerates it).

#![cfg(not(target_arch = "wasm32"))]
// The split snippet + IRI templates are LITERAL Fossil source, not Rust
// format-string args.
#![allow(clippy::literal_string_with_formatting_args)]

use std::sync::Arc;

use fossil_base::{Diagnostic, FossilDb, NativeSystem, Severity, SourceFile, Span, System};
use fossil_descriptors_output::{ShExDescriptor, ShExLoweringError, generate_split_suggestion};
use fossil_ide::code_actions;
use lsp_types::{CodeAction, Position, Range, TextEdit};

fn db() -> FossilDb {
    let system: Arc<dyn System> = Arc::new(NativeSystem::default());
    FossilDb::new(system)
}

fn file(db: &FossilDb, src: &str) -> SourceFile {
    SourceFile::new(db, src.to_string(), "a.fossil".to_string())
}

/// A whole-document UTF-16 range (covers everything) so every diagnostic is
/// considered.
fn whole(text: &str) -> Range {
    let last_line = u32::try_from(text.lines().count()).unwrap_or(0);
    Range {
        start: Position::new(0, 0),
        end: Position::new(last_line + 1, 0),
    }
}

/// Extract the `TextEdit`s from a single-document `WorkspaceEdit`.
fn edits_of(a: &CodeAction) -> Vec<TextEdit> {
    a.edit
        .as_ref()
        .and_then(|e| e.changes.as_ref())
        .and_then(|c| c.values().next())
        .cloned()
        .unwrap_or_default()
}

/// Apply a set of UTF-16 `TextEdit`s to `src` (single-line-aware: our edits are
/// either a top-of-file insertion or a whole-span replacement, so a simple
/// byte-offset application via the line index suffices for the test).
fn apply_edits(src: &str, edits: &[TextEdit]) -> String {
    // Compute byte offsets for each edit's start/end, then apply right-to-left so
    // earlier offsets stay valid.
    let line_starts: Vec<usize> = std::iter::once(0)
        .chain(src.match_indices('\n').map(|(i, _)| i + 1))
        .collect();
    let to_byte = |p: Position| -> usize {
        let line = p.line as usize;
        let base = line_starts.get(line).copied().unwrap_or(src.len());
        (base + p.character as usize).min(src.len())
    };
    let mut spans: Vec<(usize, usize, String)> = edits
        .iter()
        .map(|e| {
            (
                to_byte(e.range.start),
                to_byte(e.range.end),
                e.new_text.clone(),
            )
        })
        .collect();
    spans.sort_by_key(|(s, _, _)| *s);
    let mut out = String::new();
    let mut cursor = 0usize;
    for (start, end, text) in spans {
        if start >= cursor {
            out.push_str(&src[cursor..start]);
            out.push_str(&text);
            cursor = end.max(start);
        }
    }
    out.push_str(&src[cursor.min(src.len())..]);
    out
}

#[test]
fn did_you_mean_action_replaces_typo_with_suggestion() {
    let db = db();
    let src = "User : ex:Person from users\n    ex:n = .naem\n";
    let f = file(&db, src);
    let start = u32::try_from(src.find("naem").unwrap()).unwrap();
    let span = Span::new(start, start + 4);
    let diag = Diagnostic::new(
        Severity::Error,
        "unknown column `naem` — did you mean `name`?",
        span,
    )
    .with_did_you_mean(span, "name");

    let actions = code_actions(&db, f, whole(src), &[diag]);
    let a = actions
        .iter()
        .find(|a| a.title.contains("name"))
        .expect("a did-you-mean action must be offered");
    let edited = apply_edits(src, &edits_of(a));
    assert!(
        edited.contains(".name") && !edited.contains(".naem"),
        "the did-you-mean edit must rewrite `.naem` to `.name`; got {edited:?}",
    );
}

#[test]
fn auto_import_action_inserts_prefix_decl() {
    let db = db();
    let src = "User : ex:Person from users\n    ex:age = xsd:integer\n";
    let f = file(&db, src);
    let diag = Diagnostic::new(
        Severity::Error,
        "undeclared prefix `xsd:` in IRI expression `xsd:integer`",
        Span::new(0, 5),
    );
    let actions = code_actions(&db, f, whole(src), &[diag]);
    let a = actions
        .iter()
        .find(|a| a.title.contains("xsd"))
        .expect("an auto-import action must be offered");
    let edited = apply_edits(src, &edits_of(a));
    assert!(
        edited.starts_with("prefix xsd: <http://www.w3.org/2001/XMLSchema#>"),
        "auto-import must prepend the canonical xsd prefix decl; got {edited:?}",
    );
}

/// A `ShEx` schema declaring `ex:Contact` with a `OneOf` over (`ex:email` |
/// `ex:phone`) — the same shape Phase 3's tests use to exercise the rejection.
const CONTACT_ONEOF_SCHEMA: &str = r#"{
  "@context": "http://www.w3.org/ns/shex.jsonld",
  "type": "Schema",
  "shapes": [
    {
      "type": "ShapeDecl",
      "id": "http://example.org/Contact",
      "shapeExpr": {
        "type": "Shape",
        "expression": {
          "type": "OneOf",
          "expressions": [
            {
              "type": "TripleConstraint",
              "predicate": "http://example.org/email",
              "valueExpr": {
                "type": "NodeConstraint",
                "datatype": "http://www.w3.org/2001/XMLSchema#string"
              }
            },
            {
              "type": "TripleConstraint",
              "predicate": "http://example.org/phone",
              "valueExpr": {
                "type": "NodeConstraint",
                "datatype": "http://www.w3.org/2001/XMLSchema#string"
              }
            }
          ]
        }
      }
    }
  ]
}"#;

#[test]
fn split_mapping_action_recompiles_second_order() {
    let db = db();

    // The consuming mapping targets `ex:Contact` (the OneOf shape). Its prefix
    // is declared so the split snippet (which uses `ex:` predicates) re-compiles.
    let src = "\
prefix ex: <http://example.org/>
Contact : ex:Contact from contacts
    ex:email = .email
";
    let f = file(&db, src);

    // Drive the REAL OneOf rejection through the host-side ShEx descriptor +
    // the REAL split-suggestion generator — exactly as the production typecheck
    // emitter (`surface_shape_lowering_errors`) does. The mapping span covers
    // the offending `Contact : ...` block so the split edit replaces it.
    let descriptor =
        ShExDescriptor::from_reader(CONTACT_ONEOF_SCHEMA.as_bytes()).expect("ShEx parses");
    let rej = descriptor
        .lowering_errors()
        .iter()
        .find_map(|e| match e {
            ShExLoweringError::OneOfRejection(r) => Some(r),
            _ => None,
        })
        .expect("the OneOf shape must produce a rejection");

    // The mapping block runs from `Contact :` to end-of-file.
    let mapping_start = u32::try_from(src.find("Contact :").unwrap()).unwrap();
    let mapping_end = u32::try_from(src.len()).unwrap();

    let suggestion = generate_split_suggestion(
        "Contact",
        "`${ex:}contact/${.id}`",
        "contacts",
        rej.shape_iri.to_string().as_str(),
        &rej.suggestion_seed.one_of_node,
    );

    // The production emitter attaches this snippet to the diagnostic verbatim.
    let diag = Diagnostic::new(
        Severity::Error,
        "ShEx OneOf is not supported in v0.1 — split into separate mappings",
        Span::new(mapping_start, mapping_end),
    )
    .with_suggestion_source(suggestion.clone());

    let actions = code_actions(&db, f, whole(src), &[diag]);
    let a = actions
        .iter()
        .find(|a| a.title.contains("Split mapping"))
        .expect("a split-mapping action must be offered");
    let produced = edits_of(a);
    assert_eq!(produced.len(), 1);
    assert_eq!(
        produced[0].new_text, suggestion,
        "the split edit must use suggestion_source verbatim, never regenerated",
    );

    // SECOND-ORDER: the edited document re-compiles cleanly. Replace the OneOf
    // mapping with the split snippet (keeping the prefix decl) and assert
    // parse + def_map + typecheck_mapping produce NO error for any mapping (the
    // split mappings are flat — no OneOf — so they type-check under the
    // AcceptAll target).
    let edited = apply_edits(src, &produced);
    assert!(
        edited.contains("Contact1") && edited.contains("Contact2"),
        "the edited document must contain the two split mappings; got {edited:?}",
    );
    assert!(
        !edited.contains("ex:Contact from contacts\n    ex:email = .email\n}"),
        "the original OneOf mapping must be gone",
    );

    let edited_file = SourceFile::new(&db, edited.clone(), "edited.fossil".to_string());
    let mappings = fossil_hir::def_map::def_map(&db, edited_file)
        .mappings(&db)
        .clone();
    assert!(
        !mappings.is_empty(),
        "the edited document must lower to at least one mapping; got {edited:?}",
    );
    for m in mappings {
        let r = fossil_hir::typecheck_mapping(&db, m);
        assert!(
            r.is_ok(),
            "the split-mapping edit must re-compile cleanly (no new type errors); \
             mapping failed in edited doc:\n{edited}",
        );
    }
}
