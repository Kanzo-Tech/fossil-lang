//! `problem.schema.json` — every projection of it, which is one TypeScript file.
//!
//! The third generator, and the first whose source is not a `.bnf`: the error
//! catalogue is Rust (`fossil_graph_schema::Problem`), its JSON Schema is
//! derived and held by a bless test beside the crate, and this module turns
//! that file into `@fossil-lang/types`' `problem.gen.ts` — `Code`, `CODES`,
//! `ProblemData` and `TITLES`. `/docs/design/errors` has the argument for
//! generating a hundred-arm union rather than writing it.
//!
//! # The dialect it reads
//!
//! The subset `schemars` emits for `Problem` and nothing more: a top-level
//! `oneOf` of `{ code: { enum: [one code] }, data: object }`, and data fields
//! that are a string, an integer or number, a boolean, an array of those, or
//! any of them made nullable by a `["…", "null"]` type. A field outside it
//! panics naming the code and the field, because a union that silently widens
//! to `unknown` is the drift generation exists to stop.

use std::fmt::Write as _;
use std::path::PathBuf;

use serde_json::Value;

use crate::catalogue::repo_root;

/// One code, as the schema states it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Code {
    pub code: String,
    pub title: String,
    pub description: String,
    /// `(name, TypeScript type, required)`, in the schema's order.
    pub fields: Vec<(String, String, bool)>,
}

/// Where the schema lives.
#[must_use]
pub fn schema_path() -> PathBuf {
    repo_root().join("crates/fossil-graph-schema/problem.schema.json")
}

/// Parse `problem.schema.json`.
///
/// # Panics
/// On a schema outside the dialect in the module docs.
#[must_use]
pub fn read() -> Vec<Code> {
    let path = schema_path();
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} must be readable: {e}", path.display()));
    let schema: Value = serde_json::from_str(&text).expect("problem.schema.json is JSON");
    schema["oneOf"]
        .as_array()
        .expect("problem.schema.json: a top-level `oneOf`")
        .iter()
        .map(arm)
        .collect()
}

fn arm(arm: &Value) -> Code {
    let code = match arm["properties"]["code"]["enum"]
        .as_array()
        .map(Vec::as_slice)
    {
        Some([Value::String(code)]) => code.clone(),
        _ => panic!("problem.schema.json: an arm whose `code` is not one string: {arm}"),
    };
    let text = |key: &str| arm[key].as_str().unwrap_or_default().to_string();
    let data = &arm["properties"]["data"];
    let required: Vec<&str> = data["required"]
        .as_array()
        .map(|r| r.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let fields = data["properties"]
        .as_object()
        .map(|props| {
            props
                .iter()
                .map(|(name, ty)| {
                    let ts = ts_type(ty).unwrap_or_else(|| {
                        panic!("problem.schema.json: `{code}`'s `{name}` is outside the dialect xtask reads: {ty}")
                    });
                    (name.clone(), ts, required.contains(&name.as_str()))
                })
                .collect()
        })
        .unwrap_or_default();
    Code {
        title: text("title"),
        description: text("description"),
        code,
        fields,
    }
}

fn ts_type(ty: &Value) -> Option<String> {
    let scalar = |name: &str| -> Option<String> {
        Some(
            match name {
                "string" => "string",
                "integer" | "number" => "number",
                "boolean" => "boolean",
                "null" => "null",
                "array" => return Some(format!("{}[]", paren(&ts_type(&ty["items"])?))),
                _ => return None,
            }
            .to_string(),
        )
    };
    match &ty["type"] {
        Value::String(name) => scalar(name),
        Value::Array(names) => {
            let parts: Option<Vec<String>> =
                names.iter().map(|n| n.as_str().and_then(scalar)).collect();
            Some(parts?.join(" | "))
        }
        _ => None,
    }
}

/// `a | b` as an array element needs parentheses; a single name does not.
fn paren(ts: &str) -> String {
    if ts.contains(' ') {
        format!("({ts})")
    } else {
        ts.to_string()
    }
}

/// A TypeScript single-quoted string literal.
fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"))
}

/// A `JSDoc` block at `indent`, or nothing for empty text.
fn doc(out: &mut String, indent: &str, text: &str) {
    if text.is_empty() {
        return;
    }
    let _ = writeln!(out, "{indent}/**");
    for line in text.replace("*/", "*\\/").lines() {
        if line.is_empty() {
            let _ = writeln!(out, "{indent} *");
        } else {
            let _ = writeln!(out, "{indent} * {line}");
        }
    }
    let _ = writeln!(out, "{indent} */");
}

/// `problem.gen.ts`.
#[must_use]
pub fn emit_ts(codes: &[Code]) -> String {
    let mut out = String::from(
        "// @generated by `cargo xtask problem` from `crates/fossil-graph-schema/problem.schema.json`. DO NOT EDIT.\n\
         //\n\
         // The error catalogue — every code fossil can report, its data and its title — projected from\n\
         // the JSON Schema of `fossil_graph_schema::Problem`. Change the enum, re-bless the schema\n\
         // (`FOSSIL_BLESS=1 cargo test -p fossil-graph-schema --test problem_schema`), and re-run\n\
         // `cargo xtask problem`.\n\n",
    );
    out.push_str("/** A code fossil can report: `area/kind`. Stable once released — never reworded, never reused. */\n");
    out.push_str("export type Code =\n");
    for (i, c) in codes.iter().enumerate() {
        let end = if i + 1 == codes.len() { ";" } else { "" };
        let _ = writeln!(out, "  | {}{end}", quote(&c.code));
    }
    out.push_str("\n/** Every live code, in the catalogue's order. */\nexport const CODES: readonly Code[] = [\n");
    for c in codes {
        let _ = writeln!(out, "  {},", quote(&c.code));
    }
    out.push_str(
        "];\n\n/** What each code carries as `data`. */\nexport interface ProblemData {\n",
    );
    for c in codes {
        doc(&mut out, "  ", &c.description);
        if c.fields.is_empty() {
            let _ = writeln!(out, "  {}: Record<string, never>;", quote(&c.code));
            continue;
        }
        let _ = writeln!(out, "  {}: {{", quote(&c.code));
        for (name, ty, required) in &c.fields {
            let optional = if *required { "" } else { "?" };
            let key = if name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                name.clone()
            } else {
                quote(name)
            };
            let _ = writeln!(out, "    {key}{optional}: {ty};");
        }
        out.push_str("  };\n");
    }
    out.push_str("}\n\n/** Each code's title — fixed per code (RFC 9457 `title`); may be reworded in any release. */\n");
    out.push_str("export const TITLES: { readonly [C in Code]: string } = {\n");
    for c in codes {
        let _ = writeln!(out, "  {}: {},", quote(&c.code), quote(&c.title));
    }
    out.push_str("};\n");
    out
}

/// Every file generated from `problem.schema.json`, with what it should hold.
#[must_use]
pub fn generated() -> Vec<(PathBuf, String)> {
    vec![(
        repo_root().join("packages/types/src/problem.gen.ts"),
        emit_ts(&read()),
    )]
}
