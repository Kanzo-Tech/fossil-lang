//! A quoted member, `Knows."Person.id"`, is a field's NAME and is coloured as
//! the bare one is: property, not string. A string anywhere else stays a string.

#![cfg(not(target_arch = "wasm32"))]

use std::sync::Arc;

use fossil_base::test_support::NativeSystem;
use fossil_base::{FossilDb, SourceFile, System};
use fossil_ide::{decode_tokens, legend_type_name, semantic_tokens};

#[test]
fn a_quoted_member_is_a_property_and_a_string_literal_is_not() {
    const SRC: &str = "x = Knows.\"Person.id\" == \"Person.id\"\n";
    let system: Arc<dyn System> = Arc::new(NativeSystem::default());
    let db = FossilDb::new(system);
    let file = SourceFile::new(&db, SRC.to_string(), "q.fossil".to_string());
    let at = |col: u32| {
        decode_tokens(&semantic_tokens(&db, file))
            .into_iter()
            .find(|&(line, c, _, _)| line == 0 && c == col)
            .map(|(_, _, _, ty)| legend_type_name(ty))
    };
    assert_eq!(at(10), Some("property"), "the member after the dot");
    assert_eq!(at(25), Some("string"), "the literal on the right");
}
