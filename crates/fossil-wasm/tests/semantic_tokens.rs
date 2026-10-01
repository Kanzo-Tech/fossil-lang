//! `semantic_token_rows` — the browser's semantic tokens, through the native
//! mirror of `FossilWorkspace.semanticTokens`.
//!
//! What crosses the boundary is the classification `fossil-ide` already pins;
//! what this file pins is the boundary's own half: UTF-16 ranges, legend NAMES
//! instead of indices, and a multi-line span kept whole.

use fossil_wasm::{FossilWorkspace, SemanticTokenRow};

/// The text each row covers, with its kind and modifiers — read back through
/// the UTF-16 range, which is how a JS host will read it.
fn read(src: &str, rows: &[SemanticTokenRow]) -> Vec<(String, String, Vec<String>)> {
    let lines: Vec<Vec<u16>> = src
        .split('\n')
        .map(|l| l.encode_utf16().collect())
        .collect();
    rows.iter()
        .map(|r| {
            assert_eq!(
                r.range.start.line, r.range.end.line,
                "single-line here: {r:?}"
            );
            let line = &lines[r.range.start.line as usize];
            let text = String::from_utf16(
                &line[r.range.start.character as usize..r.range.end.character as usize],
            )
            .expect("a range on character boundaries");
            (text, r.kind.clone(), r.modifiers.clone())
        })
        .collect()
}

#[test]
fn rows_carry_names_and_utf16_ranges() {
    // `é` is two bytes and one UTF-16 unit: a byte range would land every row
    // after it one column late.
    let src =
        "// café\ntype { Person } := io.shex(\"@Almacén/p.shex\")\nUsers : Person from Users\n";
    let mut ws = FossilWorkspace::new();
    let h = ws.open_file_native("p.fossil".to_string(), src.to_string());
    let rows = read(src, &ws.semantic_token_rows(h));
    let find = |t: &str| {
        rows.iter()
            .find(|(text, _, _)| text == t)
            .unwrap_or_else(|| panic!("no row `{t}` in {rows:?}"))
            .clone()
    };
    assert_eq!(find("type").1, "keyword");
    assert_eq!(
        find("Person"),
        ("Person".into(), "type".into(), vec!["declaration".into()])
    );
    assert_eq!(find("@Almacén").1, "namespace");
    assert_eq!(find("/p.shex\"").1, "string");
    assert_eq!(
        find("Users"),
        (
            "Users".into(),
            "variable".into(),
            vec!["declaration".into()]
        )
    );
}

#[test]
fn a_multi_line_string_is_one_row() {
    let src = "x := io.csv(\"a\nb.csv\")\n";
    let mut ws = FossilWorkspace::new();
    let h = ws.open_file_native("m.fossil".to_string(), src.to_string());
    let rows = ws.semantic_token_rows(h);
    let string = rows
        .iter()
        .find(|r| r.kind == "string")
        .unwrap_or_else(|| panic!("a string row in {rows:?}"));
    assert_eq!(
        (string.range.start.line, string.range.end.line),
        (0, 1),
        "the row spans both lines: {string:?}"
    );
}

#[test]
fn an_unknown_handle_is_no_rows() {
    let mut ws = FossilWorkspace::new();
    let h = ws.open_file_native("gone.fossil".to_string(), "x := io.csv(\"x\")".to_string());
    ws.close_file_native(h).expect("close");
    assert!(ws.semantic_token_rows(h).is_empty());
}
