//! `fossil-syntax` — lossless concrete syntax tree (CST) + parser for the Fossil DSL.
//!
//! # This parser and `grammar.bnf`
//!
//! `grammar.bnf` SPECIFIES the language; this crate implements it. The one
//! difference left is stated and argued in [`parser`]'s own header.
//!
//! Everything the grammar retired is REFUSED BY NAME rather than accepted or
//! dropped, and there are SIX: `prefix ex: <…>`, the CURIE `ex:Person`, the
//! `<…>` absolute IRI, a leading `.`, the backtick, and `|>`. See
//! [`parser::diag::retired`] — the wording is written down once because
//! `tests/parse_corpus.rs` reads it, and it reads all six.
//!
//! Public API contract — this signature is locked:
//!
//! ```ignore
//! #[salsa::tracked]
//! pub fn parse<'db>(db: &'db dyn fossil_base::Db, file: SourceFile) -> Cst<'db>;
//! ```

pub mod ast;
pub mod indent;
pub mod kind;
pub mod lexer;
pub mod parser;

pub use kind::{FossilLang, SyntaxElement, SyntaxKind, SyntaxNode, SyntaxToken};
pub use parser::{Cst, CstRoot, parse};

#[cfg(all(test, not(target_arch = "wasm32")))]
mod hello_fossil_integration {
    //! End-to-end check: parse the canonical example through the Salsa query
    //! and assert the top-level CST shape (1 `TYPE_DEF` + 1 `SOURCE_DEF` +
    //! 1 `MAPPING` with header + 2-property body).
    //!
    //! The program is the target spelling, and every line of it changed in this
    //! commit: the `prefix` declaration went, the shape is a bare name resolved
    //! against the `type` binding, the identity is a quoted string with a
    //! `{expr}` hole rather than a backtick with `${ex:}`, and the property
    //! value is a qualified `User.name`. It is `apps/docs/programs/shop/`
    //! shrunk to one mapping — the reference program, not an invention.

    use super::*;
    use std::sync::Arc;

    const HELLO_FOSSIL: &str = "\
type { Person } := io.shex(\"personas.shex\")

User := io.csv(\"examples/users.csv\")

Users : Person from User
    @subject = \"https://example.org/user/{User.id}\"
    name = User.name
";

    #[test]
    fn parse_hello_produces_three_top_level_items() {
        let system: Arc<dyn fossil_base::System> =
            Arc::new(fossil_base::test_support::NativeSystem::default());
        let db = fossil_base::FossilDb::new(system);
        let file = fossil_base::SourceFile::new(
            &db,
            HELLO_FOSSIL.to_string(),
            "examples/hello.fossil".to_string(),
        );
        let cst = parse(&db, file);
        let root = cst.root(&db).syntax();
        assert_eq!(root.kind(), SyntaxKind::PROGRAM);
        let item_kinds: Vec<_> = root
            .children()
            .map(|c| c.kind())
            .filter(|k| !matches!(k, SyntaxKind::ERROR))
            .collect();
        assert_eq!(
            item_kinds,
            vec![
                SyntaxKind::TYPE_DEF,
                SyntaxKind::SOURCE_DEF,
                SyntaxKind::MAPPING,
            ]
        );
    }

    #[test]
    fn parse_hello_mapping_body_has_two_properties() {
        let system: Arc<dyn fossil_base::System> =
            Arc::new(fossil_base::test_support::NativeSystem::default());
        let db = fossil_base::FossilDb::new(system);
        let file = fossil_base::SourceFile::new(
            &db,
            HELLO_FOSSIL.to_string(),
            "examples/hello.fossil".to_string(),
        );
        let cst = parse(&db, file);
        let root = cst.root(&db).syntax();
        let mapping = root
            .children()
            .find(|c| c.kind() == SyntaxKind::MAPPING)
            .expect("expected a MAPPING child");
        let header = mapping
            .children()
            .find(|c| c.kind() == SyntaxKind::MAPPING_HEADER);
        assert!(header.is_some(), "MAPPING must contain a MAPPING_HEADER");
        let body = mapping
            .children()
            .find(|c| c.kind() == SyntaxKind::MAPPING_BODY)
            .expect("MAPPING must contain a MAPPING_BODY");
        let props: Vec<_> = body
            .children()
            .filter(|c| c.kind() == SyntaxKind::PROPERTY)
            .collect();
        assert_eq!(props.len(), 2, "MAPPING_BODY must have 2 PROPERTY children");
    }

    #[test]
    fn parse_hello_round_trips_text() {
        // CST is lossless: re-serialising the root yields the original text.
        let system: Arc<dyn fossil_base::System> =
            Arc::new(fossil_base::test_support::NativeSystem::default());
        let db = fossil_base::FossilDb::new(system);
        let file = fossil_base::SourceFile::new(
            &db,
            HELLO_FOSSIL.to_string(),
            "examples/hello.fossil".to_string(),
        );
        let cst = parse(&db, file);
        let root = cst.root(&db).syntax();
        assert_eq!(root.text().to_string(), HELLO_FOSSIL);
    }

    /// The AST view over a header's shape returns a NAME.
    ///
    /// The name `Person`, to be resolved against the `type { Person } := …`
    /// binding above it — which is `fossil_hir::lower`'s job, and possible only
    /// because the header carries no IRI of its own.
    #[test]
    fn ast_view_extracts_the_shape_name() {
        use crate::ast::{Mapping, MappingHeader};
        let system: Arc<dyn fossil_base::System> =
            Arc::new(fossil_base::test_support::NativeSystem::default());
        let db = fossil_base::FossilDb::new(system);
        let file = fossil_base::SourceFile::new(
            &db,
            HELLO_FOSSIL.to_string(),
            "examples/hello.fossil".to_string(),
        );
        let cst = parse(&db, file);
        let root = cst.root(&db).syntax();
        let mapping = root
            .children()
            .find_map(Mapping::cast)
            .expect("expected a MAPPING");
        let header: MappingHeader = mapping.header().expect("expected a MAPPING_HEADER");
        assert_eq!(header.name().as_deref(), Some("Users"));
        let shape = header.shape_expr().expect("expected a SHAPE_EXPR");
        assert_eq!(shape.name().as_deref(), Some("Person"));
    }

    /// Every retired spelling in one file, and each one reports.
    ///
    /// The parse must not be quiet about ANY of them — that is the whole point
    /// of this step, and the failure it exists to prevent is measured: a
    /// property whose lowering returned `None` without a word cost a program
    /// its `slug` column while `fossil check` said *ok*.
    #[test]
    fn the_retired_spellings_each_report() {
        const RETIRED: &str = "\
prefix ex: <https://example.org/>

User := io.csv(\"u.csv\")

Users : ex:Person from User
    <http://xmlns.com/foaf/0.1/name> = User.name
    email = .email
    slug = `u/${User.id}`
";
        let system: Arc<dyn fossil_base::System> =
            Arc::new(fossil_base::test_support::NativeSystem::default());
        let db = fossil_base::FossilDb::new(system);
        let file =
            fossil_base::SourceFile::new(&db, RETIRED.to_string(), "retired.fossil".to_string());
        let _cst = parse(&db, file);
        let messages: Vec<String> = parse::accumulated::<fossil_base::Diagnostic>(&db, file)
            .iter()
            .map(|d| d.message.clone())
            .collect();
        for needle in [
            "no vocabulary",          // `prefix ex: <…>`
            "bare",                   // `ex:Person`
            "absolute IRI",           // `<http://xmlns.com/foaf/0.1/name>`
            "qualified",              // `.email`
            "backtick opens nothing", // the backtick
        ] {
            assert!(
                messages.iter().any(|m| m.contains(needle)),
                "no diagnostic mentioning {needle:?}; got {messages:#?}",
            );
        }
    }
}
