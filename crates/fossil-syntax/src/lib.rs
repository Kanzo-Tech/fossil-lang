//! `fossil-syntax` — lossless concrete syntax tree (CST) + parser for the Fossil DSL.
//!
//! Phase 1 implements the subset of `grammar.bnf` needed for `examples/hello.fossil`:
//! prefix declarations, source bindings (`:=`), and mappings (header + property body).
//!
//! Public API contract for Phase 2-9 (see `01-02-PLAN.md` and RESEARCH.md
//! §"Architecture Patterns" Pattern 3):
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
    //! End-to-end check: parse the canonical Phase 1 example through the Salsa
    //! query and assert the top-level CST shape matches the plan's spec
    //! (1 `PREFIX_DECL` + 1 `SOURCE_DEF` + 1 `MAPPING` with header + 2-property body).

    use super::*;
    use std::sync::Arc;

    const HELLO_FOSSIL: &str = "\
prefix ex: <https://example.org/>

users := io.csv(\"examples/users.csv\")

User : ex:Person from users
    iri = `${ex:}user/${.id}`
    ex:name = .name
";

    #[test]
    fn parse_hello_produces_three_top_level_items() {
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem::default());
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
                SyntaxKind::PREFIX_DECL,
                SyntaxKind::SOURCE_DEF,
                SyntaxKind::MAPPING,
            ]
        );
    }

    #[test]
    fn parse_hello_mapping_body_has_two_properties() {
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem::default());
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
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem::default());
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

    #[test]
    fn ast_view_extracts_prefix_name_and_iri() {
        use crate::ast::PrefixDecl;
        let system: Arc<dyn fossil_base::System> = Arc::new(fossil_base::NativeSystem::default());
        let db = fossil_base::FossilDb::new(system);
        let file = fossil_base::SourceFile::new(
            &db,
            HELLO_FOSSIL.to_string(),
            "examples/hello.fossil".to_string(),
        );
        let cst = parse(&db, file);
        let root = cst.root(&db).syntax();
        let prefix_node = root
            .children()
            .find(|c| c.kind() == SyntaxKind::PREFIX_DECL)
            .expect("expected a PREFIX_DECL");
        let prefix = PrefixDecl::cast(prefix_node).expect("kind already checked");
        assert_eq!(prefix.name().as_deref(), Some("ex"));
        assert_eq!(prefix.iri().as_deref(), Some("https://example.org/"));
    }
}
