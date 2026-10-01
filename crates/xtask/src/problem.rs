//! `problem.schema.json` — every projection of it, which is one TypeScript file.
//!
//! The third generator, and the first whose source is not a `.bnf`: the error
//! catalogue is Rust (`fossil_graph_schema::Problem`), its JSON Schema is
//! derived and held by a bless test beside the crate, and this module turns
//! that file into `@fossil-lang/types`' `problem.gen.ts` — `Code`, `CODES`,
//! `ProblemData`, `TITLES`, and `DETAILS`: the TypeScript that renders a code's
//! detail from its data, translated from the variant's `#[error]` (`x-detail`)
//! wherever that is a format string over fields, so a detail TypeScript raises
//! reads as the one Rust raises. `/docs/design/errors` has the argument for
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
    /// The `#[error]` as written, when [`parse_detail`] can read it.
    pub detail: Option<Vec<Segment>>,
}

/// One piece of a detail, as a `#[error]` builds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment {
    Text(String),
    /// `{field}` — a string or a number, displayed.
    Field(String),
    /// `.field.join(sep)` — a list of strings, joined.
    Join(String, String),
    /// `.field.len()` — a list's length.
    Len(String),
}

/// Read a `#[error(…)]`'s source text — a string literal, then the arguments —
/// into segments. `None` for anything but `{field}`, `{}` over `.f.join("…")`
/// or `.f.len()`, and the `{{` `}}` escapes: a helper function, a format spec, a
/// raw string. Those codes get no TypeScript detail, and TypeScript cannot raise
/// them with [`emit_ts`]'s `FossilError.of`.
#[must_use]
pub fn parse_detail(source: &str) -> Option<Vec<Segment>> {
    let (format, rest) = string_literal(source.trim_start())?;
    let rest = rest.trim();
    let args: Vec<Segment> = if rest.is_empty() {
        Vec::new()
    } else {
        split_args(rest.strip_prefix(',')?)?
            .iter()
            .map(|arg| argument(arg))
            .collect::<Option<_>>()?
    };
    let mut args = args.into_iter();
    let mut out = Vec::new();
    let mut text = String::new();
    let mut chars = format.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' if chars.peek() == Some(&'{') => {
                chars.next();
                text.push('{');
            }
            '}' if chars.peek() == Some(&'}') => {
                chars.next();
                text.push('}');
            }
            '{' => {
                let mut name = String::new();
                loop {
                    match chars.next()? {
                        '}' => break,
                        c if c.is_alphanumeric() || c == '_' => name.push(c),
                        _ => return None,
                    }
                }
                if !text.is_empty() {
                    out.push(Segment::Text(std::mem::take(&mut text)));
                }
                out.push(if name.is_empty() {
                    args.next()?
                } else {
                    Segment::Field(name)
                });
            }
            '}' => return None,
            c => text.push(c),
        }
    }
    if !text.is_empty() {
        out.push(Segment::Text(text));
    }
    args.next().is_none().then_some(out)
}

/// A Rust string literal at the start of `s`, unescaped, and what follows it.
fn string_literal(s: &str) -> Option<(String, &str)> {
    let body = s.strip_prefix('"')?;
    let mut out = String::new();
    let mut chars = body.char_indices();
    while let Some((i, c)) = chars.next() {
        match c {
            '"' => return Some((out, &body[i + 1..])),
            '\\' => match chars.next()?.1 {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                '\\' => out.push('\\'),
                '"' => out.push('"'),
                '\'' => out.push('\''),
                '\n' => {
                    // A line continuation: the newline and the next line's indent go.
                    while chars.clone().next().is_some_and(|(_, c)| c.is_whitespace()) {
                        chars.next();
                    }
                }
                _ => return None,
            },
            c => out.push(c),
        }
    }
    None
}

/// The arguments after the format string, split on the commas between them.
fn split_args(s: &str) -> Option<Vec<String>> {
    let mut args = vec![String::new()];
    let (mut depth, mut quoted, mut escaped) = (0_i32, false, false);
    for c in s.chars() {
        if quoted {
            args.last_mut()?.push(c);
            match c {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '"' => quoted = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => quoted = true,
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                args.push(String::new());
                continue;
            }
            c if c.is_whitespace() => continue,
            _ => {}
        }
        args.last_mut()?.push(c);
    }
    Some(args.into_iter().filter(|a| !a.is_empty()).collect())
}

/// One argument: `.field.join("sep")` or `.field.len()`.
fn argument(arg: &str) -> Option<Segment> {
    let rest = arg.strip_prefix('.')?;
    let (field, call) = rest.split_once('.')?;
    if !field.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return None;
    }
    if call == "len()" {
        return Some(Segment::Len(field.to_string()));
    }
    let sep = call.strip_prefix("join(")?.strip_suffix(')')?;
    let (sep, rest) = string_literal(sep)?;
    rest.is_empty()
        .then(|| Segment::Join(field.to_string(), sep))
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
        detail: arm["x-detail"].as_str().and_then(parse_detail),
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
    let detailed: Vec<&Code> = codes.iter().filter(|c| c.detail.is_some()).collect();
    out.push_str(
        "\n/** A code whose detail is rendered from its data — the ones TypeScript may raise. */\nexport type DetailedCode =\n",
    );
    for (i, c) in detailed.iter().enumerate() {
        let end = if i + 1 == detailed.len() { ";" } else { "" };
        let _ = writeln!(out, "  | {}{end}", quote(&c.code));
    }
    out.push_str(
        "\n/** Each detailed code's detail, from its data — the variant's `#[error]`, translated. */\n\
         export const DETAILS: { readonly [C in DetailedCode]: (data: ProblemData[C]) => string } = {\n",
    );
    for c in detailed {
        let segments = c.detail.as_deref().unwrap_or_default();
        let uses_data = segments.iter().any(|s| !matches!(s, Segment::Text(_)));
        let mut body = String::new();
        for segment in segments {
            match segment {
                Segment::Text(t) => body.push_str(
                    &t.replace('\\', "\\\\")
                        .replace('`', "\\`")
                        .replace("${", "\\${"),
                ),
                Segment::Field(f) => {
                    let _ = write!(body, "${{d.{f}}}");
                }
                Segment::Join(f, sep) => {
                    let _ = write!(body, "${{d.{f}.join({})}}", quote(sep));
                }
                Segment::Len(f) => {
                    let _ = write!(body, "${{d.{f}.length}}");
                }
            }
        }
        let param = if uses_data { "d" } else { "_" };
        let _ = writeln!(out, "  {}: ({param}) => `{body}`,", quote(&c.code));
    }
    out.push_str("};\n");
    out
}

/// What a code's detail reads for `data`, by [`parse_detail`]'s segments — the
/// rendering the emitted TypeScript performs, done here so a test can hold it to
/// Rust's `Display` for the same data.
#[must_use]
pub fn render(segments: &[Segment], data: &Value) -> String {
    let shown = |v: &Value| match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    let list = |f: &str| data[f].as_array().cloned().unwrap_or_default();
    segments
        .iter()
        .map(|s| match s {
            Segment::Text(t) => t.clone(),
            Segment::Field(f) => shown(&data[f]),
            Segment::Join(f, sep) => list(f).iter().map(shown).collect::<Vec<_>>().join(sep),
            Segment::Len(f) => list(f).len().to_string(),
        })
        .collect()
}

/// Every file generated from `problem.schema.json`, with what it should hold.
#[must_use]
pub fn generated() -> Vec<(PathBuf, String)> {
    vec![(
        repo_root().join("packages/types/src/problem.gen.ts"),
        emit_ts(&read()),
    )]
}
